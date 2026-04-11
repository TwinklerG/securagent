#!/usr/bin/env python3
"""一键小样本联调：secaudit Agent -> 评估平台任务 -> 结果回读。

说明：
- 默认只跑显式指标，几乎不额外消耗 LLM（仅 agent 审计本身调用一次/少量）。
- 可选开启 fuzzy 指标（--with-fuzzy），此时评估阶段也会调用 LLM。
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


DEFAULT_BACKEND = "http://127.0.0.1:3000"
POLL_INTERVAL = 1.0
POLL_TIMEOUT_SECONDS = 120
AGENT_MAX_RETRY = 3
AGENT_RETRY_INTERVAL_SECONDS = 2


def api_request(base_url: str, method: str, path: str, payload: dict[str, Any] | None = None) -> tuple[int, dict[str, Any]]:
    data = None if payload is None else json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(f"{base_url}{path}", data=data, method=method)
    if payload is not None:
        req.add_header("Content-Type", "application/json")

    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            return resp.status, json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as err:
        body = err.read().decode("utf-8")
        try:
            return err.code, json.loads(body)
        except json.JSONDecodeError:
            return err.code, {"error": body}


def collect_ai_output_from_trajectory(trajectory_path: Path) -> str:
    data = json.loads(trajectory_path.read_text(encoding="utf-8"))
    messages = data.get("user_input", [])
    ai_chunks: list[str] = []

    for msg in messages:
        if msg.get("role") == "ai":
            content = msg.get("content")
            if isinstance(content, str) and content.strip():
                ai_chunks.append(content.strip())

    return "\n".join(ai_chunks)


def wait_task_done(base_url: str, task_id: int) -> dict[str, Any]:
    started = time.time()
    while True:
        status, body = api_request(base_url, "GET", f"/api/tasks/{task_id}")
        if status != 200:
            raise RuntimeError(f"查询任务失败：{status} {body}")

        task = body.get("data", {})
        state = task.get("status")
        if state in {"completed", "failed"}:
            return task

        if time.time() - started > POLL_TIMEOUT_SECONDS:
            raise TimeoutError(f"任务 {task_id} 超时未完成")

        time.sleep(POLL_INTERVAL)


def require_env(key: str) -> str:
    value = os.getenv(key, "").strip()
    if not value:
        raise RuntimeError(f"缺少环境变量：{key}")
    return value


def run_agent_sample(securagent_root: Path, sample_file: Path, trajectory_file: Path) -> None:
    command = [
        "cargo",
        "run",
        "-q",
        "-p",
        "secaudit",
        "--",
        str(sample_file),
        "-l",
        "java",
        "-f",
        "json",
        "-o",
        str(trajectory_file),
    ]

    last_error = ""
    for attempt in range(1, AGENT_MAX_RETRY + 1):
        result = subprocess.run(
            command,
            cwd=securagent_root,
            check=False,
            text=True,
            capture_output=True,
            env=os.environ.copy(),
        )

        if result.returncode == 0:
            return

        stderr = result.stderr.strip() or result.stdout.strip()
        last_error = stderr

        if attempt < AGENT_MAX_RETRY:
            time.sleep(AGENT_RETRY_INTERVAL_SECONDS)

    raise RuntimeError(f"secaudit 运行失败：{last_error}")


def main() -> int:
    parser = argparse.ArgumentParser(description="小样本一键联调：agent -> eval")
    parser.add_argument("--backend-url", default=DEFAULT_BACKEND, help="评估后端地址")
    parser.add_argument("--with-fuzzy", action="store_true", help="额外执行 severity_accuracy（评估阶段将调用 LLM）")
    args = parser.parse_args()

    # 1) 前置环境检查
    require_env("SECAUDIT_API_KEY")
    api_base_url = require_env("SECAUDIT_API_BASE_URL")
    model = require_env("SECAUDIT_MODEL")

    script_file = Path(__file__).resolve()
    securagent_root = script_file.parents[1]

    # 2) 小样本代码（避免消耗）
    sample_code = (
        'public class Demo {\n'
        '  public void login(String user) {\n'
        '    String sql = "SELECT * FROM users WHERE name=\'" + user + "\'";\n'
        '    System.out.println(sql);\n'
        '  }\n'
        '}\n'
    )

    with tempfile.TemporaryDirectory(prefix="secaudit-smoke-") as td:
        td_path = Path(td)
        sample_file = td_path / "sample.java"
        trajectory_file = td_path / "trajectory.json"
        sample_file.write_text(sample_code, encoding="utf-8")

        # 3) 跑 agent
        run_agent_sample(securagent_root, sample_file, trajectory_file)
        ai_output = collect_ai_output_from_trajectory(trajectory_file)
        if not ai_output:
            ai_output = "未提取到 AI 输出"

        # 4) 创建评估任务
        metrics = [
            "vulnerability_detection_rate",
            "false_positive_rate",
            "cwe_classification_accuracy",
        ]
        config: dict[str, Any] = {"metrics": metrics}

        if args.with_fuzzy:
            config["metrics"] = [*metrics, "severity_accuracy"]
            config["llm_config"] = {
                "api_base_url": api_base_url,
                "api_key": require_env("SECAUDIT_API_KEY"),
                "model": model,
            }

        status, body = api_request(
            args.backend_url,
            "POST",
            "/api/tasks",
            {
                "name": "agent-e2e-smoke",
                "description": "agent 输出自动入库并评估（小样本）",
                "agentType": "secaudit-agent",
                "config": json.dumps(config, ensure_ascii=False),
            },
        )
        if status != 201:
            raise RuntimeError(f"创建任务失败：{status} {body}")
        task_id = body["data"]["id"]

        # 5) 导入样本（用 prepared record + agent output）
        sample_input = {
            "code": sample_code,
            "language": "java",
            "cwe_ids": ["CWE-89"],
            "is_vulnerable": True,
            "source": "agent-smoke",
            "cwe_source": "manual",
        }

        status, body = api_request(
            args.backend_url,
            "POST",
            "/api/samples",
            {
                "taskId": task_id,
                "samples": [
                    {
                        "input": sample_input,
                        "output": ai_output,
                        "trajectory": json.loads(trajectory_file.read_text(encoding="utf-8")),
                    }
                ],
            },
        )
        if status != 201:
            raise RuntimeError(f"导入样本失败：{status} {body}")

        # 6) 触发评估并等待完成
        status, body = api_request(args.backend_url, "POST", f"/api/tasks/{task_id}/run", {})
        if status != 200:
            raise RuntimeError(f"启动评估失败：{status} {body}")

        task = wait_task_done(args.backend_url, task_id)
        if task.get("status") != "completed":
            raise RuntimeError(f"评估未完成：status={task.get('status')} lastError={task.get('lastError')}")

        # 7) 拉取并打印结果
        status, body = api_request(args.backend_url, "GET", f"/api/results?taskId={task_id}")
        if status != 200:
            raise RuntimeError(f"读取结果失败：{status} {body}")

        print(f"TASK_ID={task_id}")
        print(f"WITH_FUZZY={args.with_fuzzy}")
        for item in sorted(body.get("data", []), key=lambda x: x.get("metricName", "")):
            print(f"{item.get('metricName')}={item.get('score')}")

    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:  # noqa: BLE001
        print(f"ERROR: {exc}", file=sys.stderr)
        raise SystemExit(1)
