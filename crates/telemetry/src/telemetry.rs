//! See [Telemetry in Zed](https://zed.dev/docs/telemetry) for additional information.
use futures::channel::mpsc;
pub use serde_json;
use std::sync::OnceLock;
pub use telemetry_events::FlexibleEvent as Event;

/// Macro to create telemetry events and send them to the telemetry queue.
///
/// By convention, the name should be "Noun Verbed", e.g. "Keymap Changed"
/// or "Project Diagnostics Opened".
///
/// The properties can be any value that implements serde::Serialize.
///
/// ```
/// telemetry::event!("Keymap Changed", version = "1.0.0");
/// telemetry::event!("Documentation Viewed", url, source = "Extension Upsell");
/// ```
#[macro_export]
macro_rules! event {
    ($name:expr) => {{}};
    ($name:expr, $($key:ident $(= $value:expr)?),+ $(,)?) => {{}};
}

#[macro_export]
macro_rules! serialize_property {
    ($key:ident) => {
        $key
    };
    ($key:ident = $value:expr) => {
        $value
    };
}

pub fn send_event(_: Event) {
    unreachable!("Zedless: unexpected telemetry event!");
}

pub fn init(_: mpsc::UnboundedSender<Event>) {
    unreachable!("Zedless: unexpected telemetry queue init!");
}
