use client::{Client, ProxySettings, UserStore};
use extension::ExtensionHostProxy;
use fs::RealFs;
use gpui::http_client::read_proxy_from_env;
use gpui::{App, AppContext, Entity};
use gpui_tokio::Tokio;
use language::LanguageRegistry;
use language_extension::LspAccess;
use project::Project;
use project::project_settings::ProjectSettings;
use release_channel::AppVersion;
use reqwest_client::ReqwestClient;
use settings::{Settings, SettingsStore};
use std::path::PathBuf;
use std::sync::Arc;
use util::ResultExt as _;

/// Headless subset of `workspace::AppState`.
pub struct ZetaCliAppState {
    pub languages: Arc<LanguageRegistry>,
    pub client: Arc<Client>,
    pub user_store: Entity<UserStore>,
    pub fs: Arc<dyn fs::Fs>,
}

// TODO: dedupe with crates/eval/src/eval.rs
pub fn init(cx: &mut App) -> ZetaCliAppState {
    let app_version = AppVersion::load(env!("ZED_PKG_VERSION"));
    release_channel::init(app_version, cx);
    gpui_tokio::init(cx);

    let mut settings_store = SettingsStore::new(cx);
    settings_store
        .set_default_settings(settings::default_settings().as_ref(), cx)
        .unwrap();
    cx.set_global(settings_store);
    client::init_settings(cx);

    // Set User-Agent so we can download language servers from GitHub
    let user_agent = format!(
        "Zeta CLI/{} ({}; {})",
        app_version,
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let proxy_str = ProxySettings::get_global(cx).proxy.to_owned();
    let proxy_url = proxy_str
        .as_ref()
        .and_then(|input| input.parse().ok())
        .or_else(read_proxy_from_env);
    let http = {
        let _guard = Tokio::handle(cx).enter();

        ReqwestClient::proxy_and_user_agent(proxy_url, &user_agent)
            .expect("could not start HTTP client")
    };
    cx.set_http_client(Arc::new(http));

    Project::init_settings(cx);

    let client = Client::production(cx);
    cx.set_http_client(client.http_client());

    let git_binary_path = None;
    let fs = Arc::new(RealFs::new(
        git_binary_path,
        cx.background_executor().clone(),
    ));

    let mut languages = LanguageRegistry::new(cx.background_executor().clone());
    languages.set_language_server_download_dir(paths::languages_dir().clone());
    let languages = Arc::new(languages);

    let user_store = cx.new(|cx| UserStore::new(client.clone(), cx));

    extension::init(cx);

    let extension_host_proxy = ExtensionHostProxy::global(cx);

    language::init(cx);
    debug_adapter_extension::init(extension_host_proxy.clone(), cx);
    language_extension::init(
        LspAccess::Noop,
        extension_host_proxy.clone(),
        languages.clone(),
    );
    language_model::init(client.clone(), cx);
    language_models::init(client.clone(), cx);
    languages::init(languages.clone(), cx);
    prompt_store::init(cx);
    terminal_view::init(cx);

    ZetaCliAppState {
        languages,
        client,
        user_store,
        fs,
    }
}
