use gpui::{App, ClipboardItem, PromptLevel, actions};
use system_specs::SystemSpecs;
use util::ResultExt;
use workspace::Workspace;
use zed_actions::feedback::FileBugReport;

pub mod feedback_modal;

pub mod system_specs;

actions!(
    zed,
    [
        CopySystemSpecsIntoClipboard,
        OpenZedRepo,
        RequestFeature,
    ]
);

const ZED_REPO_URL: &str = "https://github.com/zedless-editor/zed";

const REQUEST_FEATURE_URL: &str = "https://github.com/zedless-editor/zed/discussions/new/choose";

const BUG_REPORT_URL: &str = "https://github.com/zedless-editor/zed/issues/new";

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };
        feedback_modal::FeedbackModal::register(workspace, window, cx);
        workspace
            .register_action(|_, _: &CopySystemSpecsIntoClipboard, window, cx| {
                let specs = SystemSpecs::new(window, cx);

                cx.spawn_in(window, async move |_, cx| {
                    let specs = specs.await.to_string();

                    cx.update(|_, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(specs.clone()))
                    })
                    .log_err();

                    cx.prompt(
                        PromptLevel::Info,
                        "Copied into clipboard",
                        Some(&specs),
                        &["OK"],
                    )
                    .await
                })
                .detach();
            })
            .register_action(|_, _: &RequestFeature, _, cx| {
                cx.open_url(REQUEST_FEATURE_URL);
            })
            .register_action(move |_, _: &FileBugReport, _, cx| {
                cx.open_url(BUG_REPORT_URL);
            })
            .register_action(move |_, _: &OpenZedRepo, _, cx| {
                cx.open_url(ZED_REPO_URL);
            });
    })
    .detach();
}
