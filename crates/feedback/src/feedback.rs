use gpui::{App, actions};
use workspace::Workspace;
use zed_actions::feedback::FileBugReport;

pub mod feedback_modal;

actions!(
    zed,
    [
        /// Opens the Zed repository on GitHub.
        OpenZedRepo,
        /// Opens the feature request form.
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
