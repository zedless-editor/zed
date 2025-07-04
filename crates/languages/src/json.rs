use anyhow::{Context as _, Result};
use async_trait::async_trait;
use collections::HashMap;
use dap::DapRegistry;
use futures::StreamExt;
use gpui::{App, AsyncApp};
use language::{LanguageRegistry, LanguageToolchainStore, LspAdapter, LspAdapterDelegate};
use lsp::{LanguageServerBinary, LanguageServerName};
use node_runtime::NodeRuntime;
use project::{ContextProviderWithTasks, Fs, lsp_store::language_server_settings};
use serde_json::{Value, json};
use settings::{KeymapFile, SettingsJsonSchemaParams, SettingsStore};
use smol::{
    fs::{self},
    lock::RwLock,
};
use std::{
    any::Any,
    ffi::OsString,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};
use task::{AdapterSchemas, TaskTemplate, TaskTemplates, VariableName};
use util::{ResultExt, maybe, merge_json_value_into};

const SERVER_PATH: &str =
    "node_modules/vscode-langservers-extracted/bin/vscode-json-language-server";

// Origin: https://github.com/SchemaStore/schemastore
const TSCONFIG_SCHEMA: &str = include_str!("json/schemas/tsconfig.json");
const PACKAGE_JSON_SCHEMA: &str = include_str!("json/schemas/package.json");

pub(super) fn json_task_context() -> ContextProviderWithTasks {
    ContextProviderWithTasks::new(TaskTemplates(vec![
        TaskTemplate {
            label: "package script $ZED_CUSTOM_script".to_owned(),
            command: "npm --prefix $ZED_DIRNAME run".to_owned(),
            args: vec![VariableName::Custom("script".into()).template_value()],
            tags: vec!["package-script".into()],
            ..TaskTemplate::default()
        },
        TaskTemplate {
            label: "composer script $ZED_CUSTOM_script".to_owned(),
            command: "composer -d $ZED_DIRNAME".to_owned(),
            args: vec![VariableName::Custom("script".into()).template_value()],
            tags: vec!["composer-script".into()],
            ..TaskTemplate::default()
        },
    ]))
}

fn server_binary_arguments(server_path: &Path) -> Vec<OsString> {
    vec![server_path.into(), "--stdio".into()]
}

pub struct JsonLspAdapter {
    node: NodeRuntime,
    languages: Arc<LanguageRegistry>,
    workspace_config: RwLock<Option<Value>>,
}

impl JsonLspAdapter {
    const PACKAGE_NAME: &str = "vscode-langservers-extracted";

    pub fn new(node: NodeRuntime, languages: Arc<LanguageRegistry>) -> Self {
        Self {
            node,
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
    let schema = schemars::r#gen::SchemaSettings::draft07()
        .with(|settings| settings.option_add_null_type = false)
        .into_generator()
        .into_root_schema_for::<gpui::StyleRefinement>();

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

    async fn check_if_version_installed(
        &self,
        version: &(dyn 'static + Send + Any),
        container_dir: &PathBuf,
        _: &dyn LspAdapterDelegate,
    ) -> Option<LanguageServerBinary> {
        let version = version.downcast_ref::<String>().unwrap();
        let server_path = container_dir.join(SERVER_PATH);

        let should_install_language_server = self
            .node
            .should_install_npm_package(Self::PACKAGE_NAME, &server_path, &container_dir, &version)
            .await;

        if should_install_language_server {
            None
        } else {
            Some(LanguageServerBinary {
                path: self.node.binary_path().await.ok()?,
                env: None,
                arguments: server_binary_arguments(&server_path),
            })
        }
    }

    async fn cached_server_binary(
        &self,
        container_dir: PathBuf,
        _: &dyn LspAdapterDelegate,
    ) -> Option<LanguageServerBinary> {
        get_cached_server_binary(container_dir, &self.node).await
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

async fn get_cached_server_binary(
    container_dir: PathBuf,
    node: &NodeRuntime,
) -> Option<LanguageServerBinary> {
    maybe!(async {
        let mut last_version_dir = None;
        let mut entries = fs::read_dir(&container_dir).await?;
        while let Some(entry) = entries.next().await {
            let entry = entry?;
            if entry.file_type().await?.is_dir() {
                last_version_dir = Some(entry.path());
            }
        }

        let last_version_dir = last_version_dir.context("no cached binary")?;
        let server_path = last_version_dir.join(SERVER_PATH);
        anyhow::ensure!(
            server_path.exists(),
            "missing executable in directory {last_version_dir:?}"
        );
        Ok(LanguageServerBinary {
            path: node.binary_path().await?,
            env: None,
            arguments: server_binary_arguments(&server_path),
        })
    })
    .await
    .log_err()
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

    async fn cached_server_binary(
        &self,
        container_dir: PathBuf,
        _delegate: &dyn LspAdapterDelegate,
    ) -> Option<LanguageServerBinary> {
        get_cached_version_server_binary(container_dir).await
    }
}

async fn get_cached_version_server_binary(container_dir: PathBuf) -> Option<LanguageServerBinary> {
    maybe!(async {
        let mut last = None;
        let mut entries = fs::read_dir(&container_dir).await?;
        while let Some(entry) = entries.next().await {
            last = Some(entry?.path());
        }

        anyhow::Ok(LanguageServerBinary {
            path: last.context("no cached binary")?,
            env: None,
            arguments: Default::default(),
        })
    })
    .await
    .log_err()
}
