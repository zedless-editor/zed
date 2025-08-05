use collections::HashMap;
use serde::Deserialize;
use serde_json::Value;
use util::ResultExt;

use crate::{TaskTemplate, TaskTemplates};

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct JustTaskParameters {
    name: String,
    default: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct JustRecipe {
    name: String,
    doc: Option<String>,
    parameters: Vec<JustTaskParameters>,
    #[serde(flatten)]
    other_attributes: HashMap<String, serde_json_lenient::Value>,
}

impl JustRecipe {
    fn into_zed_format(self, justfile_path: String) -> anyhow::Result<Option<TaskTemplate>> {
        for p in self.parameters {
            if p.default.is_none() {
                log::warn!(
                    "Skipping deserializing of just task `{}` with non-defaulted parameters",
                    self.name
                );
                return Ok(None);
            }
        }

        let template = TaskTemplate {
            label: self.doc.unwrap_or(format!("just {}", self.name)),
            command: "just".to_owned(),
            args: vec!["-f".to_string(), justfile_path, self.name],
            ..TaskTemplate::default()
        };
        Ok(Some(template))
    }
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct JustTaskDump {
    source: String,
    recipes: HashMap<String, JustRecipe>,
}

impl TryFrom<JustTaskDump> for TaskTemplates {
    type Error = anyhow::Error;

    fn try_from(value: JustTaskDump) -> Result<Self, Self::Error> {
        let templates = value
            .recipes
            .values()
            .into_iter()
            .filter_map(|just_recipe| {
                just_recipe
                    .clone()
                    .into_zed_format(value.source.clone())
                    .log_err()
                    .flatten()
            })
            .collect();
        Ok(Self(templates))
    }
}
