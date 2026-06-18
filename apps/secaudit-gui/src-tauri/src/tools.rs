use std::path::Path;
use std::sync::Arc;

use secaudit_agent::{Agent, ToolDefinition, tools::default_tools};
use serde_json::Value;

use crate::dto::{ToolCapability, ToolParameter, ToolParameterKey};

struct ToolMetadata {
    name: &'static str,
    category: &'static str,
    risk: &'static str,
    description: &'static str,
}

const TOOL_CATALOG: &[ToolMetadata] = &[
    ToolMetadata {
        name: "read_file",
        category: "文件",
        risk: "只读",
        description: "读取工作区内文件内容。",
    },
    ToolMetadata {
        name: "list_directory",
        category: "文件",
        risk: "只读",
        description: "列出目录结构，理解项目边界。",
    },
    ToolMetadata {
        name: "search_content",
        category: "检索",
        risk: "只读",
        description: "搜索工作区内容，定位入口和敏感调用。",
    },
    ToolMetadata {
        name: "find_files",
        category: "检索",
        risk: "只读",
        description: "按名称或模式定位审计目标文件。",
    },
    ToolMetadata {
        name: "semgrep_scanner",
        category: "安全扫描",
        risk: "只读",
        description: "运行静态规则扫描，定位常见漏洞模式。",
    },
    ToolMetadata {
        name: "dependency_checker",
        category: "依赖",
        risk: "只读",
        description: "识别依赖清单和供应链风险。",
    },
    ToolMetadata {
        name: "nvd_lookup",
        category: "情报",
        risk: "网络",
        description: "查询 CVE/NVD 信息以补充漏洞上下文。",
    },
    ToolMetadata {
        name: "execute_command",
        category: "命令",
        risk: "需确认",
        description: "执行受控命令，用于补充审计证据。",
    },
    ToolMetadata {
        name: "write_file",
        category: "文件",
        risk: "需确认",
        description: "写入修复补丁，默认需要用户确认。",
    },
    ToolMetadata {
        name: "use_skill",
        category: "Skill",
        risk: "只读",
        description: "激活预设 Skill，将完整指令注入本轮上下文。",
    },
];

const FALLBACK_TOOL: ToolMetadata = ToolMetadata {
    name: "",
    category: "工具",
    risk: "按需",
    description: "Agent 可调用的工作区工具。",
};

pub(crate) fn tool_capabilities(agent: Option<&Agent>, work_dir: &Path) -> Vec<ToolCapability> {
    let definitions = agent.map_or_else(
        || {
            let confirm = Arc::new(|_prompt: &str| false);
            default_tools(work_dir.to_path_buf(), confirm)
                .iter()
                .map(|tool| ToolDefinition {
                    name: tool.name().into_owned(),
                    description: tool.description().into_owned(),
                    parameters: tool.parameters_schema(),
                })
                .collect()
        },
        Agent::tool_definitions,
    );

    definitions.into_iter().map(tool_capability).collect()
}

fn tool_capability(definition: ToolDefinition) -> ToolCapability {
    let metadata = TOOL_CATALOG
        .iter()
        .find(|metadata| metadata.name == definition.name)
        .unwrap_or(&FALLBACK_TOOL);

    ToolCapability {
        name: definition.name,
        category: metadata.category.to_owned(),
        risk: metadata.risk.to_owned(),
        description: metadata.description.to_owned(),
        parameters: tool_parameters(&definition.parameters),
    }
}

fn tool_parameters(schema: &Value) -> Vec<ToolParameter> {
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();

    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| {
            properties
                .iter()
                .map(|(name, property)| ToolParameter {
                    key: ToolParameterKey::for_name(name),
                    name: name.clone(),
                    label: ToolParameterKey::for_name(name).label().to_owned(),
                    description: property_description(property),
                    type_name: property_type_name(property),
                    required: required.iter().any(|item| *item == name),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn property_description(property: &Value) -> String {
    property
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn property_type_name(property: &Value) -> String {
    match property.get("type") {
        Some(Value::String(type_name)) => type_name.clone(),
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" | "),
        _ => "unknown".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use secaudit_agent::ToolDefinition;
    use serde_json::json;

    use crate::dto::ToolParameterKey;

    use super::{tool_capabilities, tool_capability};

    #[test]
    fn tool_schema_parameters_are_known_to_gui_projection() {
        let tools = tool_capabilities(None, Path::new("."));
        let unknown_parameters: Vec<String> = tools
            .iter()
            .flat_map(|tool| {
                tool.parameters
                    .iter()
                    .filter(|parameter| parameter.key == ToolParameterKey::Other)
                    .map(|parameter| format!("{}:{}", tool.name, parameter.name))
            })
            .collect();

        assert!(
            unknown_parameters.is_empty(),
            "GUI 工具参数映射缺失：{}",
            unknown_parameters.join(", ")
        );
    }

    #[test]
    fn semgrep_project_path_gets_readable_label() -> Result<(), String> {
        let tools = tool_capabilities(None, Path::new("."));
        let Some(semgrep) = tools.iter().find(|tool| tool.name == "semgrep_scanner") else {
            return Err("semgrep_scanner should be present".to_owned());
        };
        let Some(project_path) = semgrep
            .parameters
            .iter()
            .find(|parameter| parameter.name == "project_path")
        else {
            return Err("project_path should be present".to_owned());
        };

        assert_eq!(project_path.key, ToolParameterKey::ProjectPath);
        assert_eq!(project_path.label, "项目路径");
        assert!(project_path.required, "project_path should be required");
        Ok(())
    }

    #[test]
    fn use_skill_tool_gets_readable_gui_metadata() -> Result<(), String> {
        let capability = tool_capability(ToolDefinition {
            name: "use_skill".to_owned(),
            description: "Activate a predefined skill".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "skill_name": {
                        "type": "string",
                        "description": "Name of the skill to activate"
                    },
                    "arguments": {
                        "type": "string",
                        "description": "Optional arguments to pass to the skill"
                    }
                },
                "required": ["skill_name"]
            }),
        });

        assert_eq!(capability.category, "Skill");
        assert_eq!(capability.risk, "只读");
        let Some(skill_name) = capability
            .parameters
            .iter()
            .find(|parameter| parameter.name == "skill_name")
        else {
            return Err("skill_name should be present".to_owned());
        };
        let Some(arguments) = capability
            .parameters
            .iter()
            .find(|parameter| parameter.name == "arguments")
        else {
            return Err("arguments should be present".to_owned());
        };

        assert_eq!(skill_name.key, ToolParameterKey::SkillName);
        assert_eq!(skill_name.label, "Skill");
        assert!(skill_name.required, "skill_name should be required");
        assert_eq!(arguments.key, ToolParameterKey::Arguments);
        assert_eq!(arguments.label, "参数");
        assert!(!arguments.required, "arguments should be optional");
        Ok(())
    }
}
