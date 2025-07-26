use std::sync::Arc;

use ::settings::{Settings, SettingsStore};
use client::Client;
use collections::HashSet;
use gpui::{App, Context};
use language_model::{LanguageModelProviderId, LanguageModelRegistry};

pub mod provider;
mod settings;
pub mod ui;

use crate::provider::ollama::OllamaLanguageModelProvider;
use crate::provider::open_ai::OpenAiLanguageModelProvider;
use crate::provider::open_ai_compatible::OpenAiCompatibleLanguageModelProvider;
pub use crate::settings::*;

pub fn init(client: Arc<Client>, cx: &mut App) {
    crate::settings::init_settings(cx);
    let registry = LanguageModelRegistry::global(cx);
    registry.update(cx, |registry, cx| {
        register_language_model_providers(registry, client.clone(), cx);
    });

    let mut openai_compatible_providers = AllLanguageModelSettings::get_global(cx)
        .openai_compatible
        .keys()
        .cloned()
        .collect::<HashSet<_>>();

    registry.update(cx, |registry, cx| {
        register_openai_compatible_providers(
            registry,
            &HashSet::default(),
            &openai_compatible_providers,
            client.clone(),
            cx,
        );
    });
    cx.observe_global::<SettingsStore>(move |cx| {
        let openai_compatible_providers_new = AllLanguageModelSettings::get_global(cx)
            .openai_compatible
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        if openai_compatible_providers_new != openai_compatible_providers {
            registry.update(cx, |registry, cx| {
                register_openai_compatible_providers(
                    registry,
                    &openai_compatible_providers,
                    &openai_compatible_providers_new,
                    client.clone(),
                    cx,
                );
            });
            openai_compatible_providers = openai_compatible_providers_new;
        }
    })
    .detach();
}

fn register_openai_compatible_providers(
    registry: &mut LanguageModelRegistry,
    old: &HashSet<Arc<str>>,
    new: &HashSet<Arc<str>>,
    client: Arc<Client>,
    cx: &mut Context<LanguageModelRegistry>,
) {
    for provider_id in old {
        if !new.contains(provider_id) {
            registry.unregister_provider(LanguageModelProviderId::from(provider_id.clone()), cx);
        }
    }

    for provider_id in new {
        if !old.contains(provider_id) {
            registry.register_provider(
                OpenAiCompatibleLanguageModelProvider::new(
                    provider_id.clone(),
                    client.http_client(),
                    cx,
                ),
                cx,
            );
        }
    }
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
