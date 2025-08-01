use anyhow::{Result};
use async_trait::async_trait;
use collections::HashMap;

use gpui::{App, AppContext, AsyncApp, SharedString, Task};
pub use language::*;
use lsp::{InitializeParams, LanguageServerBinary};
use project::Fs;
use project::lsp_store::rust_analyzer_ext::CARGO_DIAGNOSTICS_SOURCE_NAME;
use project::project_settings::ProjectSettings;
use regex::Regex;
use serde_json::json;
use settings::Settings as _;
use std::fmt::Display;
use std::{
    borrow::Cow,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};
use task::{TaskTemplate, TaskTemplates, TaskVariables, VariableName};
use util::merge_json_value_into;
use util::{
    ResultExt,
    maybe,
};

use crate::language_settings::language_settings;

pub struct RustLspAdapter;

#[cfg(target_os = "macos")]
impl RustLspAdapter {
    const ARCH_SERVER_NAME: &str = "apple-darwin";
}

#[cfg(target_os = "linux")]
impl RustLspAdapter {
}

#[cfg(target_os = "freebsd")]
impl RustLspAdapter {
    const ARCH_SERVER_NAME: &str = "unknown-freebsd";
}

#[cfg(target_os = "windows")]
impl RustLspAdapter {
    const ARCH_SERVER_NAME: &str = "pc-windows-msvc";
}

const SERVER_NAME: LanguageServerName = LanguageServerName::new_static("rust-analyzer");

impl RustLspAdapter {
}

pub(crate) struct CargoManifestProvider;

impl ManifestProvider for CargoManifestProvider {
    fn name(&self) -> ManifestName {
        SharedString::new_static("Cargo.toml").into()
    }

    fn search(
        &self,
        ManifestQuery {
            path,
            depth,
            delegate,
        }: ManifestQuery,
    ) -> Option<Arc<Path>> {
        let mut outermost_cargo_toml = None;
        for path in path.ancestors().take(depth) {
            let p = path.join("Cargo.toml");
            if delegate.exists(&p, Some(false)) {
                outermost_cargo_toml = Some(Arc::from(path));
            }
        }

        outermost_cargo_toml
    }
}

#[async_trait(?Send)]
impl LspAdapter for RustLspAdapter {
    fn name(&self) -> LanguageServerName {
        SERVER_NAME.clone()
    }

    fn manifest_name(&self) -> Option<ManifestName> {
        Some(SharedString::new_static("Cargo.toml").into())
    }

    async fn check_if_user_installed(
        &self,
        delegate: &dyn LspAdapterDelegate,
        _: Arc<dyn LanguageToolchainStore>,
        _: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        let path = delegate.which("rust-analyzer".as_ref()).await?;
        let env = delegate.shell_env().await;

        // It is surprisingly common for ~/.cargo/bin/rust-analyzer to be a symlink to
        // /usr/bin/rust-analyzer that fails when you run it; so we need to test it.
        log::info!("found rust-analyzer in PATH. trying to run `rust-analyzer --help`");
        let result = delegate
            .try_exec(LanguageServerBinary {
                path: path.clone(),
                arguments: vec!["--help".into()],
                env: Some(env.clone()),
            })
            .await;
        if let Err(err) = result {
            log::debug!(
                "failed to run rust-analyzer after detecting it in PATH: binary: {:?}: {}",
                path,
                err
            );
            return None;
        }

        Some(LanguageServerBinary {
            path,
            env: Some(env),
            arguments: vec![],
        })
    }

    fn disk_based_diagnostic_sources(&self) -> Vec<String> {
        vec![CARGO_DIAGNOSTICS_SOURCE_NAME.to_owned()]
    }

    fn disk_based_diagnostics_progress_token(&self) -> Option<String> {
        Some("rust-analyzer/flycheck".into())
    }

    fn process_diagnostics(
        &self,
        params: &mut lsp::PublishDiagnosticsParams,
        _: LanguageServerId,
        _: Option<&'_ Buffer>,
    ) {
        static REGEX: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?m)`([^`]+)\n`$").expect("Failed to create REGEX"));

        for diagnostic in &mut params.diagnostics {
            for message in diagnostic
                .related_information
                .iter_mut()
                .flatten()
                .map(|info| &mut info.message)
                .chain([&mut diagnostic.message])
            {
                if let Cow::Owned(sanitized) = REGEX.replace_all(message, "`$1`") {
                    *message = sanitized;
                }
            }
        }
    }

    fn diagnostic_message_to_markdown(&self, message: &str) -> Option<String> {
        static REGEX: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?m)\n *").expect("Failed to create REGEX"));
        Some(REGEX.replace_all(message, "\n\n").to_string())
    }

    async fn label_for_completion(
        &self,
        completion: &lsp::CompletionItem,
        language: &Arc<Language>,
    ) -> Option<CodeLabel> {
        let detail = completion
            .label_details
            .as_ref()
            .and_then(|detail| detail.detail.as_ref())
            .or(completion.detail.as_ref())
            .map(|detail| detail.trim());
        let function_signature = completion
            .label_details
            .as_ref()
            .and_then(|detail| detail.description.as_deref())
            .or(completion.detail.as_deref());
        match (detail, completion.kind) {
            (Some(detail), Some(lsp::CompletionItemKind::FIELD)) => {
                let name = &completion.label;
                let text = format!("{name}: {detail}");
                let prefix = "struct S { ";
                let source = Rope::from(format!("{prefix}{text} }}"));
                let runs =
                    language.highlight_text(&source, prefix.len()..prefix.len() + text.len());
                let filter_range = completion
                    .filter_text
                    .as_deref()
                    .and_then(|filter| text.find(filter).map(|ix| ix..ix + filter.len()))
                    .unwrap_or(0..name.len());
                return Some(CodeLabel {
                    text,
                    runs,
                    filter_range,
                });
            }
            (
                Some(detail),
                Some(lsp::CompletionItemKind::CONSTANT | lsp::CompletionItemKind::VARIABLE),
            ) if completion.insert_text_format != Some(lsp::InsertTextFormat::SNIPPET) => {
                let name = &completion.label;
                let text = format!(
                    "{}: {}",
                    name,
                    completion.detail.as_deref().unwrap_or(detail)
                );
                let prefix = "let ";
                let source = Rope::from(format!("{prefix}{text} = ();"));
                let runs =
                    language.highlight_text(&source, prefix.len()..prefix.len() + text.len());
                let filter_range = completion
                    .filter_text
                    .as_deref()
                    .and_then(|filter| text.find(filter).map(|ix| ix..ix + filter.len()))
                    .unwrap_or(0..name.len());
                return Some(CodeLabel {
                    text,
                    runs,
                    filter_range,
                });
            }
            (
                Some(detail),
                Some(lsp::CompletionItemKind::FUNCTION | lsp::CompletionItemKind::METHOD),
            ) => {
                static REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new("\\(…?\\)").unwrap());
                const FUNCTION_PREFIXES: [&str; 6] = [
                    "async fn",
                    "async unsafe fn",
                    "const fn",
                    "const unsafe fn",
                    "unsafe fn",
                    "fn",
                ];
                // Is it function `async`?
                let fn_keyword = FUNCTION_PREFIXES.iter().find_map(|prefix| {
                    function_signature.as_ref().and_then(|signature| {
                        signature
                            .strip_prefix(*prefix)
                            .map(|suffix| (*prefix, suffix))
                    })
                });
                // fn keyword should be followed by opening parenthesis.
                if let Some((prefix, suffix)) = fn_keyword {
                    let mut text = REGEX.replace(&completion.label, suffix).to_string();
                    let source = Rope::from(format!("{prefix} {text} {{}}"));
                    let run_start = prefix.len() + 1;
                    let runs = language.highlight_text(&source, run_start..run_start + text.len());
                    if detail.starts_with("(") {
                        text.push(' ');
                        text.push_str(&detail);
                    }
                    let filter_range = completion
                        .filter_text
                        .as_deref()
                        .and_then(|filter| text.find(filter).map(|ix| ix..ix + filter.len()))
                        .unwrap_or(0..completion.label.find('(').unwrap_or(text.len()));
                    return Some(CodeLabel {
                        filter_range,
                        text,
                        runs,
                    });
                } else if completion
                    .detail
                    .as_ref()
                    .map_or(false, |detail| detail.starts_with("macro_rules! "))
                {
                    let text = completion.label.clone();
                    let len = text.len();
                    let source = Rope::from(text.as_str());
                    let runs = language.highlight_text(&source, 0..len);
                    let filter_range = completion
                        .filter_text
                        .as_deref()
                        .and_then(|filter| text.find(filter).map(|ix| ix..ix + filter.len()))
                        .unwrap_or(0..len);
                    return Some(CodeLabel {
                        filter_range,
                        text,
                        runs,
                    });
                }
            }
            (_, Some(kind)) => {
                let highlight_name = match kind {
                    lsp::CompletionItemKind::STRUCT
                    | lsp::CompletionItemKind::INTERFACE
                    | lsp::CompletionItemKind::ENUM => Some("type"),
                    lsp::CompletionItemKind::ENUM_MEMBER => Some("variant"),
                    lsp::CompletionItemKind::KEYWORD => Some("keyword"),
                    lsp::CompletionItemKind::VALUE | lsp::CompletionItemKind::CONSTANT => {
                        Some("constant")
                    }
                    _ => None,
                };

                let mut label = completion.label.clone();
                if let Some(detail) = detail.filter(|detail| detail.starts_with("(")) {
                    label.push(' ');
                    label.push_str(detail);
                }
                let mut label = CodeLabel::plain(label, completion.filter_text.as_deref());
                if let Some(highlight_name) = highlight_name {
                    let highlight_id = language.grammar()?.highlight_id_for_name(highlight_name)?;
                    label.runs.push((
                        0..label.text.rfind('(').unwrap_or(completion.label.len()),
                        highlight_id,
                    ));
                }

                return Some(label);
            }
            _ => {}
        }
        None
    }

    async fn label_for_symbol(
        &self,
        name: &str,
        kind: lsp::SymbolKind,
        language: &Arc<Language>,
    ) -> Option<CodeLabel> {
        let (text, filter_range, display_range) = match kind {
            lsp::SymbolKind::METHOD | lsp::SymbolKind::FUNCTION => {
                let text = format!("fn {} () {{}}", name);
                let filter_range = 3..3 + name.len();
                let display_range = 0..filter_range.end;
                (text, filter_range, display_range)
            }
            lsp::SymbolKind::STRUCT => {
                let text = format!("struct {} {{}}", name);
                let filter_range = 7..7 + name.len();
                let display_range = 0..filter_range.end;
                (text, filter_range, display_range)
            }
            lsp::SymbolKind::ENUM => {
                let text = format!("enum {} {{}}", name);
                let filter_range = 5..5 + name.len();
                let display_range = 0..filter_range.end;
                (text, filter_range, display_range)
            }
            lsp::SymbolKind::INTERFACE => {
                let text = format!("trait {} {{}}", name);
                let filter_range = 6..6 + name.len();
                let display_range = 0..filter_range.end;
                (text, filter_range, display_range)
            }
            lsp::SymbolKind::CONSTANT => {
                let text = format!("const {}: () = ();", name);
                let filter_range = 6..6 + name.len();
                let display_range = 0..filter_range.end;
                (text, filter_range, display_range)
            }
            lsp::SymbolKind::MODULE => {
                let text = format!("mod {} {{}}", name);
                let filter_range = 4..4 + name.len();
                let display_range = 0..filter_range.end;
                (text, filter_range, display_range)
            }
            lsp::SymbolKind::TYPE_PARAMETER => {
                let text = format!("type {} {{}}", name);
                let filter_range = 5..5 + name.len();
                let display_range = 0..filter_range.end;
                (text, filter_range, display_range)
            }
            _ => return None,
        };

        Some(CodeLabel {
            runs: language.highlight_text(&text.as_str().into(), display_range.clone()),
            text: text[display_range].to_string(),
            filter_range,
        })
    }

    fn prepare_initialize_params(
        &self,
        mut original: InitializeParams,
        cx: &App,
    ) -> Result<InitializeParams> {
        let enable_lsp_tasks = ProjectSettings::get_global(cx)
            .lsp
            .get(&SERVER_NAME)
            .map_or(false, |s| s.enable_lsp_tasks);
        if enable_lsp_tasks {
            let experimental = json!({
                "runnables": {
                    "kinds": [ "cargo", "shell" ],
                },
            });
            if let Some(original_experimental) = &mut original.capabilities.experimental {
                merge_json_value_into(experimental, original_experimental);
            } else {
                original.capabilities.experimental = Some(experimental);
            }
        }

        let cargo_diagnostics_fetched_separately = ProjectSettings::get_global(cx)
            .diagnostics
            .fetch_cargo_diagnostics();
        if cargo_diagnostics_fetched_separately {
            let disable_check_on_save = json!({
                "checkOnSave": false,
            });
            if let Some(initialization_options) = &mut original.initialization_options {
                merge_json_value_into(disable_check_on_save, initialization_options);
            } else {
                original.initialization_options = Some(disable_check_on_save);
            }
        }

        Ok(original)
    }
}

pub(crate) struct RustContextProvider;

const RUST_PACKAGE_TASK_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("RUST_PACKAGE"));

/// The bin name corresponding to the current file in Cargo.toml
const RUST_BIN_NAME_TASK_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("RUST_BIN_NAME"));

/// The bin kind (bin/example) corresponding to the current file in Cargo.toml
const RUST_BIN_KIND_TASK_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("RUST_BIN_KIND"));

/// The flag to list required features for executing a bin, if any
const RUST_BIN_REQUIRED_FEATURES_FLAG_TASK_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("RUST_BIN_REQUIRED_FEATURES_FLAG"));

/// The list of required features for executing a bin, if any
const RUST_BIN_REQUIRED_FEATURES_TASK_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("RUST_BIN_REQUIRED_FEATURES"));

const RUST_TEST_FRAGMENT_TASK_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("RUST_TEST_FRAGMENT"));

const RUST_DOC_TEST_NAME_TASK_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("RUST_DOC_TEST_NAME"));

const RUST_TEST_NAME_TASK_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("RUST_TEST_NAME"));

const RUST_MANIFEST_DIRNAME_TASK_VARIABLE: VariableName =
    VariableName::Custom(Cow::Borrowed("RUST_MANIFEST_DIRNAME"));

impl ContextProvider for RustContextProvider {
    fn build_context(
        &self,
        task_variables: &TaskVariables,
        location: ContextLocation<'_>,
        project_env: Option<HashMap<String, String>>,
        _: Arc<dyn LanguageToolchainStore>,
        cx: &mut gpui::App,
    ) -> Task<Result<TaskVariables>> {
        let local_abs_path = location
            .file_location
            .buffer
            .read(cx)
            .file()
            .and_then(|file| Some(file.as_local()?.abs_path(cx)));

        let mut variables = TaskVariables::default();

        if let (Some(path), Some(stem)) = (&local_abs_path, task_variables.get(&VariableName::Stem))
        {
            let fragment = test_fragment(&variables, &path, stem);
            variables.insert(RUST_TEST_FRAGMENT_TASK_VARIABLE, fragment);
        };
        if let Some(test_name) =
            task_variables.get(&VariableName::Custom(Cow::Borrowed("_test_name")))
        {
            variables.insert(RUST_TEST_NAME_TASK_VARIABLE, test_name.into());
        }
        if let Some(doc_test_name) =
            task_variables.get(&VariableName::Custom(Cow::Borrowed("_doc_test_name")))
        {
            variables.insert(RUST_DOC_TEST_NAME_TASK_VARIABLE, doc_test_name.into());
        }
        cx.background_spawn(async move {
            if let Some(path) = local_abs_path
                .as_deref()
                .and_then(|local_abs_path| local_abs_path.parent())
            {
                if let Some(package_name) =
                    human_readable_package_name(path, project_env.as_ref()).await
                {
                    variables.insert(RUST_PACKAGE_TASK_VARIABLE.clone(), package_name);
                }
            }
            if let Some(path) = local_abs_path.as_ref()
                && let Some((target, manifest_path)) =
                    target_info_from_abs_path(&path, project_env.as_ref()).await
            {
                if let Some(target) = target {
                    variables.extend(TaskVariables::from_iter([
                        (RUST_PACKAGE_TASK_VARIABLE.clone(), target.package_name),
                        (RUST_BIN_NAME_TASK_VARIABLE.clone(), target.target_name),
                        (
                            RUST_BIN_KIND_TASK_VARIABLE.clone(),
                            target.target_kind.to_string(),
                        ),
                    ]));
                    if target.required_features.is_empty() {
                        variables.insert(RUST_BIN_REQUIRED_FEATURES_FLAG_TASK_VARIABLE, "".into());
                        variables.insert(RUST_BIN_REQUIRED_FEATURES_TASK_VARIABLE, "".into());
                    } else {
                        variables.insert(
                            RUST_BIN_REQUIRED_FEATURES_FLAG_TASK_VARIABLE.clone(),
                            "--features".to_string(),
                        );
                        variables.insert(
                            RUST_BIN_REQUIRED_FEATURES_TASK_VARIABLE.clone(),
                            target.required_features.join(","),
                        );
                    }
                }
                variables.extend(TaskVariables::from_iter([(
                    RUST_MANIFEST_DIRNAME_TASK_VARIABLE.clone(),
                    manifest_path.to_string_lossy().into_owned(),
                )]));
            }
            Ok(variables)
        })
    }

    fn associated_tasks(
        &self,
        _: Arc<dyn Fs>,
        file: Option<Arc<dyn language::File>>,
        cx: &App,
    ) -> Task<Option<TaskTemplates>> {
        const DEFAULT_RUN_NAME_STR: &str = "RUST_DEFAULT_PACKAGE_RUN";
        const CUSTOM_TARGET_DIR: &str = "RUST_TARGET_DIR";

        let language_sets = language_settings(Some("Rust".into()), file.as_ref(), cx);
        let package_to_run = language_sets
            .tasks
            .variables
            .get(DEFAULT_RUN_NAME_STR)
            .cloned();
        let custom_target_dir = language_sets
            .tasks
            .variables
            .get(CUSTOM_TARGET_DIR)
            .cloned();
        let run_task_args = if let Some(package_to_run) = package_to_run.clone() {
            vec!["run".into(), "-p".into(), package_to_run]
        } else {
            vec!["run".into()]
        };
        let mut task_templates = vec![
            TaskTemplate {
                label: format!(
                    "Check (package: {})",
                    RUST_PACKAGE_TASK_VARIABLE.template_value(),
                ),
                command: "cargo".into(),
                args: vec![
                    "check".into(),
                    "-p".into(),
                    RUST_PACKAGE_TASK_VARIABLE.template_value(),
                ],
                cwd: Some("$ZED_DIRNAME".to_owned()),
                ..TaskTemplate::default()
            },
            TaskTemplate {
                label: "Check all targets (workspace)".into(),
                command: "cargo".into(),
                args: vec!["check".into(), "--workspace".into(), "--all-targets".into()],
                cwd: Some("$ZED_DIRNAME".to_owned()),
                ..TaskTemplate::default()
            },
            TaskTemplate {
                label: format!(
                    "Test '{}' (package: {})",
                    RUST_TEST_NAME_TASK_VARIABLE.template_value(),
                    RUST_PACKAGE_TASK_VARIABLE.template_value(),
                ),
                command: "cargo".into(),
                args: vec![
                    "test".into(),
                    "-p".into(),
                    RUST_PACKAGE_TASK_VARIABLE.template_value(),
                    "--".into(),
                    "--nocapture".into(),
                    "--include-ignored".into(),
                    RUST_TEST_NAME_TASK_VARIABLE.template_value(),
                ],
                tags: vec!["rust-test".to_owned()],
                cwd: Some(RUST_MANIFEST_DIRNAME_TASK_VARIABLE.template_value()),
                ..TaskTemplate::default()
            },
            TaskTemplate {
                label: format!(
                    "Doc test '{}' (package: {})",
                    RUST_DOC_TEST_NAME_TASK_VARIABLE.template_value(),
                    RUST_PACKAGE_TASK_VARIABLE.template_value(),
                ),
                command: "cargo".into(),
                args: vec![
                    "test".into(),
                    "--doc".into(),
                    "-p".into(),
                    RUST_PACKAGE_TASK_VARIABLE.template_value(),
                    "--".into(),
                    "--nocapture".into(),
                    "--include-ignored".into(),
                    RUST_DOC_TEST_NAME_TASK_VARIABLE.template_value(),
                ],
                tags: vec!["rust-doc-test".to_owned()],
                cwd: Some(RUST_MANIFEST_DIRNAME_TASK_VARIABLE.template_value()),
                ..TaskTemplate::default()
            },
            TaskTemplate {
                label: format!(
                    "Test mod '{}' (package: {})",
                    VariableName::Stem.template_value(),
                    RUST_PACKAGE_TASK_VARIABLE.template_value(),
                ),
                command: "cargo".into(),
                args: vec![
                    "test".into(),
                    "-p".into(),
                    RUST_PACKAGE_TASK_VARIABLE.template_value(),
                    "--".into(),
                    RUST_TEST_FRAGMENT_TASK_VARIABLE.template_value(),
                ],
                tags: vec!["rust-mod-test".to_owned()],
                cwd: Some(RUST_MANIFEST_DIRNAME_TASK_VARIABLE.template_value()),
                ..TaskTemplate::default()
            },
            TaskTemplate {
                label: format!(
                    "Run {} {} (package: {})",
                    RUST_BIN_KIND_TASK_VARIABLE.template_value(),
                    RUST_BIN_NAME_TASK_VARIABLE.template_value(),
                    RUST_PACKAGE_TASK_VARIABLE.template_value(),
                ),
                command: "cargo".into(),
                args: vec![
                    "run".into(),
                    "-p".into(),
                    RUST_PACKAGE_TASK_VARIABLE.template_value(),
                    format!("--{}", RUST_BIN_KIND_TASK_VARIABLE.template_value()),
                    RUST_BIN_NAME_TASK_VARIABLE.template_value(),
                    RUST_BIN_REQUIRED_FEATURES_FLAG_TASK_VARIABLE.template_value(),
                    RUST_BIN_REQUIRED_FEATURES_TASK_VARIABLE.template_value(),
                ],
                cwd: Some("$ZED_DIRNAME".to_owned()),
                tags: vec!["rust-main".to_owned()],
                ..TaskTemplate::default()
            },
            TaskTemplate {
                label: format!(
                    "Test (package: {})",
                    RUST_PACKAGE_TASK_VARIABLE.template_value()
                ),
                command: "cargo".into(),
                args: vec![
                    "test".into(),
                    "-p".into(),
                    RUST_PACKAGE_TASK_VARIABLE.template_value(),
                ],
                cwd: Some(RUST_MANIFEST_DIRNAME_TASK_VARIABLE.template_value()),
                ..TaskTemplate::default()
            },
            TaskTemplate {
                label: "Run".into(),
                command: "cargo".into(),
                args: run_task_args,
                cwd: Some("$ZED_DIRNAME".to_owned()),
                ..TaskTemplate::default()
            },
            TaskTemplate {
                label: "Clean".into(),
                command: "cargo".into(),
                args: vec!["clean".into()],
                cwd: Some("$ZED_DIRNAME".to_owned()),
                ..TaskTemplate::default()
            },
        ];

        if let Some(custom_target_dir) = custom_target_dir {
            task_templates = task_templates
                .into_iter()
                .map(|mut task_template| {
                    let mut args = task_template.args.split_off(1);
                    task_template.args.append(&mut vec![
                        "--target-dir".to_string(),
                        custom_target_dir.clone(),
                    ]);
                    task_template.args.append(&mut args);

                    task_template
                })
                .collect();
        }

        Task::ready(Some(TaskTemplates(task_templates)))
    }

    fn lsp_task_source(&self) -> Option<LanguageServerName> {
        Some(SERVER_NAME)
    }
}

/// Part of the data structure of Cargo metadata
#[derive(Debug, serde::Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
}

#[derive(Debug, serde::Deserialize)]
struct CargoPackage {
    id: String,
    targets: Vec<CargoTarget>,
    manifest_path: Arc<Path>,
}

#[derive(Debug, serde::Deserialize)]
struct CargoTarget {
    name: String,
    kind: Vec<String>,
    src_path: String,
    #[serde(rename = "required-features", default)]
    required_features: Vec<String>,
}

#[derive(Debug, PartialEq)]
enum TargetKind {
    Bin,
    Example,
}

impl Display for TargetKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetKind::Bin => write!(f, "bin"),
            TargetKind::Example => write!(f, "example"),
        }
    }
}

impl TryFrom<&str> for TargetKind {
    type Error = ();
    fn try_from(value: &str) -> Result<Self, ()> {
        match value {
            "bin" => Ok(Self::Bin),
            "example" => Ok(Self::Example),
            _ => Err(()),
        }
    }
}
/// Which package and binary target are we in?
#[derive(Debug, PartialEq)]
struct TargetInfo {
    package_name: String,
    target_name: String,
    target_kind: TargetKind,
    required_features: Vec<String>,
}

async fn target_info_from_abs_path(
    abs_path: &Path,
    project_env: Option<&HashMap<String, String>>,
) -> Option<(Option<TargetInfo>, Arc<Path>)> {
    let mut command = util::command::new_smol_command("cargo");
    if let Some(envs) = project_env {
        command.envs(envs);
    }
    let output = command
        .current_dir(abs_path.parent()?)
        .arg("metadata")
        .arg("--no-deps")
        .arg("--format-version")
        .arg("1")
        .output()
        .await
        .log_err()?
        .stdout;

    let metadata: CargoMetadata = serde_json::from_slice(&output).log_err()?;
    target_info_from_metadata(metadata, abs_path)
}

fn target_info_from_metadata(
    metadata: CargoMetadata,
    abs_path: &Path,
) -> Option<(Option<TargetInfo>, Arc<Path>)> {
    let mut manifest_path = None;
    for package in metadata.packages {
        let Some(manifest_dir_path) = package.manifest_path.parent() else {
            continue;
        };

        let Some(path_from_manifest_dir) = abs_path.strip_prefix(manifest_dir_path).ok() else {
            continue;
        };
        let candidate_path_length = path_from_manifest_dir.components().count();
        // Pick the most specific manifest path
        if let Some((path, current_length)) = &mut manifest_path {
            if candidate_path_length > *current_length {
                *path = Arc::from(manifest_dir_path);
                *current_length = candidate_path_length;
            }
        } else {
            manifest_path = Some((Arc::from(manifest_dir_path), candidate_path_length));
        };

        for target in package.targets {
            let Some(bin_kind) = target
                .kind
                .iter()
                .find_map(|kind| TargetKind::try_from(kind.as_ref()).ok())
            else {
                continue;
            };
            let target_path = PathBuf::from(target.src_path);
            if target_path == abs_path {
                return manifest_path.map(|(path, _)| {
                    (
                        package_name_from_pkgid(&package.id).map(|package_name| TargetInfo {
                            package_name: package_name.to_owned(),
                            target_name: target.name,
                            required_features: target.required_features,
                            target_kind: bin_kind,
                        }),
                        path,
                    )
                });
            }
        }
    }

    manifest_path.map(|(path, _)| (None, path))
}

async fn human_readable_package_name(
    package_directory: &Path,
    project_env: Option<&HashMap<String, String>>,
) -> Option<String> {
    let mut command = util::command::new_smol_command("cargo");
    if let Some(envs) = project_env {
        command.envs(envs);
    }
    let pkgid = String::from_utf8(
        command
            .current_dir(package_directory)
            .arg("pkgid")
            .output()
            .await
            .log_err()?
            .stdout,
    )
    .ok()?;
    Some(package_name_from_pkgid(&pkgid)?.to_owned())
}

// For providing local `cargo check -p $pkgid` task, we do not need most of the information we have returned.
// Output example in the root of Zed project:
// ```sh
// ❯ cargo pkgid zed
// path+file:///absolute/path/to/project/zed/crates/zed#0.131.0
// ```
// Another variant, if a project has a custom package name or hyphen in the name:
// ```
// path+file:///absolute/path/to/project/custom-package#my-custom-package@0.1.0
// ```
//
// Extracts the package name from the output according to the spec:
// https://doc.rust-lang.org/cargo/reference/pkgid-spec.html#specification-grammar
fn package_name_from_pkgid(pkgid: &str) -> Option<&str> {
    fn split_off_suffix(input: &str, suffix_start: char) -> &str {
        match input.rsplit_once(suffix_start) {
            Some((without_suffix, _)) => without_suffix,
            None => input,
        }
    }

    let (version_prefix, version_suffix) = pkgid.trim().rsplit_once('#')?;
    let package_name = match version_suffix.rsplit_once('@') {
        Some((custom_package_name, _version)) => custom_package_name,
        None => {
            let host_and_path = split_off_suffix(version_prefix, '?');
            let (_, package_name) = host_and_path.rsplit_once('/')?;
            package_name
        }
    };
    Some(package_name)
}

fn test_fragment(variables: &TaskVariables, path: &Path, stem: &str) -> String {
    let fragment = if stem == "lib" {
        // This isn't quite right---it runs the tests for the entire library, rather than
        // just for the top-level `mod tests`. But we don't really have the means here to
        // filter out just that module.
        Some("--lib".to_owned())
    } else if stem == "mod" {
        maybe!({ Some(path.parent()?.file_name()?.to_string_lossy().to_string()) })
    } else if stem == "main" {
        if let (Some(bin_name), Some(bin_kind)) = (
            variables.get(&RUST_BIN_NAME_TASK_VARIABLE),
            variables.get(&RUST_BIN_KIND_TASK_VARIABLE),
        ) {
            Some(format!("--{bin_kind}={bin_name}"))
        } else {
            None
        }
    } else {
        Some(stem.to_owned())
    };
    fragment.unwrap_or_else(|| "--".to_owned())
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::*;
    use crate::language;
    use anyhow::Context as _;
    use gpui::{BorrowAppContext, Hsla, TestAppContext};
    use language::language_settings::AllLanguageSettings;
    use lsp::CompletionItemLabelDetails;
    use settings::SettingsStore;
    use theme::SyntaxTheme;
    use util::path;

    #[gpui::test]
    async fn test_process_rust_diagnostics() {
        let mut params = lsp::PublishDiagnosticsParams {
            uri: lsp::Url::from_file_path(path!("/a")).unwrap(),
            version: None,
            diagnostics: vec![
                // no newlines
                lsp::Diagnostic {
                    message: "use of moved value `a`".to_string(),
                    ..Default::default()
                },
                // newline at the end of a code span
                lsp::Diagnostic {
                    message: "consider importing this struct: `use b::c;\n`".to_string(),
                    ..Default::default()
                },
                // code span starting right after a newline
                lsp::Diagnostic {
                    message: "cannot borrow `self.d` as mutable\n`self` is a `&` reference"
                        .to_string(),
                    ..Default::default()
                },
            ],
        };
        RustLspAdapter.process_diagnostics(&mut params, LanguageServerId(0), None);

        assert_eq!(params.diagnostics[0].message, "use of moved value `a`");

        // remove trailing newline from code span
        assert_eq!(
            params.diagnostics[1].message,
            "consider importing this struct: `use b::c;`"
        );

        // do not remove newline before the start of code span
        assert_eq!(
            params.diagnostics[2].message,
            "cannot borrow `self.d` as mutable\n`self` is a `&` reference"
        );
    }

    #[gpui::test]
    async fn test_rust_label_for_completion() {
        let adapter = Arc::new(RustLspAdapter);
        let language = language("rust", tree_sitter_rust::LANGUAGE.into());
        let grammar = language.grammar().unwrap();
        let theme = SyntaxTheme::new_test([
            ("type", Hsla::default()),
            ("keyword", Hsla::default()),
            ("function", Hsla::default()),
            ("property", Hsla::default()),
        ]);

        language.set_theme(&theme);

        let highlight_function = grammar.highlight_id_for_name("function").unwrap();
        let highlight_type = grammar.highlight_id_for_name("type").unwrap();
        let highlight_keyword = grammar.highlight_id_for_name("keyword").unwrap();
        let highlight_field = grammar.highlight_id_for_name("property").unwrap();

        assert_eq!(
            adapter
                .label_for_completion(
                    &lsp::CompletionItem {
                        kind: Some(lsp::CompletionItemKind::FUNCTION),
                        label: "hello(…)".to_string(),
                        label_details: Some(CompletionItemLabelDetails {
                            detail: Some("(use crate::foo)".into()),
                            description: Some("fn(&mut Option<T>) -> Vec<T>".to_string())
                        }),
                        ..Default::default()
                    },
                    &language
                )
                .await,
            Some(CodeLabel {
                text: "hello(&mut Option<T>) -> Vec<T> (use crate::foo)".to_string(),
                filter_range: 0..5,
                runs: vec![
                    (0..5, highlight_function),
                    (7..10, highlight_keyword),
                    (11..17, highlight_type),
                    (18..19, highlight_type),
                    (25..28, highlight_type),
                    (29..30, highlight_type),
                ],
            })
        );
        assert_eq!(
            adapter
                .label_for_completion(
                    &lsp::CompletionItem {
                        kind: Some(lsp::CompletionItemKind::FUNCTION),
                        label: "hello(…)".to_string(),
                        label_details: Some(CompletionItemLabelDetails {
                            detail: Some(" (use crate::foo)".into()),
                            description: Some("async fn(&mut Option<T>) -> Vec<T>".to_string()),
                        }),
                        ..Default::default()
                    },
                    &language
                )
                .await,
            Some(CodeLabel {
                text: "hello(&mut Option<T>) -> Vec<T> (use crate::foo)".to_string(),
                filter_range: 0..5,
                runs: vec![
                    (0..5, highlight_function),
                    (7..10, highlight_keyword),
                    (11..17, highlight_type),
                    (18..19, highlight_type),
                    (25..28, highlight_type),
                    (29..30, highlight_type),
                ],
            })
        );
        assert_eq!(
            adapter
                .label_for_completion(
                    &lsp::CompletionItem {
                        kind: Some(lsp::CompletionItemKind::FIELD),
                        label: "len".to_string(),
                        detail: Some("usize".to_string()),
                        ..Default::default()
                    },
                    &language
                )
                .await,
            Some(CodeLabel {
                text: "len: usize".to_string(),
                filter_range: 0..3,
                runs: vec![(0..3, highlight_field), (5..10, highlight_type),],
            })
        );

        assert_eq!(
            adapter
                .label_for_completion(
                    &lsp::CompletionItem {
                        kind: Some(lsp::CompletionItemKind::FUNCTION),
                        label: "hello(…)".to_string(),
                        label_details: Some(CompletionItemLabelDetails {
                            detail: Some(" (use crate::foo)".to_string()),
                            description: Some("fn(&mut Option<T>) -> Vec<T>".to_string()),
                        }),

                        ..Default::default()
                    },
                    &language
                )
                .await,
            Some(CodeLabel {
                text: "hello(&mut Option<T>) -> Vec<T> (use crate::foo)".to_string(),
                filter_range: 0..5,
                runs: vec![
                    (0..5, highlight_function),
                    (7..10, highlight_keyword),
                    (11..17, highlight_type),
                    (18..19, highlight_type),
                    (25..28, highlight_type),
                    (29..30, highlight_type),
                ],
            })
        );

        assert_eq!(
            adapter
                .label_for_completion(
                    &lsp::CompletionItem {
                        kind: Some(lsp::CompletionItemKind::METHOD),
                        label: "await.as_deref_mut()".to_string(),
                        filter_text: Some("as_deref_mut".to_string()),
                        label_details: Some(CompletionItemLabelDetails {
                            detail: None,
                            description: Some("fn(&mut self) -> IterMut<'_, T>".to_string()),
                        }),
                        ..Default::default()
                    },
                    &language
                )
                .await,
            Some(CodeLabel {
                text: "await.as_deref_mut()".to_string(),
                filter_range: 6..18,
                runs: vec![],
            })
        );

        assert_eq!(
            adapter
                .label_for_completion(
                    &lsp::CompletionItem {
                        kind: Some(lsp::CompletionItemKind::FIELD),
                        label: "inner_value".to_string(),
                        filter_text: Some("value".to_string()),
                        detail: Some("String".to_string()),
                        ..Default::default()
                    },
                    &language,
                )
                .await,
            Some(CodeLabel {
                text: "inner_value: String".to_string(),
                filter_range: 6..11,
                runs: vec![(0..11, HighlightId(3)), (13..19, HighlightId(0))],
            })
        );
    }

    #[gpui::test]
    async fn test_rust_label_for_symbol() {
        let adapter = Arc::new(RustLspAdapter);
        let language = language("rust", tree_sitter_rust::LANGUAGE.into());
        let grammar = language.grammar().unwrap();
        let theme = SyntaxTheme::new_test([
            ("type", Hsla::default()),
            ("keyword", Hsla::default()),
            ("function", Hsla::default()),
            ("property", Hsla::default()),
        ]);

        language.set_theme(&theme);

        let highlight_function = grammar.highlight_id_for_name("function").unwrap();
        let highlight_type = grammar.highlight_id_for_name("type").unwrap();
        let highlight_keyword = grammar.highlight_id_for_name("keyword").unwrap();

        assert_eq!(
            adapter
                .label_for_symbol("hello", lsp::SymbolKind::FUNCTION, &language)
                .await,
            Some(CodeLabel {
                text: "fn hello".to_string(),
                filter_range: 3..8,
                runs: vec![(0..2, highlight_keyword), (3..8, highlight_function)],
            })
        );

        assert_eq!(
            adapter
                .label_for_symbol("World", lsp::SymbolKind::TYPE_PARAMETER, &language)
                .await,
            Some(CodeLabel {
                text: "type World".to_string(),
                filter_range: 5..10,
                runs: vec![(0..4, highlight_keyword), (5..10, highlight_type)],
            })
        );
    }

    #[gpui::test]
    async fn test_rust_autoindent(cx: &mut TestAppContext) {
        // cx.executor().set_block_on_ticks(usize::MAX..=usize::MAX);
        cx.update(|cx| {
            let test_settings = SettingsStore::test(cx);
            cx.set_global(test_settings);
            language::init(cx);
            cx.update_global::<SettingsStore, _>(|store, cx| {
                store.update_user_settings::<AllLanguageSettings>(cx, |s| {
                    s.defaults.tab_size = NonZeroU32::new(2);
                });
            });
        });

        let language = crate::language("rust", tree_sitter_rust::LANGUAGE.into());

        cx.new(|cx| {
            let mut buffer = Buffer::local("", cx).with_language(language, cx);

            // indent between braces
            buffer.set_text("fn a() {}", cx);
            let ix = buffer.len() - 1;
            buffer.edit([(ix..ix, "\n\n")], Some(AutoindentMode::EachLine), cx);
            assert_eq!(buffer.text(), "fn a() {\n  \n}");

            // indent between braces, even after empty lines
            buffer.set_text("fn a() {\n\n\n}", cx);
            let ix = buffer.len() - 2;
            buffer.edit([(ix..ix, "\n")], Some(AutoindentMode::EachLine), cx);
            assert_eq!(buffer.text(), "fn a() {\n\n\n  \n}");

            // indent a line that continues a field expression
            buffer.set_text("fn a() {\n  \n}", cx);
            let ix = buffer.len() - 2;
            buffer.edit([(ix..ix, "b\n.c")], Some(AutoindentMode::EachLine), cx);
            assert_eq!(buffer.text(), "fn a() {\n  b\n    .c\n}");

            // indent further lines that continue the field expression, even after empty lines
            let ix = buffer.len() - 2;
            buffer.edit([(ix..ix, "\n\n.d")], Some(AutoindentMode::EachLine), cx);
            assert_eq!(buffer.text(), "fn a() {\n  b\n    .c\n    \n    .d\n}");

            // dedent the line after the field expression
            let ix = buffer.len() - 2;
            buffer.edit([(ix..ix, ";\ne")], Some(AutoindentMode::EachLine), cx);
            assert_eq!(
                buffer.text(),
                "fn a() {\n  b\n    .c\n    \n    .d;\n  e\n}"
            );

            // indent inside a struct within a call
            buffer.set_text("const a: B = c(D {});", cx);
            let ix = buffer.len() - 3;
            buffer.edit([(ix..ix, "\n\n")], Some(AutoindentMode::EachLine), cx);
            assert_eq!(buffer.text(), "const a: B = c(D {\n  \n});");

            // indent further inside a nested call
            let ix = buffer.len() - 4;
            buffer.edit([(ix..ix, "e: f(\n\n)")], Some(AutoindentMode::EachLine), cx);
            assert_eq!(buffer.text(), "const a: B = c(D {\n  e: f(\n    \n  )\n});");

            // keep that indent after an empty line
            let ix = buffer.len() - 8;
            buffer.edit([(ix..ix, "\n")], Some(AutoindentMode::EachLine), cx);
            assert_eq!(
                buffer.text(),
                "const a: B = c(D {\n  e: f(\n    \n    \n  )\n});"
            );

            buffer
        });
    }

    #[test]
    fn test_package_name_from_pkgid() {
        for (input, expected) in [
            (
                "path+file:///absolute/path/to/project/zed/crates/zed#0.131.0",
                "zed",
            ),
            (
                "path+file:///absolute/path/to/project/custom-package#my-custom-package@0.1.0",
                "my-custom-package",
            ),
        ] {
            assert_eq!(package_name_from_pkgid(input), Some(expected));
        }
    }

    #[test]
    fn test_target_info_from_metadata() {
        for (input, absolute_path, expected) in [
            (
                r#"{"packages":[{"id":"path+file:///absolute/path/to/project/zed/crates/zed#0.131.0","manifest_path":"/path/to/zed/Cargo.toml","targets":[{"name":"zed","kind":["bin"],"src_path":"/path/to/zed/src/main.rs"}]}]}"#,
                "/path/to/zed/src/main.rs",
                Some((
                    Some(TargetInfo {
                        package_name: "zed".into(),
                        target_name: "zed".into(),
                        required_features: Vec::new(),
                        target_kind: TargetKind::Bin,
                    }),
                    Arc::from("/path/to/zed".as_ref()),
                )),
            ),
            (
                r#"{"packages":[{"id":"path+file:///path/to/custom-package#my-custom-package@0.1.0","manifest_path":"/path/to/custom-package/Cargo.toml","targets":[{"name":"my-custom-bin","kind":["bin"],"src_path":"/path/to/custom-package/src/main.rs"}]}]}"#,
                "/path/to/custom-package/src/main.rs",
                Some((
                    Some(TargetInfo {
                        package_name: "my-custom-package".into(),
                        target_name: "my-custom-bin".into(),
                        required_features: Vec::new(),
                        target_kind: TargetKind::Bin,
                    }),
                    Arc::from("/path/to/custom-package".as_ref()),
                )),
            ),
            (
                r#"{"packages":[{"id":"path+file:///path/to/custom-package#my-custom-package@0.1.0","targets":[{"name":"my-custom-bin","kind":["example"],"src_path":"/path/to/custom-package/src/main.rs"}],"manifest_path":"/path/to/custom-package/Cargo.toml"}]}"#,
                "/path/to/custom-package/src/main.rs",
                Some((
                    Some(TargetInfo {
                        package_name: "my-custom-package".into(),
                        target_name: "my-custom-bin".into(),
                        required_features: Vec::new(),
                        target_kind: TargetKind::Example,
                    }),
                    Arc::from("/path/to/custom-package".as_ref()),
                )),
            ),
            (
                r#"{"packages":[{"id":"path+file:///path/to/custom-package#my-custom-package@0.1.0","manifest_path":"/path/to/custom-package/Cargo.toml","targets":[{"name":"my-custom-bin","kind":["example"],"src_path":"/path/to/custom-package/src/main.rs","required-features":["foo","bar"]}]}]}"#,
                "/path/to/custom-package/src/main.rs",
                Some((
                    Some(TargetInfo {
                        package_name: "my-custom-package".into(),
                        target_name: "my-custom-bin".into(),
                        required_features: vec!["foo".to_owned(), "bar".to_owned()],
                        target_kind: TargetKind::Example,
                    }),
                    Arc::from("/path/to/custom-package".as_ref()),
                )),
            ),
            (
                r#"{"packages":[{"id":"path+file:///path/to/custom-package#my-custom-package@0.1.0","targets":[{"name":"my-custom-bin","kind":["example"],"src_path":"/path/to/custom-package/src/main.rs","required-features":[]}],"manifest_path":"/path/to/custom-package/Cargo.toml"}]}"#,
                "/path/to/custom-package/src/main.rs",
                Some((
                    Some(TargetInfo {
                        package_name: "my-custom-package".into(),
                        target_name: "my-custom-bin".into(),
                        required_features: vec![],
                        target_kind: TargetKind::Example,
                    }),
                    Arc::from("/path/to/custom-package".as_ref()),
                )),
            ),
            (
                r#"{"packages":[{"id":"path+file:///path/to/custom-package#my-custom-package@0.1.0","targets":[{"name":"my-custom-package","kind":["lib"],"src_path":"/path/to/custom-package/src/main.rs"}],"manifest_path":"/path/to/custom-package/Cargo.toml"}]}"#,
                "/path/to/custom-package/src/main.rs",
                Some((None, Arc::from("/path/to/custom-package".as_ref()))),
            ),
        ] {
            let metadata: CargoMetadata = serde_json::from_str(input).context(input).unwrap();

            let absolute_path = Path::new(absolute_path);

            assert_eq!(target_info_from_metadata(metadata, absolute_path), expected);
        }
    }

    #[test]
    fn test_rust_test_fragment() {
        #[track_caller]
        fn check(
            variables: impl IntoIterator<Item = (VariableName, &'static str)>,
            path: &str,
            expected: &str,
        ) {
            let path = Path::new(path);
            let found = test_fragment(
                &TaskVariables::from_iter(variables.into_iter().map(|(k, v)| (k, v.to_owned()))),
                path,
                &path.file_stem().unwrap().to_str().unwrap(),
            );
            assert_eq!(expected, found);
        }

        check([], "/project/src/lib.rs", "--lib");
        check([], "/project/src/foo/mod.rs", "foo");
        check(
            [
                (RUST_BIN_KIND_TASK_VARIABLE.clone(), "bin"),
                (RUST_BIN_NAME_TASK_VARIABLE, "x"),
            ],
            "/project/src/main.rs",
            "--bin=x",
        );
        check([], "/project/src/main.rs", "--");
    }
}
