use anyhow::Result;
use async_trait::async_trait;
use gpui::AsyncApp;
use language::{LanguageToolchainStore, LspAdapter, LspAdapterDelegate};
use lsp::{LanguageServerBinary, LanguageServerName};
use project::{Fs, lsp_store::language_server_settings};
use serde_json::json;
use util::merge_json_value_into;
use std::sync::Arc;

pub struct CssLspAdapter {}

impl CssLspAdapter {
    pub fn new() -> Self {
        CssLspAdapter {}
    }
}

#[async_trait(?Send)]
impl LspAdapter for CssLspAdapter {
    fn name(&self) -> LanguageServerName {
        LanguageServerName("vscode-css-language-server".into())
    }

    async fn check_if_user_installed(
        &self,
        delegate: &dyn LspAdapterDelegate,
        _: Arc<dyn LanguageToolchainStore>,
        _: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        let path = delegate
            .which("vscode-css-language-server".as_ref())
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
    ) -> Result<serde_json::Value> {
        let mut default_config = json!({
            "css": {
                "lint": {}
            },
            "less": {
                "lint": {}
            },
            "scss": {
                "lint": {}
            }
        });

        let project_options = cx.update(|cx| {
            language_server_settings(delegate.as_ref(), &self.name(), cx)
                .and_then(|s| s.settings.clone())
        })?;

        if let Some(override_options) = project_options {
            merge_json_value_into(override_options, &mut default_config);
        }

        Ok(default_config)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, TestAppContext};
    use unindent::Unindent;

    #[gpui::test]
    async fn test_outline(cx: &mut TestAppContext) {
        let language = crate::language("css", tree_sitter_css::LANGUAGE.into());

        let text = r#"
            /* Import statement */
            @import './fonts.css';

            /* multiline list of selectors with nesting */
            .test-class,
            div {
                .nested-class {
                    color: red;
                }
            }

            /* descendant selectors */
            .test .descendant {}

            /* pseudo */
            .test:not(:hover) {}

            /* media queries */
            @media screen and (min-width: 3000px) {
                .desktop-class {}
            }
        "#
        .unindent();

        let buffer = cx.new(|cx| language::Buffer::local(text, cx).with_language(language, cx));
        let outline = buffer.read_with(cx, |buffer, _| buffer.snapshot().outline(None).unwrap());
        assert_eq!(
            outline
                .items
                .iter()
                .map(|item| (item.text.as_str(), item.depth))
                .collect::<Vec<_>>(),
            &[
                ("@import './fonts.css'", 0),
                (".test-class, div", 0),
                (".nested-class", 1),
                (".test .descendant", 0),
                (".test:not(:hover)", 0),
                ("@media screen and (min-width: 3000px)", 0),
                (".desktop-class", 1),
            ]
        );
    }
}
