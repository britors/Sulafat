//! Main window: `AdwNavigationSplitView` with the host sidebar and a tabbed session area.

use std::cell::RefCell;
use std::collections::HashSet;
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
use crate::i18n::{format as tr_format, tr};
use crate::launcher_badge;
use crate::prefs::{self, Settings};
use crate::quick_connect;
use crate::terminal_tab::{color_dot_texture, TerminalTab};

const RUNNING_DATA_KEY: &str = "sulafat-running";
const ALIAS_DATA_KEY: &str = "sulafat-alias";

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
            SshConfig::load_from(std::env::temp_dir().join("unreachable-sulafat-config"))
                .expect("empty in-memory config")
        });
        let metadata = Metadata::load().unwrap_or_default();
        let settings = Settings::load();
        Self {
            cfg,
            metadata,
            settings,
        }
    }

    fn known_aliases(&self) -> Vec<String> {
        self.cfg
            .list_hosts()
            .into_iter()
            .filter(|h| !h.read_only)
            .map(|h| h.alias)
            .collect()
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

fn refresh(state: &Rc<RefCell<AppState>>, host_list: &HostList, tab_view: &adw::TabView) {
    let window_id = Rc::as_ptr(state) as usize;
    let borrowed = state.borrow();
    host_list.set_hosts(borrowed.cfg.list_hosts(), &borrowed.metadata);
    drop(borrowed);
    let connected = connected_aliases(tab_view);
    launcher_badge::report(window_id, connected.len() as u32);
    host_list.set_connected(&connected);
}

/// Aliases of every tab in `tab_view` whose `ssh` child is currently running — used to light up
/// the "connected" indicator in the sidebar. Quick-connect tabs (no saved alias) aren't in the
/// sidebar, so they're naturally excluded.
fn connected_aliases(tab_view: &adw::TabView) -> HashSet<String> {
    let pages = tab_view.pages();
    (0..pages.n_items())
        .filter_map(|i| pages.item(i).and_downcast::<adw::TabPage>())
        .filter(is_running)
        .filter_map(|page| alias_of(&page))
        .collect()
}

fn alias_of(page: &adw::TabPage) -> Option<String> {
    let child = page.child();
    unsafe {
        child
            .data::<Option<String>>(ALIAS_DATA_KEY)
            .and_then(|p| p.as_ref().clone())
    }
}

/// The existing tab for `alias`, if any — regardless of whether its session is still connecting,
/// running, or already ended, so `open_host` never spawns a second `ssh` for the same host.
fn find_tab_by_alias(tab_view: &adw::TabView, alias: &str) -> Option<adw::TabPage> {
    let pages = tab_view.pages();
    (0..pages.n_items())
        .filter_map(|i| pages.item(i).and_downcast::<adw::TabPage>())
        .find(|page| alias_of(page).as_deref() == Some(alias))
}

fn sftp_uri(host: &SshHost) -> String {
    let host_part = host.host_name.clone().unwrap_or_else(|| host.alias.clone());
    let user_part = host
        .user
        .as_ref()
        .map(|u| format!("{u}@"))
        .unwrap_or_default();
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

fn confirm(
    window: &adw::ApplicationWindow,
    heading: &str,
    body: &str,
    on_confirm: impl FnOnce() + 'static,
) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("confirm", &tr("Confirm"))]);
    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.choose(Some(window), None::<&gio::Cancellable>, move |response| {
        if response == "confirm" {
            on_confirm();
        }
    });
}

pub fn build(app: &adw::Application, initial_host: Option<SshHost>) {
    let state = Rc::new(RefCell::new(AppState::load()));

    let host_list = Rc::new(HostList::new());

    let split_view = adw::NavigationSplitView::new();

    let sidebar_header = adw::HeaderBar::new();
    let add_btn = gtk::Button::from_icon_name("list-add-symbolic");
    add_btn.set_tooltip_text(Some(&tr("New connection")));
    let accessible_label = tr("New connection");
    add_btn.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
    sidebar_header.pack_start(&add_btn);

    let quick_btn = gtk::MenuButton::builder()
        .icon_name("edit-find-symbolic")
        .tooltip_text(tr("Quick connection"))
        .build();
    let accessible_label = tr("Quick connection");
    quick_btn.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
    sidebar_header.pack_end(&quick_btn);

    let menu_btn = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text(tr("Main menu"))
        .build();
    let accessible_label = tr("Main menu");
    menu_btn.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
    let app_menu = gio::Menu::new();
    let settings_section = gio::Menu::new();
    settings_section.append(Some(&tr("Settings")), Some("win.preferences"));
    app_menu.append_section(None, &settings_section);
    let about_section = gio::Menu::new();
    about_section.append(Some(&tr("About Sulafat")), Some("win.about"));
    app_menu.append_section(None, &about_section);
    menu_btn.set_menu_model(Some(&app_menu));

    let sidebar_toolbar = adw::ToolbarView::new();
    sidebar_toolbar.add_top_bar(&sidebar_header);
    sidebar_toolbar.set_content(Some(host_list.widget()));
    let sidebar_page = adw::NavigationPage::new(&sidebar_toolbar, &tr("Hosts"));
    split_view.set_sidebar(Some(&sidebar_page));

    let tab_view = adw::TabView::new();
    let tab_bar = adw::TabBar::builder().view(&tab_view).build();

    let empty_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .halign(gtk::Align::Center)
        .build();
    let empty_new_btn = gtk::Button::builder()
        .label(tr("New connection"))
        .css_classes(["pill"])
        .build();
    let empty_quick_btn = gtk::Button::builder()
        .label(tr("Quick connection"))
        .css_classes(["pill", "suggested-action"])
        .build();
    empty_box.append(&empty_new_btn);
    empty_box.append(&empty_quick_btn);
    let status_page = adw::StatusPage::builder()
        .title(tr("No open sessions"))
        .description(tr("Select a host in the sidebar or use quick connection"))
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
        move || content_stack.set_visible_child_name(if tab_view.n_pages() == 0 {
            "empty"
        } else {
            "tabs"
        })
    );
    let update_connected = clone!(
        #[weak]
        tab_view,
        #[strong]
        host_list,
        #[strong]
        state,
        move || {
            let connected = connected_aliases(&tab_view);
            launcher_badge::report(Rc::as_ptr(&state) as usize, connected.len() as u32);
            host_list.set_connected(&connected);
        }
    );
    tab_view.connect_close_page(|_, _| glib::Propagation::Proceed);
    // `tab_view.pages()`'s `items-changed` never fires in this gtk4-rs/libadwaita combo; the
    // `n-pages` property notify is the reliable signal for "a page was added or removed" (e.g. a
    // closed tab dropping out of the "connected" set).
    tab_view.connect_n_pages_notify(clone!(
        #[strong]
        update_stack,
        #[strong]
        update_connected,
        move |_| {
            update_stack();
            update_connected();
        }
    ));

    let content_toolbar = adw::ToolbarView::new();
    let content_header = adw::HeaderBar::new();
    content_header.pack_end(&menu_btn);
    content_toolbar.add_top_bar(&content_header);
    content_toolbar.add_top_bar(&tab_bar);
    content_toolbar.set_content(Some(&content_stack));
    let content_page = adw::NavigationPage::new(&content_toolbar, &tr("Sessions"));
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
        let update_connected = update_connected.clone();
        move |argv: Vec<String>, title: String, color: Option<String>, alias: Option<String>| {
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
                clone!(
                    #[strong]
                    update_connected,
                    move |_running| update_connected()
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
            unsafe { tab.widget().set_data(ALIAS_DATA_KEY, alias) };
            *page_cell.borrow_mut() = Some(page.clone());
            tab_view.set_selected_page(&page);
            tab.terminal().grab_focus();
        }
    };

    let open_host = {
        let state = state.clone();
        let spawn_tab = spawn_tab.clone();
        let tab_view = tab_view.clone();
        move |host: SshHost| {
            // Already has a tab (connecting, connected, or ended) for this host — just bring it
            // forward instead of spawning a second `ssh` process for the same alias.
            if let Some(page) = find_tab_by_alias(&tab_view, &host.alias) {
                tab_view.set_selected_page(&page);
                return;
            }
            let argv = build_ssh_command(&ConnectTarget::Alias(host.alias.clone()));
            let color = state
                .borrow()
                .metadata
                .get(&host.alias)
                .and_then(|m| m.color.clone());
            spawn_tab(argv, host.alias.clone(), color, Some(host.alias.clone()));
        }
    };

    // --- host list actions -------------------------------------------------------------------
    let on_action = {
        let state = state.clone();
        let host_list = host_list.clone();
        let tab_view = tab_view.clone();
        let window = window.clone();
        let open_host = open_host.clone();
        move |action: HostAction| match action {
            HostAction::Connect(host) => open_host(host),
            HostAction::Disconnect(host) => {
                // Menu-driven disconnect: go through the same close-page path a manual tab close
                // would, so the "sessão ativa" confirmation and process teardown stay identical.
                if let Some(page) = find_tab_by_alias(&tab_view, &host.alias) {
                    tab_view.close_page(&page);
                }
            }
            HostAction::Edit(host, meta) => {
                let state = state.clone();
                let host_list = host_list.clone();
                let tab_view = tab_view.clone();
                let previous_alias = host.alias.clone();
                let groups = state.borrow().metadata.groups();
                host_dialog::edit(&window, Some(host), meta, &groups, move |result| {
                    let Some((new_host, new_meta)) = result else {
                        return;
                    };
                    let mut s = state.borrow_mut();
                    if new_host.alias == previous_alias {
                        s.cfg.upsert_host(new_host.clone());
                    } else {
                        s.cfg
                            .upsert_host_renaming(&previous_alias, new_host.clone());
                    }
                    s.metadata.set(new_host.alias.clone(), new_meta);
                    s.persist();
                    drop(s);
                    refresh(&state, &host_list, &tab_view);
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
                refresh(&state, &host_list, &tab_view);
            }
            HostAction::OpenFiles(host) => {
                gtk::UriLauncher::new(&sftp_uri(&host)).launch(
                    Some(&window),
                    None::<&gio::Cancellable>,
                    |_| {},
                );
            }
            HostAction::Delete(host) => {
                let state = state.clone();
                let host_list = host_list.clone();
                let tab_view = tab_view.clone();
                let alias = host.alias.clone();
                confirm(
                    &window,
                    &tr("Delete host?"),
                    &tr_format("Delete “{alias}” from ~/.ssh/config? A backup is always kept in config.sulafat.bak.", &[("alias", &alias)]),
                    move || {
                        let mut s = state.borrow_mut();
                        s.cfg.remove_host(&alias);
                        s.metadata.set(alias, Default::default());
                        s.persist();
                        drop(s);
                        refresh(&state, &host_list, &tab_view);
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
        #[strong]
        tab_view,
        move |_| {
            let groups = state.borrow().metadata.groups();
            let state = state.clone();
            let host_list = host_list.clone();
            let tab_view = tab_view.clone();
            host_dialog::edit(&window, None, Default::default(), &groups, move |result| {
                let Some((new_host, new_meta)) = result else {
                    return;
                };
                let mut s = state.borrow_mut();
                s.cfg.upsert_host(new_host.clone());
                s.metadata.set(new_host.alias, new_meta);
                s.persist();
                drop(s);
                refresh(&state, &host_list, &tab_view);
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
            spawn_tab(build_ssh_command(&target), title, None, None);
        }
    };
    quick_btn.set_popover(Some(&quick_connect::popover(open_quick)));
    empty_quick_btn.connect_clicked(clone!(
        #[weak]
        quick_btn,
        move |_| quick_btn.popup()
    ));

    // --- window actions (settings / about) ----------------------------------------------------
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

    let about_action = gio::SimpleAction::new("about", None);
    about_action.connect_activate(clone!(
        #[weak]
        window,
        move |_, _| {
            let dialog = adw::AboutDialog::builder()
                .application_name("Sulafat")
                .application_icon("org.lyraos.Sulafat")
                .developer_name("Lyra OS")
                .version(env!("CARGO_PKG_VERSION"))
                .website("https://github.com/britors/Sulafat")
                .issue_url("https://github.com/britors/Sulafat/issues")
                .license_type(gtk::License::Gpl30)
                .build();
            dialog.set_developers(&["Rodrigo Brito"]);
            dialog.present(Some(&window));
        }
    ));
    window.add_action(&about_action);

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
        glib::timeout_add_local(
            sulafat_core::watch::DEBOUNCE,
            clone!(
                #[strong]
                state,
                #[strong]
                host_list,
                #[strong]
                tab_view,
                move || {
                    if watcher.poll() {
                        let path = state.borrow().cfg.path().to_path_buf();
                        if let Ok(fresh) = SshConfig::load_from(path) {
                            state.borrow_mut().cfg = fresh;
                            refresh(&state, &host_list, &tab_view);
                        }
                    }
                    glib::ControlFlow::Continue
                }
            ),
        );
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
                &tr("Close window?"),
                &tr(
                    "There are active SSH sessions in this window. Do you really want to close it?",
                ),
                clone!(
                    #[weak]
                    window,
                    move || window.destroy()
                ),
            );
            glib::Propagation::Stop
        }
    ));

    window.connect_destroy(clone!(
        #[strong]
        state,
        move |_| launcher_badge::forget(Rc::as_ptr(&state) as usize)
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
            confirm(
                &window,
                &tr("End session?"),
                &tr("This tab has an active SSH session. Do you really want to close it?"),
                move || {
                    tab_view.close_page_finish(&page, true);
                },
            );
            glib::Propagation::Stop
        }
    ));

    refresh(&state, &host_list, &tab_view);

    if let Some(host) = initial_host {
        open_host(host);
    }

    window.present();
}

fn is_running(page: &adw::TabPage) -> bool {
    let child = page.child();
    unsafe {
        child
            .data::<Box<dyn Fn() -> bool>>(RUNNING_DATA_KEY)
            .map(|p| p.as_ref()())
            .unwrap_or(false)
    }
}
