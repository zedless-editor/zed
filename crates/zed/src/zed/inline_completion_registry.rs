use client::{Client, UserStore};
use collections::HashMap;
use editor::Editor;
use gpui::{AnyWindowHandle, App, AppContext as _, Context, Entity, WeakEntity};
use language::language_settings::{EditPredictionProvider, all_language_settings};
use settings::SettingsStore;
use smol::stream::StreamExt;
use std::{cell::RefCell, rc::Rc, sync::Arc};
use ui::Window;
use util::ResultExt;
use workspace::Workspace;
use zeta::{ProviderDataCollection, ZetaInlineCompletionProvider};

pub fn init(client: Arc<Client>, user_store: Entity<UserStore>, cx: &mut App) {
    let editors: Rc<RefCell<HashMap<WeakEntity<Editor>, AnyWindowHandle>>> = Rc::default();
    cx.observe_new({
        let editors = editors.clone();
        let client = client.clone();
        let user_store = user_store.clone();
        move |editor: &mut Editor, window, cx: &mut Context<Editor>| {
            if !editor.mode().is_full() {
                return;
            }

            let Some(window) = window else {
                return;
            };

            let editor_handle = cx.entity().downgrade();
            cx.on_release({
                let editor_handle = editor_handle.clone();
                let editors = editors.clone();
                move |_, _| {
                    editors.borrow_mut().remove(&editor_handle);
                }
            })
            .detach();

            editors
                .borrow_mut()
                .insert(editor_handle, window.window_handle());
            let provider = all_language_settings(None, cx).edit_predictions.provider;
            assign_edit_prediction_provider(
                editor,
                provider,
                &client,
                user_store.clone(),
                window,
                cx,
            );
        }
    })
    .detach();

    cx.on_action(clear_zeta_edit_history);

    let mut provider = all_language_settings(None, cx).edit_predictions.provider;
    cx.spawn({
        let user_store = user_store.clone();
        let editors = editors.clone();
        let client = client.clone();

        async move |cx| {
            let mut status = client.status();
            while let Some(_status) = status.next().await {
                cx.update(|cx| {
                    assign_edit_prediction_providers(
                        &editors,
                        provider,
                        &client,
                        user_store.clone(),
                        cx,
                    );
                })
                .log_err();
            }
        }
    })
    .detach();

    cx.observe_global::<SettingsStore>({
        let editors = editors.clone();
        let client = client.clone();
        let user_store = user_store.clone();
        move |cx| {
            let new_provider = all_language_settings(None, cx).edit_predictions.provider;

            if new_provider != provider {
                let tos_accepted = user_store
                    .read(cx)
                    .current_user_has_accepted_terms()
                    .unwrap_or(false);

                provider = new_provider;
                assign_edit_prediction_providers(
                    &editors,
                    provider,
                    &client,
                    user_store.clone(),
                    cx,
                );

                if !tos_accepted {
                    match provider {
                        EditPredictionProvider::Zed => {
                            let Some(window) = cx.active_window() else {
                                return;
                            };

                            window
                                .update(cx, |_, window, cx| {
                                    window.dispatch_action(
                                        Box::new(zed_actions::OpenZedPredictOnboarding),
                                        cx,
                                    );
                                })
                                .ok();
                        }
                        EditPredictionProvider::None => {}
                    }
                }
            }
        }
    })
    .detach();
}

fn clear_zeta_edit_history(_: &zeta::ClearHistory, cx: &mut App) {
    if let Some(zeta) = zeta::Zeta::global(cx) {
        zeta.update(cx, |zeta, _| zeta.clear_history());
    }
}

fn assign_edit_prediction_providers(
    editors: &Rc<RefCell<HashMap<WeakEntity<Editor>, AnyWindowHandle>>>,
    provider: EditPredictionProvider,
    client: &Arc<Client>,
    user_store: Entity<UserStore>,
    cx: &mut App,
) {
    for (editor, window) in editors.borrow().iter() {
        _ = window.update(cx, |_window, window, cx| {
            _ = editor.update(cx, |editor, cx| {
                assign_edit_prediction_provider(
                    editor,
                    provider,
                    &client,
                    user_store.clone(),
                    window,
                    cx,
                );
            })
        });
    }
}

fn assign_edit_prediction_provider(
    editor: &mut Editor,
    provider: EditPredictionProvider,
    client: &Arc<Client>,
    user_store: Entity<UserStore>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    // TODO: Do we really want to collect data only for singleton buffers?
    let singleton_buffer = editor.buffer().read(cx).as_singleton();

    match provider {
        EditPredictionProvider::None => {
            editor.set_edit_prediction_provider::<ZetaInlineCompletionProvider>(None, window, cx);
        }
        EditPredictionProvider::Zed => {
            if client.status().borrow().is_connected() {
                let mut worktree = None;

                if let Some(buffer) = &singleton_buffer {
                    if let Some(file) = buffer.read(cx).file() {
                        let id = file.worktree_id(cx);
                        if let Some(inner_worktree) = editor
                            .project
                            .as_ref()
                            .and_then(|project| project.read(cx).worktree_for_id(id, cx))
                        {
                            worktree = Some(inner_worktree);
                        }
                    }
                }

                let workspace = window
                    .root::<Workspace>()
                    .flatten()
                    .map(|workspace| workspace.downgrade());

                let zeta =
                    zeta::Zeta::register(workspace, worktree, client.clone(), user_store, cx);

                if let Some(buffer) = &singleton_buffer {
                    if buffer.read(cx).file().is_some() {
                        zeta.update(cx, |zeta, cx| {
                            zeta.register_buffer(&buffer, cx);
                        });
                    }
                }

                let data_collection =
                    ProviderDataCollection::new(zeta.clone(), singleton_buffer, cx);

                let provider =
                    cx.new(|_| zeta::ZetaInlineCompletionProvider::new(zeta, data_collection));

                editor.set_edit_prediction_provider(Some(provider), window, cx);
            }
        }
    }
}
