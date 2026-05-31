use std::borrow::Cow;
use std::sync::Arc;

use async_trait::async_trait;
use secaudit_skills::SkillRegistry;
use serde_json::{Value, json};

use crate::error::Error;
use crate::tools::Tool;

pub(crate) struct UseSkillTool {
    registry: Arc<SkillRegistry>,
}

impl UseSkillTool {
    pub(crate) fn new(registry: Arc<SkillRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for UseSkillTool {
    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed("use_skill")
    }

    fn description(&self) -> Cow<'_, str> {
        let skills = self.registry.list();
        if skills.is_empty() {
            return Cow::Borrowed(
                "Activate a predefined skill by name. No skills are currently available.",
            );
        }
        let items: Vec<String> = skills
            .iter()
            .map(|(name, desc)| format!("- {name}: {desc}"))
            .collect();
        Cow::Owned(format!(
            "Activate a predefined skill by name. When called, the skill's full instructions \
             are returned and must be followed carefully.\n\nAvailable skills:\n{}",
            items.join("\n")
        ))
    }

    fn parameters_schema(&self) -> Value {
        let skill_names: Vec<Value> = self
            .registry
            .list()
            .iter()
            .map(|(name, _)| json!(name))
            .collect();
        json!({
            "type": "object",
            "properties": {
                "skill_name": {
                    "type": "string",
                    "description": "Name of the skill to activate",
                    "enum": skill_names
                },
                "arguments": {
                    "type": "string",
                    "description": "Optional arguments to pass to the skill"
                }
            },
            "required": ["skill_name"]
        })
    }

    async fn execute(&self, params: &Value) -> Result<String, Error> {
        let skill_name = params
            .get("skill_name")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Tool("缺少必要参数 skill_name".into()))?;

        let arguments = params
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or("");

        let skill = self.registry.get(skill_name).ok_or_else(|| {
            let available: Vec<String> = self.registry.list().into_iter().map(|(n, _)| n).collect();
            Error::Tool(format!(
                "Skill「{skill_name}」未找到。可用 Skills：{}",
                available.join(", ")
            ))
        })?;

        let user_input = if arguments.is_empty() {
            format!("/{skill_name}")
        } else {
            format!("/{skill_name} {arguments}")
        };

        let expanded = skill.build_prompt(&user_input, "");

        Ok(expanded)
    }
}
