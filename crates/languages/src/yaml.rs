use anyhow::{Result};
use async_trait::async_trait;
use gpui::AsyncApp;
use language::{
    LanguageToolchainStore, LspAdapter, LspAdapterDelegate, language_settings::AllLanguageSettings,
};
use lsp::{LanguageServerBinary, LanguageServerName};
use project::{Fs, lsp_store::language_server_settings};
use serde_json::Value;
use settings::{Settings, SettingsLocation};
use std::{
    sync::Arc,
};
use util::{merge_json_value_into};

pub struct YamlLspAdapter {
}

impl YamlLspAdapter {
    const SERVER_NAME: LanguageServerName = LanguageServerName::new_static("yaml-language-server");
    pub fn new() -> Self {
        YamlLspAdapter { }
    }
}

#[async_trait(?Send)]
impl LspAdapter for YamlLspAdapter {
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
        let env = delegate.shell_env().await;

        Some(LanguageServerBinary {
            path,
            env: Some(env),
            arguments: vec!["--stdio".into()],
        })
    }

    async fn workspace_configuration(
        self: Arc<Self>,
        _: &dyn Fs,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _: Arc<dyn LanguageToolchainStore>,
        cx: &mut AsyncApp,
    ) -> Result<Value> {
        let location = SettingsLocation {
            worktree_id: delegate.worktree_id(),
            path: delegate.worktree_root_path(),
        };

        let tab_size = cx.update(|cx| {
            AllLanguageSettings::get(Some(location), cx)
                .language(Some(location), Some(&"YAML".into()), cx)
                .tab_size
        })?;

        let mut options = serde_json::json!({
            "[yaml]": {"editor.tabSize": tab_size},
            "yaml": {"format": {"enable": true}}
        });

        let project_options = cx.update(|cx| {
            language_server_settings(delegate.as_ref(), &Self::SERVER_NAME, cx)
                .and_then(|s| s.settings.clone())
        })?;
        if let Some(override_options) = project_options {
            merge_json_value_into(override_options, &mut options);
        }
        Ok(options)
    }
}
