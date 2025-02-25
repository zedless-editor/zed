use gpui::{prelude::*, Action, MouseButton, WindowStyle};

use ui::prelude::*;

use crate::window_controls::{WindowControl, WindowControlType};

#[derive(IntoElement)]
pub struct LinuxWindowControls {
    close_window_action: Box<dyn Action>,
    window_style: WindowStyle,
}

impl LinuxWindowControls {
    pub fn new(close_window_action: Box<dyn Action>, window_style: WindowStyle) -> Self {
        Self {
            close_window_action,
            window_style
        }
    }
}

impl RenderOnce for LinuxWindowControls {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        h_flex()
            .id("generic-window-controls")
            .px_3()
            .gap_3()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(self.window_style != WindowStyle::Gnome, |c| {
                c.child(WindowControl::new(
                    "minimize",
                    WindowControlType::Minimize,
                    cx,
                ))
                .child(WindowControl::new(
                    "maximize-or-restore",
                    if window.is_maximized() {
                        WindowControlType::Restore
                    } else {
                        WindowControlType::Maximize
                    },
                    cx,
                ))
            })
            .child(WindowControl::new_close(
                "close",
                WindowControlType::Close,
                self.close_window_action,
                cx,
            ))
    }
}
