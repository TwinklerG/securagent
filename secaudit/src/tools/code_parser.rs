//! 代码解析工具 — 提取源代码结构信息（函数定义、用户输入点、敏感 API 调用）。

use async_trait::async_trait;
use regex::Regex;
use serde_json::{Value, json};

use crate::error::Error;
use crate::tools::Tool;

// —— 参数字段名 ——

const PARAM_CODE: &str = "code";
const PARAM_LANGUAGE: &str = "language";

// —— 支持的语言标识 ——

const LANG_PYTHON: &str = "python";
const LANG_JAVASCRIPT: &str = "javascript";
const LANG_RUST: &str = "rust";

// —— Python 模式 ——

const PY_FUNC_PATTERN: &str = r"(?m)^\s*def\s+(\w+)\s*\(([^)]*)\)";
const PY_INPUT_PATTERNS: &[&str] = &[
    r"input\s*\(",
    r"request\.(get|post|args|form|json|data|files|cookies|headers)",
    r"sys\.argv",
    r"os\.environ",
];
const PY_SENSITIVE_PATTERNS: &[&str] = &[
    r"eval\s*\(",
    r"exec\s*\(",
    r"os\.system\s*\(",
    r"subprocess\.(call|run|Popen)\s*\(",
    r"cursor\.execute\s*\(",
    r"pickle\.loads?\s*\(",
    r"yaml\.load\s*\(",
    r"open\s*\(",
];

// —— JavaScript 模式 ——

const JS_FUNC_PATTERN: &str = r"(?m)(?:function\s+(\w+)\s*\(([^)]*)\)|(?:const|let|var)\s+(\w+)\s*=\s*(?:async\s+)?\([^)]*\)\s*=>)";
const JS_INPUT_PATTERNS: &[&str] = &[
    r"req\.(query|params|body|headers|cookies)",
    r"process\.argv",
    r"document\.(getElementById|querySelector|forms)",
    r"window\.location",
];
const JS_SENSITIVE_PATTERNS: &[&str] = &[
    r"eval\s*\(",
    r"exec\s*\(",
    r"\.innerHTML\s*=",
    r"document\.write\s*\(",
    "child_process",
    r"\.query\s*\(",
    r"fs\.(readFile|writeFile|unlink)",
];

// —— Rust 模式 ——

const RS_FUNC_PATTERN: &str =
    r"(?m)(?:pub\s+)?(?:async\s+)?fn\s+(\w+)\s*(?:<[^>]*>)?\s*\(([^)]*)\)";
const RS_INPUT_PATTERNS: &[&str] = &[
    "std::env::(args|vars)",
    "std::io::stdin",
    "actix_web|axum|rocket",
];
const RS_SENSITIVE_PATTERNS: &[&str] = &[
    r"Command::new\s*\(",
    "std::process::Command",
    r"unsafe\s*\{",
    "std::ptr",
    "libc::",
    "std::fs::(read|write|remove)",
];

/// 代码解析工具
pub struct CodeParser;

impl CodeParser {
    pub const fn new() -> Self {
        Self
    }
}

/// 对源代码运行一组正则模式，收集所有匹配项。
fn collect_matches(code: &str, patterns: &[&str]) -> Vec<String> {
    patterns
        .iter()
        .filter_map(|p| Regex::new(p).ok())
        .flat_map(|re| {
            re.find_iter(code)
                .map(|m| m.as_str().to_owned())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// 提取函数定义名称列表。
fn extract_functions(code: &str, pattern: &str) -> Vec<String> {
    Regex::new(pattern).ok().map_or_else(Vec::new, |re| {
        re.captures_iter(code)
            .filter_map(|cap| {
                (1..cap.len()).find_map(|i| cap.get(i).map(|m| m.as_str().to_owned()))
            })
            .collect()
    })
}

#[async_trait]
impl Tool for CodeParser {
    fn name(&self) -> &'static str {
        "code_parser"
    }

    fn description(&self) -> &'static str {
        "解析源代码结构，提取函数定义、用户输入点、敏感 API 调用等关键信息"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                PARAM_CODE: {
                    "type": "string",
                    "description": "待解析的源代码"
                },
                PARAM_LANGUAGE: {
                    "type": "string",
                    "description": "编程语言（python/javascript/rust）"
                }
            },
            "required": [PARAM_CODE, PARAM_LANGUAGE]
        })
    }

    async fn execute(&self, params: &Value) -> Result<String, Error> {
        let code = params
            .get(PARAM_CODE)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Tool("缺少 code 参数".into()))?;

        let language = params
            .get(PARAM_LANGUAGE)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Tool("缺少 language 参数".into()))?;

        let (func_pattern, input_patterns, sensitive_patterns) = match language {
            LANG_PYTHON => (PY_FUNC_PATTERN, PY_INPUT_PATTERNS, PY_SENSITIVE_PATTERNS),
            LANG_JAVASCRIPT => (JS_FUNC_PATTERN, JS_INPUT_PATTERNS, JS_SENSITIVE_PATTERNS),
            LANG_RUST => (RS_FUNC_PATTERN, RS_INPUT_PATTERNS, RS_SENSITIVE_PATTERNS),
            other => {
                return Err(Error::Tool(format!("不支持的语言：{other}")));
            }
        };

        let functions = extract_functions(code, func_pattern);
        let input_points = collect_matches(code, input_patterns);
        let sensitive_calls = collect_matches(code, sensitive_patterns);

        let result = json!({
            "language": language,
            "functions": functions,
            "input_points": input_points,
            "sensitive_calls": sensitive_calls,
            "summary": format!(
                "发现 {} 个函数, {} 个用户输入点, {} 个敏感调用",
                functions.len(),
                input_points.len(),
                sensitive_calls.len()
            )
        });

        serde_json::to_string_pretty(&result).map_err(|e| Error::Tool(e.to_string()))
    }
}
