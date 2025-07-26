use crate::AgentServerCommand;
use anyhow::Result;
use gpui::App;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings::{Settings, SettingsSources};

pub fn init(cx: &mut App) {
    AllAgentServersSettings::register(cx);
}

#[derive(Default, Deserialize, Serialize, Clone, JsonSchema, Debug)]
pub struct AllAgentServersSettings {
}

#[derive(Deserialize, Serialize, Clone, JsonSchema, Debug)]
pub struct AgentServerSettings {
    #[serde(flatten)]
    pub command: AgentServerCommand,
}

impl settings::Settings for AllAgentServersSettings {
    const KEY: Option<&'static str> = Some("agent_servers");

    type FileContent = Self;

    fn load(sources: SettingsSources<Self::FileContent>, _: &mut App) -> Result<Self> {
        let settings = AllAgentServersSettings::default();

        for AllAgentServersSettings { } in sources.defaults_and_customizations() {
        }

        Ok(settings)
    }

    fn import_from_vscode(_vscode: &settings::VsCodeSettings, _current: &mut Self::FileContent) {}
}
