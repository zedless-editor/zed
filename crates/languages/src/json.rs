use anyhow::{Result};
use async_trait::async_trait;
use collections::HashMap;
use dap::DapRegistry;
use futures::StreamExt;
use gpui::{App, AsyncApp, Task};
use language::{
    ContextProvider, LanguageRegistry, LanguageToolchainStore, LocalFile as _, LspAdapter,
    LspAdapterDelegate,
};
use lsp::{LanguageServerBinary, LanguageServerName};
use project::{Fs, lsp_store::language_server_settings};
use serde_json::{Value, json};
use settings::{KeymapFile, SettingsJsonSchemaParams, SettingsStore};
use smol::{
    lock::RwLock,
};
use std::{
    path::{Path},
    str::FromStr,
    sync::Arc,
};
use task::{AdapterSchemas, TaskTemplate, TaskTemplates, VariableName};
use util::merge_json_value_into;

use crate::PackageJsonData;

// Origin: https://github.com/SchemaStore/schemastore
const TSCONFIG_SCHEMA: &str = include_str!("json/schemas/tsconfig.json");
const PACKAGE_JSON_SCHEMA: &str = include_str!("json/schemas/package.json");

pub(crate) struct JsonTaskProvider;

impl ContextProvider for JsonTaskProvider {
    fn associated_tasks(
        &self,
        _: Arc<dyn Fs>,
        file: Option<Arc<dyn language::File>>,
        cx: &App,
    ) -> gpui::Task<Option<TaskTemplates>> {
        let Some(file) = project::File::from_dyn(file.as_ref()).cloned() else {
            return Task::ready(None);
        };
        let is_package_json = file.path.ends_with("package.json");
        let is_composer_json = file.path.ends_with("composer.json");
        if !is_package_json && !is_composer_json {
            return Task::ready(None);
        }

        cx.spawn(async move |cx| {
            let contents = file
                .worktree
                .update(cx, |this, cx| this.load_file(&file.path, cx))
                .ok()?
                .await
                .ok()?;
            let path = cx.update(|cx| file.abs_path(cx)).ok()?.as_path().into();

            let task_templates = if is_package_json {
                let package_json = serde_json_lenient::from_str::<
                    HashMap<String, serde_json_lenient::Value>,
                >(&contents.text)
                .ok()?;
                let package_json = PackageJsonData::new(path, package_json);
                let command = package_json.package_manager.unwrap_or("npm").to_owned();
                package_json
                    .scripts
                    .into_iter()
                    .map(|(_, key)| TaskTemplate {
                        label: format!("run {key}"),
                        command: command.clone(),
                        args: vec!["run".into(), key],
                        cwd: Some(VariableName::Dirname.template_value()),
                        ..TaskTemplate::default()
                    })
                    .chain([TaskTemplate {
                        label: "package script $ZED_CUSTOM_script".to_owned(),
                        command: command.clone(),
                        args: vec![
                            "run".into(),
                            VariableName::Custom("script".into()).template_value(),
                        ],
                        cwd: Some(VariableName::Dirname.template_value()),
                        tags: vec!["package-script".into()],
                        ..TaskTemplate::default()
                    }])
                    .collect()
            } else if is_composer_json {
                serde_json_lenient::Value::from_str(&contents.text)
                    .ok()?
                    .get("scripts")?
                    .as_object()?
                    .keys()
                    .map(|key| TaskTemplate {
                        label: format!("run {key}"),
                        command: "composer".to_owned(),
                        args: vec!["-d".into(), "$ZED_DIRNAME".into(), key.into()],
                        ..TaskTemplate::default()
                    })
                    .chain([TaskTemplate {
                        label: "composer script $ZED_CUSTOM_script".to_owned(),
                        command: "composer".to_owned(),
                        args: vec![
                            "-d".into(),
                            "$ZED_DIRNAME".into(),
                            VariableName::Custom("script".into()).template_value(),
                        ],
                        tags: vec!["composer-script".into()],
                        ..TaskTemplate::default()
                    }])
                    .collect()
            } else {
                vec![]
            };

            Some(TaskTemplates(task_templates))
        })
    }
}

pub struct JsonLspAdapter {
    languages: Arc<LanguageRegistry>,
    workspace_config: RwLock<Option<Value>>,
}

impl JsonLspAdapter {
    pub fn new(languages: Arc<LanguageRegistry>) -> Self {
        Self {
            languages,
            workspace_config: Default::default(),
        }
    }

    fn get_workspace_config(
        language_names: Vec<String>,
        adapter_schemas: AdapterSchemas,
        cx: &mut App,
    ) -> Value {
        let keymap_schema = KeymapFile::generate_json_schema_for_registered_actions(cx);
        let font_names = &cx.text_system().all_font_names();
        let settings_schema = cx.global::<SettingsStore>().json_schema(
            &SettingsJsonSchemaParams {
                language_names: &language_names,
                font_names,
            },
            cx,
        );

        let tasks_schema = task::TaskTemplates::generate_json_schema();
        let debug_schema = task::DebugTaskFile::generate_json_schema(&adapter_schemas);
        let snippets_schema = snippet_provider::format::VsSnippetsFile::generate_json_schema();
        let tsconfig_schema = serde_json::Value::from_str(TSCONFIG_SCHEMA).unwrap();
        let package_json_schema = serde_json::Value::from_str(PACKAGE_JSON_SCHEMA).unwrap();

        #[allow(unused_mut)]
        let mut schemas = serde_json::json!([
            {
                "fileMatch": ["tsconfig.json"],
                "schema":tsconfig_schema
            },
            {
                "fileMatch": ["package.json"],
                "schema":package_json_schema
            },
            {
                "fileMatch": [
                    schema_file_match(paths::settings_file()),
                    paths::local_settings_file_relative_path()
                ],
                "schema": settings_schema,
            },
            {
                "fileMatch": [schema_file_match(paths::keymap_file())],
                "schema": keymap_schema,
            },
            {
                "fileMatch": [
                    schema_file_match(paths::tasks_file()),
                    paths::local_tasks_file_relative_path()
                ],
                "schema": tasks_schema,
            },
            {
                "fileMatch": [
                    schema_file_match(
                        paths::snippets_dir()
                            .join("*.json")
                            .as_path()
                    )
                ],
                "schema": snippets_schema,
            },
            {
                "fileMatch": [
                    schema_file_match(paths::debug_scenarios_file()),
                    paths::local_debug_file_relative_path()
                ],
                "schema": debug_schema,
            },
        ]);

        #[cfg(debug_assertions)]
        {
            schemas.as_array_mut().unwrap().push(serde_json::json!(
                {
                    "fileMatch": [
                        "zed-inspector-style.json"
                    ],
                    "schema": generate_inspector_style_schema(),
                }
            ))
        }

        schemas
            .as_array_mut()
            .unwrap()
            .extend(cx.all_action_names().into_iter().map(|&name| {
                project::lsp_store::json_language_server_ext::url_schema_for_action(name)
            }));

        // This can be viewed via `dev: open language server logs` -> `json-language-server` ->
        // `Server Info`
        serde_json::json!({
            "json": {
                "format": {
                    "enable": true,
                },
                "validate":
                {
                    "enable": true,
                },
                "schemas": schemas
            }
        })
    }

    async fn get_or_init_workspace_config(&self, cx: &mut AsyncApp) -> Result<Value> {
        {
            let reader = self.workspace_config.read().await;
            if let Some(config) = reader.as_ref() {
                return Ok(config.clone());
            }
        }
        let mut writer = self.workspace_config.write().await;

        let adapter_schemas = cx
            .read_global::<DapRegistry, _>(|dap_registry, _| dap_registry.to_owned())?
            .adapters_schema()
            .await;

        let config = cx.update(|cx| {
            Self::get_workspace_config(self.languages.language_names().clone(), adapter_schemas, cx)
        })?;
        writer.replace(config.clone());
        return Ok(config);
    }
}

#[cfg(debug_assertions)]
fn generate_inspector_style_schema() -> serde_json_lenient::Value {
    let schema = schemars::generate::SchemaSettings::draft2019_09()
        .with_transform(util::schemars::DefaultDenyUnknownFields)
        .into_generator()
        .root_schema_for::<gpui::StyleRefinement>();

    serde_json_lenient::to_value(schema).unwrap()
}

#[async_trait(?Send)]
impl LspAdapter for JsonLspAdapter {
    fn name(&self) -> LanguageServerName {
        LanguageServerName("json-language-server".into())
    }

    async fn check_if_user_installed(
        &self,
        delegate: &dyn LspAdapterDelegate,
        _: Arc<dyn LanguageToolchainStore>,
        _: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        let path = delegate
            .which("vscode-json-language-server".as_ref())
            .await?;
        let env = delegate.shell_env().await;

        Some(LanguageServerBinary {
            path,
            env: Some(env),
            arguments: vec!["--stdio".into()],
        })
    }

    async fn initialization_options(
        self: Arc<Self>,
        _: &dyn Fs,
        _: &Arc<dyn LspAdapterDelegate>,
    ) -> Result<Option<serde_json::Value>> {
        Ok(Some(json!({
            "provideFormatter": true
        })))
    }

    async fn workspace_configuration(
        self: Arc<Self>,
        _: &dyn Fs,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _: Arc<dyn LanguageToolchainStore>,
        cx: &mut AsyncApp,
    ) -> Result<Value> {
        let mut config = self.get_or_init_workspace_config(cx).await?;

        let project_options = cx.update(|cx| {
            language_server_settings(delegate.as_ref(), &self.name(), cx)
                .and_then(|s| s.settings.clone())
        })?;

        if let Some(override_options) = project_options {
            merge_json_value_into(override_options, &mut config);
        }

        Ok(config)
    }

    fn language_ids(&self) -> HashMap<String, String> {
        [
            ("JSON".into(), "json".into()),
            ("JSONC".into(), "jsonc".into()),
        ]
        .into_iter()
        .collect()
    }

    fn is_primary_zed_json_schema_adapter(&self) -> bool {
        true
    }

    async fn clear_zed_json_schema_cache(&self) {
        self.workspace_config.write().await.take();
    }
}

#[inline]
fn schema_file_match(path: &Path) -> String {
    path.strip_prefix(path.parent().unwrap().parent().unwrap())
        .unwrap()
        .display()
        .to_string()
        .replace('\\', "/")
}

pub struct NodeVersionAdapter;

impl NodeVersionAdapter {
    const SERVER_NAME: LanguageServerName =
        LanguageServerName::new_static("package-version-server");
}

#[async_trait(?Send)]
impl LspAdapter for NodeVersionAdapter {
    fn name(&self) -> LanguageServerName {
        Self::SERVER_NAME.clone()
    }

    async fn check_if_user_installed(
        &self,
        delegate: &dyn LspAdapterDelegate,
        _: Arc<dyn LanguageToolchainStore>,
        _: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        let path = delegate.which(Self::SERVER_NAME.as_ref()).await?;
        Some(LanguageServerBinary {
            path,
            env: None,
            arguments: Default::default(),
        })
    }
}
