use std::any::Any;
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic;
use std::sync::atomic::AtomicUsize;

use editor::Editor;
use file_finder::OpenPathDelegate;
use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::Shared;
use futures::select;
use gpui::ClickEvent;
use gpui::ClipboardItem;
use gpui::Subscription;
use gpui::Task;
use gpui::WeakEntity;
use gpui::canvas;
use gpui::{
    AnyElement, App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    PromptLevel, ScrollHandle, Window,
};
use paths::global_ssh_config_file;
use paths::user_ssh_config_file;
use picker::Picker;
use project::Fs;
use project::Project;
use remote::ssh_session::ConnectionIdentifier;
use remote::{SshConnectionOptions, SshRemoteClient};
use settings::Settings;
use settings::SettingsStore;
use settings::update_settings_file;
use settings::watch_config_file;
use smol::stream::StreamExt as _;
use ui::Navigable;
use ui::NavigableEntry;
use ui::{
    IconButtonShape, List, ListItem, ListSeparator, Modal, ModalHeader, Scrollbar, ScrollbarState,
    Section, Tooltip, prelude::*,
};
use util::{
    ResultExt,
    paths::{PathStyle, RemotePathBuf},
};
use workspace::OpenOptions;
use workspace::Toast;
use workspace::notifications::NotificationId;
use workspace::{
    ModalView, Workspace, notifications::DetachAndPromptErr,
    open_ssh_project_with_existing_connection,
};

use crate::ssh_config::parse_ssh_config_hosts;
use crate::ssh_connections::RemoteSettingsContent;
use crate::ssh_connections::SshConnection;
use crate::ssh_connections::SshConnectionHeader;
use crate::ssh_connections::SshConnectionModal;
use crate::ssh_connections::SshProject;
use crate::ssh_connections::SshPrompt;
use crate::ssh_connections::SshSettings;
use crate::ssh_connections::connect_over_ssh;
use crate::ssh_connections::open_ssh_project;

mod navigation_base {}
pub struct RemoteServerProjects {
    mode: Mode,
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    retained_connections: Vec<Entity<SshRemoteClient>>,
    ssh_config_updates: Task<()>,
    ssh_config_servers: BTreeSet<SharedString>,
    create_new_window: bool,
    _subscription: Subscription,
}

struct CreateRemoteServer {
    address_editor: Entity<Editor>,
    address_error: Option<SharedString>,
    ssh_prompt: Option<Entity<SshPrompt>>,
    _creating: Option<Task<Option<()>>>,
}

impl CreateRemoteServer {
    fn new(window: &mut Window, cx: &mut App) -> Self {
        let address_editor = cx.new(|cx| Editor::single_line(window, cx));
        address_editor.update(cx, |this, cx| {
            this.focus_handle(cx).focus(window);
        });
        Self {
            address_editor,
            address_error: None,
            ssh_prompt: None,
            _creating: None,
        }
    }
}

struct ProjectPicker {
    connection_string: SharedString,
    nickname: Option<SharedString>,
    picker: Entity<Picker<OpenPathDelegate>>,
    _path_task: Shared<Task<Option<()>>>,
}

struct EditNicknameState {
    index: usize,
    editor: Entity<Editor>,
}

impl EditNicknameState {
    fn new(index: usize, window: &mut Window, cx: &mut App) -> Self {
        let this = Self {
            index,
            editor: cx.new(|cx| Editor::single_line(window, cx)),
        };
        let starting_text = SshSettings::get_global(cx)
            .ssh_connections()
            .nth(index)
            .and_then(|state| state.nickname.clone())
            .filter(|text| !text.is_empty());
        this.editor.update(cx, |this, cx| {
            this.set_placeholder_text("Add a nickname for this server", cx);
            if let Some(starting_text) = starting_text {
                this.set_text(starting_text, window, cx);
            }
        });
        this.editor.focus_handle(cx).focus(window);
        this
    }
}

impl Focusable for ProjectPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl ProjectPicker {
    fn new(
        create_new_window: bool,
        ix: usize,
        connection: SshConnectionOptions,
        project: Entity<Project>,
        home_dir: RemotePathBuf,
        path_style: PathStyle,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<RemoteServerProjects>,
    ) -> Entity<Self> {
        let (tx, rx) = oneshot::channel();
        let lister = project::DirectoryLister::Project(project.clone());
        let delegate = file_finder::OpenPathDelegate::new(tx, lister, false, path_style);

        let picker = cx.new(|cx| {
            let picker = Picker::uniform_list(delegate, window, cx)
                .width(rems(34.))
                .modal(false);
            picker.set_query(home_dir.to_string(), window, cx);
            picker
        });
        let connection_string = connection.connection_string().into();
        let nickname = connection.nickname.clone().map(|nick| nick.into());
        let _path_task = cx
            .spawn_in(window, {
                let workspace = workspace.clone();
                async move |this, cx| {
                    let Ok(Some(paths)) = rx.await else {
                        workspace
                            .update_in(cx, |workspace, window, cx| {
                                let fs = workspace.project().read(cx).fs().clone();
                                let weak = cx.entity().downgrade();
                                workspace.toggle_modal(window, cx, |window, cx| {
                                    RemoteServerProjects::new(
                                        create_new_window,
                                        fs,
                                        window,
                                        weak,
                                        cx,
                                    )
                                });
                            })
                            .log_err()?;
                        return None;
                    };

                    let app_state = workspace
                        .read_with(cx, |workspace, _| workspace.app_state().clone())
                        .ok()?;

                    cx.update(|_, cx| {
                        let fs = app_state.fs.clone();
                        update_settings_file::<SshSettings>(fs, cx, {
                            let paths = paths
                                .iter()
                                .map(|path| path.to_string_lossy().to_string())
                                .collect();
                            move |setting, _| {
                                if let Some(server) = setting
                                    .ssh_connections
                                    .as_mut()
                                    .and_then(|connections| connections.get_mut(ix))
                                {
                                    server.projects.insert(SshProject { paths });
                                }
                            }
                        });
                    })
                    .log_err();

                    let options = cx
                        .update(|_, cx| (app_state.build_window_options)(None, cx))
                        .log_err()?;
                    let window = cx
                        .open_window(options, |window, cx| {
                            cx.new(|cx| {
                                Workspace::new(None, project.clone(), app_state.clone(), window, cx)
                            })
                        })
                        .log_err()?;

                    open_ssh_project_with_existing_connection(
                        connection, project, paths, app_state, window, cx,
                    )
                    .await
                    .log_err();

                    this.update(cx, |_, cx| {
                        cx.emit(DismissEvent);
                    })
                    .ok();
                    Some(())
                }
            })
            .shared();
        cx.new(|_| Self {
            _path_task,
            picker,
            connection_string,
            nickname,
        })
    }
}

impl gpui::Render for ProjectPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .child(
                SshConnectionHeader {
                    connection_string: self.connection_string.clone(),
                    paths: Default::default(),
                    nickname: self.nickname.clone(),
                }
                .render(window, cx),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(cx.theme().colors().border_variant)
                    .child(self.picker.clone()),
            )
    }
}

#[derive(Clone)]
enum RemoteEntry {
    Project {
        open_folder: NavigableEntry,
        projects: Vec<(NavigableEntry, SshProject)>,
        configure: NavigableEntry,
        connection: SshConnection,
    },
    SshConfig {
        open_folder: NavigableEntry,
        host: SharedString,
    },
}

impl RemoteEntry {
    fn is_from_zed(&self) -> bool {
        matches!(self, Self::Project { .. })
    }

    fn connection(&self) -> Cow<'_, SshConnection> {
        match self {
            Self::Project { connection, .. } => Cow::Borrowed(connection),
            Self::SshConfig { host, .. } => Cow::Owned(SshConnection {
                host: host.clone(),
                ..SshConnection::default()
            }),
        }
    }
}

#[derive(Clone)]
struct DefaultState {
    scrollbar: ScrollbarState,
    add_new_server: NavigableEntry,
    servers: Vec<RemoteEntry>,
}

impl DefaultState {
    fn new(ssh_config_servers: &BTreeSet<SharedString>, cx: &mut App) -> Self {
        let handle = ScrollHandle::new();
        let scrollbar = ScrollbarState::new(handle.clone());
        let add_new_server = NavigableEntry::new(&handle, cx);

        let ssh_settings = SshSettings::get_global(cx);
        let read_ssh_config = ssh_settings.read_ssh_config;

        let mut servers: Vec<RemoteEntry> = ssh_settings
            .ssh_connections()
            .map(|connection| {
                let open_folder = NavigableEntry::new(&handle, cx);
                let configure = NavigableEntry::new(&handle, cx);
                let projects = connection
                    .projects
                    .iter()
                    .map(|project| (NavigableEntry::new(&handle, cx), project.clone()))
                    .collect();
                RemoteEntry::Project {
                    open_folder,
                    configure,
                    projects,
                    connection,
                }
            })
            .collect();

        if read_ssh_config {
            let mut extra_servers_from_config = ssh_config_servers.clone();
            for server in &servers {
                if let RemoteEntry::Project { connection, .. } = server {
                    extra_servers_from_config.remove(&connection.host);
                }
            }
            servers.extend(extra_servers_from_config.into_iter().map(|host| {
                RemoteEntry::SshConfig {
                    open_folder: NavigableEntry::new(&handle, cx),
                    host,
                }
            }));
        }

        Self {
            scrollbar,
            add_new_server,
            servers,
        }
    }
}

#[derive(Clone)]
struct ViewServerOptionsState {
    server_index: usize,
    connection: SshConnection,
    entries: [NavigableEntry; 4],
}
enum Mode {
    Default(DefaultState),
    ViewServerOptions(ViewServerOptionsState),
    EditNickname(EditNicknameState),
    ProjectPicker(Entity<ProjectPicker>),
    CreateRemoteServer(CreateRemoteServer),
}

impl Mode {
    fn default_mode(ssh_config_servers: &BTreeSet<SharedString>, cx: &mut App) -> Self {
        Self::Default(DefaultState::new(ssh_config_servers, cx))
    }
}
impl RemoteServerProjects {
    pub fn new(
        create_new_window: bool,
        fs: Arc<dyn Fs>,
        window: &mut Window,
        workspace: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let mut read_ssh_config = SshSettings::get_global(cx).read_ssh_config;
        let ssh_config_updates = if read_ssh_config {
            spawn_ssh_config_watch(fs.clone(), cx)
        } else {
            Task::ready(())
        };

        let mut base_style = window.text_style();
        base_style.refine(&gpui::TextStyleRefinement {
            color: Some(cx.theme().colors().editor_foreground),
            ..Default::default()
        });

        let _subscription =
            cx.observe_global_in::<SettingsStore>(window, move |recent_projects, _, cx| {
                let new_read_ssh_config = SshSettings::get_global(cx).read_ssh_config;
                if read_ssh_config != new_read_ssh_config {
                    read_ssh_config = new_read_ssh_config;
                    if read_ssh_config {
                        recent_projects.ssh_config_updates = spawn_ssh_config_watch(fs.clone(), cx);
                    } else {
                        recent_projects.ssh_config_servers.clear();
                        recent_projects.ssh_config_updates = Task::ready(());
                    }
                }
            });

        Self {
            mode: Mode::default_mode(&BTreeSet::new(), cx),
            focus_handle,
            workspace,
            retained_connections: Vec::new(),
            ssh_config_updates,
            ssh_config_servers: BTreeSet::new(),
            create_new_window,
            _subscription,
        }
    }

    pub fn project_picker(
        create_new_window: bool,
        ix: usize,
        connection_options: remote::SshConnectionOptions,
        project: Entity<Project>,
        home_dir: RemotePathBuf,
        path_style: PathStyle,
        window: &mut Window,
        cx: &mut Context<Self>,
        workspace: WeakEntity<Workspace>,
    ) -> Self {
        let fs = project.read(cx).fs().clone();
        let mut this = Self::new(create_new_window, fs, window, workspace.clone(), cx);
        this.mode = Mode::ProjectPicker(ProjectPicker::new(
            create_new_window,
            ix,
            connection_options,
            project,
            home_dir,
            path_style,
            workspace,
            window,
            cx,
        ));
        cx.notify();

        this
    }

    fn create_ssh_server(
        &mut self,
        editor: Entity<Editor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = get_text(&editor, cx);
        if input.is_empty() {
            return;
        }

        let connection_options = match SshConnectionOptions::parse_command_line(&input) {
            Ok(c) => c,
            Err(e) => {
                self.mode = Mode::CreateRemoteServer(CreateRemoteServer {
                    address_editor: editor,
                    address_error: Some(format!("could not parse: {:?}", e).into()),
                    ssh_prompt: None,
                    _creating: None,
                });
                return;
            }
        };
        let ssh_prompt = cx.new(|cx| SshPrompt::new(&connection_options, window, cx));

        let connection = connect_over_ssh(
            ConnectionIdentifier::setup(),
            connection_options.clone(),
            ssh_prompt.clone(),
            window,
            cx,
        )
        .prompt_err("Failed to connect", window, cx, |_, _, _| None);

        let address_editor = editor.clone();
        let creating = cx.spawn_in(window, async move |this, cx| {
            match connection.await {
                Some(Some(client)) => this
                    .update_in(cx, |this, window, cx| {
                        this.retained_connections.push(client);
                        this.add_ssh_server(connection_options, cx);
                        this.mode = Mode::default_mode(&this.ssh_config_servers, cx);
                        this.focus_handle(cx).focus(window);
                        cx.notify()
                    })
                    .log_err(),
                _ => this
                    .update(cx, |this, cx| {
                        address_editor.update(cx, |this, _| {
                            this.set_read_only(false);
                        });
                        this.mode = Mode::CreateRemoteServer(CreateRemoteServer {
                            address_editor,
                            address_error: None,
                            ssh_prompt: None,
                            _creating: None,
                        });
                        cx.notify()
                    })
                    .log_err(),
            };
            None
        });

        editor.update(cx, |this, _| {
            this.set_read_only(true);
        });
        self.mode = Mode::CreateRemoteServer(CreateRemoteServer {
            address_editor: editor,
            address_error: None,
            ssh_prompt: Some(ssh_prompt.clone()),
            _creating: Some(creating),
        });
    }

    fn view_server_options(
        &mut self,
        (server_index, connection): (usize, SshConnection),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mode = Mode::ViewServerOptions(ViewServerOptionsState {
            server_index,
            connection,
            entries: std::array::from_fn(|_| NavigableEntry::focusable(cx)),
        });
        self.focus_handle(cx).focus(window);
        cx.notify();
    }

    fn create_ssh_project(
        &mut self,
        ix: usize,
        ssh_connection: SshConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };

        let create_new_window = self.create_new_window;
        let connection_options = ssh_connection.into();
        workspace.update(cx, |_, cx| {
            cx.defer_in(window, move |workspace, window, cx| {
                let app_state = workspace.app_state().clone();
                workspace.toggle_modal(window, cx, |window, cx| {
                    SshConnectionModal::new(&connection_options, Vec::new(), window, cx)
                });
                let prompt = workspace
                    .active_modal::<SshConnectionModal>(cx)
                    .unwrap()
                    .read(cx)
                    .prompt
                    .clone();

                let connect = connect_over_ssh(
                    ConnectionIdentifier::setup(),
                    connection_options.clone(),
                    prompt,
                    window,
                    cx,
                )
                .prompt_err("Failed to connect", window, cx, |_, _, _| None);

                cx.spawn_in(window, async move |workspace, cx| {
                    let session = connect.await;

                    workspace.update(cx, |workspace, cx| {
                        if let Some(prompt) = workspace.active_modal::<SshConnectionModal>(cx) {
                            prompt.update(cx, |prompt, cx| prompt.finished(cx))
                        }
                    })?;

                    let Some(Some(session)) = session else {
                        return workspace.update_in(cx, |workspace, window, cx| {
                            let weak = cx.entity().downgrade();
                            let fs = workspace.project().read(cx).fs().clone();
                            workspace.toggle_modal(window, cx, |window, cx| {
                                RemoteServerProjects::new(create_new_window, fs, window, weak, cx)
                            });
                        });
                    };

                    let (path_style, project) = cx.update(|_, cx| {
                        (
                            session.read(cx).path_style(),
                            project::Project::ssh(
                                session,
                                app_state.client.clone(),
                                app_state.user_store.clone(),
                                app_state.languages.clone(),
                                app_state.fs.clone(),
                                cx,
                            ),
                        )
                    })?;

                    let home_dir = project
                        .read_with(cx, |project, cx| project.resolve_abs_path("~", cx))?
                        .await
                        .and_then(|path| path.into_abs_path())
                        .map(|path| RemotePathBuf::new(path, path_style))
                        .unwrap_or_else(|| match path_style {
                            PathStyle::Posix => RemotePathBuf::from_str("/", PathStyle::Posix),
                            PathStyle::Windows => {
                                RemotePathBuf::from_str("C:\\", PathStyle::Windows)
                            }
                        });

                    workspace
                        .update_in(cx, |workspace, window, cx| {
                            let weak = cx.entity().downgrade();
                            workspace.toggle_modal(window, cx, |window, cx| {
                                RemoteServerProjects::project_picker(
                                    create_new_window,
                                    ix,
                                    connection_options,
                                    project,
                                    home_dir,
                                    path_style,
                                    window,
                                    cx,
                                    weak,
                                )
                            });
                        })
                        .ok();
                    Ok(())
                })
                .detach();
            })
        })
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        match &self.mode {
            Mode::Default(_) | Mode::ViewServerOptions(_) => {}
            Mode::ProjectPicker(_) => {}
            Mode::CreateRemoteServer(state) => {
                if let Some(prompt) = state.ssh_prompt.as_ref() {
                    prompt.update(cx, |prompt, cx| {
                        prompt.confirm(window, cx);
                    });
                    return;
                }

                self.create_ssh_server(state.address_editor.clone(), window, cx);
            }
            Mode::EditNickname(state) => {
                let text = Some(state.editor.read(cx).text(cx)).filter(|text| !text.is_empty());
                let index = state.index;
                self.update_settings_file(cx, move |setting, _| {
                    if let Some(connections) = setting.ssh_connections.as_mut() {
                        if let Some(connection) = connections.get_mut(index) {
                            connection.nickname = text;
                        }
                    }
                });
                self.mode = Mode::default_mode(&self.ssh_config_servers, cx);
                self.focus_handle.focus(window);
            }
        }
    }

    fn cancel(&mut self, _: &menu::Cancel, window: &mut Window, cx: &mut Context<Self>) {
        match &self.mode {
            Mode::Default(_) => cx.emit(DismissEvent),
            Mode::CreateRemoteServer(state) if state.ssh_prompt.is_some() => {
                let new_state = CreateRemoteServer::new(window, cx);
                let old_prompt = state.address_editor.read(cx).text(cx);
                new_state.address_editor.update(cx, |this, cx| {
                    this.set_text(old_prompt, window, cx);
                });

                self.mode = Mode::CreateRemoteServer(new_state);
                cx.notify();
            }
            _ => {
                self.mode = Mode::default_mode(&self.ssh_config_servers, cx);
                self.focus_handle(cx).focus(window);
                cx.notify();
            }
        }
    }

    fn render_ssh_connection(
        &mut self,
        ix: usize,
        ssh_server: RemoteEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let connection = ssh_server.connection().into_owned();
        let (main_label, aux_label) = if let Some(nickname) = connection.nickname.clone() {
            let aux_label = SharedString::from(format!("({})", connection.host));
            (nickname.into(), Some(aux_label))
        } else {
            (connection.host.clone(), None)
        };
        v_flex()
            .w_full()
            .child(ListSeparator)
            .child(
                h_flex()
                    .group("ssh-server")
                    .w_full()
                    .pt_0p5()
                    .px_3()
                    .gap_1()
                    .overflow_hidden()
                    .child(
                        div().max_w_96().overflow_hidden().text_ellipsis().child(
                            Label::new(main_label)
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    )
                    .children(
                        aux_label.map(|label| {
                            Label::new(label).size(LabelSize::Small).color(Color::Muted)
                        }),
                    ),
            )
            .child(match &ssh_server {
                RemoteEntry::Project {
                    open_folder,
                    projects,
                    configure,
                    connection,
                } => List::new()
                    .empty_message("No projects.")
                    .children(projects.iter().enumerate().map(|(pix, p)| {
                        v_flex().gap_0p5().child(self.render_ssh_project(
                            ix,
                            ssh_server.clone(),
                            pix,
                            p,
                            window,
                            cx,
                        ))
                    }))
                    .child(
                        h_flex()
                            .id(("new-remote-project-container", ix))
                            .track_focus(&open_folder.focus_handle)
                            .anchor_scroll(open_folder.scroll_anchor.clone())
                            .on_action(cx.listener({
                                let ssh_connection = connection.clone();
                                move |this, _: &menu::Confirm, window, cx| {
                                    this.create_ssh_project(ix, ssh_connection.clone(), window, cx);
                                }
                            }))
                            .child(
                                ListItem::new(("new-remote-project", ix))
                                    .toggle_state(
                                        open_folder.focus_handle.contains_focused(window, cx),
                                    )
                                    .inset(true)
                                    .spacing(ui::ListItemSpacing::Sparse)
                                    .start_slot(Icon::new(IconName::Plus).color(Color::Muted))
                                    .child(Label::new("Open Folder"))
                                    .on_click(cx.listener({
                                        let ssh_connection = connection.clone();
                                        move |this, _, window, cx| {
                                            this.create_ssh_project(
                                                ix,
                                                ssh_connection.clone(),
                                                window,
                                                cx,
                                            );
                                        }
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .id(("server-options-container", ix))
                            .track_focus(&configure.focus_handle)
                            .anchor_scroll(configure.scroll_anchor.clone())
                            .on_action(cx.listener({
                                let ssh_connection = connection.clone();
                                move |this, _: &menu::Confirm, window, cx| {
                                    this.view_server_options(
                                        (ix, ssh_connection.clone()),
                                        window,
                                        cx,
                                    );
                                }
                            }))
                            .child(
                                ListItem::new(("server-options", ix))
                                    .toggle_state(
                                        configure.focus_handle.contains_focused(window, cx),
                                    )
                                    .inset(true)
                                    .spacing(ui::ListItemSpacing::Sparse)
                                    .start_slot(Icon::new(IconName::Settings).color(Color::Muted))
                                    .child(Label::new("View Server Options"))
                                    .on_click(cx.listener({
                                        let ssh_connection = connection.clone();
                                        move |this, _, window, cx| {
                                            this.view_server_options(
                                                (ix, ssh_connection.clone()),
                                                window,
                                                cx,
                                            );
                                        }
                                    })),
                            ),
                    ),
                RemoteEntry::SshConfig { open_folder, host } => List::new().child(
                    h_flex()
                        .id(("new-remote-project-container", ix))
                        .track_focus(&open_folder.focus_handle)
                        .anchor_scroll(open_folder.scroll_anchor.clone())
                        .on_action(cx.listener({
                            let ssh_connection = connection.clone();
                            let host = host.clone();
                            move |this, _: &menu::Confirm, window, cx| {
                                let new_ix = this.create_host_from_ssh_config(&host, cx);
                                this.create_ssh_project(new_ix, ssh_connection.clone(), window, cx);
                            }
                        }))
                        .child(
                            ListItem::new(("new-remote-project", ix))
                                .toggle_state(open_folder.focus_handle.contains_focused(window, cx))
                                .inset(true)
                                .spacing(ui::ListItemSpacing::Sparse)
                                .start_slot(Icon::new(IconName::Plus).color(Color::Muted))
                                .child(Label::new("Open Folder"))
                                .on_click(cx.listener({
                                    let ssh_connection = connection.clone();
                                    let host = host.clone();
                                    move |this, _, window, cx| {
                                        let new_ix = this.create_host_from_ssh_config(&host, cx);
                                        this.create_ssh_project(
                                            new_ix,
                                            ssh_connection.clone(),
                                            window,
                                            cx,
                                        );
                                    }
                                })),
                        ),
                ),
            })
    }

    fn render_ssh_project(
        &mut self,
        server_ix: usize,
        server: RemoteEntry,
        ix: usize,
        (navigation, project): &(NavigableEntry, SshProject),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let create_new_window = self.create_new_window;
        let is_from_zed = server.is_from_zed();
        let element_id_base = SharedString::from(format!("remote-project-{server_ix}"));
        let container_element_id_base =
            SharedString::from(format!("remote-project-container-{element_id_base}"));

        let callback = Rc::new({
            let project = project.clone();
            move |remote_server_projects: &mut Self,
                  secondary_confirm: bool,
                  window: &mut Window,
                  cx: &mut Context<Self>| {
                let Some(app_state) = remote_server_projects
                    .workspace
                    .read_with(cx, |workspace, _| workspace.app_state().clone())
                    .log_err()
                else {
                    return;
                };
                let project = project.clone();
                let server = server.connection().into_owned();
                cx.emit(DismissEvent);

                let replace_window = match (create_new_window, secondary_confirm) {
                    (true, false) | (false, true) => None,
                    (true, true) | (false, false) => window.window_handle().downcast::<Workspace>(),
                };

                cx.spawn_in(window, async move |_, cx| {
                    let result = open_ssh_project(
                        server.into(),
                        project.paths.into_iter().map(PathBuf::from).collect(),
                        app_state,
                        OpenOptions {
                            replace_window,
                            ..OpenOptions::default()
                        },
                        cx,
                    )
                    .await;
                    if let Err(e) = result {
                        log::error!("Failed to connect: {e:#}");
                        cx.prompt(
                            gpui::PromptLevel::Critical,
                            "Failed to connect",
                            Some(&e.to_string()),
                            &["Ok"],
                        )
                        .await
                        .ok();
                    }
                })
                .detach();
            }
        });

        div()
            .id((container_element_id_base, ix))
            .track_focus(&navigation.focus_handle)
            .anchor_scroll(navigation.scroll_anchor.clone())
            .on_action(cx.listener({
                let callback = callback.clone();
                move |this, _: &menu::Confirm, window, cx| {
                    callback(this, false, window, cx);
                }
            }))
            .on_action(cx.listener({
                let callback = callback.clone();
                move |this, _: &menu::SecondaryConfirm, window, cx| {
                    callback(this, true, window, cx);
                }
            }))
            .child(
                ListItem::new((element_id_base, ix))
                    .toggle_state(navigation.focus_handle.contains_focused(window, cx))
                    .inset(true)
                    .spacing(ui::ListItemSpacing::Sparse)
                    .start_slot(
                        Icon::new(IconName::Folder)
                            .color(Color::Muted)
                            .size(IconSize::Small),
                    )
                    .child(Label::new(project.paths.join(", ")))
                    .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                        let secondary_confirm = e.down.modifiers.platform;
                        callback(this, secondary_confirm, window, cx)
                    }))
                    .when(is_from_zed, |server_list_item| {
                        server_list_item.end_hover_slot::<AnyElement>(Some(
                            div()
                                .mr_2()
                                .child({
                                    let project = project.clone();
                                    // Right-margin to offset it from the Scrollbar
                                    IconButton::new("remove-remote-project", IconName::TrashAlt)
                                        .icon_size(IconSize::Small)
                                        .shape(IconButtonShape::Square)
                                        .size(ButtonSize::Large)
                                        .tooltip(Tooltip::text("Delete Remote Project"))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.delete_ssh_project(server_ix, &project, cx)
                                        }))
                                })
                                .into_any_element(),
                        ))
                    }),
            )
    }

    fn update_settings_file(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut RemoteSettingsContent, &App) + Send + Sync + 'static,
    ) {
        let Some(fs) = self
            .workspace
            .read_with(cx, |workspace, _| workspace.app_state().fs.clone())
            .log_err()
        else {
            return;
        };
        update_settings_file::<SshSettings>(fs, cx, move |setting, cx| f(setting, cx));
    }

    fn delete_ssh_server(&mut self, server: usize, cx: &mut Context<Self>) {
        self.update_settings_file(cx, move |setting, _| {
            if let Some(connections) = setting.ssh_connections.as_mut() {
                connections.remove(server);
            }
        });
    }

    fn delete_ssh_project(&mut self, server: usize, project: &SshProject, cx: &mut Context<Self>) {
        let project = project.clone();
        self.update_settings_file(cx, move |setting, _| {
            if let Some(server) = setting
                .ssh_connections
                .as_mut()
                .and_then(|connections| connections.get_mut(server))
            {
                server.projects.remove(&project);
            }
        });
    }

    fn add_ssh_server(
        &mut self,
        connection_options: remote::SshConnectionOptions,
        cx: &mut Context<Self>,
    ) {
        self.update_settings_file(cx, move |setting, _| {
            setting
                .ssh_connections
                .get_or_insert(Default::default())
                .push(SshConnection {
                    host: SharedString::from(connection_options.host),
                    username: connection_options.username,
                    port: connection_options.port,
                    projects: BTreeSet::new(),
                    nickname: None,
                    args: connection_options.args.unwrap_or_default(),
                    upload_binary_over_ssh: None,
                    port_forwards: connection_options.port_forwards,
                })
        });
    }

    fn render_create_remote_server(
        &self,
        state: &CreateRemoteServer,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ssh_prompt = state.ssh_prompt.clone();

        state.address_editor.update(cx, |editor, cx| {
            if editor.text(cx).is_empty() {
                editor.set_placeholder_text("ssh user@example -p 2222", cx);
            }
        });

        let theme = cx.theme();

        v_flex()
            .track_focus(&self.focus_handle(cx))
            .id("create-remote-server")
            .overflow_hidden()
            .size_full()
            .flex_1()
            .child(
                div()
                    .p_2()
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .child(state.address_editor.clone()),
            )
            .child(
                h_flex()
                    .bg(theme.colors().editor_background)
                    .rounded_b_sm()
                    .w_full()
                    .map(|this| {
                        if let Some(ssh_prompt) = ssh_prompt {
                            this.child(h_flex().w_full().child(ssh_prompt))
                        } else if let Some(address_error) = &state.address_error {
                            this.child(
                                h_flex().p_2().w_full().gap_2().child(
                                    Label::new(address_error.clone())
                                        .size(LabelSize::Small)
                                        .color(Color::Error),
                                ),
                            )
                        } else {
                            this.child(
                                h_flex()
                                    .p_2()
                                    .w_full()
                                    .gap_1()
                                    .child(
                                        Label::new(
                                            "Enter the command you use to SSH into this server.",
                                        )
                                        .color(Color::Muted)
                                        .size(LabelSize::Small),
                                    )
                                    .child(
                                        Button::new("learn-more", "Learn more…")
                                            .label_size(LabelSize::Small)
                                            .size(ButtonSize::None)
                                            .color(Color::Accent)
                                            .style(ButtonStyle::Transparent)
                                            .on_click(|_, _, cx| {
                                                cx.open_url(
                                                    "https://zed.dev/docs/remote-development",
                                                );
                                            }),
                                    ),
                            )
                        }
                    }),
            )
    }

    fn render_view_options(
        &mut self,
        ViewServerOptionsState {
            server_index,
            connection,
            entries,
        }: ViewServerOptionsState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let connection_string = connection.host.clone();

        let mut view = Navigable::new(
            div()
                .track_focus(&self.focus_handle(cx))
                .size_full()
                .child(
                    SshConnectionHeader {
                        connection_string: connection_string.clone(),
                        paths: Default::default(),
                        nickname: connection.nickname.clone().map(|s| s.into()),
                    }
                    .render(window, cx),
                )
                .child(
                    v_flex()
                        .pb_1()
                        .child(ListSeparator)
                        .child({
                            let label = if connection.nickname.is_some() {
                                "Edit Nickname"
                            } else {
                                "Add Nickname to Server"
                            };
                            div()
                                .id("ssh-options-add-nickname")
                                .track_focus(&entries[0].focus_handle)
                                .on_action(cx.listener(
                                    move |this, _: &menu::Confirm, window, cx| {
                                        this.mode = Mode::EditNickname(EditNicknameState::new(
                                            server_index,
                                            window,
                                            cx,
                                        ));
                                        cx.notify();
                                    },
                                ))
                                .child(
                                    ListItem::new("add-nickname")
                                        .toggle_state(
                                            entries[0].focus_handle.contains_focused(window, cx),
                                        )
                                        .inset(true)
                                        .spacing(ui::ListItemSpacing::Sparse)
                                        .start_slot(Icon::new(IconName::Pencil).color(Color::Muted))
                                        .child(Label::new(label))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.mode = Mode::EditNickname(EditNicknameState::new(
                                                server_index,
                                                window,
                                                cx,
                                            ));
                                            cx.notify();
                                        })),
                                )
                        })
                        .child({
                            let workspace = self.workspace.clone();
                            fn callback(
                                workspace: WeakEntity<Workspace>,
                                connection_string: SharedString,
                                cx: &mut App,
                            ) {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    connection_string.to_string(),
                                ));
                                workspace
                                    .update(cx, |this, cx| {
                                        struct SshServerAddressCopiedToClipboard;
                                        let notification = format!(
                                            "Copied server address ({}) to clipboard",
                                            connection_string
                                        );

                                        this.show_toast(
                                            Toast::new(
                                                NotificationId::composite::<
                                                    SshServerAddressCopiedToClipboard,
                                                >(
                                                    connection_string.clone()
                                                ),
                                                notification,
                                            )
                                            .autohide(),
                                            cx,
                                        );
                                    })
                                    .ok();
                            }
                            div()
                                .id("ssh-options-copy-server-address")
                                .track_focus(&entries[1].focus_handle)
                                .on_action({
                                    let connection_string = connection_string.clone();
                                    let workspace = self.workspace.clone();
                                    move |_: &menu::Confirm, _, cx| {
                                        callback(workspace.clone(), connection_string.clone(), cx);
                                    }
                                })
                                .child(
                                    ListItem::new("copy-server-address")
                                        .toggle_state(
                                            entries[1].focus_handle.contains_focused(window, cx),
                                        )
                                        .inset(true)
                                        .spacing(ui::ListItemSpacing::Sparse)
                                        .start_slot(Icon::new(IconName::Copy).color(Color::Muted))
                                        .child(Label::new("Copy Server Address"))
                                        .end_hover_slot(
                                            Label::new(connection_string.clone())
                                                .color(Color::Muted),
                                        )
                                        .on_click({
                                            let connection_string = connection_string.clone();
                                            move |_, _, cx| {
                                                callback(
                                                    workspace.clone(),
                                                    connection_string.clone(),
                                                    cx,
                                                );
                                            }
                                        }),
                                )
                        })
                        .child({
                            fn remove_ssh_server(
                                remote_servers: Entity<RemoteServerProjects>,
                                index: usize,
                                connection_string: SharedString,
                                window: &mut Window,
                                cx: &mut App,
                            ) {
                                let prompt_message =
                                    format!("Remove server `{}`?", connection_string);

                                let confirmation = window.prompt(
                                    PromptLevel::Warning,
                                    &prompt_message,
                                    None,
                                    &["Yes, remove it", "No, keep it"],
                                    cx,
                                );

                                cx.spawn(async move |cx| {
                                    if confirmation.await.ok() == Some(0) {
                                        remote_servers
                                            .update(cx, |this, cx| {
                                                this.delete_ssh_server(index, cx);
                                            })
                                            .ok();
                                        remote_servers
                                            .update(cx, |this, cx| {
                                                this.mode = Mode::default_mode(
                                                    &this.ssh_config_servers,
                                                    cx,
                                                );
                                                cx.notify();
                                            })
                                            .ok();
                                    }
                                    anyhow::Ok(())
                                })
                                .detach_and_log_err(cx);
                            }
                            div()
                                .id("ssh-options-copy-server-address")
                                .track_focus(&entries[2].focus_handle)
                                .on_action(cx.listener({
                                    let connection_string = connection_string.clone();
                                    move |_, _: &menu::Confirm, window, cx| {
                                        remove_ssh_server(
                                            cx.entity().clone(),
                                            server_index,
                                            connection_string.clone(),
                                            window,
                                            cx,
                                        );
                                        cx.focus_self(window);
                                    }
                                }))
                                .child(
                                    ListItem::new("remove-server")
                                        .toggle_state(
                                            entries[2].focus_handle.contains_focused(window, cx),
                                        )
                                        .inset(true)
                                        .spacing(ui::ListItemSpacing::Sparse)
                                        .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                                        .child(Label::new("Remove Server").color(Color::Error))
                                        .on_click(cx.listener(move |_, _, window, cx| {
                                            remove_ssh_server(
                                                cx.entity().clone(),
                                                server_index,
                                                connection_string.clone(),
                                                window,
                                                cx,
                                            );
                                            cx.focus_self(window);
                                        })),
                                )
                        })
                        .child(ListSeparator)
                        .child({
                            div()
                                .id("ssh-options-copy-server-address")
                                .track_focus(&entries[3].focus_handle)
                                .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| {
                                    this.mode = Mode::default_mode(&this.ssh_config_servers, cx);
                                    cx.focus_self(window);
                                    cx.notify();
                                }))
                                .child(
                                    ListItem::new("go-back")
                                        .toggle_state(
                                            entries[3].focus_handle.contains_focused(window, cx),
                                        )
                                        .inset(true)
                                        .spacing(ui::ListItemSpacing::Sparse)
                                        .start_slot(
                                            Icon::new(IconName::ArrowLeft).color(Color::Muted),
                                        )
                                        .child(Label::new("Go Back"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.mode =
                                                Mode::default_mode(&this.ssh_config_servers, cx);
                                            cx.focus_self(window);
                                            cx.notify()
                                        })),
                                )
                        }),
                )
                .into_any_element(),
        );
        for entry in entries {
            view = view.entry(entry);
        }

        view.render(window, cx).into_any_element()
    }

    fn render_edit_nickname(
        &self,
        state: &EditNicknameState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(connection) = SshSettings::get_global(cx)
            .ssh_connections()
            .nth(state.index)
        else {
            return v_flex()
                .id("ssh-edit-nickname")
                .track_focus(&self.focus_handle(cx));
        };

        let connection_string = connection.host.clone();
        let nickname = connection.nickname.clone().map(|s| s.into());

        v_flex()
            .id("ssh-edit-nickname")
            .track_focus(&self.focus_handle(cx))
            .child(
                SshConnectionHeader {
                    connection_string,
                    paths: Default::default(),
                    nickname,
                }
                .render(window, cx),
            )
            .child(
                h_flex()
                    .p_2()
                    .border_t_1()
                    .border_color(cx.theme().colors().border_variant)
                    .child(state.editor.clone()),
            )
    }

    fn render_default(
        &mut self,
        mut state: DefaultState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ssh_settings = SshSettings::get_global(cx);
        let mut should_rebuild = false;

        if ssh_settings
            .ssh_connections
            .as_ref()
            .map_or(false, |connections| {
                state
                    .servers
                    .iter()
                    .filter_map(|server| match server {
                        RemoteEntry::Project { connection, .. } => Some(connection),
                        RemoteEntry::SshConfig { .. } => None,
                    })
                    .ne(connections.iter())
            })
        {
            should_rebuild = true;
        };

        if !should_rebuild && ssh_settings.read_ssh_config {
            let current_ssh_hosts: BTreeSet<SharedString> = state
                .servers
                .iter()
                .filter_map(|server| match server {
                    RemoteEntry::SshConfig { host, .. } => Some(host.clone()),
                    _ => None,
                })
                .collect();
            let mut expected_ssh_hosts = self.ssh_config_servers.clone();
            for server in &state.servers {
                if let RemoteEntry::Project { connection, .. } = server {
                    expected_ssh_hosts.remove(&connection.host);
                }
            }
            should_rebuild = current_ssh_hosts != expected_ssh_hosts;
        }

        if should_rebuild {
            self.mode = Mode::default_mode(&self.ssh_config_servers, cx);
            if let Mode::Default(new_state) = &self.mode {
                state = new_state.clone();
            }
        }

        let scroll_state = state.scrollbar.parent_entity(&cx.entity());
        let connect_button = div()
            .id("ssh-connect-new-server-container")
            .track_focus(&state.add_new_server.focus_handle)
            .anchor_scroll(state.add_new_server.scroll_anchor.clone())
            .child(
                ListItem::new("register-remove-server-button")
                    .toggle_state(
                        state
                            .add_new_server
                            .focus_handle
                            .contains_focused(window, cx),
                    )
                    .inset(true)
                    .spacing(ui::ListItemSpacing::Sparse)
                    .start_slot(Icon::new(IconName::Plus).color(Color::Muted))
                    .child(Label::new("Connect New Server"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        let state = CreateRemoteServer::new(window, cx);
                        this.mode = Mode::CreateRemoteServer(state);

                        cx.notify();
                    })),
            )
            .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| {
                let state = CreateRemoteServer::new(window, cx);
                this.mode = Mode::CreateRemoteServer(state);

                cx.notify();
            }));

        let handle = &**scroll_state.scroll_handle() as &dyn Any;
        let Some(scroll_handle) = handle.downcast_ref::<ScrollHandle>() else {
            unreachable!()
        };

        let mut modal_section = Navigable::new(
            v_flex()
                .track_focus(&self.focus_handle(cx))
                .id("ssh-server-list")
                .overflow_y_scroll()
                .track_scroll(&scroll_handle)
                .size_full()
                .child(connect_button)
                .child(
                    List::new()
                        .empty_message(
                            v_flex()
                                .child(
                                    div().px_3().child(
                                        Label::new("No remote servers registered yet.")
                                            .color(Color::Muted),
                                    ),
                                )
                                .into_any_element(),
                        )
                        .children(state.servers.iter().enumerate().map(|(ix, connection)| {
                            self.render_ssh_connection(ix, connection.clone(), window, cx)
                                .into_any_element()
                        })),
                )
                .into_any_element(),
        )
        .entry(state.add_new_server.clone());

        for server in &state.servers {
            match server {
                RemoteEntry::Project {
                    open_folder,
                    projects,
                    configure,
                    ..
                } => {
                    for (navigation_state, _) in projects {
                        modal_section = modal_section.entry(navigation_state.clone());
                    }
                    modal_section = modal_section
                        .entry(open_folder.clone())
                        .entry(configure.clone());
                }
                RemoteEntry::SshConfig { open_folder, .. } => {
                    modal_section = modal_section.entry(open_folder.clone());
                }
            }
        }
        let mut modal_section = modal_section.render(window, cx).into_any_element();

        let (create_window, reuse_window) = if self.create_new_window {
            (
                window.keystroke_text_for(&menu::Confirm),
                window.keystroke_text_for(&menu::SecondaryConfirm),
            )
        } else {
            (
                window.keystroke_text_for(&menu::SecondaryConfirm),
                window.keystroke_text_for(&menu::Confirm),
            )
        };
        let placeholder_text = Arc::from(format!(
            "{reuse_window} reuses this window, {create_window} opens a new one",
        ));

        Modal::new("remote-projects", None)
            .header(
                ModalHeader::new()
                    .child(Headline::new("Remote Projects").size(HeadlineSize::XSmall))
                    .child(
                        Label::new(placeholder_text)
                            .color(Color::Muted)
                            .size(LabelSize::XSmall),
                    ),
            )
            .section(
                Section::new().padded(false).child(
                    v_flex()
                        .min_h(rems(20.))
                        .size_full()
                        .relative()
                        .child(ListSeparator)
                        .child(
                            canvas(
                                |bounds, window, cx| {
                                    modal_section.prepaint_as_root(
                                        bounds.origin,
                                        bounds.size.into(),
                                        window,
                                        cx,
                                    );
                                    modal_section
                                },
                                |_, mut modal_section, window, cx| {
                                    modal_section.paint(window, cx);
                                },
                            )
                            .size_full(),
                        )
                        .child(
                            div()
                                .occlude()
                                .h_full()
                                .absolute()
                                .top_1()
                                .bottom_1()
                                .right_1()
                                .w(px(8.))
                                .children(Scrollbar::vertical(scroll_state)),
                        ),
                ),
            )
            .into_any_element()
    }

    fn create_host_from_ssh_config(
        &mut self,
        ssh_config_host: &SharedString,
        cx: &mut Context<'_, Self>,
    ) -> usize {
        let new_ix = Arc::new(AtomicUsize::new(0));

        let update_new_ix = new_ix.clone();
        self.update_settings_file(cx, move |settings, _| {
            update_new_ix.store(
                settings
                    .ssh_connections
                    .as_ref()
                    .map_or(0, |connections| connections.len()),
                atomic::Ordering::Release,
            );
        });

        self.add_ssh_server(
            SshConnectionOptions {
                host: ssh_config_host.to_string(),
                ..SshConnectionOptions::default()
            },
            cx,
        );
        self.mode = Mode::default_mode(&self.ssh_config_servers, cx);
        new_ix.load(atomic::Ordering::Acquire)
    }
}

fn spawn_ssh_config_watch(fs: Arc<dyn Fs>, cx: &Context<RemoteServerProjects>) -> Task<()> {
    let mut user_ssh_config_watcher =
        watch_config_file(cx.background_executor(), fs.clone(), user_ssh_config_file());
    let mut global_ssh_config_watcher = watch_config_file(
        cx.background_executor(),
        fs,
        global_ssh_config_file().to_owned(),
    );

    cx.spawn(async move |remote_server_projects, cx| {
        let mut global_hosts = BTreeSet::default();
        let mut user_hosts = BTreeSet::default();
        let mut running_receivers = 2;

        loop {
            select! {
                new_global_file_contents = global_ssh_config_watcher.next().fuse() => {
                    match new_global_file_contents {
                        Some(new_global_file_contents) => {
                            global_hosts = parse_ssh_config_hosts(&new_global_file_contents);
                            if remote_server_projects.update(cx, |remote_server_projects, cx| {
                                remote_server_projects.ssh_config_servers = global_hosts.iter().chain(user_hosts.iter()).map(SharedString::from).collect();
                                cx.notify();
                            }).is_err() {
                                return;
                            }
                        },
                        None => {
                            running_receivers -= 1;
                            if running_receivers == 0 {
                                return;
                            }
                        }
                    }
                },
                new_user_file_contents = user_ssh_config_watcher.next().fuse() => {
                    match new_user_file_contents {
                        Some(new_user_file_contents) => {
                            user_hosts = parse_ssh_config_hosts(&new_user_file_contents);
                            if remote_server_projects.update(cx, |remote_server_projects, cx| {
                                remote_server_projects.ssh_config_servers = global_hosts.iter().chain(user_hosts.iter()).map(SharedString::from).collect();
                                cx.notify();
                            }).is_err() {
                                return;
                            }
                        },
                        None => {
                            running_receivers -= 1;
                            if running_receivers == 0 {
                                return;
                            }
                        }
                    }
                },
            }
        }
    })
}

fn get_text(element: &Entity<Editor>, cx: &mut App) -> String {
    element.read(cx).text(cx).trim().to_string()
}

impl ModalView for RemoteServerProjects {}

impl Focusable for RemoteServerProjects {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.mode {
            Mode::ProjectPicker(picker) => picker.focus_handle(cx),
            _ => self.focus_handle.clone(),
        }
    }
}

impl EventEmitter<DismissEvent> for RemoteServerProjects {}

impl Render for RemoteServerProjects {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .elevation_3(cx)
            .w(rems(34.))
            .key_context("RemoteServerModal")
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::confirm))
            .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                this.focus_handle(cx).focus(window);
            }))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if matches!(this.mode, Mode::Default(_)) {
                    cx.emit(DismissEvent)
                }
            }))
            .child(match &self.mode {
                Mode::Default(state) => self
                    .render_default(state.clone(), window, cx)
                    .into_any_element(),
                Mode::ViewServerOptions(state) => self
                    .render_view_options(state.clone(), window, cx)
                    .into_any_element(),
                Mode::ProjectPicker(element) => element.clone().into_any_element(),
                Mode::CreateRemoteServer(state) => self
                    .render_create_remote_server(state, cx)
                    .into_any_element(),
                Mode::EditNickname(state) => self
                    .render_edit_nickname(state, window, cx)
                    .into_any_element(),
            })
    }
}
