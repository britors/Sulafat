//! Main window: `AdwNavigationSplitView` with the host sidebar and a tabbed session area.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use gtk::glib::clone;
use sulafat_core::command::{build_ssh_command, ConnectTarget};
use sulafat_core::metadata::Metadata;
use sulafat_core::ssh_config::{SshConfig, SshHost};

use crate::host_dialog;
use crate::host_list::{HostAction, HostList};
use crate::prefs::{self, Settings};
use crate::quick_connect;
use crate::terminal_tab::{color_dot_texture, TerminalTab};

const RUNNING_DATA_KEY: &str = "sulafat-running";

struct AppState {
    cfg: SshConfig,
    metadata: Metadata,
    settings: Settings,
}

impl AppState {
    fn load() -> Self {
        let cfg = SshConfig::load().unwrap_or_else(|e| {
            tracing::error!("falha ao carregar ~/.ssh/config: {e}");
            // A config we can't load is treated like an empty one; the user can still create
            // hosts (though saving will surface the same underlying error).
            SshConfig::load_from(std::env::temp_dir().join("unreachable-sulafat-config")).expect("empty in-memory config")
        });
        let metadata = Metadata::load().unwrap_or_default();
        let settings = Settings::load();
        Self { cfg, metadata, settings }
    }

    fn known_aliases(&self) -> Vec<String> {
        self.cfg.list_hosts().into_iter().filter(|h| !h.read_only).map(|h| h.alias).collect()
    }

    fn persist(&mut self) {
        if let Err(e) = self.cfg.save() {
            tracing::error!("falha ao salvar ~/.ssh/config: {e}");
        }
        let known = self.known_aliases();
        if let Err(e) = self.metadata.save(&known) {
            tracing::error!("falha ao salvar metadados: {e}");
        }
    }
}

fn refresh(state: &Rc<RefCell<AppState>>, host_list: &HostList) {
    let state = state.borrow();
    host_list.set_hosts(state.cfg.list_hosts(), &state.metadata);
}

fn sftp_uri(host: &SshHost) -> String {
    let host_part = host.host_name.clone().unwrap_or_else(|| host.alias.clone());
    let user_part = host.user.as_ref().map(|u| format!("{u}@")).unwrap_or_default();
    let port_part = host.port.map(|p| format!(":{p}")).unwrap_or_default();
    format!("sftp://{user_part}{host_part}{port_part}/")
}

fn unique_alias(base: &str, existing: &[String]) -> String {
    let candidate = format!("{base}-copia");
    if !existing.contains(&candidate) {
        return candidate;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-copia-{n}");
        if !existing.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn confirm(window: &adw::ApplicationWindow, heading: &str, body: &str, on_confirm: impl FnOnce() + 'static) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_responses(&[("cancel", "Cancelar"), ("confirm", "Confirmar")]);
    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.choose(
        Some(window),
        None::<&gio::Cancellable>,
        move |response| {
            if response == "confirm" {
                on_confirm();
            }
        },
    );
}

pub fn build(app: &adw::Application, initial_host: Option<SshHost>) {
    let state = Rc::new(RefCell::new(AppState::load()));

    let host_list = Rc::new(HostList::new());

    let split_view = adw::NavigationSplitView::new();

    let sidebar_header = adw::HeaderBar::new();
    let add_btn = gtk::Button::from_icon_name("list-add-symbolic");
    add_btn.set_tooltip_text(Some("Nova conexão"));
    sidebar_header.pack_start(&add_btn);

    let quick_btn = gtk::MenuButton::builder().icon_name("edit-find-symbolic").tooltip_text("Conexão rápida").build();
    sidebar_header.pack_end(&quick_btn);

    let menu_btn = gtk::MenuButton::builder().icon_name("open-menu-symbolic").build();
    let app_menu = gio::Menu::new();
    app_menu.append(Some("Nova janela"), Some("win.new-window"));
    app_menu.append(Some("Preferências"), Some("win.preferences"));
    menu_btn.set_menu_model(Some(&app_menu));
    sidebar_header.pack_end(&menu_btn);

    let sidebar_toolbar = adw::ToolbarView::new();
    sidebar_toolbar.add_top_bar(&sidebar_header);
    sidebar_toolbar.set_content(Some(host_list.widget()));
    let sidebar_page = adw::NavigationPage::new(&sidebar_toolbar, "Hosts");
    split_view.set_sidebar(Some(&sidebar_page));

    let tab_view = adw::TabView::new();
    let tab_bar = adw::TabBar::builder().view(&tab_view).build();

    let empty_box = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(12).halign(gtk::Align::Center).build();
    let empty_new_btn = gtk::Button::builder().label("Nova conexão").css_classes(["pill"]).build();
    let empty_quick_btn = gtk::Button::builder().label("Conexão rápida").css_classes(["pill", "suggested-action"]).build();
    empty_box.append(&empty_new_btn);
    empty_box.append(&empty_quick_btn);
    let status_page = adw::StatusPage::builder()
        .title("Nenhuma sessão aberta")
        .description("Selecione um host na barra lateral ou use a conexão rápida")
        .icon_name("utilities-terminal-symbolic")
        .child(&empty_box)
        .vexpand(true)
        .build();

    let content_stack = gtk::Stack::new();
    content_stack.add_named(&status_page, Some("empty"));
    content_stack.add_named(&tab_view, Some("tabs"));
    let update_stack = clone!(
        #[weak]
        content_stack,
        #[weak]
        tab_view,
        move || content_stack.set_visible_child_name(if tab_view.n_pages() == 0 { "empty" } else { "tabs" })
    );
    tab_view.connect_close_page(|_, _| glib::Propagation::Proceed);
    tab_view.pages().connect_items_changed(clone!(
        #[strong]
        update_stack,
        move |_, _, _, _| update_stack()
    ));

    let content_toolbar = adw::ToolbarView::new();
    content_toolbar.add_top_bar(&adw::HeaderBar::new());
    content_toolbar.add_top_bar(&tab_bar);
    content_toolbar.set_content(Some(&content_stack));
    let content_page = adw::NavigationPage::new(&content_toolbar, "Sessões");
    split_view.set_content(Some(&content_page));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Sulafat")
        .default_width(1000)
        .default_height(680)
        .content(&split_view)
        .build();

    // --- tab spawning -----------------------------------------------------------------------
    let spawn_tab = {
        let state = state.clone();
        let tab_view = tab_view.clone();
        move |argv: Vec<String>, title: String, color: Option<String>| {
            let settings = state.borrow().settings.clone();
            let page_cell: Rc<RefCell<Option<adw::TabPage>>> = Rc::new(RefCell::new(None));
            let tab = Rc::new(TerminalTab::new(
                argv,
                &settings,
                clone!(
                    #[strong]
                    page_cell,
                    #[weak]
                    tab_view,
                    move || {
                        if let Some(page) = page_cell.borrow().clone() {
                            tab_view.close_page(&page);
                        }
                    }
                ),
            ));
            let page = tab_view.append(tab.widget());
            page.set_title(&title);
            page.set_icon(Some(&color_dot_texture(color.as_deref())));
            let getter: Box<dyn Fn() -> bool> = {
                let tab = tab.clone();
                Box::new(move || tab.is_running())
            };
            unsafe { tab.widget().set_data(RUNNING_DATA_KEY, getter) };
            *page_cell.borrow_mut() = Some(page.clone());
            tab_view.set_selected_page(&page);
            tab.terminal().grab_focus();
        }
    };

    let open_host = {
        let state = state.clone();
        let spawn_tab = spawn_tab.clone();
        move |host: SshHost| {
            let argv = build_ssh_command(&ConnectTarget::Alias(host.alias.clone()));
            let color = state.borrow().metadata.get(&host.alias).and_then(|m| m.color.clone());
            spawn_tab(argv, host.alias.clone(), color);
        }
    };

    // --- host list actions -------------------------------------------------------------------
    let on_action = {
        let state = state.clone();
        let host_list = host_list.clone();
        let window = window.clone();
        let app = app.clone();
        let open_host = open_host.clone();
        move |action: HostAction| match action {
            HostAction::Connect(host) => open_host(host),
            HostAction::ConnectNewWindow(host) => build(&app, Some(host)),
            HostAction::Edit(host, meta) => {
                let state = state.clone();
                let host_list = host_list.clone();
                let previous_alias = host.alias.clone();
                let groups = state.borrow().metadata.groups();
                host_dialog::edit(&window, Some(host), meta, &groups, move |result| {
                    let Some((new_host, new_meta)) = result else { return };
                    let mut s = state.borrow_mut();
                    if new_host.alias == previous_alias {
                        s.cfg.upsert_host(new_host.clone());
                    } else {
                        s.cfg.upsert_host_renaming(&previous_alias, new_host.clone());
                    }
                    s.metadata.set(new_host.alias.clone(), new_meta);
                    s.persist();
                    drop(s);
                    refresh(&state, &host_list);
                });
            }
            HostAction::Duplicate(host, meta) => {
                let mut s = state.borrow_mut();
                let existing = s.known_aliases();
                let new_alias = unique_alias(&host.alias, &existing);
                let mut new_host = host.clone();
                new_host.alias = new_alias.clone();
                s.cfg.upsert_host(new_host);
                s.metadata.set(new_alias, meta);
                s.persist();
                drop(s);
                refresh(&state, &host_list);
            }
            HostAction::OpenFiles(host) => {
                gtk::UriLauncher::new(&sftp_uri(&host)).launch(Some(&window), None::<&gio::Cancellable>, |_| {});
            }
            HostAction::Delete(host) => {
                let state = state.clone();
                let host_list = host_list.clone();
                let alias = host.alias.clone();
                confirm(
                    &window,
                    "Excluir host?",
                    &format!("Excluir \u{201c}{alias}\u{201d} de ~/.ssh/config? Um backup é sempre mantido em config.sulafat.bak."),
                    move || {
                        let mut s = state.borrow_mut();
                        s.cfg.remove_host(&alias);
                        s.metadata.set(alias, Default::default());
                        s.persist();
                        drop(s);
                        refresh(&state, &host_list);
                    },
                );
            }
        }
    };
    host_list.set_action_handler(on_action);

    // --- header buttons -----------------------------------------------------------------------
    add_btn.connect_clicked(clone!(
        #[weak]
        window,
        #[strong]
        state,
        #[strong]
        host_list,
        move |_| {
            let groups = state.borrow().metadata.groups();
            let state = state.clone();
            let host_list = host_list.clone();
            host_dialog::edit(&window, None, Default::default(), &groups, move |result| {
                let Some((new_host, new_meta)) = result else { return };
                let mut s = state.borrow_mut();
                s.cfg.upsert_host(new_host.clone());
                s.metadata.set(new_host.alias, new_meta);
                s.persist();
                drop(s);
                refresh(&state, &host_list);
            });
        }
    ));
    empty_new_btn.connect_clicked(clone!(
        #[weak]
        add_btn,
        move |_| add_btn.emit_clicked()
    ));

    let open_quick = {
        let spawn_tab = spawn_tab.clone();
        move |target: ConnectTarget| {
            let title = match &target {
                ConnectTarget::Alias(alias) => alias.clone(),
                ConnectTarget::Quick { user, host, .. } => match user {
                    Some(u) => format!("{u}@{host}"),
                    None => host.clone(),
                },
            };
            spawn_tab(build_ssh_command(&target), title, None);
        }
    };
    quick_btn.set_popover(Some(&quick_connect::popover(open_quick)));
    empty_quick_btn.connect_clicked(clone!(
        #[weak]
        quick_btn,
        move |_| quick_btn.popup()
    ));

    // --- window actions (new window / preferences) --------------------------------------------
    let new_window_action = gio::SimpleAction::new("new-window", None);
    new_window_action.connect_activate(clone!(
        #[weak]
        app,
        move |_, _| build(&app, None)
    ));
    window.add_action(&new_window_action);

    let prefs_action = gio::SimpleAction::new("preferences", None);
    prefs_action.connect_activate(clone!(
        #[weak]
        window,
        #[strong]
        state,
        move |_, _| {
            let current = state.borrow().settings.clone();
            let state = state.clone();
            prefs::show(&window, current, move |new_settings| {
                state.borrow_mut().settings = new_settings;
            });
        }
    ));
    window.add_action(&prefs_action);

    let focus_search_action = gio::SimpleAction::new("focus-search", None);
    focus_search_action.connect_activate(clone!(
        #[strong]
        host_list,
        move |_, _| {
            host_list.search_entry().grab_focus();
        }
    ));
    window.add_action(&focus_search_action);
    app.set_accels_for_action("win.focus-search", &["<Ctrl>F"]);

    // --- external ~/.ssh/config changes ---------------------------------------------------------
    if let Ok(watcher) = sulafat_core::watch::ConfigWatcher::watch(state.borrow().cfg.path()) {
        glib::timeout_add_local(sulafat_core::watch::DEBOUNCE, clone!(
            #[strong]
            state,
            #[strong]
            host_list,
            move || {
                if watcher.poll() {
                    if let Ok(fresh) = SshConfig::load_from(state.borrow().cfg.path()) {
                        state.borrow_mut().cfg = fresh;
                        refresh(&state, &host_list);
                    }
                }
                glib::ControlFlow::Continue
            }
        ));
    }

    // --- close confirmation for active sessions -------------------------------------------------
    window.connect_close_request(clone!(
        #[weak]
        window,
        #[weak]
        tab_view,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_| {
            let pages = tab_view.pages();
            let any_running = (0..pages.n_items()).any(|i| {
                pages
                    .item(i)
                    .and_downcast::<adw::TabPage>()
                    .map(|page| is_running(&page))
                    .unwrap_or(false)
            });
            if !any_running {
                return glib::Propagation::Proceed;
            }
            confirm(
                &window,
                "Fechar janela?",
                "Há sessões SSH ativas nesta janela. Deseja mesmo fechar?",
                clone!(
                    #[weak]
                    window,
                    move || window.destroy()
                ),
            );
            glib::Propagation::Stop
        }
    ));

    tab_view.connect_close_page(clone!(
        #[weak]
        window,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |tab_view, page| {
            if !is_running(page) {
                return glib::Propagation::Proceed;
            }
            let tab_view = tab_view.clone();
            let page = page.clone();
            confirm(&window, "Encerrar sessão?", "Esta aba tem uma sessão SSH ativa. Deseja mesmo fechá-la?", move || {
                tab_view.close_page_finish(&page, true);
            });
            glib::Propagation::Stop
        }
    ));

    refresh(&state, &host_list);

    if let Some(host) = initial_host {
        open_host(host);
    }

    window.present();
}

fn is_running(page: &adw::TabPage) -> bool {
    let child = page.child();
    unsafe { child.data::<Box<dyn Fn() -> bool>>(RUNNING_DATA_KEY).map(|p| p.as_ref()()).unwrap_or(false) }
}
