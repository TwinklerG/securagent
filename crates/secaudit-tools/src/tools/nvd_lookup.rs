//! NVD（国家漏洞数据库）查询工具 — 通过 NVD REST API 查询 CVE 信息。

use std::fmt::Write as _;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::Error;
use crate::tools::Tool;

// —— 参数字段名 ——

const PARAM_QUERY: &str = "query";
const PARAM_CWE_ID: &str = "cwe_id";

// —— API 配置 ——

/// NVD CVE API 端点
const NVD_API_URL: &str = "https://services.nvd.nist.gov/rest/json/cves/2.0";

/// 查询结果最大数量
const MAX_RESULTS: u32 = 5;

/// 请求超时时间（秒）
const REQUEST_TIMEOUT_SECS: u64 = 15;

/// NVD API 响应
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NvdResponse {
    /// 结果总数
    #[serde(default)]
    total_results: u32,
    /// CVE 条目列表
    #[serde(default)]
    vulnerabilities: Vec<NvdVulnerability>,
}

/// CVE 条目包装
#[derive(Debug, Deserialize)]
struct NvdVulnerability {
    /// CVE 详情
    cve: NvdCve,
}

/// CVE 详情
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NvdCve {
    /// CVE ID（如 CVE-2024-1234）
    id: String,
    /// 描述列表
    #[serde(default)]
    descriptions: Vec<NvdDescription>,
    /// CVSS 度量
    #[serde(default)]
    metrics: NvdMetrics,
}

/// CVE 描述
#[derive(Debug, Deserialize)]
struct NvdDescription {
    /// 语言
    lang: String,
    /// 描述文本
    value: String,
}

/// CVSS 度量
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NvdMetrics {
    /// CVSS v3.1 评分
    #[serde(default)]
    cvss_metric_v31: Vec<CvssEntry>,
}

/// CVSS 评分条目
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CvssEntry {
    /// CVSS 数据
    cvss_data: CvssData,
}

/// CVSS 数据
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CvssData {
    /// 基础评分
    base_score: f64,
    /// 严重度
    base_severity: String,
}

/// NVD 漏洞查询工具
pub struct NvdLookup {
    /// HTTP 客户端
    client: reqwest::Client,
}

impl NvdLookup {
    /// 创建实例。
    #[must_use]
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client }
    }
}

impl Default for NvdLookup {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for NvdLookup {
    fn name(&self) -> &'static str {
        "nvd_lookup"
    }

    fn description(&self) -> &'static str {
        "查询 NVD（国家漏洞数据库）获取 CVE 信息，支持按关键词或 CWE 编号搜索已知漏洞"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                PARAM_QUERY: {
                    "type": "string",
                    "description": "搜索关键词（如 'SQL injection python'）"
                },
                PARAM_CWE_ID: {
                    "type": "string",
                    "description": "CWE 编号（如 'CWE-89'），按漏洞类型查询相关 CVE"
                }
            }
        })
    }

    async fn execute(&self, params: &Value) -> Result<String, Error> {
        let keyword = params.get(PARAM_QUERY).and_then(Value::as_str);
        let cwe_id = params.get(PARAM_CWE_ID).and_then(Value::as_str);

        if keyword.is_none() && cwe_id.is_none() {
            return Err(Error::Tool("至少需要提供 query 或 cwe_id 参数之一".into()));
        }

        // 构建查询参数
        let mut url = format!("{NVD_API_URL}?resultsPerPage={MAX_RESULTS}");

        if let Some(kw) = keyword {
            let _ = write!(url, "&keywordSearch={kw}");
        }
        if let Some(cwe) = cwe_id {
            // NVD API 使用纯编号格式（如 CWE-89）
            let cwe_clean = cwe.trim_start_matches("CWE-");
            let _ = write!(url, "&cweId=CWE-{cwe_clean}");
        }

        let response = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| Error::Tool(format!("NVD API 请求失败：{e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            return Ok(format!(
                "NVD API 返回错误状态 {status}，可能是请求频率过高，请稍后重试"
            ));
        }

        let nvd_resp: NvdResponse = response
            .json()
            .await
            .map_err(|e| Error::Tool(format!("NVD 响应解析失败：{e}")))?;

        // 转换为简洁结果
        let cves: Vec<Value> = nvd_resp
            .vulnerabilities
            .iter()
            .map(|v| {
                let desc = v
                    .cve
                    .descriptions
                    .iter()
                    .find(|d| d.lang == "en")
                    .map(|d| d.value.as_str())
                    .unwrap_or_default();

                let (score, severity) = v
                    .cve
                    .metrics
                    .cvss_metric_v31
                    .first()
                    .map_or((0.0, "UNKNOWN"), |m| {
                        (m.cvss_data.base_score, m.cvss_data.base_severity.as_str())
                    });

                json!({
                    "cve_id": v.cve.id,
                    "description": desc,
                    "cvss_score": score,
                    "severity": severity,
                })
            })
            .collect();

        let result = json!({
            "total_results": nvd_resp.total_results,
            "returned": cves.len(),
            "cves": cves,
            "summary": format!(
                "NVD 查询到 {} 条结果（共 {} 条匹配）",
                cves.len(),
                nvd_resp.total_results
            )
        });

        serde_json::to_string_pretty(&result).map_err(|e| Error::Tool(e.to_string()))
    }
}
