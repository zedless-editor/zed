use anyhow::Result;
use client::{DisableAiSettings, UserStore, zed_urls};
use editor::{
    Editor, SelectionEffects,
    actions::{ShowEditPrediction, ToggleEditPrediction},
    scroll::Autoscroll,
};
use feature_flags::{FeatureFlagAppExt, PredictEditsRateCompletionsFeatureFlag};
use fs::Fs;
use gpui::{
    Action, Animation, AnimationExt, App, AsyncWindowContext, Corner, Entity, FocusHandle,
    Focusable, IntoElement, ParentElement, Render, Subscription, WeakEntity, actions, div,
    pulsating_between,
};
use indoc::indoc;
use language::{
    EditPredictionsMode, File, Language,
    language_settings::{self, AllLanguageSettings, EditPredictionProvider, all_language_settings},
};
use regex::Regex;
use settings::{Settings, SettingsStore, update_settings_file};
use zedless_settings::ZedlessSettings;
use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};
use ui::{
    Clickable, ContextMenu, ContextMenuEntry, DocumentationSide, IconButton, IconButtonShape,
    Indicator, PopoverMenu, PopoverMenuHandle, ProgressBar, Tooltip, prelude::*,
};
use workspace::{
    StatusItemView, Workspace, create_and_open_local_file, item::ItemHandle,
};
use zed_llm_client::UsageLimit;

actions!(
    edit_prediction,
    [
        /// Toggles the inline completion menu.
        ToggleMenu
    ]
);

pub struct InlineCompletionButton {
    editor_subscription: Option<(Subscription, usize)>,
    editor_enabled: Option<bool>,
    editor_show_predictions: bool,
    editor_focus_handle: Option<FocusHandle>,
    language: Option<Arc<Language>>,
    file: Option<Arc<dyn File>>,
    edit_prediction_provider: Option<Arc<dyn inline_completion::InlineCompletionProviderHandle>>,
    fs: Arc<dyn Fs>,
    user_store: Entity<UserStore>,
    popover_menu_handle: PopoverMenuHandle<ContextMenu>,
}

impl Render for InlineCompletionButton {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Return empty div if AI is disabled
        if DisableAiSettings::get_global(cx).disable_ai {
            return div();
        }

        let all_language_settings = all_language_settings(None, cx);

        match all_language_settings.edit_predictions.provider {
            EditPredictionProvider::None => div(),

            EditPredictionProvider::Zed => {
                let enabled = self.editor_enabled.unwrap_or(true);

                let zeta_icon = if enabled {
                    IconName::ZedPredict
                } else {
                    IconName::ZedPredictDisabled
                };

                if ZedlessSettings::get_global(cx).zeta_url.is_none() {
                    let tooltip_meta = "Configure Zeta server URL in settings";

                    return div().child(
                        IconButton::new("zed-predict-pending-button", zeta_icon)
                            .shape(IconButtonShape::Square)
                            .indicator(Indicator::dot().color(Color::Muted))
                            .indicator_border_color(Some(cx.theme().colors().status_bar_background))
                            .tooltip(move |window, cx| {
                                Tooltip::with_meta(
                                    "Edit Predictions",
                                    None,
                                    tooltip_meta,
                                    window,
                                    cx,
                                )
                            })
                            .on_click(cx.listener(move |_, _, window, cx| {
                                window.dispatch_action(
                                    zed_actions::OpenSettings.boxed_clone(),
                                    cx,
                                );
                            })),
                    );
                }

                let mut over_limit = false;

                if let Some(usage) = self
                    .edit_prediction_provider
                    .as_ref()
                    .and_then(|provider| provider.usage(cx))
                {
                    over_limit = usage.over_limit()
                }

                let show_editor_predictions = self.editor_show_predictions;

                let icon_button = IconButton::new("zed-predict-pending-button", zeta_icon)
                    .shape(IconButtonShape::Square)
                    .when(
                        enabled && (!show_editor_predictions || over_limit),
                        |this| {
                            this.indicator(Indicator::dot().when_else(
                                over_limit,
                                |dot| dot.color(Color::Error),
                                |dot| dot.color(Color::Muted),
                            ))
                            .indicator_border_color(Some(cx.theme().colors().status_bar_background))
                        },
                    )
                    .when(!self.popover_menu_handle.is_deployed(), |element| {
                        element.tooltip(move |window, cx| {
                            if enabled {
                                if show_editor_predictions {
                                    Tooltip::for_action("Edit Prediction", &ToggleMenu, window, cx)
                                } else {
                                    Tooltip::with_meta(
                                        "Edit Prediction",
                                        Some(&ToggleMenu),
                                        "Hidden For This File",
                                        window,
                                        cx,
                                    )
                                }
                            } else {
                                Tooltip::with_meta(
                                    "Edit Prediction",
                                    Some(&ToggleMenu),
                                    "Disabled For This File",
                                    window,
                                    cx,
                                )
                            }
                        })
                    });

                let this = cx.entity().clone();

                let mut popover_menu = PopoverMenu::new("zeta")
                    .menu(move |window, cx| {
                        Some(this.update(cx, |this, cx| this.build_zeta_context_menu(window, cx)))
                    })
                    .anchor(Corner::BottomRight)
                    .with_handle(self.popover_menu_handle.clone());

                let is_refreshing = self
                    .edit_prediction_provider
                    .as_ref()
                    .map_or(false, |provider| provider.is_refreshing(cx));

                if is_refreshing {
                    popover_menu = popover_menu.trigger(
                        icon_button.with_animation(
                            "pulsating-label",
                            Animation::new(Duration::from_secs(2))
                                .repeat()
                                .with_easing(pulsating_between(0.2, 1.0)),
                            |icon_button, delta| icon_button.alpha(delta),
                        ),
                    );
                } else {
                    popover_menu = popover_menu.trigger(icon_button);
                }

                div().child(popover_menu.into_any_element())
            }
        }
    }
}

impl InlineCompletionButton {
    pub fn new(
        fs: Arc<dyn Fs>,
        user_store: Entity<UserStore>,
        popover_menu_handle: PopoverMenuHandle<ContextMenu>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe_global::<SettingsStore>(move |_, cx| cx.notify())
            .detach();

        Self {
            editor_subscription: None,
            editor_enabled: None,
            editor_show_predictions: true,
            editor_focus_handle: None,
            language: None,
            file: None,
            edit_prediction_provider: None,
            popover_menu_handle,
            fs,
            user_store,
        }
    }

    pub fn build_language_settings_menu(
        &self,
        mut menu: ContextMenu,
        _window: &Window,
        cx: &mut App,
    ) -> ContextMenu {
        let fs = self.fs.clone();

        menu = menu.header("Show Edit Predictions For");

        let language_state = self.language.as_ref().map(|language| {
            (
                language.clone(),
                language_settings::language_settings(Some(language.name()), None, cx)
                    .show_edit_predictions,
            )
        });

        if let Some(editor_focus_handle) = self.editor_focus_handle.clone() {
            let entry = ContextMenuEntry::new("This Buffer")
                .toggleable(IconPosition::Start, self.editor_show_predictions)
                .action(Box::new(ToggleEditPrediction))
                .handler(move |window, cx| {
                    editor_focus_handle.dispatch_action(&ToggleEditPrediction, window, cx);
                });

            match language_state.clone() {
                Some((language, false)) => {
                    menu = menu.item(
                        entry
                            .disabled(true)
                            .documentation_aside(DocumentationSide::Left, move |_cx| {
                                Label::new(format!("Edit predictions cannot be toggled for this buffer because they are disabled for {}", language.name()))
                                    .into_any_element()
                            })
                    );
                }
                Some(_) | None => menu = menu.item(entry),
            }
        }

        if let Some((language, language_enabled)) = language_state {
            let fs = fs.clone();

            menu = menu.toggleable_entry(
                language.name(),
                language_enabled,
                IconPosition::Start,
                None,
                move |_, cx| {
                    toggle_show_inline_completions_for_language(language.clone(), fs.clone(), cx)
                },
            );
        }

        let settings = AllLanguageSettings::get_global(cx);

        let globally_enabled = settings.show_edit_predictions(None, cx);
        menu = menu.toggleable_entry("All Files", globally_enabled, IconPosition::Start, None, {
            let fs = fs.clone();
            move |_, cx| toggle_inline_completions_globally(fs.clone(), cx)
        });

        let provider = settings.edit_predictions.provider;
        let current_mode = settings.edit_predictions_mode();
        let subtle_mode = matches!(current_mode, EditPredictionsMode::Subtle);
        let eager_mode = matches!(current_mode, EditPredictionsMode::Eager);

        if matches!(provider, EditPredictionProvider::Zed) {
            menu = menu
                .separator()
                .header("Display Modes")
                .item(
                    ContextMenuEntry::new("Eager")
                        .toggleable(IconPosition::Start, eager_mode)
                        .documentation_aside(DocumentationSide::Left, move |_| {
                            Label::new("Display predictions inline when there are no language server completions available.").into_any_element()
                        })
                        .handler({
                            let fs = fs.clone();
                            move |_, cx| {
                                toggle_edit_prediction_mode(fs.clone(), EditPredictionsMode::Eager, cx)
                            }
                        }),
                )
                .item(
                    ContextMenuEntry::new("Subtle")
                        .toggleable(IconPosition::Start, subtle_mode)
                        .documentation_aside(DocumentationSide::Left, move |_| {
                            Label::new("Display predictions inline only when holding a modifier key (alt by default).").into_any_element()
                        })
                        .handler({
                            let fs = fs.clone();
                            move |_, cx| {
                                toggle_edit_prediction_mode(fs.clone(), EditPredictionsMode::Subtle, cx)
                            }
                        }),
                );
        }

        menu = menu.item(
            ContextMenuEntry::new("Configure Excluded Files")
                .icon(IconName::LockOutlined)
                .icon_color(Color::Muted)
                .documentation_aside(DocumentationSide::Left, |_| {
                    Label::new(indoc!{"
                        Open your settings to add sensitive paths for which Zed will never predict edits."}).into_any_element()
                })
                .handler(move |window, cx| {
                    if let Some(workspace) = window.root().flatten() {
                        let workspace = workspace.downgrade();
                        window
                            .spawn(cx, async |cx| {
                                open_disabled_globs_setting_in_editor(
                                    workspace,
                                    cx,
                                ).await
                            })
                            .detach_and_log_err(cx);
                    }
                }),
        );

        if !self.editor_enabled.unwrap_or(true) {
            menu = menu.item(
                ContextMenuEntry::new("This file is excluded.")
                    .disabled(true)
                    .icon(IconName::ZedPredictDisabled)
                    .icon_size(IconSize::Small),
            );
        }

        if let Some(editor_focus_handle) = self.editor_focus_handle.clone() {
            menu = menu
                .separator()
                .entry(
                    "Predict Edit at Cursor",
                    Some(Box::new(ShowEditPrediction)),
                    {
                        let editor_focus_handle = editor_focus_handle.clone();
                        move |window, cx| {
                            editor_focus_handle.dispatch_action(&ShowEditPrediction, window, cx);
                        }
                    },
                )
                .context(editor_focus_handle);
        }

        menu
    }

    fn build_zeta_context_menu(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ContextMenu> {
        ContextMenu::build(window, cx, |mut menu, window, cx| {
            if let Some(usage) = self
                .edit_prediction_provider
                .as_ref()
                .and_then(|provider| provider.usage(cx))
            {
                menu = menu.header("Usage");
                menu = menu
                    .custom_entry(
                        move |_window, cx| {
                            let used_percentage = match usage.limit {
                                UsageLimit::Limited(limit) => {
                                    Some((usage.amount as f32 / limit as f32) * 100.)
                                }
                                UsageLimit::Unlimited => None,
                            };

                            h_flex()
                                .flex_1()
                                .gap_1p5()
                                .children(
                                    used_percentage.map(|percent| {
                                        ProgressBar::new("usage", percent, 100., cx)
                                    }),
                                )
                                .child(
                                    Label::new(match usage.limit {
                                        UsageLimit::Limited(limit) => {
                                            format!("{} / {limit}", usage.amount)
                                        }
                                        UsageLimit::Unlimited => format!("{} / ∞", usage.amount),
                                    })
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                                )
                                .into_any_element()
                        },
                        move |_, cx| cx.open_url(&zed_urls::account_url(cx)),
                    )
                    .when(usage.over_limit(), |menu| -> ContextMenu {
                        menu.entry("Subscribe to increase your limit", None, |_window, cx| {
                            cx.open_url(&zed_urls::account_url(cx))
                        })
                    })
                    .separator();
            } else if self.user_store.read(cx).account_too_young() {
                menu = menu
                    .custom_entry(
                        |_window, _cx| {
                            Label::new("Your GitHub account is less than 30 days old.")
                                .size(LabelSize::Small)
                                .color(Color::Warning)
                                .into_any_element()
                        },
                        |_window, cx| cx.open_url(&zed_urls::account_url(cx)),
                    )
                    .entry("Upgrade to Zed Pro or contact us.", None, |_window, cx| {
                        cx.open_url(&zed_urls::account_url(cx))
                    })
                    .separator();
            } else if self.user_store.read(cx).has_overdue_invoices() {
                menu = menu
                    .custom_entry(
                        |_window, _cx| {
                            Label::new("You have an outstanding invoice")
                                .size(LabelSize::Small)
                                .color(Color::Warning)
                                .into_any_element()
                        },
                        |_window, cx| {
                            cx.open_url(&zed_urls::account_url(cx))
                        },
                    )
                    .entry(
                        "Check your payment status or contact us at billing-support@zed.dev to continue using this feature.",
                        None,
                        |_window, cx| {
                            cx.open_url(&zed_urls::account_url(cx))
                        },
                    )
                    .separator();
            }

            self.build_language_settings_menu(menu, window, cx)
        })
    }

    pub fn update_enabled(&mut self, editor: Entity<Editor>, cx: &mut Context<Self>) {
        let editor = editor.read(cx);
        let snapshot = editor.buffer().read(cx).snapshot(cx);
        let suggestion_anchor = editor.selections.newest_anchor().start;
        let language = snapshot.language_at(suggestion_anchor);
        let file = snapshot.file_at(suggestion_anchor).cloned();
        self.editor_enabled = {
            let file = file.as_ref();
            Some(
                file.map(|file| {
                    all_language_settings(Some(file), cx)
                        .edit_predictions_enabled_for_file(file, cx)
                })
                .unwrap_or(true),
            )
        };
        self.editor_show_predictions = editor.edit_predictions_enabled();
        self.edit_prediction_provider = editor.edit_prediction_provider();
        self.language = language.cloned();
        self.file = file;
        self.editor_focus_handle = Some(editor.focus_handle(cx));

        cx.notify();
    }
}

impl StatusItemView for InlineCompletionButton {
    fn set_active_pane_item(
        &mut self,
        item: Option<&dyn ItemHandle>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = item.and_then(|item| item.act_as::<Editor>(cx)) {
            self.editor_subscription = Some((
                cx.observe(&editor, Self::update_enabled),
                editor.entity_id().as_u64() as usize,
            ));
            self.update_enabled(editor, cx);
        } else {
            self.language = None;
            self.editor_subscription = None;
            self.editor_enabled = None;
        }
        cx.notify();
    }
}

async fn open_disabled_globs_setting_in_editor(
    workspace: WeakEntity<Workspace>,
    cx: &mut AsyncWindowContext,
) -> Result<()> {
    let settings_editor = workspace
        .update_in(cx, |_, window, cx| {
            create_and_open_local_file(paths::settings_file(), window, cx, || {
                settings::initial_user_settings_content().as_ref().into()
            })
        })?
        .await?
        .downcast::<Editor>()
        .unwrap();

    settings_editor
        .downgrade()
        .update_in(cx, |item, window, cx| {
            let text = item.buffer().read(cx).snapshot(cx).text();

            let settings = cx.global::<SettingsStore>();

            // Ensure that we always have "inline_completions { "disabled_globs": [] }"
            let edits = settings.edits_for_update::<AllLanguageSettings>(&text, |file| {
                file.edit_predictions
                    .get_or_insert_with(Default::default)
                    .disabled_globs
                    .get_or_insert_with(Vec::new);
            });

            if !edits.is_empty() {
                item.edit(edits, cx);
            }

            let text = item.buffer().read(cx).snapshot(cx).text();

            static DISABLED_GLOBS_REGEX: LazyLock<Regex> = LazyLock::new(|| {
                Regex::new(r#""disabled_globs":\s*\[\s*(?P<content>(?:.|\n)*?)\s*\]"#).unwrap()
            });
            // Only capture [...]
            let range = DISABLED_GLOBS_REGEX.captures(&text).and_then(|captures| {
                captures
                    .name("content")
                    .map(|inner_match| inner_match.start()..inner_match.end())
            });
            if let Some(range) = range {
                item.change_selections(
                    SelectionEffects::scroll(Autoscroll::newest()),
                    window,
                    cx,
                    |selections| {
                        selections.select_ranges(vec![range]);
                    },
                );
            }
        })?;

    anyhow::Ok(())
}

fn toggle_inline_completions_globally(fs: Arc<dyn Fs>, cx: &mut App) {
    let show_edit_predictions = all_language_settings(None, cx).show_edit_predictions(None, cx);
    update_settings_file::<AllLanguageSettings>(fs, cx, move |file, _| {
        file.defaults.show_edit_predictions = Some(!show_edit_predictions)
    });
}

fn set_completion_provider(fs: Arc<dyn Fs>, cx: &mut App, provider: EditPredictionProvider) {
    update_settings_file::<AllLanguageSettings>(fs, cx, move |file, _| {
        file.features
            .get_or_insert(Default::default())
            .edit_prediction_provider = Some(provider);
    });
}

fn toggle_show_inline_completions_for_language(
    language: Arc<Language>,
    fs: Arc<dyn Fs>,
    cx: &mut App,
) {
    let show_edit_predictions =
        all_language_settings(None, cx).show_edit_predictions(Some(&language), cx);
    update_settings_file::<AllLanguageSettings>(fs, cx, move |file, _| {
        file.languages
            .0
            .entry(language.name())
            .or_default()
            .show_edit_predictions = Some(!show_edit_predictions);
    });
}

fn toggle_edit_prediction_mode(fs: Arc<dyn Fs>, mode: EditPredictionsMode, cx: &mut App) {
    let settings = AllLanguageSettings::get_global(cx);
    let current_mode = settings.edit_predictions_mode();

    if current_mode != mode {
        update_settings_file::<AllLanguageSettings>(fs, cx, move |settings, _cx| {
            if let Some(edit_predictions) = settings.edit_predictions.as_mut() {
                edit_predictions.mode = mode;
            } else {
                settings.edit_predictions =
                    Some(language_settings::EditPredictionSettingsContent {
                        mode,
                        ..Default::default()
                    });
            }
        });
    }
}
