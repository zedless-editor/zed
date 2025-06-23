use std::sync::Arc;

use anyhow::Result;
use gpui::App;
use project::Fs;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings::{Settings, SettingsSources, update_settings_file};

use crate::provider::{
    self,
    ollama::OllamaSettings,
    open_ai::OpenAiSettings,
};

/// Initializes the language model settings.
pub fn init(fs: Arc<dyn Fs>, cx: &mut App) {
    AllLanguageModelSettings::register(cx);

    if AllLanguageModelSettings::get_global(cx)
        .openai
        .needs_setting_migration
    {
        update_settings_file::<AllLanguageModelSettings>(fs.clone(), cx, move |setting, _| {
            if let Some(settings) = setting.openai.clone() {
                let (newest_version, _) = settings.upgrade();
                setting.openai = Some(OpenAiSettingsContent::Versioned(
                    VersionedOpenAiSettingsContent::V1(newest_version),
                ));
            }
        });
    }
}

#[derive(Default)]
pub struct AllLanguageModelSettings {
    pub ollama: OllamaSettings,
    pub openai: OpenAiSettings,
}

#[derive(Default, Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema)]
pub struct AllLanguageModelSettingsContent {
    pub ollama: Option<OllamaSettingsContent>,
    pub openai: Option<OpenAiSettingsContent>,
}

#[derive(Default, Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema)]
pub struct OllamaSettingsContent {
    pub api_url: Option<String>,
    pub available_models: Option<Vec<provider::ollama::AvailableModel>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema)]
#[serde(untagged)]
pub enum OpenAiSettingsContent {
    Versioned(VersionedOpenAiSettingsContent),
    Legacy(LegacyOpenAiSettingsContent),
}

impl OpenAiSettingsContent {
    pub fn upgrade(self) -> (OpenAiSettingsContentV1, bool) {
        match self {
            OpenAiSettingsContent::Legacy(content) => (
                OpenAiSettingsContentV1 {
                    api_url: content.api_url,
                    available_models: content.available_models.map(|models| {
                        models
                            .into_iter()
                            .filter_map(|model| match model {
                                open_ai::Model::Custom {
                                    name,
                                    display_name,
                                    max_tokens,
                                    max_output_tokens,
                                    max_completion_tokens,
                                } => Some(provider::open_ai::AvailableModel {
                                    name,
                                    max_tokens,
                                    max_output_tokens,
                                    display_name,
                                    max_completion_tokens,
                                }),
                                _ => None,
                            })
                            .collect()
                    }),
                },
                true,
            ),
            OpenAiSettingsContent::Versioned(content) => match content {
                VersionedOpenAiSettingsContent::V1(content) => (content, false),
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema)]
pub struct LegacyOpenAiSettingsContent {
    pub api_url: Option<String>,
    pub available_models: Option<Vec<open_ai::Model>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema)]
#[serde(tag = "version")]
pub enum VersionedOpenAiSettingsContent {
    #[serde(rename = "1")]
    V1(OpenAiSettingsContentV1),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema)]
pub struct OpenAiSettingsContentV1 {
    pub api_url: Option<String>,
    pub available_models: Option<Vec<provider::open_ai::AvailableModel>>,
}

impl settings::Settings for AllLanguageModelSettings {
    const KEY: Option<&'static str> = Some("language_models");

    const PRESERVED_KEYS: Option<&'static [&'static str]> = Some(&["version"]);

    type FileContent = AllLanguageModelSettingsContent;

    fn load(sources: SettingsSources<Self::FileContent>, _: &mut App) -> Result<Self> {
        fn merge<T>(target: &mut T, value: Option<T>) {
            if let Some(value) = value {
                *target = value;
            }
        }

        let mut settings = AllLanguageModelSettings::default();

        for value in sources.defaults_and_customizations() {
            // Ollama
            let ollama = value.ollama.clone();

            merge(
                &mut settings.ollama.api_url,
                value.ollama.as_ref().and_then(|s| s.api_url.clone()),
            );
            merge(
                &mut settings.ollama.available_models,
                ollama.as_ref().and_then(|s| s.available_models.clone()),
            );

            // OpenAI
            let (openai, upgraded) = match value.openai.clone().map(|s| s.upgrade()) {
                Some((content, upgraded)) => (Some(content), upgraded),
                None => (None, false),
            };

            if upgraded {
                settings.openai.needs_setting_migration = true;
            }

            merge(
                &mut settings.openai.api_url,
                openai.as_ref().and_then(|s| s.api_url.clone()),
            );
            merge(
                &mut settings.openai.available_models,
                openai.as_ref().and_then(|s| s.available_models.clone()),
            );
        }

        Ok(settings)
    }

    fn import_from_vscode(_vscode: &settings::VsCodeSettings, _current: &mut Self::FileContent) {}
}
