#!/usr/bin/env python3
"""数据集准备脚本

从多个来源下载安全漏洞相关数据集，处理为统一 JSON 格式，
输出到 securagent/datasets/ 目录。

数据来源：
  1. HuggingFace: lemon42-ai/Code_Vulnerability_Labeled_Dataset
  2. OWASP Benchmark: expectedresults-1.2.csv

输出记录格式：
  {
    "code": "源代码内容",
    "language": "编程语言",
    "cwe_ids": ["CWE-xxx"],
    "is_vulnerable": true/false,
    "source": "数据来源标识"
  }

用法：
  uv run prepare_dataset.py [--output-dir PATH]
"""

from __future__ import annotations

import argparse
import csv
from datetime import datetime, timezone
from collections import Counter
import hashlib
import io
import json
import logging
import re
import tarfile
from pathlib import Path

import requests
from datasets import load_dataset
from requests.adapters import HTTPAdapter
from urllib3.util import Retry

# ── 常量 ──────────────────────────────────────────────────────────────

# HuggingFace 数据集标识
HF_DATASET_ID = "lemon42-ai/Code_Vulnerability_Labeled_Dataset"
HF_SOURCE_TAG = "huggingface/code-vulnerability-labeled"
DEFAULT_HF_LABEL_CWE_MAP_PATH = (
    Path(__file__).resolve().parent / "hf_label_cwe_map.json"
)

# OWASP Benchmark 配置
OWASP_CSV_URL = (
    "https://raw.githubusercontent.com/OWASP-Benchmark/BenchmarkJava"
    "/master/expectedresults-1.2.csv"
)
OWASP_SOURCE_TAG = "owasp-benchmark/1.2"
OWASP_LANGUAGE = "java"
OWASP_SOURCE_CODE_URL_TEMPLATE = (
    "https://raw.githubusercontent.com/OWASP-Benchmark/BenchmarkJava/master/"
    "src/main/java/org/owasp/benchmark/testcode/{test_name}.java"
)
OWASP_SOURCE_ARCHIVE_URL = (
    "https://codeload.github.com/OWASP-Benchmark/BenchmarkJava/"
    "tar.gz/refs/heads/master"
)

# 输出文件名
OUTPUT_HF_FILENAME = "code_vulnerability_labeled.json"
OUTPUT_OWASP_FILENAME = "owasp_benchmark.json"
OUTPUT_COVERAGE_FILENAME = "coverage_report.json"

# 默认输出目录（相对于脚本位置）
DEFAULT_OUTPUT_DIR = Path(__file__).resolve().parent.parent / "datasets"

# 请求超时（秒）
REQUEST_TIMEOUT = 60
REQUEST_RETRY_TOTAL = 3

# 非漏洞标签关键词（用于 HF 启发式判断）
NON_VULNERABLE_LABEL_KEYWORDS = (
    "non-vulnerable",
    "not vulnerable",
    "no vulnerability",
    "safe",
    "benign",
    "clean",
)

# CWE 正则
CWE_PATTERN = re.compile(r"\\bCWE[-_\\s]?(\\d{1,5})\\b", re.IGNORECASE)

# 日志格式
LOG_FORMAT = "%(asctime)s [%(levelname)s] %(message)s"

# ── 日志配置 ────────────────────────────────────────────────────────

logging.basicConfig(level=logging.INFO, format=LOG_FORMAT)
logger = logging.getLogger(__name__)


# ── HuggingFace 数据集处理 ──────────────────────────────────────────


def process_hf_dataset(
    output_dir: Path,
    label_cwe_map: dict[str, list[str]],
    mapping_version: str,
) -> dict:
    """下载并处理 HuggingFace 漏洞标注数据集"""
    logger.info("正在从 HuggingFace 加载数据集：%s", HF_DATASET_ID)
    ds = load_dataset(HF_DATASET_ID)
    logger.info("已加载 HF 标签-CWE 映射：%d 条", len(label_cwe_map))

    records: list[dict] = []
    cwe_source_counter: Counter[str] = Counter()

    for split_name in ds:
        logger.info("处理分片：%s（%d 条）", split_name, len(ds[split_name]))
        for row in ds[split_name]:
            # 提取 CWE 编号列表
            cwe_ids, cwe_source = _extract_cwe_ids(row)
            vulnerability_label = str(row.get("label", "")).strip()
            if not cwe_ids and vulnerability_label:
                mapped = label_cwe_map.get(_normalize_label(vulnerability_label), [])
                cwe_ids = [cwe for cwe in mapped if cwe]
                if cwe_ids:
                    cwe_source = "mapped_label"
            # 判断语言
            language = row.get("language", "unknown")
            if isinstance(language, str):
                language = language.strip().lower()
            # 判断是否存在漏洞
            is_vulnerable = _determine_vulnerability(row)
            cwe_source_counter[cwe_source] += 1

            records.append({
                "code": row.get("code", row.get("func", "")),
                "language": language,
                "cwe_ids": cwe_ids,
                "cwe_source": cwe_source,
                "is_vulnerable": is_vulnerable,
                "vulnerability_label": vulnerability_label,
                "mapping_version": mapping_version,
                "source": HF_SOURCE_TAG,
            })

    output_path = output_dir / OUTPUT_HF_FILENAME
    _write_json(records, output_path)
    logger.info("HuggingFace 数据集处理完成：%d 条 -> %s", len(records), output_path)
    vulnerable_count = sum(1 for r in records if r["is_vulnerable"])
    cwe_covered_count = sum(1 for r in records if r["cwe_ids"])
    return {
        "dataset": "huggingface",
        "total": len(records),
        "vulnerable": vulnerable_count,
        "safe": len(records) - vulnerable_count,
        "cwe_covered": cwe_covered_count,
        "cwe_coverage_rate": _safe_ratio(cwe_covered_count, len(records)),
        "cwe_source_distribution": dict(cwe_source_counter),
        "unmapped_vulnerable_labels": sorted({
            r["vulnerability_label"]
            for r in records
            if r["is_vulnerable"] and not r["cwe_ids"]
        }),
    }


def _extract_cwe_ids(row: dict) -> tuple[list[str], str]:
    """从数据行中提取 CWE 编号列表及其来源"""
    # 尝试多种可能的字段名
    for field in ("cwe_ids", "cwe", "cwe_id", "CWE"):
        value = row.get(field)
        if value is None:
            continue
        if isinstance(value, list):
            return [str(v).strip() for v in value if v], "raw_field"
        if isinstance(value, str) and value.strip():
            # 可能是逗号分隔的字符串
            return [v.strip() for v in value.split(",") if v.strip()], "raw_field"
    # 兜底：从常见文本字段中提取 CWE 编号
    text_candidates = (
        row.get("label"),
        row.get("vulnerability"),
        row.get("code"),
        row.get("func"),
    )
    extracted: list[str] = []
    for text in text_candidates:
        if not isinstance(text, str) or not text.strip():
            continue
        for match in CWE_PATTERN.findall(text):
            cwe_id = f"CWE-{match}"
            if cwe_id not in extracted:
                extracted.append(cwe_id)
    if extracted:
        return extracted, "text_extract"
    return [], "none"


def _determine_vulnerability(row: dict) -> bool:
    """从数据行中判断是否存在漏洞"""
    for field in ("is_vulnerable", "vulnerable", "label", "target"):
        value = row.get(field)
        if value is None:
            continue
        if isinstance(value, bool):
            return value
        if isinstance(value, (int, float)):
            return bool(value)
        if isinstance(value, str):
            normalized = value.strip().lower()
            if normalized in ("true", "1", "yes", "vulnerable"):
                return True
            if normalized in ("false", "0", "no", "non-vulnerable", "not vulnerable"):
                return False
            if field in ("label", "target") and normalized:
                return not any(
                    keyword in normalized for keyword in NON_VULNERABLE_LABEL_KEYWORDS
                )
    return False


# ── OWASP Benchmark 处理 ───────────────────────────────────────────


def process_owasp_benchmark(output_dir: Path) -> dict:
    """下载并处理 OWASP Benchmark 预期结果数据集"""
    logger.info("正在下载 OWASP Benchmark CSV: %s", OWASP_CSV_URL)
    response = _http_get(OWASP_CSV_URL)

    logger.info("正在下载 OWASP Benchmark 源码归档：%s", OWASP_SOURCE_ARCHIVE_URL)
    source_code_map = _download_owasp_source_code_map()
    logger.info("已加载 OWASP 测试源码：%d 条", len(source_code_map))

    reader = csv.DictReader(io.StringIO(response.text), skipinitialspace=True)
    records: list[dict] = []
    source_missing_count = 0

    for raw_row in reader:
        row = _normalize_csv_row(raw_row)
        # CSV 字段：# test name, category, real vulnerability, CWE, ...
        test_name = row.get("# test name", row.get("test name", "")).strip()
        cwe_raw = row.get("cwe", "").strip()
        is_vulnerable = row.get("real vulnerability", "").strip().lower() == "true"
        category = row.get("category", "").strip()
        source_url = OWASP_SOURCE_CODE_URL_TEMPLATE.format(test_name=test_name)

        code_content = source_code_map.get(test_name)
        if not code_content:
            source_missing_count += 1
            code_content = (
                f"// OWASP Benchmark 测试用例：{test_name}\\n"
                f"// 类别：{category}\\n"
                f"// 源码获取失败：{source_url}"
            )

        cwe_ids = [f"CWE-{cwe_raw}"] if cwe_raw else []

        records.append({
            "code": code_content,
            "language": OWASP_LANGUAGE,
            "cwe_ids": cwe_ids,
            "cwe_source": "owasp_csv",
            "is_vulnerable": is_vulnerable,
            "test_name": test_name,
            "category": category,
            "source_url": source_url,
            "source": OWASP_SOURCE_TAG,
        })

    output_path = output_dir / OUTPUT_OWASP_FILENAME
    _write_json(records, output_path)
    logger.info("OWASP 数据集处理完成：%d 条 -> %s", len(records), output_path)
    vulnerable_count = sum(1 for r in records if r["is_vulnerable"])
    cwe_covered_count = sum(1 for r in records if r["cwe_ids"])
    return {
        "dataset": "owasp",
        "total": len(records),
        "vulnerable": vulnerable_count,
        "safe": len(records) - vulnerable_count,
        "cwe_covered": cwe_covered_count,
        "cwe_coverage_rate": _safe_ratio(cwe_covered_count, len(records)),
        "source_missing_count": source_missing_count,
    }


# ── 工具函数 ────────────────────────────────────────────────────────


def _normalize_csv_row(row: dict) -> dict[str, str]:
    """规范化 CSV 行键名和值（去空白、键名小写）"""
    normalized: dict[str, str] = {}
    for key, value in row.items():
        if key is None:
            continue
        normalized_key = str(key).strip().lower()
        normalized_value = "" if value is None else str(value).strip()
        normalized[normalized_key] = normalized_value
    return normalized


def _normalize_label(label: str) -> str:
    """规范化标签文本，用于映射查询"""
    return " ".join(label.strip().lower().split())


def _load_hf_label_cwe_map(path: Path) -> dict[str, list[str]]:
    """加载 HF 标签到 CWE 的映射配置"""
    if not path.exists():
        logger.warning("HF 标签-CWE 映射文件不存在，跳过映射：%s", path)
        return {}
    with open(path, "r", encoding="utf-8") as f:
        raw = json.load(f)

    mapping: dict[str, list[str]] = {}
    if not isinstance(raw, dict):
        logger.warning("HF 标签-CWE 映射文件格式无效（需要对象）: %s", path)
        return mapping

    for label, cwes in raw.items():
        if not isinstance(label, str):
            continue
        normalized_label = _normalize_label(label)
        if isinstance(cwes, list):
            mapping[normalized_label] = [
                str(cwe).strip() for cwe in cwes if str(cwe).strip()
            ]
        elif isinstance(cwes, str) and cwes.strip():
            mapping[normalized_label] = [cwes.strip()]
        else:
            mapping[normalized_label] = []
    return mapping


def _compute_hf_mapping_version(path: Path, mapping: dict[str, list[str]]) -> str:
    """根据映射文件路径和内容计算稳定版本号"""
    digest_input = json.dumps(mapping, ensure_ascii=False, sort_keys=True).encode(
        "utf-8"
    )
    digest = hashlib.sha1(digest_input).hexdigest()[:10]
    return f"{path.name}:{digest}"


def _write_coverage_report(
    output_dir: Path,
    hf_summary: dict,
    owasp_summary: dict,
    mapping_path: Path,
    mapping_version: str,
    mapping_size: int,
) -> None:
    """写入数据覆盖率与来源统计报告"""
    report = {
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "datasets": {
            "huggingface": hf_summary,
            "owasp": owasp_summary,
        },
        "mapping": {
            "path": str(mapping_path),
            "version": mapping_version,
            "labels": mapping_size,
        },
    }
    _write_json(report, output_dir / OUTPUT_COVERAGE_FILENAME)


def _safe_ratio(numerator: int, denominator: int) -> float:
    """安全计算比例"""
    if denominator == 0:
        return 0.0
    return numerator / denominator


def _http_get(url: str) -> requests.Response:
    """统一 HTTP GET：带重试，并在失败时回退为无代理请求"""
    session = requests.Session()
    retry = Retry(
        total=REQUEST_RETRY_TOTAL,
        connect=REQUEST_RETRY_TOTAL,
        read=REQUEST_RETRY_TOTAL,
        backoff_factor=0.5,
        status_forcelist=(429, 500, 502, 503, 504),
        allowed_methods=frozenset({"GET"}),
    )
    adapter = HTTPAdapter(max_retries=retry)
    session.mount("http://", adapter)
    session.mount("https://", adapter)

    try:
        response = session.get(url, timeout=REQUEST_TIMEOUT)
        response.raise_for_status()
        return response
    except requests.RequestException as primary_err:
        logger.warning("网络请求失败，尝试无代理回退：%s (%s)", url, primary_err)
        fallback_session = requests.Session()
        fallback_session.trust_env = False
        fallback_response = fallback_session.get(url, timeout=REQUEST_TIMEOUT)
        fallback_response.raise_for_status()
        return fallback_response


def _download_owasp_source_code_map() -> dict[str, str]:
    """下载 OWASP 源码归档并提取 Benchmark 测试用例源码"""
    archive_response = _http_get(OWASP_SOURCE_ARCHIVE_URL)
    code_map: dict[str, str] = {}

    with tarfile.open(fileobj=io.BytesIO(archive_response.content), mode="r:gz") as tar:
        for member in tar.getmembers():
            if not member.isfile():
                continue
            if not member.name.endswith(".java"):
                continue
            if (
                "/src/main/java/org/owasp/benchmark/testcode/BenchmarkTest"
                not in member.name
            ):
                continue

            file_obj = tar.extractfile(member)
            if file_obj is None:
                continue
            content = file_obj.read().decode("utf-8", errors="replace").strip()
            test_name = Path(member.name).stem
            code_map[test_name] = content

    return code_map


def _write_json(records: list[dict], path: Path) -> None:
    """将记录列表写入 JSON 文件"""
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        json.dump(records, f, ensure_ascii=False, indent=2)


# ── 主入口 ──────────────────────────────────────────────────────────


def main() -> None:
    parser = argparse.ArgumentParser(description="安全漏洞数据集准备脚本")
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=DEFAULT_OUTPUT_DIR,
        help=f"输出目录（默认：{DEFAULT_OUTPUT_DIR}）",
    )
    parser.add_argument(
        "--hf-label-cwe-map",
        type=Path,
        default=DEFAULT_HF_LABEL_CWE_MAP_PATH,
        help=f"HF 标签到 CWE 映射配置文件（默认：{DEFAULT_HF_LABEL_CWE_MAP_PATH}）",
    )
    args = parser.parse_args()

    output_dir: Path = args.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    logger.info("输出目录：%s", output_dir)

    mapping_path = args.hf_label_cwe_map.resolve()
    mapping = _load_hf_label_cwe_map(mapping_path)
    mapping_version = _compute_hf_mapping_version(mapping_path, mapping)

    hf_summary = process_hf_dataset(output_dir, mapping, mapping_version)
    owasp_summary = process_owasp_benchmark(output_dir)
    _write_coverage_report(
        output_dir=output_dir,
        hf_summary=hf_summary,
        owasp_summary=owasp_summary,
        mapping_path=mapping_path,
        mapping_version=mapping_version,
        mapping_size=len(mapping),
    )

    logger.info("全部数据集准备完成")


if __name__ == "__main__":
    main()
