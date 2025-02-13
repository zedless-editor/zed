use crate::stdout_is_a_pty;
use anyhow::{Context as _, Result};
use backtrace::{self, Backtrace};
use chrono::Utc;
use db::kvp::KEY_VALUE_STORE;
use gpui::{App, SemanticVersion};
use http_client::{self, HttpClient, HttpClientWithUrl, HttpRequestExt, Method};
use paths::{crashes_dir, crashes_retired_dir};
use project::Project;
use release_channel::{AppCommitSha, ReleaseChannel, RELEASE_CHANNEL};
use settings::Settings;
use smol::stream::StreamExt;
use std::{
    env,
    ffi::{c_void, OsStr},
    sync::{atomic::Ordering, Arc},
};
use std::{io::Write, panic, sync::atomic::AtomicU32, thread};
use url::Url;
use util::ResultExt;

static PANIC_COUNT: AtomicU32 = AtomicU32::new(0);

pub fn init_panic_hook(
    app_version: SemanticVersion,
    app_commit_sha: Option<AppCommitSha>,
    system_id: Option<String>,
    installation_id: Option<String>,
    session_id: String,
) {
    let is_pty = stdout_is_a_pty();

    panic::set_hook(Box::new(move |info| {
        let prior_panic_count = PANIC_COUNT.fetch_add(1, Ordering::SeqCst);
        if prior_panic_count > 0 {
            // Give the panic-ing thread time to write the panic file
            loop {
                std::thread::yield_now();
            }
        }

        let thread = thread::current();
        let thread_name = thread.name().unwrap_or("<unnamed>");

        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "Box<Any>".to_string());

        if *release_channel::RELEASE_CHANNEL == ReleaseChannel::Dev {
            let location = info.location().unwrap();
            let backtrace = Backtrace::new();
            eprintln!(
                "Thread {:?} panicked with {:?} at {}:{}:{}\n{}{:?}",
                thread_name,
                payload,
                location.file(),
                location.line(),
                location.column(),
                match app_commit_sha.as_ref() {
                    Some(commit_sha) => format!(
                        "https://github.com/zed-industries/zed/blob/{}/src/{}#L{} \
                        (may not be uploaded, line may be incorrect if files modified)\n",
                        commit_sha.0,
                        location.file(),
                        location.line()
                    ),
                    None => "".to_string(),
                },
                backtrace,
            );
            std::process::exit(-1);
        }
        let main_module_base_address = get_main_module_base_address();

        let backtrace = Backtrace::new();
        let mut symbols = backtrace
            .frames()
            .iter()
            .flat_map(|frame| {
                let base = frame
                    .module_base_address()
                    .unwrap_or(main_module_base_address);
                frame.symbols().iter().map(move |symbol| {
                    format!(
                        "{}+{}",
                        symbol
                            .name()
                            .as_ref()
                            .map_or("<unknown>".to_owned(), <_>::to_string),
                        (frame.ip() as isize).saturating_sub(base as isize)
                    )
                })
            })
            .collect::<Vec<_>>();

        // Strip out leading stack frames for rust panic-handling.
        if let Some(ix) = symbols
            .iter()
            .position(|name| name == "rust_begin_unwind" || name == "_rust_begin_unwind")
        {
            symbols.drain(0..=ix);
        }

        std::process::abort();
    }));
}

#[cfg(not(target_os = "windows"))]
fn get_main_module_base_address() -> *mut c_void {
    let mut dl_info = libc::Dl_info {
        dli_fname: std::ptr::null(),
        dli_fbase: std::ptr::null_mut(),
        dli_sname: std::ptr::null(),
        dli_saddr: std::ptr::null_mut(),
    };
    unsafe {
        libc::dladdr(get_main_module_base_address as _, &mut dl_info);
    }
    dl_info.dli_fbase
}

#[cfg(target_os = "windows")]
fn get_main_module_base_address() -> *mut c_void {
    std::ptr::null_mut()
}

pub fn init(
    http_client: Arc<HttpClientWithUrl>,
    system_id: Option<String>,
    installation_id: Option<String>,
    session_id: String,
    cx: &mut App,
) {
    #[cfg(target_os = "macos")]
    monitor_main_thread_hangs(http_client.clone(), installation_id.clone(), cx);

    cx.observe_new(move |project: &mut Project, _, cx| {
        let http_client = http_client.clone();
        let session_id = session_id.clone();
        let installation_id = installation_id.clone();
        let system_id = system_id.clone();
    })
    .detach();
}

#[cfg(target_os = "macos")]
pub fn monitor_main_thread_hangs(
    http_client: Arc<HttpClientWithUrl>,
    installation_id: Option<String>,
    cx: &App,
) {
    // This is too noisy to ship to stable for now.
    if !matches!(
        ReleaseChannel::global(cx),
        ReleaseChannel::Dev | ReleaseChannel::Nightly | ReleaseChannel::Preview
    ) {
        return;
    }

    use nix::sys::signal::{
        sigaction, SaFlags, SigAction, SigHandler, SigSet,
        Signal::{self, SIGUSR2},
    };

    use parking_lot::Mutex;

    use http_client::Method;
    use std::{
        ffi::c_int,
        sync::{mpsc, OnceLock},
        time::Duration,
    };

    use nix::sys::pthread;

    let foreground_executor = cx.foreground_executor();
    let background_executor = cx.background_executor();

    // Initialize SIGUSR2 handler to send a backtrace to a channel.
    let (backtrace_tx, backtrace_rx) = mpsc::channel();
    static BACKTRACE: Mutex<Vec<backtrace::Frame>> = Mutex::new(Vec::new());
    static BACKTRACE_SENDER: OnceLock<mpsc::Sender<()>> = OnceLock::new();
    BACKTRACE_SENDER.get_or_init(|| backtrace_tx);
    BACKTRACE.lock().reserve(100);

    fn handle_backtrace_signal() {
        unsafe {
            extern "C" fn handle_sigusr2(_i: c_int) {
                unsafe {
                    // ASYNC SIGNAL SAFETY: This lock is only accessed one other time,
                    // which can only be triggered by This signal handler. In addition,
                    // this signal handler is immediately removed by SA_RESETHAND, and this
                    // signal handler cannot be re-entrant due to to the SIGUSR2 mask defined
                    // below
                    let mut bt = BACKTRACE.lock();
                    bt.clear();
                    backtrace::trace_unsynchronized(|frame| {
                        if bt.len() < bt.capacity() {
                            bt.push(frame.clone());
                            true
                        } else {
                            false
                        }
                    });
                }

                BACKTRACE_SENDER.get().unwrap().send(()).ok();
            }

            let mut mask = SigSet::empty();
            mask.add(SIGUSR2);
            sigaction(
                Signal::SIGUSR2,
                &SigAction::new(
                    SigHandler::Handler(handle_sigusr2),
                    SaFlags::SA_RESTART | SaFlags::SA_RESETHAND,
                    mask,
                ),
            )
            .log_err();
        }
    }

    handle_backtrace_signal();
    let main_thread = pthread::pthread_self();

    let (mut tx, mut rx) = futures::channel::mpsc::channel(3);
    foreground_executor
        .spawn(async move { while (rx.next().await).is_some() {} })
        .detach();

    background_executor
        .spawn({
            let background_executor = background_executor.clone();
            async move {
                loop {
                    background_executor.timer(Duration::from_secs(1)).await;
                    match tx.try_send(()) {
                        Ok(_) => continue,
                        Err(e) => {
                            if e.into_send_error().is_full() {
                                pthread::pthread_kill(main_thread, SIGUSR2).log_err();
                            }
                            // Only detect the first hang
                            break;
                        }
                    }
                }
            }
        })
        .detach();
}
