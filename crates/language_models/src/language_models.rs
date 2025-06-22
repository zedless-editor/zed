use std::sync::Arc;

use client::{Client};
use fs::Fs;
use gpui::{App, Context};
use language_model::LanguageModelRegistry;

pub mod provider;
mod settings;
pub mod ui;

use crate::provider::ollama::OllamaLanguageModelProvider;
use crate::provider::open_ai::OpenAiLanguageModelProvider;
pub use crate::settings::*;

pub fn init(client: Arc<Client>, fs: Arc<dyn Fs>, cx: &mut App) {
    crate::settings::init(fs, cx);
    let registry = LanguageModelRegistry::global(cx);
    registry.update(cx, |registry, cx| {
        register_language_model_providers(registry, client, cx);
    });
}

fn register_language_model_providers(
    registry: &mut LanguageModelRegistry,
    client: Arc<Client>,
    cx: &mut Context<LanguageModelRegistry>,
) {
    registry.register_provider(
        OpenAiLanguageModelProvider::new(client.http_client(), cx),
        cx,
    );
    registry.register_provider(
        OllamaLanguageModelProvider::new(client.http_client(), cx),
        cx,
    );
}
