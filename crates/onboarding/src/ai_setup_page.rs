use std::sync::Arc;

use client::{Client, UserStore, zed_urls};
use fs::Fs;
use gpui::{
    Action, AnyView, App, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, WeakEntity,
    Window, prelude::*,
};
use language_model::LanguageModelProvider;
use project::DisableAiSettings;
use settings::{Settings, update_settings_file};
use ui::{
    Badge, KeyBinding, Modal, ModalFooter, ModalHeader, Section, SwitchField,
    ToggleState, prelude::*, tooltip_container,
};
use workspace::{ModalView, Workspace};

fn render_privacy_card(tab_index: &mut isize, disabled: bool, cx: &mut App) -> impl IntoElement {
    let (title, description) = if disabled {
        (
            "AI is disabled across Zed",
            "Re-enable it any time in Settings.",
        )
    } else {
        (
            "Privacy is the default for Zed",
            "Any use or storage of your data is with your explicit, single-use, opt-in consent.",
        )
    };

    v_flex()
        .relative()
        .pt_2()
        .pb_2p5()
        .pl_3()
        .pr_2()
        .border_1()
        .border_dashed()
        .border_color(cx.theme().colors().border.opacity(0.5))
        .bg(cx.theme().colors().surface_background.opacity(0.3))
        .rounded_lg()
        .overflow_hidden()
        .child(
            h_flex()
                .gap_2()
                .justify_between()
                .child(Label::new(title))
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Badge::new("Privacy")
                                .icon(IconName::ShieldCheck)
                                .tooltip(move |_, cx| cx.new(|_| AiPrivacyTooltip::new()).into()),
                        )
                        .child(
                            Button::new("learn_more", "Learn More")
                                .style(ButtonStyle::Outlined)
                                .label_size(LabelSize::Small)
                                .icon(IconName::ArrowUpRight)
                                .icon_size(IconSize::XSmall)
                                .icon_color(Color::Muted)
                                .on_click(|_, _, cx| {
                                    cx.open_url(&zed_urls::ai_privacy_and_security(cx))
                                })
                                .tab_index({
                                    *tab_index += 1;
                                    *tab_index - 1
                                }),
                        ),
                ),
        )
        .child(
            Label::new(description)
                .size(LabelSize::Small)
                .color(Color::Muted),
        )
}

pub(crate) fn render_ai_setup_page(
    workspace: WeakEntity<Workspace>,
    user_store: Entity<UserStore>,
    client: Arc<Client>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let mut tab_index = 0;
    let is_ai_disabled = DisableAiSettings::get_global(cx).disable_ai;

    v_flex()
        .gap_2()
        .child(
            SwitchField::new(
                "enable_ai",
                "Enable AI features",
                None,
                if is_ai_disabled {
                    ToggleState::Unselected
                } else {
                    ToggleState::Selected
                },
                |&toggle_state, _, cx| {
                    let fs = <dyn Fs>::global(cx);
                    update_settings_file::<DisableAiSettings>(
                        fs,
                        cx,
                        move |ai_settings: &mut Option<bool>, _| {
                            *ai_settings = match toggle_state {
                                ToggleState::Indeterminate => None,
                                ToggleState::Unselected => Some(true),
                                ToggleState::Selected => Some(false),
                            };
                        },
                    );
                },
            )
            .tab_index({
                tab_index += 1;
                tab_index - 1
            }),
        )
        .child(render_privacy_card(&mut tab_index, is_ai_disabled, cx))
}

struct AiConfigurationModal {
    focus_handle: FocusHandle,
    selected_provider: Arc<dyn LanguageModelProvider>,
    configuration_view: AnyView,
}

impl AiConfigurationModal {
    fn new(
        selected_provider: Arc<dyn LanguageModelProvider>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let configuration_view = selected_provider.configuration_view(window, cx);

        Self {
            focus_handle,
            configuration_view,
            selected_provider,
        }
    }

    fn cancel(&mut self, _: &menu::Cancel, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }
}

impl ModalView for AiConfigurationModal {}

impl EventEmitter<DismissEvent> for AiConfigurationModal {}

impl Focusable for AiConfigurationModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AiConfigurationModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("OnboardingAiConfigurationModal")
            .w(rems(34.))
            .elevation_3(cx)
            .track_focus(&self.focus_handle)
            .on_action(
                cx.listener(|this, _: &menu::Cancel, _window, cx| this.cancel(&menu::Cancel, cx)),
            )
            .child(
                Modal::new("onboarding-ai-setup-modal", None)
                    .header(
                        ModalHeader::new()
                            .icon(
                                Icon::new(self.selected_provider.icon())
                                    .color(Color::Muted)
                                    .size(IconSize::Small),
                            )
                            .headline(self.selected_provider.name().0),
                    )
                    .section(Section::new().child(self.configuration_view.clone()))
                    .footer(
                        ModalFooter::new().end_slot(
                            Button::new("ai-onb-modal-Done", "Done")
                                .key_binding(
                                    KeyBinding::for_action_in(
                                        &menu::Cancel,
                                        &self.focus_handle.clone(),
                                        window,
                                        cx,
                                    )
                                    .map(|kb| kb.size(rems_from_px(12.))),
                                )
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.cancel(&menu::Cancel, cx)
                                })),
                        ),
                    ),
            )
    }
}

pub struct AiPrivacyTooltip {}

impl AiPrivacyTooltip {
    pub fn new() -> Self {
        Self {}
    }
}

impl Render for AiPrivacyTooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        const DESCRIPTION: &'static str = "We believe in opt-in data sharing as the default for building AI products, rather than opt-out. We'll only use or store your data if you affirmatively send it to us. ";

        tooltip_container(window, cx, move |this, _, _| {
            this.child(
                h_flex()
                    .gap_1()
                    .child(
                        Icon::new(IconName::ShieldCheck)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(Label::new("Privacy First")),
            )
            .child(
                div().max_w_64().child(
                    Label::new(DESCRIPTION)
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
            )
        })
    }
}
