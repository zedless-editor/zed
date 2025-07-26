use anyhow::{Result};
use async_trait::async_trait;
use collections::HashMap;
use gpui::AsyncApp;
use language::{LanguageToolchainStore, LspAdapter, LspAdapterDelegate};
use lsp::{LanguageServerBinary, LanguageServerName};
use project::{Fs, lsp_store::language_server_settings};
use serde_json::{Value, json};
use std::{
    sync::Arc,
};

pub struct TailwindLspAdapter {
}

impl TailwindLspAdapter {
    const SERVER_NAME: LanguageServerName =
        LanguageServerName::new_static("tailwindcss-language-server");

    pub fn new() -> Self {
        TailwindLspAdapter { }
    }
}

#[async_trait(?Send)]
impl LspAdapter for TailwindLspAdapter {
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

    async fn initialization_options(
        self: Arc<Self>,
        _: &dyn Fs,
        _: &Arc<dyn LspAdapterDelegate>,
    ) -> Result<Option<serde_json::Value>> {
        Ok(Some(json!({
            "provideFormatter": true,
            "userLanguages": {
                "html": "html",
                "css": "css",
                "javascript": "javascript",
                "typescriptreact": "typescriptreact",
            },
        })))
    }

    async fn workspace_configuration(
        self: Arc<Self>,
        _: &dyn Fs,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _: Arc<dyn LanguageToolchainStore>,
        cx: &mut AsyncApp,
    ) -> Result<Value> {
        let mut tailwind_user_settings = cx.update(|cx| {
            language_server_settings(delegate.as_ref(), &Self::SERVER_NAME, cx)
                .and_then(|s| s.settings.clone())
                .unwrap_or_default()
        })?;

        if tailwind_user_settings.get("emmetCompletions").is_none() {
            tailwind_user_settings["emmetCompletions"] = Value::Bool(true);
        }

        Ok(json!({
            "tailwindCSS": tailwind_user_settings,
        }))
    }

    fn language_ids(&self) -> HashMap<String, String> {
        HashMap::from_iter([
            ("Astro".to_string(), "astro".to_string()),
            ("HTML".to_string(), "html".to_string()),
            ("CSS".to_string(), "css".to_string()),
            ("JavaScript".to_string(), "javascript".to_string()),
            ("TSX".to_string(), "typescriptreact".to_string()),
            ("Svelte".to_string(), "svelte".to_string()),
            ("Elixir".to_string(), "phoenix-heex".to_string()),
            ("HEEX".to_string(), "phoenix-heex".to_string()),
            ("ERB".to_string(), "erb".to_string()),
            ("HTML/ERB".to_string(), "erb".to_string()),
            ("PHP".to_string(), "php".to_string()),
            ("Vue.js".to_string(), "vue".to_string()),
        ])
    }
}
