#!/usr/bin/env python3
"""批量评估脚本（Agent 侧）。

目标：
- 基于本仓库 demo manifest 跑 react/reflexion 两组实验
- 自动调用 `just run` 导出 trajectory
- 计算显式指标（漏洞检出率、误报率、CWE 准确率、token、时延）
- 产出逐 case 指标 JSONL 与策略级汇总 JSON

约束：
- 不依赖外部 Python 三方包（仅标准库）
- 尽量零交互
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = ROOT / "demo-projects" / "eval_manifest.json"
DEFAULT_OUTPUT_DIR = ROOT / "outputs" / "eval-batch"
DEFAULT_RUNS = 1
DEFAULT_STRATEGIES = ("react", "reflexion")
DEFAULT_CASE_TIMEOUT_SECONDS = 180
DEFAULT_BUILD_TIMEOUT_SECONDS = 600

CWE_PATTERN = re.compile(r"CWE[-_\s]?(\d{1,5})", re.IGNORECASE)


def normalize_cwe(value: str) -> str:
    m = CWE_PATTERN.search(value)
    if not m:
        return value.strip().upper()
    return f"CWE-{m.group(1)}"


def extract_cwes_from_text(text: str) -> set[str]:
    result: set[str] = set()
    for match in CWE_PATTERN.findall(text):
        result.add(f"CWE-{match}")
    return result


def load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def parse_dotenv(dotenv_path: Path) -> dict[str, str]:
    if not dotenv_path.exists():
        return {}

    result: dict[str, str] = {}
    for raw_line in dotenv_path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        value = value.strip()
        if not key:
            continue
        if (value.startswith('"') and value.endswith('"')) or (
            value.startswith("'") and value.endswith("'")
        ):
            value = value[1:-1]
        result[key] = value
    return result


def dump_json(path: Path, data: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")


def dump_jsonl(path: Path, rows: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as f:
        for row in rows:
            f.write(json.dumps(row, ensure_ascii=False) + "\n")


@dataclass
class CaseDef:
    case_id: str
    project: str
    eval_mode: str
    target: str
    language: str
    chat_message: str | None
    expected_cwe_ids: list[str]
    expected_vulnerable: bool


@dataclass
class CaseMetric:
    case_id: str
    strategy: str
    run_index: int
    target: str
    expected_cwe_ids: list[str]
    predicted_cwe_ids: list[str]
    true_positive: int
    false_positive: int
    false_negative: int
    vulnerability_detection_rate: float
    false_positive_rate: float
    cwe_classification_accuracy: float
    duration_ms: int
    token_prompt: int
    token_completion: int
    token_total: int
    token_efficiency: float | None
    trajectory_path: str
    success: bool
    error: str | None


def parse_manifest(path: Path) -> list[CaseDef]:
    payload = load_json(path)
    raw_cases = payload.get("cases", [])
    cases: list[CaseDef] = []
    for item in raw_cases:
        expected = [normalize_cwe(x) for x in item.get("expected_cwe_ids", [])]
        eval_mode = str(item.get("eval_mode", "file")).strip().lower()
        target = str(item.get("target", "")).strip()
        if not target:
            raise ValueError(f"case 缺少 target: {item}")
        cases.append(
            CaseDef(
                case_id=item["case_id"],
                project=item["project"],
                eval_mode=eval_mode,
                target=target,
                language=str(item.get("language", "unknown")),
                chat_message=item.get("chat_message"),
                expected_cwe_ids=expected,
                expected_vulnerable=bool(item.get("expected_vulnerable", True)),
            )
        )
    return cases


def run_single_case(
    case: CaseDef,
    strategy: str,
    run_index: int,
    output_dir: Path,
    env_map: dict[str, str],
    timeout_seconds: int,
    secaudit_binary: Path,
) -> tuple[Path, int]:
    trajectory_path = (
        output_dir
        / strategy
        / f"run-{run_index:02d}"
        / f"{case.case_id}.trajectory.json"
    )
    trajectory_path.parent.mkdir(parents=True, exist_ok=True)
    if trajectory_path.exists():
        trajectory_path.unlink()

    case_target_path = (ROOT / case.target).resolve()
    if case.eval_mode == "project":
        message = case.chat_message or "请审计当前项目高风险漏洞并给出 CWE 与修复建议。"
        command = [
            str(secaudit_binary),
            "--mode",
            "chat",
            "--output-format",
            "json",
            "--message",
            message,
            "--confirm-mode",
            "deny",
        ]
        command_cwd = case_target_path
    else:
        command = [
            str(secaudit_binary),
            case.target,
            "-l",
            case.language,
            "-f",
            "json",
            "-s",
            strategy,
            "-o",
            str(trajectory_path),
        ]
        command_cwd = ROOT

    started = time.perf_counter()
    try:
        result = subprocess.run(
            command,
            cwd=command_cwd,
            check=False,
            capture_output=True,
            text=True,
            env=env_map,
            timeout=timeout_seconds,
        )
    except subprocess.TimeoutExpired as exc:
        duration_ms = int((time.perf_counter() - started) * 1000)
        if trajectory_path.exists():
            try:
                _ = load_json(trajectory_path)
                return trajectory_path, duration_ms
            except Exception:  # noqa: BLE001
                pass
        raise RuntimeError(
            f"执行超时 case={case.case_id} strategy={strategy} run={run_index}: {exc}"
        ) from exc
    duration_ms = int((time.perf_counter() - started) * 1000)

    if result.returncode != 0:
        stderr = (result.stderr or result.stdout).strip()
        raise RuntimeError(
            f"执行失败 case={case.case_id} strategy={strategy} run={run_index}: {stderr}"
        )

    if case.eval_mode == "project":
        try:
            response = json.loads(result.stdout)
        except json.JSONDecodeError as exc:
            raise RuntimeError(
                f"project 模式输出非 JSON case={case.case_id}: {result.stdout[:400]}"
            ) from exc

        if response.get("status") == "error":
            error_msg = response.get("error") or "未知错误"
            raise RuntimeError(f"project 模式失败 case={case.case_id}: {error_msg}")

        session = response.get("session") or {}
        messages = session.get("messages") or []
        user_input: list[dict[str, Any]] = []
        for message in messages:
            role = str(message.get("role", "")).strip().lower()
            content = message.get("content")
            if not isinstance(content, str):
                content = ""
            if role == "assistant":
                mapped_role = "ai"
            elif role == "user":
                mapped_role = "human"
            elif role == "system":
                mapped_role = "system"
            elif role == "tool":
                mapped_role = "tool"
            else:
                mapped_role = role or "unknown"
            entry: dict[str, Any] = {"role": mapped_role, "content": content}
            tool_calls = message.get("tool_calls")
            if isinstance(tool_calls, list) and mapped_role == "ai":
                normalized_calls: list[dict[str, Any]] = []
                for call in tool_calls:
                    function = call.get("function") if isinstance(call, dict) else None
                    if not isinstance(function, dict):
                        continue
                    name = function.get("name")
                    arguments_raw = function.get("arguments")
                    arguments: dict[str, Any] = {}
                    if isinstance(arguments_raw, str) and arguments_raw.strip():
                        try:
                            parsed_args = json.loads(arguments_raw)
                            if isinstance(parsed_args, dict):
                                arguments = parsed_args
                        except json.JSONDecodeError:
                            arguments = {"_raw": arguments_raw}
                    normalized_calls.append(
                        {"name": str(name or "unknown"), "arguments": arguments}
                    )
                if normalized_calls:
                    entry["tool_calls"] = normalized_calls
            if mapped_role == "tool":
                entry["name"] = "tool_result"
            user_input.append(entry)

        metrics = response.get("metrics") or {}
        token_usage = metrics.get("token_usage") if isinstance(metrics, dict) else None
        trajectory_payload = {
            "user_input": user_input,
            "reference": None,
            "reference_tool_calls": None,
            "metadata": {
                "token_usage": token_usage if isinstance(token_usage, dict) else {},
                "duration_ms": response.get("duration_ms"),
                "work_dir": response.get("work_dir"),
                "confirm_mode": response.get("confirm_mode"),
                "eval_mode": "project",
            },
        }
        dump_json(trajectory_path, trajectory_payload)

    return trajectory_path, duration_ms


def read_token_usage(trajectory: dict[str, Any]) -> tuple[int, int, int]:
    metadata = trajectory.get("metadata") or {}
    usage = metadata.get("token_usage") or {}
    prompt = int(usage.get("prompt_tokens", 0) or 0)
    completion = int(usage.get("completion_tokens", 0) or 0)
    total = int(usage.get("total_tokens", 0) or 0)
    return prompt, completion, total


def read_predicted_cwes(trajectory: dict[str, Any]) -> list[str]:
    all_text: list[str] = []
    for message in trajectory.get("user_input", []):
        role = message.get("role")
        content = message.get("content")
        if role in {"ai", "tool"} and isinstance(content, str):
            all_text.append(content)

    found: set[str] = set()
    for text in all_text:
        found.update(extract_cwes_from_text(text))

    return sorted(found)


def safe_div(n: float, d: float) -> float:
    if d == 0:
        return 0.0
    return n / d


def evaluate_case(
    case: CaseDef,
    strategy: str,
    run_index: int,
    trajectory_path: Path,
    duration_ms: int,
) -> CaseMetric:
    traj = load_json(trajectory_path)
    predicted = read_predicted_cwes(traj)
    expected = sorted({normalize_cwe(x) for x in case.expected_cwe_ids})

    expected_set = set(expected)
    predicted_set = set(predicted)

    tp = len(expected_set & predicted_set)
    fp = len(predicted_set - expected_set)
    fn = len(expected_set - predicted_set)

    detection_rate = safe_div(tp, len(expected_set))
    fp_rate = safe_div(fp, max(len(predicted_set), 1))
    cwe_acc = safe_div(tp, len(expected_set))

    token_prompt, token_completion, token_total = read_token_usage(traj)
    token_efficiency = None
    if tp > 0:
        token_efficiency = token_total / tp

    return CaseMetric(
        case_id=case.case_id,
        strategy=strategy,
        run_index=run_index,
        target=case.target,
        expected_cwe_ids=expected,
        predicted_cwe_ids=predicted,
        true_positive=tp,
        false_positive=fp,
        false_negative=fn,
        vulnerability_detection_rate=detection_rate,
        false_positive_rate=fp_rate,
        cwe_classification_accuracy=cwe_acc,
        duration_ms=duration_ms,
        token_prompt=token_prompt,
        token_completion=token_completion,
        token_total=token_total,
        token_efficiency=token_efficiency,
        trajectory_path=str(trajectory_path.relative_to(ROOT)),
        success=True,
        error=None,
    )


def aggregate(metrics: list[CaseMetric]) -> dict[str, Any]:
    if not metrics:
        return {}

    def avg(values: list[float]) -> float:
        return sum(values) / len(values)

    grouped: dict[str, list[CaseMetric]] = {}
    for metric in metrics:
        grouped.setdefault(metric.strategy, []).append(metric)

    by_strategy: dict[str, Any] = {}
    for strategy, items in grouped.items():
        success_items = [x for x in items if x.success]
        failed_items = [x for x in items if not x.success]
        by_strategy[strategy] = {
            "case_count": len(items),
            "success_count": len(success_items),
            "failed_count": len(failed_items),
            "avg_vulnerability_detection_rate": avg(
                [x.vulnerability_detection_rate for x in success_items]
            )
            if success_items
            else 0.0,
            "avg_false_positive_rate": avg([x.false_positive_rate for x in success_items])
            if success_items
            else 0.0,
            "avg_cwe_classification_accuracy": avg(
                [x.cwe_classification_accuracy for x in success_items]
            )
            if success_items
            else 0.0,
            "avg_duration_ms": avg([float(x.duration_ms) for x in success_items])
            if success_items
            else 0.0,
            "avg_token_total": avg([float(x.token_total) for x in success_items])
            if success_items
            else 0.0,
            "avg_token_efficiency": avg(
                [
                    x.token_efficiency
                    for x in success_items
                    if x.token_efficiency is not None
                ]
            )
            if any(x.token_efficiency is not None for x in success_items)
            else None,
        }

    return {
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "strategy_summary": by_strategy,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="批量评估 securagent")
    parser.add_argument(
        "--manifest",
        type=Path,
        default=DEFAULT_MANIFEST,
        help=f"样本清单路径（默认：{DEFAULT_MANIFEST}）",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=DEFAULT_OUTPUT_DIR,
        help=f"输出目录（默认：{DEFAULT_OUTPUT_DIR}）",
    )
    parser.add_argument(
        "--runs",
        type=int,
        default=DEFAULT_RUNS,
        help=f"每个策略重复次数（默认：{DEFAULT_RUNS}）",
    )
    parser.add_argument(
        "--strategies",
        type=str,
        default=",".join(DEFAULT_STRATEGIES),
        help="策略列表，逗号分隔（默认：react,reflexion）",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="仅输出执行计划，不实际运行 agent",
    )
    parser.add_argument(
        "--case-timeout-seconds",
        type=int,
        default=DEFAULT_CASE_TIMEOUT_SECONDS,
        help=f"单样本执行超时秒数（默认：{DEFAULT_CASE_TIMEOUT_SECONDS}）",
    )
    parser.add_argument(
        "--build-timeout-seconds",
        type=int,
        default=DEFAULT_BUILD_TIMEOUT_SECONDS,
        help=f"构建 secaudit 二进制超时秒数（默认：{DEFAULT_BUILD_TIMEOUT_SECONDS}）",
    )
    parser.add_argument(
        "--max-iterations",
        type=int,
        default=8,
        help="覆盖 SECAUDIT_MAX_ITERATIONS（默认：8）",
    )
    parser.add_argument(
        "--continue-on-error",
        action="store_true",
        help="单样本失败时继续后续样本（默认中断）",
    )
    parser.add_argument(
        "--case-filter",
        type=str,
        default="",
        help="仅运行 case_id 包含该子串的样本",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()

    if args.runs <= 0:
        raise ValueError("runs 必须 > 0")
    if args.case_timeout_seconds <= 0:
        raise ValueError("case-timeout-seconds 必须 > 0")
    if args.build_timeout_seconds <= 0:
        raise ValueError("build-timeout-seconds 必须 > 0")
    if args.max_iterations <= 0:
        raise ValueError("max-iterations 必须 > 0")

    strategies = [x.strip() for x in args.strategies.split(",") if x.strip()]
    if not strategies:
        raise ValueError("strategies 不能为空")

    manifest_path = args.manifest.resolve()
    output_dir = args.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    dotenv_values = parse_dotenv(ROOT / ".env")
    run_env = os.environ.copy()
    for key, value in dotenv_values.items():
        run_env.setdefault(key, value)
    run_env.pop("RUSTC_WRAPPER", None)
    run_env["SECAUDIT_MAX_ITERATIONS"] = str(args.max_iterations)

    cases = parse_manifest(manifest_path)
    if args.case_filter:
        cases = [c for c in cases if args.case_filter in c.case_id]
    if not cases:
        raise RuntimeError(f"manifest 无 case: {manifest_path}")

    plan = {
        "manifest": str(manifest_path),
        "case_count": len(cases),
        "strategies": strategies,
        "runs": args.runs,
        "output_dir": str(output_dir),
        "case_timeout_seconds": args.case_timeout_seconds,
        "build_timeout_seconds": args.build_timeout_seconds,
        "max_iterations": args.max_iterations,
        "continue_on_error": args.continue_on_error,
        "case_filter": args.case_filter or None,
    }

    print(json.dumps({"plan": plan}, ensure_ascii=False, indent=2))

    if args.dry_run:
        return 0

    secaudit_binary = ROOT / "target" / "debug" / "secaudit"
    if not secaudit_binary.exists():
        build_started = time.perf_counter()
        build_result = subprocess.run(
            ["cargo", "build", "-q", "-p", "secaudit"],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
            env=run_env,
            timeout=args.build_timeout_seconds,
        )
        build_ms = int((time.perf_counter() - build_started) * 1000)
        if build_result.returncode != 0:
            stderr = (build_result.stderr or build_result.stdout).strip()
            raise RuntimeError(f"构建 secaudit 失败（{build_ms} ms）：{stderr}")
        if not secaudit_binary.exists():
            raise RuntimeError("构建完成但未找到 target/debug/secaudit")

    all_metrics: list[CaseMetric] = []

    for strategy in strategies:
        for run_index in range(1, args.runs + 1):
            for case in cases:
                try:
                    trajectory_path, duration_ms = run_single_case(
                        case=case,
                        strategy=strategy,
                        run_index=run_index,
                        output_dir=output_dir,
                        env_map=run_env,
                        timeout_seconds=args.case_timeout_seconds,
                        secaudit_binary=secaudit_binary,
                    )
                    metric = evaluate_case(
                        case=case,
                        strategy=strategy,
                        run_index=run_index,
                        trajectory_path=trajectory_path,
                        duration_ms=duration_ms,
                    )
                    all_metrics.append(metric)
                    print(
                        json.dumps(
                            {
                                "case_id": case.case_id,
                                "strategy": strategy,
                                "run_index": run_index,
                                "detection": metric.vulnerability_detection_rate,
                                "fp_rate": metric.false_positive_rate,
                                "cwe_acc": metric.cwe_classification_accuracy,
                                "token_total": metric.token_total,
                                "duration_ms": metric.duration_ms,
                            },
                            ensure_ascii=False,
                        )
                    )
                except Exception as exc:  # noqa: BLE001
                    failed_metric = CaseMetric(
                        case_id=case.case_id,
                        strategy=strategy,
                        run_index=run_index,
                        target=case.target,
                        expected_cwe_ids=case.expected_cwe_ids,
                        predicted_cwe_ids=[],
                        true_positive=0,
                        false_positive=0,
                        false_negative=len(case.expected_cwe_ids),
                        vulnerability_detection_rate=0.0,
                        false_positive_rate=0.0,
                        cwe_classification_accuracy=0.0,
                        duration_ms=0,
                        token_prompt=0,
                        token_completion=0,
                        token_total=0,
                        token_efficiency=None,
                        trajectory_path="",
                        success=False,
                        error=str(exc),
                    )
                    all_metrics.append(failed_metric)
                    print(
                        json.dumps(
                            {
                                "case_id": case.case_id,
                                "strategy": strategy,
                                "run_index": run_index,
                                "success": False,
                                "error": str(exc),
                            },
                            ensure_ascii=False,
                        )
                    )
                    if not args.continue_on_error:
                        raise

    rows = [metric.__dict__ for metric in all_metrics]
    summary = aggregate(all_metrics)

    dump_jsonl(output_dir / "case-metrics.jsonl", rows)
    dump_json(output_dir / "summary.json", summary)

    print(
        json.dumps(
            {
                "done": True,
                "summary_path": str((output_dir / "summary.json").relative_to(ROOT)),
                "case_metrics_path": str(
                    (output_dir / "case-metrics.jsonl").relative_to(ROOT)
                ),
            },
            ensure_ascii=False,
            indent=2,
        )
    )

    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:  # noqa: BLE001
        print(f"ERROR: {exc}", file=sys.stderr)
        raise SystemExit(1)
