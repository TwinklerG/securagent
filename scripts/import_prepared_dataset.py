#!/usr/bin/env python3
"""将 prepare_dataset 输出批量导入 eval-platform。

支持导入任意 `prepare_dataset.py` 生成的数据文件（HF/OWASP）。
导入时会把每条记录包装成 eval-platform 所需格式：

{
  "taskId": <task_id>,
  "samples": [
    {"input": <record_object>},
    ...
  ]
}

后端在评测执行时会自动把该记录适配为 ragrs 可处理的 MultiTurnSample。

用法示例：
  uv run import_prepared_dataset.py \
    --input-file ../datasets/code_vulnerability_labeled.json \
    --task-id 1

  uv run import_prepared_dataset.py \
    --input-file ../datasets/owasp_benchmark.json \
    --task-id 2 --dataset-id 3 --batch-size 100
"""

from __future__ import annotations

import argparse
import json
import logging
from pathlib import Path
from typing import Any

import requests

DEFAULT_API_BASE = "http://localhost:3000/api"
DEFAULT_BATCH_SIZE = 200
REQUEST_TIMEOUT = 30
LOG_FORMAT = "%(asctime)s [%(levelname)s] %(message)s"

logging.basicConfig(level=logging.INFO, format=LOG_FORMAT)
logger = logging.getLogger(__name__)


def _load_records(input_file: Path) -> list[dict[str, Any]]:
    with open(input_file, "r", encoding="utf-8") as f:
        data = json.load(f)

    if not isinstance(data, list):
        raise ValueError(f"输入文件不是 JSON 数组：{input_file}")

    records: list[dict[str, Any]] = []
    for index, item in enumerate(data):
        if not isinstance(item, dict):
            raise ValueError(f"第 {index} 条记录不是 JSON 对象：{input_file}")
        records.append(item)
    return records


def _chunk_records(
    records: list[dict[str, Any]], batch_size: int
) -> list[list[dict[str, Any]]]:
    return [records[i : i + batch_size] for i in range(0, len(records), batch_size)]


def _build_endpoint(api_base: str, dataset_id: int | None) -> str:
    base = api_base.rstrip("/")
    if dataset_id is None:
        return f"{base}/samples"
    return f"{base}/datasets/{dataset_id}/samples"


def _post_batch(
    endpoint: str,
    task_id: int,
    batch: list[dict[str, Any]],
) -> int:
    payload = {
        "taskId": task_id,
        "samples": [{"input": record} for record in batch],
    }
    response = requests.post(endpoint, json=payload, timeout=REQUEST_TIMEOUT)
    response.raise_for_status()
    data = response.json()
    return int(data.get("count", 0))


def main() -> None:
    parser = argparse.ArgumentParser(
        description="导入 prepare_dataset 产出的数据到 eval-platform"
    )
    parser.add_argument(
        "--input-file",
        type=Path,
        action="append",
        required=True,
        help="输入 JSON 文件路径，可重复传入多个",
    )
    parser.add_argument("--task-id", type=int, required=True, help="目标评测任务 ID")
    parser.add_argument(
        "--dataset-id",
        type=int,
        help="可选：目标数据集 ID（对应 /datasets/:id/samples）",
    )
    parser.add_argument(
        "--api-base",
        type=str,
        default=DEFAULT_API_BASE,
        help=f"eval-platform API 根路径（默认：{DEFAULT_API_BASE}）",
    )
    parser.add_argument(
        "--batch-size",
        type=int,
        default=DEFAULT_BATCH_SIZE,
        help=f"分批导入大小（默认：{DEFAULT_BATCH_SIZE}）",
    )
    parser.add_argument(
        "--dry-run", action="store_true", help="仅检查与统计，不发送请求"
    )
    args = parser.parse_args()

    if args.batch_size <= 0:
        raise ValueError("batch-size 必须大于 0")

    endpoint = _build_endpoint(args.api_base, args.dataset_id)
    logger.info("导入端点：%s", endpoint)

    total_records = 0
    total_imported = 0

    for input_file in args.input_file:
        resolved = input_file.resolve()
        records = _load_records(resolved)
        batches = _chunk_records(records, args.batch_size)

        logger.info(
            "文件：%s，记录数：%d，批次数：%d",
            resolved,
            len(records),
            len(batches),
        )

        total_records += len(records)

        if args.dry_run:
            continue

        for idx, batch in enumerate(batches, start=1):
            imported = _post_batch(endpoint, args.task_id, batch)
            total_imported += imported
            logger.info("  批次 %d/%d 导入完成：%d 条", idx, len(batches), imported)

    if args.dry_run:
        logger.info("Dry-run 完成：共检查 %d 条记录，未发送请求", total_records)
    else:
        logger.info("导入完成：共导入 %d/%d 条", total_imported, total_records)


if __name__ == "__main__":
    main()
