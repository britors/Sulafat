//! Sidebar host list: search, per-group headers and a color dot, backed by a plain `gtk::ListBox`
//! (rebuilt on every refresh — host counts here are small enough that this is simpler, and just
//! as responsive, as a `ListView`/`ListStore`/factory pipeline).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use gtk::glib::clone;
use sulafat_core::metadata::{HostMeta, Metadata};
use sulafat_core::ssh_config::SshHost;

const ROW_DATA_KEY: &str = "sulafat-host-meta";

/// Every context-menu / activation outcome the sidebar can produce; `window_main` is the one
/// that actually acts on these (talking to `SshConfig`/`Metadata`, opening tabs and dialogs).
pub enum HostAction {
    Connect(SshHost),
    ConnectNewWindow(SshHost),
    Edit(SshHost, HostMeta),
    Duplicate(SshHost, HostMeta),
    OpenFiles(SshHost),
    Delete(SshHost),
}

type ActionHandler = Rc<RefCell<Option<Rc<dyn Fn(HostAction)>>>>;

pub struct HostList {
    root: gtk::Box,
    list_box: gtk::ListBox,
    search_entry: gtk::SearchEntry,
    on_action: ActionHandler,
}

fn parse_hex_color(color: Option<&str>) -> (f64, f64, f64) {
    const DEFAULT: (f64, f64, f64) = (0.6, 0.6, 0.6);
    let Some(color) = color else { return DEFAULT };
    let hex = color.trim_start_matches('#');
    if hex.len() != 6 {
        return DEFAULT;
    }
    let byte = |range: std::ops::Range<usize>| u8::from_str_radix(&hex[range], 16).ok();
    match (byte(0..2), byte(2..4), byte(4..6)) {
        (Some(r), Some(g), Some(b)) => (f64::from(r) / 255.0, f64::from(g) / 255.0, f64::from(b) / 255.0),
        _ => DEFAULT,
    }
}

fn color_dot(color: Option<&str>) -> gtk::DrawingArea {
    let (r, g, b) = parse_hex_color(color);
    let area = gtk::DrawingArea::builder().content_width(12).content_height(12).valign(gtk::Align::Center).build();
    area.set_draw_func(move |_, cr, w, h| {
        cr.set_source_rgb(r, g, b);
        cr.arc(f64::from(w) / 2.0, f64::from(h) / 2.0, f64::from(w.min(h)) / 2.0, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
    });
    area
}

fn row_data(row: &adw::ActionRow) -> (SshHost, HostMeta) {
    // Safe by construction: every row this module creates has `ROW_DATA_KEY` set exactly once,
    // right after construction, to a `(SshHost, HostMeta)` — nothing else ever writes this key.
    unsafe { row.data::<(SshHost, HostMeta)>(ROW_DATA_KEY).map(|p| p.as_ref().clone()).expect("row created without host data") }
}

fn subtitle_for(host: &SshHost) -> String {
    let mut parts = Vec::new();
    if let Some(user) = &host.user {
        if let Some(host_name) = &host.host_name {
            parts.push(format!("{user}@{host_name}"));
        } else {
            parts.push(user.clone());
        }
    } else if let Some(host_name) = &host.host_name {
        parts.push(host_name.clone());
    }
    if let Some(port) = host.port {
        parts.push(format!(":{port}"));
    }
    if host.read_only {
        parts.push("somente leitura".to_string());
    }
    parts.join(" · ")
}

fn dispatch(on_action: &ActionHandler, action: HostAction) {
    if let Some(handler) = on_action.borrow().as_ref() {
        handler(action);
    }
}

fn build_row(host: &SshHost, meta: &HostMeta, on_action: ActionHandler) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&host.alias))
        .subtitle(glib::markup_escape_text(&subtitle_for(host)))
        .activatable(true)
        .build();
    unsafe { row.set_data(ROW_DATA_KEY, (host.clone(), meta.clone())) };

    row.add_prefix(&color_dot(meta.color.as_deref()));

    let menu_btn = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    row.add_suffix(&menu_btn);

    let menu = gio::Menu::new();
    menu.append(Some("Conectar"), Some("row.connect"));
    menu.append(Some("Conectar em nova janela"), Some("row.connect-new-window"));
    if !host.read_only {
        menu.append(Some("Editar"), Some("row.edit"));
        menu.append(Some("Duplicar"), Some("row.duplicate"));
    }
    menu.append(Some("Abrir arquivos"), Some("row.open-files"));
    if !host.read_only {
        menu.append(Some("Excluir"), Some("row.delete"));
    }
    menu_btn.set_popover(Some(&gtk::PopoverMenu::from_model(Some(&menu))));

    let actions = gio::SimpleActionGroup::new();
    macro_rules! bind_action {
        ($name:literal, $variant:expr) => {
            let action = gio::SimpleAction::new($name, None);
            action.connect_activate(clone!(
                #[weak]
                row,
                #[strong]
                on_action,
                move |_, _| {
                    let (host, meta) = row_data(&row);
                    dispatch(&on_action, ($variant)(host, meta));
                }
            ));
            actions.add_action(&action);
        };
    }
    bind_action!("connect", |host: SshHost, _meta: HostMeta| HostAction::Connect(host));
    bind_action!("connect-new-window", |host: SshHost, _meta: HostMeta| HostAction::ConnectNewWindow(host));
    bind_action!("edit", HostAction::Edit);
    bind_action!("duplicate", HostAction::Duplicate);
    bind_action!("open-files", |host: SshHost, _meta: HostMeta| HostAction::OpenFiles(host));
    bind_action!("delete", |host: SshHost, _meta: HostMeta| HostAction::Delete(host));
    row.insert_action_group("row", Some(&actions));

    row.connect_activated(clone!(
        #[strong]
        on_action,
        move |row| {
            let (host, _meta) = row_data(row);
            dispatch(&on_action, HostAction::Connect(host));
        }
    ));

    row
}

impl HostList {
    /// The action handler is set later, via [`Self::set_action_handler`] — the sidebar widget
    /// has to exist before the window that owns the real handler (dialogs, tab spawning) does.
    pub fn new() -> Self {
        let on_action: ActionHandler = Rc::new(RefCell::new(None));

        let search_entry = gtk::SearchEntry::builder()
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(6)
            .placeholder_text("Buscar hosts…")
            .build();

        let list_box = gtk::ListBox::builder().css_classes(["boxed-list"]).selection_mode(gtk::SelectionMode::None).build();
        list_box.set_filter_func(clone!(
            #[weak]
            search_entry,
            #[upgrade_or]
            true,
            move |row| {
                let query = search_entry.text().to_lowercase();
                if query.is_empty() {
                    return true;
                }
                let Some(row) = row.downcast_ref::<adw::ActionRow>() else { return true };
                let (host, _) = row_data(row);
                host.alias.to_lowercase().contains(&query)
                    || host.host_name.as_deref().unwrap_or_default().to_lowercase().contains(&query)
                    || host.user.as_deref().unwrap_or_default().to_lowercase().contains(&query)
            }
        ));
        search_entry.connect_search_changed(clone!(
            #[weak]
            list_box,
            move |_| list_box.invalidate_filter()
        ));

        list_box.set_header_func(|row, before| {
            let Some(row) = row.downcast_ref::<adw::ActionRow>() else { return };
            let (_, meta) = row_data(row);
            let group = meta.group.clone().unwrap_or_else(|| "Sem grupo".to_string());
            let prev_group = before
                .and_then(|b| b.downcast_ref::<adw::ActionRow>().map(row_data))
                .map(|(_, m)| m.group.unwrap_or_else(|| "Sem grupo".to_string()));
            if prev_group.as_deref() == Some(group.as_str()) {
                row.set_header(None::<&gtk::Widget>);
            } else {
                let label = gtk::Label::builder()
                    .label(&group)
                    .halign(gtk::Align::Start)
                    .css_classes(["heading", "dim-label"])
                    .margin_top(12)
                    .margin_start(6)
                    .margin_bottom(2)
                    .build();
                row.set_header(Some(&label));
            }
        });

        let scroller = gtk::ScrolledWindow::builder().child(&list_box).vexpand(true).build();

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&search_entry);
        root.append(&scroller);

        Self { root, list_box, search_entry, on_action }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn set_action_handler(&self, handler: impl Fn(HostAction) + 'static) {
        *self.on_action.borrow_mut() = Some(Rc::new(handler));
    }

    pub fn search_entry(&self) -> &gtk::SearchEntry {
        &self.search_entry
    }

    /// Replace every row with a fresh set built from `hosts`/`metadata`, sorted by group then
    /// alias so the header func's "new group starts here" comparison works by simple adjacency.
    pub fn set_hosts(&self, hosts: Vec<SshHost>, metadata: &Metadata) {
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        let mut entries: Vec<(SshHost, HostMeta)> =
            hosts.into_iter().map(|h| { let meta = metadata.get(&h.alias).cloned().unwrap_or_default(); (h, meta) }).collect();
        entries.sort_by(|(host_a, meta_a), (host_b, meta_b)| {
            let group_a = meta_a.group.clone().unwrap_or_default();
            let group_b = meta_b.group.clone().unwrap_or_default();
            group_a.cmp(&group_b).then_with(|| host_a.alias.to_lowercase().cmp(&host_b.alias.to_lowercase()))
        });

        for (host, meta) in &entries {
            self.list_box.append(&build_row(host, meta, self.on_action.clone()));
        }
        self.list_box.invalidate_headers();
        self.list_box.invalidate_filter();
    }
}
