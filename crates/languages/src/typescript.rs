use anyhow::{Context as _, Result};
use async_trait::async_trait;
use chrono::{DateTime, Local};
use collections::HashMap;
use futures::future::join_all;
use gpui::{App, AppContext, AsyncApp, Task};
use language::{
    ContextLocation, ContextProvider, File, LanguageToolchainStore, LspAdapter, LspAdapterDelegate,
};
use lsp::{CodeActionKind, LanguageServerName};
use project::{Fs, lsp_store::language_server_settings};
use serde_json::{Value, json};
use smol::lock::RwLock;
use std::{
    borrow::Cow,
    path::{Path, PathBuf},
    sync::Arc,
};
use task::{TaskTemplate, TaskTemplates, VariableName};
use util::merge_json_value_into;
use util::{ResultExt};

use crate::{PackageJson, PackageJsonData};

#[derive(Debug)]
pub(crate) struct TypeScriptContextProvider {
    last_package_json: PackageJsonContents,
}

const TYPESCRIPT_RUNNER_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("TYPESCRIPT_RUNNER"));

const TYPESCRIPT_JEST_TEST_NAME_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("TYPESCRIPT_JEST_TEST_NAME"));

const TYPESCRIPT_VITEST_TEST_NAME_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("TYPESCRIPT_VITEST_TEST_NAME"));

const TYPESCRIPT_JEST_PACKAGE_PATH_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("TYPESCRIPT_JEST_PACKAGE_PATH"));

const TYPESCRIPT_MOCHA_PACKAGE_PATH_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("TYPESCRIPT_MOCHA_PACKAGE_PATH"));

const TYPESCRIPT_VITEST_PACKAGE_PATH_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("TYPESCRIPT_VITEST_PACKAGE_PATH"));

const TYPESCRIPT_JASMINE_PACKAGE_PATH_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("TYPESCRIPT_JASMINE_PACKAGE_PATH"));

#[derive(Clone, Debug, Default)]
struct PackageJsonContents(Arc<RwLock<HashMap<PathBuf, PackageJson>>>);

impl PackageJsonData {
    fn fill_task_templates(&self, task_templates: &mut TaskTemplates) {
        if self.jest_package_path.is_some() {
            task_templates.0.push(TaskTemplate {
                label: "jest file test".to_owned(),
                command: TYPESCRIPT_RUNNER_VARIABLE.template_value(),
                args: vec![
                    "exec".to_owned(),
                    "--".to_owned(),
                    "jest".to_owned(),
                    "--runInBand".to_owned(),
                    VariableName::File.template_value(),
                ],
                cwd: Some(TYPESCRIPT_JEST_PACKAGE_PATH_VARIABLE.template_value()),
                ..TaskTemplate::default()
            });
            task_templates.0.push(TaskTemplate {
                label: format!("jest test {}", VariableName::Symbol.template_value()),
                command: TYPESCRIPT_RUNNER_VARIABLE.template_value(),
                args: vec![
                    "exec".to_owned(),
                    "--".to_owned(),
                    "jest".to_owned(),
                    "--runInBand".to_owned(),
                    "--testNamePattern".to_owned(),
                    format!(
                        "\"{}\"",
                        TYPESCRIPT_JEST_TEST_NAME_VARIABLE.template_value()
                    ),
                    VariableName::File.template_value(),
                ],
                tags: vec![
                    "ts-test".to_owned(),
                    "js-test".to_owned(),
                    "tsx-test".to_owned(),
                ],
                cwd: Some(TYPESCRIPT_JEST_PACKAGE_PATH_VARIABLE.template_value()),
                ..TaskTemplate::default()
            });
        }

        if self.vitest_package_path.is_some() {
            task_templates.0.push(TaskTemplate {
                label: format!("{} file test", "vitest".to_owned()),
                command: TYPESCRIPT_RUNNER_VARIABLE.template_value(),
                args: vec![
                    "exec".to_owned(),
                    "--".to_owned(),
                    "vitest".to_owned(),
                    "run".to_owned(),
                    "--poolOptions.forks.minForks=0".to_owned(),
                    "--poolOptions.forks.maxForks=1".to_owned(),
                    VariableName::File.template_value(),
                ],
                cwd: Some(TYPESCRIPT_VITEST_PACKAGE_PATH_VARIABLE.template_value()),
                ..TaskTemplate::default()
            });
            task_templates.0.push(TaskTemplate {
                label: format!(
                    "{} test {}",
                    "vitest".to_owned(),
                    VariableName::Symbol.template_value(),
                ),
                command: TYPESCRIPT_RUNNER_VARIABLE.template_value(),
                args: vec![
                    "exec".to_owned(),
                    "--".to_owned(),
                    "vitest".to_owned(),
                    "run".to_owned(),
                    "--poolOptions.forks.minForks=0".to_owned(),
                    "--poolOptions.forks.maxForks=1".to_owned(),
                    "--testNamePattern".to_owned(),
                    format!(
                        "\"{}\"",
                        TYPESCRIPT_VITEST_TEST_NAME_VARIABLE.template_value()
                    ),
                    VariableName::File.template_value(),
                ],
                tags: vec![
                    "ts-test".to_owned(),
                    "js-test".to_owned(),
                    "tsx-test".to_owned(),
                ],
                cwd: Some(TYPESCRIPT_VITEST_PACKAGE_PATH_VARIABLE.template_value()),
                ..TaskTemplate::default()
            });
        }

        if self.mocha_package_path.is_some() {
            task_templates.0.push(TaskTemplate {
                label: format!("{} file test", "mocha".to_owned()),
                command: TYPESCRIPT_RUNNER_VARIABLE.template_value(),
                args: vec![
                    "exec".to_owned(),
                    "--".to_owned(),
                    "mocha".to_owned(),
                    VariableName::File.template_value(),
                ],
                cwd: Some(TYPESCRIPT_MOCHA_PACKAGE_PATH_VARIABLE.template_value()),
                ..TaskTemplate::default()
            });
            task_templates.0.push(TaskTemplate {
                label: format!(
                    "{} test {}",
                    "mocha".to_owned(),
                    VariableName::Symbol.template_value(),
                ),
                command: TYPESCRIPT_RUNNER_VARIABLE.template_value(),
                args: vec![
                    "exec".to_owned(),
                    "--".to_owned(),
                    "mocha".to_owned(),
                    "--grep".to_owned(),
                    format!("\"{}\"", VariableName::Symbol.template_value()),
                    VariableName::File.template_value(),
                ],
                tags: vec![
                    "ts-test".to_owned(),
                    "js-test".to_owned(),
                    "tsx-test".to_owned(),
                ],
                cwd: Some(TYPESCRIPT_MOCHA_PACKAGE_PATH_VARIABLE.template_value()),
                ..TaskTemplate::default()
            });
        }

        if self.jasmine_package_path.is_some() {
            task_templates.0.push(TaskTemplate {
                label: format!("{} file test", "jasmine".to_owned()),
                command: TYPESCRIPT_RUNNER_VARIABLE.template_value(),
                args: vec![
                    "exec".to_owned(),
                    "--".to_owned(),
                    "jasmine".to_owned(),
                    VariableName::File.template_value(),
                ],
                cwd: Some(TYPESCRIPT_JASMINE_PACKAGE_PATH_VARIABLE.template_value()),
                ..TaskTemplate::default()
            });
            task_templates.0.push(TaskTemplate {
                label: format!(
                    "{} test {}",
                    "jasmine".to_owned(),
                    VariableName::Symbol.template_value(),
                ),
                command: TYPESCRIPT_RUNNER_VARIABLE.template_value(),
                args: vec![
                    "exec".to_owned(),
                    "--".to_owned(),
                    "jasmine".to_owned(),
                    format!("--filter={}", VariableName::Symbol.template_value()),
                    VariableName::File.template_value(),
                ],
                tags: vec![
                    "ts-test".to_owned(),
                    "js-test".to_owned(),
                    "tsx-test".to_owned(),
                ],
                cwd: Some(TYPESCRIPT_JASMINE_PACKAGE_PATH_VARIABLE.template_value()),
                ..TaskTemplate::default()
            });
        }

        let script_name_counts: HashMap<_, usize> =
            self.scripts
                .iter()
                .fold(HashMap::default(), |mut acc, (_, script)| {
                    *acc.entry(script).or_default() += 1;
                    acc
                });
        for (path, script) in &self.scripts {
            let label = if script_name_counts.get(script).copied().unwrap_or_default() > 1
                && let Some(parent) = path.parent().and_then(|parent| parent.file_name())
            {
                let parent = parent.to_string_lossy();
                format!("{parent}/package.json > {script}")
            } else {
                format!("package.json > {script}")
            };
            task_templates.0.push(TaskTemplate {
                label,
                command: TYPESCRIPT_RUNNER_VARIABLE.template_value(),
                args: vec!["run".to_owned(), script.to_owned()],
                tags: vec!["package-script".into()],
                cwd: Some(
                    path.parent()
                        .unwrap_or(Path::new("/"))
                        .to_string_lossy()
                        .to_string(),
                ),
                ..TaskTemplate::default()
            });
        }
    }
}

impl TypeScriptContextProvider {
    pub fn new() -> Self {
        Self {
            last_package_json: PackageJsonContents::default(),
        }
    }

    fn combined_package_json_data(
        &self,
        fs: Arc<dyn Fs>,
        worktree_root: &Path,
        file_relative_path: &Path,
        cx: &App,
    ) -> Task<anyhow::Result<PackageJsonData>> {
        let new_json_data = file_relative_path
            .ancestors()
            .map(|path| worktree_root.join(path))
            .map(|parent_path| {
                self.package_json_data(&parent_path, self.last_package_json.clone(), fs.clone(), cx)
            })
            .collect::<Vec<_>>();

        cx.background_spawn(async move {
            let mut package_json_data = PackageJsonData::default();
            for new_data in join_all(new_json_data).await.into_iter().flatten() {
                package_json_data.merge(new_data);
            }
            Ok(package_json_data)
        })
    }

    fn package_json_data(
        &self,
        directory_path: &Path,
        existing_package_json: PackageJsonContents,
        fs: Arc<dyn Fs>,
        cx: &App,
    ) -> Task<anyhow::Result<PackageJsonData>> {
        let package_json_path = directory_path.join("package.json");
        let metadata_check_fs = fs.clone();
        cx.background_spawn(async move {
            let metadata = metadata_check_fs
                .metadata(&package_json_path)
                .await
                .with_context(|| format!("getting metadata for {package_json_path:?}"))?
                .with_context(|| format!("missing FS metadata for {package_json_path:?}"))?;
            let mtime = DateTime::<Local>::from(metadata.mtime.timestamp_for_user());
            let existing_data = {
                let contents = existing_package_json.0.read().await;
                contents
                    .get(&package_json_path)
                    .filter(|package_json| package_json.mtime == mtime)
                    .map(|package_json| package_json.data.clone())
            };
            match existing_data {
                Some(existing_data) => Ok(existing_data),
                None => {
                    let package_json_string =
                        fs.load(&package_json_path).await.with_context(|| {
                            format!("loading package.json from {package_json_path:?}")
                        })?;
                    let package_json: HashMap<String, serde_json_lenient::Value> =
                        serde_json_lenient::from_str(&package_json_string).with_context(|| {
                            format!("parsing package.json from {package_json_path:?}")
                        })?;
                    let new_data =
                        PackageJsonData::new(package_json_path.as_path().into(), package_json);
                    {
                        let mut contents = existing_package_json.0.write().await;
                        contents.insert(
                            package_json_path,
                            PackageJson {
                                mtime,
                                data: new_data.clone(),
                            },
                        );
                    }
                    Ok(new_data)
                }
            }
        })
    }
}

async fn detect_package_manager(
    worktree_root: PathBuf,
    fs: Arc<dyn Fs>,
    package_json_data: Option<PackageJsonData>,
) -> &'static str {
    if let Some(package_json_data) = package_json_data {
        if let Some(package_manager) = package_json_data.package_manager {
            return package_manager;
        }
    }
    if fs.is_file(&worktree_root.join("pnpm-lock.yaml")).await {
        return "pnpm";
    }
    if fs.is_file(&worktree_root.join("yarn.lock")).await {
        return "yarn";
    }
    "npm"
}

impl ContextProvider for TypeScriptContextProvider {
    fn associated_tasks(
        &self,
        fs: Arc<dyn Fs>,
        file: Option<Arc<dyn File>>,
        cx: &App,
    ) -> Task<Option<TaskTemplates>> {
        let Some(file) = project::File::from_dyn(file.as_ref()).cloned() else {
            return Task::ready(None);
        };
        let Some(worktree_root) = file.worktree.read(cx).root_dir() else {
            return Task::ready(None);
        };
        let file_relative_path = file.path().clone();
        let package_json_data =
            self.combined_package_json_data(fs.clone(), &worktree_root, &file_relative_path, cx);

        cx.background_spawn(async move {
            let mut task_templates = TaskTemplates(Vec::new());
            task_templates.0.push(TaskTemplate {
                label: format!(
                    "execute selection {}",
                    VariableName::SelectedText.template_value()
                ),
                command: "node".to_owned(),
                args: vec![
                    "-e".to_owned(),
                    format!("\"{}\"", VariableName::SelectedText.template_value()),
                ],
                ..TaskTemplate::default()
            });

            match package_json_data.await {
                Ok(package_json) => {
                    package_json.fill_task_templates(&mut task_templates);
                }
                Err(e) => {
                    log::error!(
                        "Failed to read package.json for worktree {file_relative_path:?}: {e:#}"
                    );
                }
            }

            Some(task_templates)
        })
    }

    fn build_context(
        &self,
        current_vars: &task::TaskVariables,
        location: ContextLocation<'_>,
        _project_env: Option<HashMap<String, String>>,
        _toolchains: Arc<dyn LanguageToolchainStore>,
        cx: &mut App,
    ) -> Task<Result<task::TaskVariables>> {
        let mut vars = task::TaskVariables::default();

        if let Some(symbol) = current_vars.get(&VariableName::Symbol) {
            vars.insert(
                TYPESCRIPT_JEST_TEST_NAME_VARIABLE,
                replace_test_name_parameters(symbol),
            );
            vars.insert(
                TYPESCRIPT_VITEST_TEST_NAME_VARIABLE,
                replace_test_name_parameters(symbol),
            );
        }
        let file_path = location
            .file_location
            .buffer
            .read(cx)
            .file()
            .map(|file| file.path());

        let args = location.worktree_root.zip(location.fs).zip(file_path).map(
            |((worktree_root, fs), file_path)| {
                (
                    self.combined_package_json_data(fs.clone(), &worktree_root, file_path, cx),
                    worktree_root,
                    fs,
                )
            },
        );
        cx.background_spawn(async move {
            if let Some((task, worktree_root, fs)) = args {
                let package_json_data = task.await.log_err();
                vars.insert(
                    TYPESCRIPT_RUNNER_VARIABLE,
                    detect_package_manager(worktree_root, fs, package_json_data.clone())
                        .await
                        .to_owned(),
                );

                if let Some(package_json_data) = package_json_data {
                    if let Some(path) = package_json_data.jest_package_path {
                        vars.insert(
                            TYPESCRIPT_JEST_PACKAGE_PATH_VARIABLE,
                            path.parent()
                                .unwrap_or(Path::new(""))
                                .to_string_lossy()
                                .to_string(),
                        );
                    }

                    if let Some(path) = package_json_data.mocha_package_path {
                        vars.insert(
                            TYPESCRIPT_MOCHA_PACKAGE_PATH_VARIABLE,
                            path.parent()
                                .unwrap_or(Path::new(""))
                                .to_string_lossy()
                                .to_string(),
                        );
                    }

                    if let Some(path) = package_json_data.vitest_package_path {
                        vars.insert(
                            TYPESCRIPT_VITEST_PACKAGE_PATH_VARIABLE,
                            path.parent()
                                .unwrap_or(Path::new(""))
                                .to_string_lossy()
                                .to_string(),
                        );
                    }

                    if let Some(path) = package_json_data.jasmine_package_path {
                        vars.insert(
                            TYPESCRIPT_JASMINE_PACKAGE_PATH_VARIABLE,
                            path.parent()
                                .unwrap_or(Path::new(""))
                                .to_string_lossy()
                                .to_string(),
                        );
                    }
                }
            }
            Ok(vars)
        })
    }
}

fn replace_test_name_parameters(test_name: &str) -> String {
    let pattern = regex::Regex::new(r"(%|\$)[0-9a-zA-Z]+").unwrap();

    regex::escape(&pattern.replace_all(test_name, "(.+?)"))
}

pub struct TypeScriptLspAdapter {
}

impl TypeScriptLspAdapter {
    const SERVER_NAME: LanguageServerName =
        LanguageServerName::new_static("typescript-language-server");
    pub fn new() -> Self {
        TypeScriptLspAdapter { }
    }
    async fn tsdk_path(fs: &dyn Fs, adapter: &Arc<dyn LspAdapterDelegate>) -> Option<&'static str> {
        let is_yarn = adapter
            .read_text_file(PathBuf::from(".yarn/sdks/typescript/lib/typescript.js"))
            .await
            .is_ok();

        let tsdk_path = if is_yarn {
            ".yarn/sdks/typescript/lib"
        } else {
            "node_modules/typescript/lib"
        };

        if fs
            .is_dir(&adapter.worktree_root_path().join(tsdk_path))
            .await
        {
            Some(tsdk_path)
        } else {
            None
        }
    }
}

#[async_trait(?Send)]
impl LspAdapter for TypeScriptLspAdapter {
    fn name(&self) -> LanguageServerName {
        Self::SERVER_NAME.clone()
    }

    fn code_action_kinds(&self) -> Option<Vec<CodeActionKind>> {
        Some(vec![
            CodeActionKind::QUICKFIX,
            CodeActionKind::REFACTOR,
            CodeActionKind::REFACTOR_EXTRACT,
            CodeActionKind::SOURCE,
        ])
    }

    async fn label_for_completion(
        &self,
        item: &lsp::CompletionItem,
        language: &Arc<language::Language>,
    ) -> Option<language::CodeLabel> {
        use lsp::CompletionItemKind as Kind;
        let len = item.label.len();
        let grammar = language.grammar()?;
        let highlight_id = match item.kind? {
            Kind::CLASS | Kind::INTERFACE | Kind::ENUM => grammar.highlight_id_for_name("type"),
            Kind::CONSTRUCTOR => grammar.highlight_id_for_name("type"),
            Kind::CONSTANT => grammar.highlight_id_for_name("constant"),
            Kind::FUNCTION | Kind::METHOD => grammar.highlight_id_for_name("function"),
            Kind::PROPERTY | Kind::FIELD => grammar.highlight_id_for_name("property"),
            Kind::VARIABLE => grammar.highlight_id_for_name("variable"),
            _ => None,
        }?;

        let text = if let Some(description) = item
            .label_details
            .as_ref()
            .and_then(|label_details| label_details.description.as_ref())
        {
            format!("{} {}", item.label, description)
        } else if let Some(detail) = &item.detail {
            format!("{} {}", item.label, detail)
        } else {
            item.label.clone()
        };
        let filter_range = item
            .filter_text
            .as_deref()
            .and_then(|filter| text.find(filter).map(|ix| ix..ix + filter.len()))
            .unwrap_or(0..len);
        Some(language::CodeLabel {
            text,
            runs: vec![(0..len, highlight_id)],
            filter_range,
        })
    }

    async fn initialization_options(
        self: Arc<Self>,
        fs: &dyn Fs,
        adapter: &Arc<dyn LspAdapterDelegate>,
    ) -> Result<Option<serde_json::Value>> {
        let tsdk_path = Self::tsdk_path(fs, adapter).await;
        Ok(Some(json!({
            "provideFormatter": true,
            "hostInfo": "zed",
            "tsserver": {
                "path": tsdk_path,
            },
            "preferences": {
                "includeInlayParameterNameHints": "all",
                "includeInlayParameterNameHintsWhenArgumentMatchesName": true,
                "includeInlayFunctionParameterTypeHints": true,
                "includeInlayVariableTypeHints": true,
                "includeInlayVariableTypeHintsWhenTypeMatchesName": true,
                "includeInlayPropertyDeclarationTypeHints": true,
                "includeInlayFunctionLikeReturnTypeHints": true,
                "includeInlayEnumMemberValueHints": true,
            }
        })))
    }

    async fn workspace_configuration(
        self: Arc<Self>,
        _: &dyn Fs,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _: Arc<dyn LanguageToolchainStore>,
        cx: &mut AsyncApp,
    ) -> Result<Value> {
        let override_options = cx.update(|cx| {
            language_server_settings(delegate.as_ref(), &Self::SERVER_NAME, cx)
                .and_then(|s| s.settings.clone())
        })?;
        if let Some(options) = override_options {
            return Ok(options);
        }
        Ok(json!({
            "completions": {
              "completeFunctionCalls": true
            }
        }))
    }

    fn language_ids(&self) -> HashMap<String, String> {
        HashMap::from_iter([
            ("TypeScript".into(), "typescript".into()),
            ("JavaScript".into(), "javascript".into()),
            ("TSX".into(), "typescriptreact".into()),
        ])
    }
}

pub struct EsLintLspAdapter {
}

impl EsLintLspAdapter {
    const SERVER_NAME: LanguageServerName = LanguageServerName::new_static("eslint");

    const FLAT_CONFIG_FILE_NAMES: &'static [&'static str] = &[
        "eslint.config.js",
        "eslint.config.mjs",
        "eslint.config.cjs",
        "eslint.config.ts",
        "eslint.config.cts",
        "eslint.config.mts",
    ];

    pub fn new() -> Self {
        EsLintLspAdapter { }
    }
}

#[async_trait(?Send)]
impl LspAdapter for EsLintLspAdapter {
    fn code_action_kinds(&self) -> Option<Vec<CodeActionKind>> {
        Some(vec![
            CodeActionKind::QUICKFIX,
            CodeActionKind::new("source.fixAll.eslint"),
        ])
    }

    async fn workspace_configuration(
        self: Arc<Self>,
        _: &dyn Fs,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _: Arc<dyn LanguageToolchainStore>,
        cx: &mut AsyncApp,
    ) -> Result<Value> {
        let workspace_root = delegate.worktree_root_path();
        let use_flat_config = Self::FLAT_CONFIG_FILE_NAMES
            .iter()
            .any(|file| workspace_root.join(file).is_file());

        let mut default_workspace_configuration = json!({
            "validate": "on",
            "rulesCustomizations": [],
            "run": "onType",
            "nodePath": null,
            "workingDirectory": {
                "mode": "auto"
            },
            "workspaceFolder": {
                "uri": workspace_root,
                "name": workspace_root.file_name()
                    .unwrap_or(workspace_root.as_os_str())
                    .to_string_lossy(),
            },
            "problems": {},
            "codeActionOnSave": {
                // We enable this, but without also configuring code_actions_on_format
                // in the Zed configuration, it doesn't have an effect.
                "enable": true,
            },
            "codeAction": {
                "disableRuleComment": {
                    "enable": true,
                    "location": "separateLine",
                },
                "showDocumentation": {
                    "enable": true
                }
            },
            "experimental": {
                "useFlatConfig": use_flat_config,
            }
        });

        let override_options = cx.update(|cx| {
            language_server_settings(delegate.as_ref(), &Self::SERVER_NAME, cx)
                .and_then(|s| s.settings.clone())
        })?;

        if let Some(override_options) = override_options {
            merge_json_value_into(override_options, &mut default_workspace_configuration);
        }

        Ok(json!({
            "": default_workspace_configuration
        }))
    }

    fn name(&self) -> LanguageServerName {
        Self::SERVER_NAME.clone()
    }
}

#[cfg(target_os = "windows")]
async fn handle_symlink(src_dir: PathBuf, dest_dir: PathBuf) -> Result<()> {
    anyhow::ensure!(
        fs::metadata(&src_dir).await.is_ok(),
        "Directory {src_dir:?} is not present"
    );
    if fs::metadata(&dest_dir).await.is_ok() {
        fs::remove_file(&dest_dir).await?;
    }
    fs::create_dir_all(&dest_dir).await?;
    let mut entries = fs::read_dir(&src_dir).await?;
    while let Some(entry) = entries.try_next().await? {
        let entry_path = entry.path();
        let entry_name = entry.file_name();
        let dest_path = dest_dir.join(&entry_name);
        fs::copy(&entry_path, &dest_path).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gpui::{AppContext as _, BackgroundExecutor, TestAppContext};
    use language::language_settings;
    use project::{FakeFs, Project};
    use serde_json::json;
    use task::TaskTemplates;
    use unindent::Unindent;
    use util::path;

    use crate::typescript::{PackageJsonData, TypeScriptContextProvider};

    #[gpui::test]
    async fn test_outline(cx: &mut TestAppContext) {
        let language = crate::language(
            "typescript",
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        );

        let text = r#"
            function a() {
              // local variables are omitted
              let a1 = 1;
              // all functions are included
              async function a2() {}
            }
            // top-level variables are included
            let b: C
            function getB() {}
            // exported variables are included
            export const d = e;
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
                ("function a()", 0),
                ("async function a2()", 1),
                ("let b", 0),
                ("function getB()", 0),
                ("const d", 0),
            ]
        );
    }

    #[gpui::test]
    async fn test_generator_function_outline(cx: &mut TestAppContext) {
        let language = crate::language("javascript", tree_sitter_typescript::LANGUAGE_TSX.into());

        let text = r#"
            function normalFunction() {
                console.log("normal");
            }

            function* simpleGenerator() {
                yield 1;
                yield 2;
            }

            async function* asyncGenerator() {
                yield await Promise.resolve(1);
            }

            function* generatorWithParams(start, end) {
                for (let i = start; i <= end; i++) {
                    yield i;
                }
            }

            class TestClass {
                *methodGenerator() {
                    yield "method";
                }

                async *asyncMethodGenerator() {
                    yield "async method";
                }
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
                ("function normalFunction()", 0),
                ("function* simpleGenerator()", 0),
                ("async function* asyncGenerator()", 0),
                ("function* generatorWithParams( )", 0),
                ("class TestClass", 0),
                ("*methodGenerator()", 1),
                ("async *asyncMethodGenerator()", 1),
            ]
        );
    }

    #[gpui::test]
    async fn test_package_json_discovery(executor: BackgroundExecutor, cx: &mut TestAppContext) {
        cx.update(|cx| {
            settings::init(cx);
            Project::init_settings(cx);
            language_settings::init(cx);
        });

        let package_json_1 = json!({
            "dependencies": {
                "mocha": "1.0.0",
                "vitest": "1.0.0"
            },
            "scripts": {
                "test": ""
            }
        })
        .to_string();

        let package_json_2 = json!({
            "devDependencies": {
                "vitest": "2.0.0"
            },
            "scripts": {
                "test": ""
            }
        })
        .to_string();

        let fs = FakeFs::new(executor);
        fs.insert_tree(
            path!("/root"),
            json!({
                "package.json": package_json_1,
                "sub": {
                    "package.json": package_json_2,
                    "file.js": "",
                }
            }),
        )
        .await;

        let provider = TypeScriptContextProvider::new();
        let package_json_data = cx
            .update(|cx| {
                provider.combined_package_json_data(
                    fs.clone(),
                    path!("/root").as_ref(),
                    "sub/file1.js".as_ref(),
                    cx,
                )
            })
            .await
            .unwrap();
        pretty_assertions::assert_eq!(
            package_json_data,
            PackageJsonData {
                jest_package_path: None,
                mocha_package_path: Some(Path::new(path!("/root/package.json")).into()),
                vitest_package_path: Some(Path::new(path!("/root/sub/package.json")).into()),
                jasmine_package_path: None,
                scripts: [
                    (
                        Path::new(path!("/root/package.json")).into(),
                        "test".to_owned()
                    ),
                    (
                        Path::new(path!("/root/sub/package.json")).into(),
                        "test".to_owned()
                    )
                ]
                .into_iter()
                .collect(),
                package_manager: None,
            }
        );

        let mut task_templates = TaskTemplates::default();
        package_json_data.fill_task_templates(&mut task_templates);
        let task_templates = task_templates
            .0
            .into_iter()
            .map(|template| (template.label, template.cwd))
            .collect::<Vec<_>>();
        pretty_assertions::assert_eq!(
            task_templates,
            [
                (
                    "vitest file test".into(),
                    Some("$ZED_CUSTOM_TYPESCRIPT_VITEST_PACKAGE_PATH".into()),
                ),
                (
                    "vitest test $ZED_SYMBOL".into(),
                    Some("$ZED_CUSTOM_TYPESCRIPT_VITEST_PACKAGE_PATH".into()),
                ),
                (
                    "mocha file test".into(),
                    Some("$ZED_CUSTOM_TYPESCRIPT_MOCHA_PACKAGE_PATH".into()),
                ),
                (
                    "mocha test $ZED_SYMBOL".into(),
                    Some("$ZED_CUSTOM_TYPESCRIPT_MOCHA_PACKAGE_PATH".into()),
                ),
                (
                    "root/package.json > test".into(),
                    Some(path!("/root").into())
                ),
                (
                    "sub/package.json > test".into(),
                    Some(path!("/root/sub").into())
                ),
            ]
        );
    }
}
