//! Sidebar host list: search, per-group headers and a color dot, backed by a plain `gtk::ListBox`
//! (rebuilt on every refresh — host counts here are small enough that this is simpler, and just
//! as responsive, as a `ListView`/`ListStore`/factory pipeline).

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use crate::i18n::tr;
use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use gtk::glib::clone;
use sulafat_core::metadata::{HostMeta, Metadata};
use sulafat_core::ssh_config::SshHost;

const ROW_DATA_KEY: &str = "sulafat-host-meta";
const STATUS_STATE_KEY: &str = "sulafat-status-state";
const STATUS_DOT_KEY: &str = "sulafat-status-dot";
const CONNECT_MENU_KEY: &str = "sulafat-connect-menu";
const CONNECT_MENU_INDEX: i32 = 0;

const STATUS_CONNECTED: (f64, f64, f64) = (0.18, 0.71, 0.30);
const STATUS_DISCONNECTED: (f64, f64, f64) = (0.85, 0.23, 0.23);

/// Every context-menu / activation outcome the sidebar can produce; `window_main` is the one
/// that actually acts on these (talking to `SshConfig`/`Metadata`, opening tabs and dialogs).
pub enum HostAction {
    Connect(SshHost),
    Disconnect(SshHost),
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
        (Some(r), Some(g), Some(b)) => (
            f64::from(r) / 255.0,
            f64::from(g) / 255.0,
            f64::from(b) / 255.0,
        ),
        _ => DEFAULT,
    }
}

/// The circle next to the host name — a connection "farol": green while a session for this host
/// is running, red otherwise. `connected` is shared with [`HostList::set_connected`], which
/// flips it and asks the widget to redraw.
fn status_dot(connected: Rc<Cell<bool>>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(12)
        .content_height(12)
        .valign(gtk::Align::Center)
        .tooltip_text(tr("Connection status"))
        .build();
    let accessible_label = tr("Connection status");
    area.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
    area.set_draw_func(move |_, cr, w, h| {
        let (r, g, b) = if connected.get() {
            STATUS_CONNECTED
        } else {
            STATUS_DISCONNECTED
        };
        cr.set_source_rgb(r, g, b);
        cr.arc(
            f64::from(w) / 2.0,
            f64::from(h) / 2.0,
            f64::from(w.min(h)) / 2.0,
            0.0,
            std::f64::consts::TAU,
        );
        let _ = cr.fill();
    });
    area
}

/// The user's chosen host color — shown as a small square (rather than the farol's circle) so
/// the two indicators read as clearly distinct at a glance.
fn color_square(color: Option<&str>) -> gtk::DrawingArea {
    let (r, g, b) = parse_hex_color(color);
    let area = gtk::DrawingArea::builder()
        .content_width(10)
        .content_height(10)
        .valign(gtk::Align::Center)
        .tooltip_text(tr("Host color"))
        .build();
    let accessible_label = tr("Host color");
    area.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
    area.set_draw_func(move |_, cr, w, h| {
        cr.set_source_rgb(r, g, b);
        cr.rectangle(0.0, 0.0, f64::from(w), f64::from(h));
        let _ = cr.fill();
    });
    area
}

fn row_data(row: &adw::ActionRow) -> (SshHost, HostMeta) {
    // Safe by construction: every row this module creates has `ROW_DATA_KEY` set exactly once,
    // right after construction, to a `(SshHost, HostMeta)` — nothing else ever writes this key.
    unsafe {
        row.data::<(SshHost, HostMeta)>(ROW_DATA_KEY)
            .map(|p| p.as_ref().clone())
            .expect("row created without host data")
    }
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
        parts.push(tr("read-only"));
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

    let connected_state = Rc::new(Cell::new(false));
    let dot = status_dot(connected_state.clone());
    row.add_prefix(&dot);
    unsafe { row.set_data(STATUS_STATE_KEY, connected_state.clone()) };
    unsafe { row.set_data(STATUS_DOT_KEY, dot) };

    let menu_btn = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .tooltip_text(tr("Host actions"))
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let accessible_label = tr("Host actions");
    menu_btn.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);
    row.add_suffix(&color_square(meta.color.as_deref()));
    row.add_suffix(&menu_btn);

    let menu = gio::Menu::new();
    menu.append(Some(&tr("Connect")), Some("row.connect"));
    if !host.read_only {
        menu.append(Some(&tr("Edit")), Some("row.edit"));
        menu.append(Some(&tr("Duplicate")), Some("row.duplicate"));
    }
    menu.append(Some(&tr("Open files")), Some("row.open-files"));
    if !host.read_only {
        menu.append(Some(&tr("Delete")), Some("row.delete"));
    }
    menu_btn.set_popover(Some(&gtk::PopoverMenu::from_model(Some(&menu))));
    unsafe { row.set_data(CONNECT_MENU_KEY, menu) };

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
    // Single action for the menu's first entry: it dispatches `Connect` or `Disconnect`
    // depending on the row's current farol state, and the label is kept in sync in
    // `HostList::set_connected` (see `CONNECT_MENU_KEY`/`CONNECT_MENU_INDEX`).
    let connect_action = gio::SimpleAction::new("connect", None);
    connect_action.connect_activate(clone!(
        #[weak]
        row,
        #[strong]
        on_action,
        #[strong]
        connected_state,
        move |_, _| {
            let (host, _meta) = row_data(&row);
            let action = if connected_state.get() {
                HostAction::Disconnect(host)
            } else {
                HostAction::Connect(host)
            };
            dispatch(&on_action, action);
        }
    ));
    actions.add_action(&connect_action);
    bind_action!("edit", HostAction::Edit);
    bind_action!("duplicate", HostAction::Duplicate);
    bind_action!("open-files", |host: SshHost, _meta: HostMeta| {
        HostAction::OpenFiles(host)
    });
    bind_action!("delete", |host: SshHost, _meta: HostMeta| {
        HostAction::Delete(host)
    });
    row.insert_action_group("row", Some(&actions));

    row.connect_activated(clone!(
        #[strong]
        on_action,
        move |row| {
            // A plain click always connects (or, if already connected, brings the existing tab
            // forward — see `window_main::open_host`); disconnecting is a deliberate menu action.
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
            .placeholder_text(tr("Search hosts…"))
            .build();
        let accessible_label = tr("Search hosts");
        search_entry.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);

        let list_box = gtk::ListBox::builder()
            .css_classes(["boxed-list"])
            .selection_mode(gtk::SelectionMode::None)
            .build();
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
                let Some(row) = row.downcast_ref::<adw::ActionRow>() else {
                    return true;
                };
                let (host, _) = row_data(row);
                host.alias.to_lowercase().contains(&query)
                    || host
                        .host_name
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&query)
                    || host
                        .user
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&query)
            }
        ));
        search_entry.connect_search_changed(clone!(
            #[weak]
            list_box,
            move |_| list_box.invalidate_filter()
        ));

        list_box.set_header_func(|row, before| {
            let Some(row) = row.downcast_ref::<adw::ActionRow>() else {
                return;
            };
            let (_, meta) = row_data(row);
            let group = meta.group.clone().unwrap_or_else(|| tr("No group"));
            let prev_group = before
                .and_then(|b| b.downcast_ref::<adw::ActionRow>().map(row_data))
                .map(|(_, m)| m.group.unwrap_or_else(|| tr("No group")));
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

        let scroller = gtk::ScrolledWindow::builder()
            .child(&list_box)
            .vexpand(true)
            .build();

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&search_entry);
        root.append(&scroller);

        Self {
            root,
            list_box,
            search_entry,
            on_action,
        }
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

        let mut entries: Vec<(SshHost, HostMeta)> = hosts
            .into_iter()
            .map(|h| {
                let meta = metadata.get(&h.alias).cloned().unwrap_or_default();
                (h, meta)
            })
            .collect();
        entries.sort_by(|(host_a, meta_a), (host_b, meta_b)| {
            let group_a = meta_a.group.clone().unwrap_or_default();
            let group_b = meta_b.group.clone().unwrap_or_default();
            group_a.cmp(&group_b).then_with(|| {
                host_a
                    .alias
                    .to_lowercase()
                    .cmp(&host_b.alias.to_lowercase())
            })
        });

        for (host, meta) in &entries {
            self.list_box
                .append(&build_row(host, meta, self.on_action.clone()));
        }
        self.list_box.invalidate_headers();
        self.list_box.invalidate_filter();
    }

    /// Updates the farol (green/red) on each row and its menu's "Conectar"/"Desconectar" entry
    /// depending on whether the row's alias is in `aliases`. Cheap enough to call on every tab
    /// open/close/exit — it never rebuilds rows.
    pub fn set_connected(&self, aliases: &HashSet<String>) {
        let mut child = self.list_box.first_child();
        while let Some(row) = child {
            child = row.next_sibling();
            let Some(action_row) = row.downcast_ref::<adw::ActionRow>() else {
                continue;
            };
            let (host, _) = row_data(action_row);
            let connected = aliases.contains(&host.alias);

            if let Some(state) = unsafe { action_row.data::<Rc<Cell<bool>>>(STATUS_STATE_KEY) } {
                let state = unsafe { state.as_ref() };
                if state.get() == connected {
                    continue;
                }
                state.set(connected);
            }
            if let Some(dot) = unsafe { action_row.data::<gtk::DrawingArea>(STATUS_DOT_KEY) } {
                unsafe { dot.as_ref() }.queue_draw();
            }
            if let Some(menu) = unsafe { action_row.data::<gio::Menu>(CONNECT_MENU_KEY) } {
                let menu = unsafe { menu.as_ref() };
                menu.remove(CONNECT_MENU_INDEX);
                let label = if connected {
                    tr("Disconnect")
                } else {
                    tr("Connect")
                };
                menu.insert(CONNECT_MENU_INDEX, Some(&label), Some("row.connect"));
            }
        }
    }
}
