use std::any::{Any, TypeId};

use client::DisableAiSettings;
use command_palette_hooks::CommandPaletteFilter;
use gpui::actions;
use language::language_settings::{AllLanguageSettings, EditPredictionProvider};
use settings::{Settings, SettingsStore, update_settings_file};
use ui::App;
use workspace::Workspace;

actions!(
    edit_prediction,
    [
        /// Resets the edit prediction onboarding state.
        ResetOnboarding,
    ]
);

pub fn init(cx: &mut App) {
    feature_gate_predict_edits_actions(cx);

    cx.observe_new(move |workspace: &mut Workspace, _, _cx| {
        workspace.register_action(|workspace, _: &ResetOnboarding, _window, cx| {
            update_settings_file::<AllLanguageSettings>(
                workspace.app_state().fs.clone(),
                cx,
                move |file, _| {
                    file.features
                        .get_or_insert(Default::default())
                        .edit_prediction_provider = Some(EditPredictionProvider::None)
                },
            );
        });
    })
    .detach();
}

fn feature_gate_predict_edits_actions(cx: &mut App) {
    let reset_onboarding_action_types = [TypeId::of::<ResetOnboarding>()];
    let zeta_all_action_types = [
        TypeId::of::<ResetOnboarding>(),
        zed_actions::OpenZedPredictOnboarding.type_id(),
        TypeId::of::<crate::ClearHistory>(),
    ];

    CommandPaletteFilter::update_global(cx, |filter, _cx| {
        filter.hide_action_types(&reset_onboarding_action_types);
        filter.hide_action_types(&[zed_actions::OpenZedPredictOnboarding.type_id()]);
    });

    cx.observe_global::<SettingsStore>(move |cx| {
        let is_ai_disabled = DisableAiSettings::get_global(cx).disable_ai;

        CommandPaletteFilter::update_global(cx, |filter, _cx| {
            if is_ai_disabled {
                filter.hide_action_types(&zeta_all_action_types);
            }
        });
    })
    .detach();
}
