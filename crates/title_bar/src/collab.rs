use std::rc::Rc;
use std::sync::Arc;

use call::{ActiveCall, ParticipantLocation, Room};
use client::{User, proto::PeerId};
use gpui::{
    AnyElement, Hsla, IntoElement, MouseButton, Path, ScreenCaptureSource, Styled, canvas, point,
};
use gpui::{App, Task, Window, actions};
use rpc::proto::{self};
use theme::ActiveTheme;
use ui::{
    Avatar, AvatarAudioStatusIndicator, Divider, DividerColor, Facepile, TintColor, Tooltip,
    prelude::*,
};
use util::maybe;
use workspace::notifications::DetachAndPromptErr;

use crate::TitleBar;

actions!(
    collab,
    [
        /// Toggles screen sharing on or off.
        ToggleScreenSharing,
        /// Toggles microphone mute.
        ToggleMute,
        /// Toggles deafen mode (mute both microphone and speakers).
        ToggleDeafen
    ]
);

fn toggle_screen_sharing(
    screen: Option<Rc<dyn ScreenCaptureSource>>,
    window: &mut Window,
    cx: &mut App,
) {
    let call = ActiveCall::global(cx).read(cx);
    if let Some(room) = call.room().cloned() {
        let toggle_screen_sharing = room.update(cx, |room, cx| {
            let clicked_on_currently_shared_screen =
                room.shared_screen_id().is_some_and(|screen_id| {
                    Some(screen_id)
                        == screen
                            .as_deref()
                            .and_then(|s| s.metadata().ok().map(|meta| meta.id))
                });
            let should_unshare_current_screen = room.is_sharing_screen();
            let unshared_current_screen = should_unshare_current_screen.then(|| {
                room.unshare_screen(clicked_on_currently_shared_screen || screen.is_none(), cx)
            });
            if let Some(screen) = screen {
                cx.spawn(async move |room, cx| {
                    unshared_current_screen.transpose()?;
                    if !clicked_on_currently_shared_screen {
                        room.update(cx, |room, cx| room.share_screen(screen, cx))?
                            .await
                    } else {
                        Ok(())
                    }
                })
            } else {
                Task::ready(Ok(()))
            }
        });
        toggle_screen_sharing.detach_and_prompt_err("Sharing Screen Failed", window, cx, |e, _, _| Some(format!("{:?}\n\nPlease check that you have given Zed permissions to record your screen in Settings.", e)));
    }
}

fn toggle_mute(_: &ToggleMute, cx: &mut App) {
    let call = ActiveCall::global(cx).read(cx);
    if let Some(room) = call.room().cloned() {
        room.update(cx, |room, cx| room.toggle_mute(cx));
    }
}

fn toggle_deafen(_: &ToggleDeafen, cx: &mut App) {
    if let Some(room) = ActiveCall::global(cx).read(cx).room().cloned() {
        room.update(cx, |room, cx| room.toggle_deafen(cx));
    }
}

fn render_color_ribbon(color: Hsla) -> impl Element {
    canvas(
        move |_, _, _| {},
        move |bounds, _, window, _| {
            let height = bounds.size.height;
            let horizontal_offset = height;
            let vertical_offset = px(height.0 / 2.0);
            let mut path = Path::new(bounds.bottom_left());
            path.curve_to(
                bounds.origin + point(horizontal_offset, vertical_offset),
                bounds.origin + point(px(0.0), vertical_offset),
            );
            path.line_to(bounds.top_right() + point(-horizontal_offset, vertical_offset));
            path.curve_to(
                bounds.bottom_right(),
                bounds.top_right() + point(px(0.0), vertical_offset),
            );
            path.line_to(bounds.bottom_left());
            window.paint_path(path, color);
        },
    )
    .h_1()
    .w_full()
}

impl TitleBar {
    pub(crate) fn render_call_controls(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(room) = ActiveCall::global(cx).read(cx).room().cloned() else {
            return Vec::new();
        };

        let is_connecting_to_project = self
            .workspace
            .update(cx, |workspace, cx| workspace.has_active_modal(window, cx))
            .unwrap_or(false);

        let room = room.read(cx);
        let project = self.project.read(cx);
        let is_local = project.is_local() || project.is_via_ssh();
        let is_shared = is_local && project.is_shared();
        let is_muted = room.is_muted();
        let muted_by_user = room.muted_by_user();
        let is_deafened = room.is_deafened().unwrap_or(false);
        let is_screen_sharing = room.is_sharing_screen();
        let can_use_microphone = room.can_use_microphone();
        let can_share_projects = room.can_share_projects();
        let screen_sharing_supported = cx.is_screen_capture_supported();

        let mut children = Vec::new();

        children.push(
            h_flex()
                .gap_1()
                .child(
                    IconButton::new("leave-call", IconName::Exit)
                        .style(ButtonStyle::Subtle)
                        .tooltip(Tooltip::text("Leave Call"))
                        .icon_size(IconSize::Small)
                        .on_click(move |_, _window, cx| {
                            ActiveCall::global(cx)
                                .update(cx, |call, cx| call.hang_up(cx))
                                .detach_and_log_err(cx);
                        }),
                )
                .child(Divider::vertical().color(DividerColor::Border))
                .into_any_element(),
        );

        if is_local && can_share_projects && !is_connecting_to_project {
            children.push(
                Button::new(
                    "toggle_sharing",
                    if is_shared { "Unshare" } else { "Share" },
                )
                .tooltip(Tooltip::text(if is_shared {
                    "Stop sharing project with call participants"
                } else {
                    "Share project with call participants"
                }))
                .style(ButtonStyle::Subtle)
                .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                .toggle_state(is_shared)
                .label_size(LabelSize::Small)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if is_shared {
                        this.unshare_project(window, cx);
                    } else {
                        this.share_project(cx);
                    }
                }))
                .into_any_element(),
            );
        }

        if can_use_microphone {
            children.push(
                IconButton::new(
                    "mute-microphone",
                    if is_muted {
                        IconName::MicMute
                    } else {
                        IconName::Mic
                    },
                )
                .tooltip(move |window, cx| {
                    if is_muted {
                        if is_deafened {
                            Tooltip::with_meta(
                                "Unmute Microphone",
                                None,
                                "Audio will be unmuted",
                                window,
                                cx,
                            )
                        } else {
                            Tooltip::simple("Unmute Microphone", cx)
                        }
                    } else {
                        Tooltip::simple("Mute Microphone", cx)
                    }
                })
                .style(ButtonStyle::Subtle)
                .icon_size(IconSize::Small)
                .toggle_state(is_muted)
                .selected_style(ButtonStyle::Tinted(TintColor::Error))
                .on_click(move |_, _window, cx| {
                    toggle_mute(&Default::default(), cx);
                })
                .into_any_element(),
            );
        }

        children.push(
            IconButton::new(
                "mute-sound",
                if is_deafened {
                    IconName::AudioOff
                } else {
                    IconName::AudioOn
                },
            )
            .style(ButtonStyle::Subtle)
            .selected_style(ButtonStyle::Tinted(TintColor::Error))
            .icon_size(IconSize::Small)
            .toggle_state(is_deafened)
            .tooltip(move |window, cx| {
                if is_deafened {
                    let label = "Unmute Audio";

                    if !muted_by_user {
                        Tooltip::with_meta(label, None, "Microphone will be unmuted", window, cx)
                    } else {
                        Tooltip::simple(label, cx)
                    }
                } else {
                    let label = "Mute Audio";

                    if !muted_by_user {
                        Tooltip::with_meta(label, None, "Microphone will be muted", window, cx)
                    } else {
                        Tooltip::simple(label, cx)
                    }
                }
            })
            .on_click(move |_, _, cx| toggle_deafen(&Default::default(), cx))
            .into_any_element(),
        );

        if can_use_microphone && screen_sharing_supported {
            let trigger = IconButton::new("screen-share", IconName::Screen)
                .style(ButtonStyle::Subtle)
                .icon_size(IconSize::Small)
                .toggle_state(is_screen_sharing)
                .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                .tooltip(Tooltip::text(if is_screen_sharing {
                    "Stop Sharing Screen"
                } else {
                    "Share Screen"
                }))
                .on_click(move |_, window, cx| {
                    let should_share = ActiveCall::global(cx)
                        .read(cx)
                        .room()
                        .is_some_and(|room| !room.read(cx).is_sharing_screen());

                    window
                        .spawn(cx, async move |cx| {
                            let screen = if should_share {
                                cx.update(|_, cx| pick_default_screen(cx))?.await
                            } else {
                                None
                            };

                            cx.update(|window, cx| toggle_screen_sharing(screen, window, cx))?;

                            Result::<_, anyhow::Error>::Ok(())
                        })
                        .detach();
                });
        }

        children.push(div().pr_2().into_any_element());

        children
    }
}

/// Picks the screen to share when clicking on the main screen sharing button.
fn pick_default_screen(cx: &App) -> Task<Option<Rc<dyn ScreenCaptureSource>>> {
    let source = cx.screen_capture_sources();
    cx.spawn(async move |_| {
        let available_sources = maybe!(async move { source.await? }).await.ok()?;
        available_sources
            .iter()
            .find(|it| {
                it.as_ref()
                    .metadata()
                    .is_ok_and(|meta| meta.is_main.unwrap_or_default())
            })
            .or_else(|| available_sources.iter().next())
            .cloned()
    })
}
