//! CWE 知识库查询工具 — 内嵌常见 CWE 条目，支持按 ID 或关键词检索。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::Error;
use crate::tools::Tool;

// —— 参数字段名 ——

const PARAM_QUERY: &str = "query";

/// CWE 知识库条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CweEntry {
    /// CWE 编号（如 "CWE-89"）
    pub id: String,
    /// 漏洞名称
    pub name: String,
    /// 漏洞描述
    pub description: String,
    /// 严重度
    pub severity: String,
    /// 修复建议
    pub remediation: String,
    /// 代码示例
    pub examples: Vec<String>,
}

/// CWE 知识库查询工具
pub struct CweKnowledgeBase {
    entries: Vec<CweEntry>,
}

impl CweKnowledgeBase {
    /// 使用内置知识库创建实例。
    pub fn new() -> Self {
        Self {
            entries: builtin_entries(),
        }
    }
}

/// 构建内置 CWE 知识库。
fn builtin_entries() -> Vec<CweEntry> {
    let mut entries = injection_entries();
    entries.extend(data_handling_entries());
    entries.extend(access_control_entries());
    entries
}

/// 注入类 CWE 条目（XSS、SQL 注入、命令注入、路径遍历）。
fn injection_entries() -> Vec<CweEntry> {
    vec![
        CweEntry {
            id: "CWE-79".into(),
            name: "跨站脚本（XSS）".into(),
            description: "应用程序在生成的网页中包含未经验证或未转义的用户输入，\
                          攻击者可注入恶意脚本在受害者浏览器中执行。"
                .into(),
            severity: "high".into(),
            remediation: "对所有输出到 HTML 的用户数据进行上下文敏感的转义编码；\
                         使用内容安全策略（CSP）作为深层防御；\
                         优先使用自动转义的模板引擎。"
                .into(),
            examples: vec![
                "document.innerHTML = userInput  // 危险".into(),
                "{{ user_input | escape }}  // 安全".into(),
            ],
        },
        CweEntry {
            id: "CWE-89".into(),
            name: "SQL 注入".into(),
            description: "应用程序将未经验证的用户输入直接拼接到 SQL 语句中，\
                          攻击者可篡改查询逻辑获取或修改数据库数据。"
                .into(),
            severity: "high".into(),
            remediation: "使用参数化查询（预编译语句）替代字符串拼接；\
                         使用 ORM 框架；\
                         对用户输入进行严格的白名单校验。"
                .into(),
            examples: vec![
                "cursor.execute(f\"SELECT * FROM users WHERE id={uid}\")  // 危险".into(),
                "cursor.execute(\"SELECT * FROM users WHERE id=?\", (uid,))  // 安全".into(),
            ],
        },
        CweEntry {
            id: "CWE-78".into(),
            name: "命令注入".into(),
            description: "应用程序将未经验证的用户输入拼接到操作系统命令中，\
                          攻击者可执行任意系统命令。"
                .into(),
            severity: "high".into(),
            remediation: "避免直接调用系统命令；\
                         如必须调用，使用参数列表而非字符串拼接；\
                         严格校验和转义用户输入。"
                .into(),
            examples: vec![
                "os.system(f\"ping {host}\")  // 危险".into(),
                "subprocess.run([\"ping\", host])  // 较安全".into(),
            ],
        },
        CweEntry {
            id: "CWE-22".into(),
            name: "路径遍历".into(),
            description: "应用程序使用用户输入构造文件路径而未充分验证，\
                          攻击者可通过 `../` 等序列访问受限目录之外的文件。"
                .into(),
            severity: "high".into(),
            remediation: "对用户提供的文件名进行规范化并验证其在允许的基目录内；\
                         使用白名单限制可访问的文件；\
                         避免直接将用户输入拼接到文件路径中。"
                .into(),
            examples: vec![
                "open(\"/data/\" + filename)  // 危险：filename 可含 ../".into(),
                "检查 realpath 是否仍在基目录内  // 安全".into(),
            ],
        },
    ]
}

/// 凭据、序列化与网络请求类 CWE 条目。
fn data_handling_entries() -> Vec<CweEntry> {
    vec![
        CweEntry {
            id: "CWE-798".into(),
            name: "硬编码凭据".into(),
            description: "程序源代码中直接包含密码、密钥等凭据信息，\
                          泄露代码即泄露凭据。"
                .into(),
            severity: "medium".into(),
            remediation: "将凭据存储在环境变量或密钥管理服务中；\
                         使用配置文件（不纳入版本控制）管理敏感信息；\
                         定期轮换凭据。"
                .into(),
            examples: vec![
                "password = \"admin123\"  // 危险".into(),
                "password = os.environ[\"DB_PASSWORD\"]  // 安全".into(),
            ],
        },
        CweEntry {
            id: "CWE-502".into(),
            name: "不安全的反序列化".into(),
            description: "应用程序对不可信数据进行反序列化，\
                          攻击者可构造恶意数据实现任意代码执行或拒绝服务。"
                .into(),
            severity: "high".into(),
            remediation: "避免反序列化不可信数据；\
                         使用安全的序列化格式（如 JSON）替代 pickle/marshal；\
                         如必须使用，添加签名验证和类型白名单。"
                .into(),
            examples: vec![
                "pickle.loads(user_data)  // 危险".into(),
                "json.loads(user_data)  // 较安全".into(),
            ],
        },
        CweEntry {
            id: "CWE-918".into(),
            name: "服务端请求伪造（SSRF）".into(),
            description: "应用程序根据用户输入发起服务端 HTTP 请求，\
                          攻击者可令服务器访问内部网络资源或敏感端点。"
                .into(),
            severity: "high".into(),
            remediation: "使用 URL 白名单限制可请求的目标；\
                         禁止请求私有 IP 地址范围；\
                         在网络层进行出站流量过滤。"
                .into(),
            examples: vec![
                "requests.get(user_url)  // 危险".into(),
                "验证 URL 协议和主机后再请求  // 安全".into(),
            ],
        },
    ]
}

/// 认证、授权与信息泄露类 CWE 条目。
fn access_control_entries() -> Vec<CweEntry> {
    vec![
        CweEntry {
            id: "CWE-287".into(),
            name: "认证缺陷".into(),
            description: "应用程序的认证机制存在缺陷，\
                          攻击者可绕过身份验证以其他用户身份访问系统。"
                .into(),
            severity: "high".into(),
            remediation: "使用成熟的认证框架而非自行实现；\
                         实施多因素认证；\
                         对认证失败实施速率限制和账户锁定。"
                .into(),
            examples: vec![
                "仅依赖客户端 cookie 中的用户名判断身份  // 危险".into(),
                "使用签名的 JWT/session 并在服务端验证  // 安全".into(),
            ],
        },
        CweEntry {
            id: "CWE-862".into(),
            name: "授权缺失".into(),
            description: "应用程序未对关键操作进行授权检查，\
                          低权限用户可执行高权限操作。"
                .into(),
            severity: "high".into(),
            remediation: "对每个敏感操作实施服务端授权检查；\
                         采用基于角色的访问控制（RBAC）；\
                         默认拒绝访问，仅显式授权。"
                .into(),
            examples: vec![
                "直接通过 URL 参数中的 user_id 访问数据  // 危险".into(),
                "验证当前会话用户是否有权访问目标资源  // 安全".into(),
            ],
        },
        CweEntry {
            id: "CWE-200".into(),
            name: "信息泄露".into(),
            description: "应用程序向未授权的用户暴露了敏感信息，\
                          如错误堆栈、内部路径、配置详情等。"
                .into(),
            severity: "medium".into(),
            remediation: "在生产环境禁用详细错误信息；\
                         使用统一的错误处理返回通用错误页面；\
                         记录详细错误到服务端日志而非返回给客户端。"
                .into(),
            examples: vec![
                "返回完整的异常堆栈给客户端  // 危险".into(),
                "记录错误日志，返回通用错误码  // 安全".into(),
            ],
        },
    ]
}

#[async_trait]
impl Tool for CweKnowledgeBase {
    fn name(&self) -> &'static str {
        "cwe_knowledge_base"
    }

    fn description(&self) -> &'static str {
        "查询 CWE 漏洞分类知识库，获取漏洞描述、严重度和修复建议"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                PARAM_QUERY: {
                    "type": "string",
                    "description": "CWE 编号（如 CWE-89）或关键词（如 SQL 注入）"
                }
            },
            "required": [PARAM_QUERY]
        })
    }

    async fn execute(&self, params: &Value) -> Result<String, Error> {
        let query = params
            .get(PARAM_QUERY)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Tool("缺少 query 参数".into()))?;

        let query_lower = query.to_lowercase();

        let matched: Vec<&CweEntry> = self
            .entries
            .iter()
            .filter(|e| {
                e.id.to_lowercase().contains(&query_lower)
                    || e.name.to_lowercase().contains(&query_lower)
                    || e.description.to_lowercase().contains(&query_lower)
            })
            .collect();

        if matched.is_empty() {
            return Ok(json!({
                "found": 0,
                "message": format!("未找到与 '{query}' 匹配的 CWE 条目")
            })
            .to_string());
        }

        let result = json!({
            "found": matched.len(),
            "entries": matched
        });

        serde_json::to_string_pretty(&result).map_err(|e| Error::Tool(e.to_string()))
    }
}
