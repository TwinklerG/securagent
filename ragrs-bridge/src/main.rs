// CLI 工具需要通过 stdout 输出评估结果
#![allow(clippy::print_stdout)]

//! **ragrs-bridge** — 评估桥接 CLI 工具。
//!
//! 从 stdin 读取 JSON 输入，执行指定指标评估，将结果以 JSON 输出到 stdout。
//!
//! # 输入格式
//!
//! ```json
//! {
//!   "sample": { "user_input": [...], "reference": "...", "reference_tool_calls": [...] },
//!   "metrics": ["vulnerability_detection_rate", "false_positive_rate"],
//!   "llm_config": { "api_base_url": "...", "api_key": "...", "model": "..." }
//! }
//! ```

use std::collections::HashMap;
use std::error::Error as StdError;
use std::io::{self, Read as _};
use std::sync::Arc;

use clap::Parser;
use serde::{Deserialize, Serialize};

use llm_common::{HttpLlmClient, LlmConfig};
use ragrs::{
    AgentGoalAccuracy, CustomLlmJudge, CweClassificationAccuracy, Error, FalsePositiveRate,
    HarmfulContentRate, LlmClient, Metric, MultiTurnSample, PromptInjectionResistance,
    ResponseLatency, SeverityAccuracy, StepCount, TokenEfficiency, ToolCallAccuracy,
    TopicAdherence, VulnerabilityDetectionRate,
};

// ── CLI 参数 ─────────────────────────────────────────────────────────────────

/// ragrs-bridge: 安全评估桥接 CLI
#[derive(Parser)]
#[command(
    name = "ragrs-bridge",
    version,
    about = "从 stdin 读取评估请求，输出 JSON 结果"
)]
struct Cli {
    /// 启用详细日志输出
    #[arg(short, long, default_value_t = false)]
    verbose: bool,
}

// ── 输入/输出类型 ────────────────────────────────────────────────────────────

/// 从 stdin 读入的评估请求。
#[derive(Debug, Deserialize)]
struct EvalRequest {
    /// 待评估的多轮对话样本
    sample: MultiTurnSample,

    /// 要执行的指标名称列表
    metrics: Vec<String>,

    /// LLM 配置（仅 LLM-as-Judge 指标需要）
    llm_config: Option<LlmConfig>,

    /// 自定义指标定义列表（平台传入）
    custom_metrics: Option<Vec<CustomMetricDef>>,
}

/// 自定义指标定义。
#[derive(Debug, Deserialize)]
struct CustomMetricDef {
    /// 指标名称
    name: String,
    /// prompt 模板（含 `{conversation}` 和 `{reference}` 占位符）
    prompt_template: String,
}

/// 输出到 stdout 的评估结果。
#[derive(Debug, Serialize)]
struct EvalResponse {
    /// 各指标评分
    scores: HashMap<String, f64>,
}

// ── 指标注册 ─────────────────────────────────────────────────────────────────

/// 所有支持的指标名称。
const SUPPORTED_METRICS: &[&str] = &[
    "vulnerability_detection_rate",
    "false_positive_rate",
    "cwe_classification_accuracy",
    "severity_accuracy",
    "tool_call_accuracy",
    "agent_goal_accuracy",
    "topic_adherence",
    "harmful_content_rate",
    "prompt_injection_resistance",
    "step_count",
    "token_efficiency",
    "response_latency",
];

/// 根据名称构建指标实例。
fn build_metric(
    name: &str,
    llm: Option<&Arc<dyn LlmClient>>,
    custom_metrics: Option<&[CustomMetricDef]>,
) -> Result<Box<dyn Metric<MultiTurnSample>>, Error> {
    match name {
        "vulnerability_detection_rate" => Ok(Box::new(VulnerabilityDetectionRate::new())),
        "false_positive_rate" => Ok(Box::new(FalsePositiveRate::new())),
        "cwe_classification_accuracy" => Ok(Box::new(CweClassificationAccuracy::new())),
        "severity_accuracy" => {
            let llm =
                llm.ok_or_else(|| Error::Llm("severity_accuracy 需要 llm_config".to_owned()))?;
            Ok(Box::new(SeverityAccuracy::new(Arc::clone(llm))))
        }
        "tool_call_accuracy" => {
            let llm =
                llm.ok_or_else(|| Error::Llm("tool_call_accuracy 需要 llm_config".to_owned()))?;
            Ok(Box::new(ToolCallAccuracy::new(Arc::clone(llm))))
        }
        "agent_goal_accuracy" => {
            let llm =
                llm.ok_or_else(|| Error::Llm("agent_goal_accuracy 需要 llm_config".to_owned()))?;
            Ok(Box::new(AgentGoalAccuracy::new(Arc::clone(llm))))
        }
        "topic_adherence" => {
            let llm =
                llm.ok_or_else(|| Error::Llm("topic_adherence 需要 llm_config".to_owned()))?;
            Ok(Box::new(TopicAdherence::new(Arc::clone(llm))))
        }
        "harmful_content_rate" => {
            let llm =
                llm.ok_or_else(|| Error::Llm("harmful_content_rate 需要 llm_config".to_owned()))?;
            Ok(Box::new(HarmfulContentRate::new(Arc::clone(llm))))
        }
        "prompt_injection_resistance" => {
            let llm = llm.ok_or_else(|| {
                Error::Llm("prompt_injection_resistance 需要 llm_config".to_owned())
            })?;
            Ok(Box::new(PromptInjectionResistance::new(Arc::clone(llm))))
        }
        "step_count" => Ok(Box::new(StepCount::new())),
        "token_efficiency" => Ok(Box::new(TokenEfficiency::new())),
        "response_latency" => Ok(Box::new(ResponseLatency::new())),
        _ => {
            // 尝试从自定义指标中查找
            if let Some(customs) = custom_metrics
                && let Some(def) = customs.iter().find(|c| c.name == name)
            {
                let llm =
                    llm.ok_or_else(|| Error::Llm(format!("自定义指标 {name} 需要 llm_config")))?;
                return Ok(Box::new(CustomLlmJudge::new(
                    def.name.clone(),
                    def.prompt_template.clone(),
                    Arc::clone(llm),
                )));
            }
            Err(Error::Parse(format!(
                "未知指标：{name}，支持：{SUPPORTED_METRICS:?}"
            )))
        }
    }
}

// ── 入口 ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn StdError>> {
    let _cli = Cli::parse();

    // 从 stdin 读取全部输入
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;

    let request: EvalRequest =
        serde_json::from_str(&input).map_err(|e| format!("输入 JSON 解析失败：{e}"))?;

    // 按需创建 LLM 客户端
    let llm: Option<Arc<dyn LlmClient>> = request
        .llm_config
        .as_ref()
        .map(|cfg| Arc::new(HttpLlmClient::new(cfg)) as Arc<dyn LlmClient>);

    // 构建指标列表
    let custom_metrics = request.custom_metrics.as_deref();
    let metrics: Vec<Box<dyn Metric<MultiTurnSample>>> = request
        .metrics
        .iter()
        .map(|name| build_metric(name, llm.as_ref(), custom_metrics))
        .collect::<Result<Vec<_>, _>>()?;

    let metric_refs: Vec<&dyn Metric<MultiTurnSample>> =
        metrics.iter().map(AsRef::as_ref).collect();

    // 执行评估
    let scores = ragrs::evaluate(&request.sample, &metric_refs).await?;

    // 输出结果
    let response = EvalResponse { scores };
    let output = serde_json::to_string(&response)?;
    println!("{output}");

    Ok(())
}
