//! Create/edit dialog for an [`SshHost`] plus its [`HostMeta`] (group/color/notes).
//!
//! Callback-based rather than `async` on purpose: `sulafat-gtk` has no async runtime (VTE is
//! already callback-driven through the GLib main loop), so there's no executor to `.await` on.

use crate::i18n::tr;
use adw::prelude::*;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::glib::clone;
use sulafat_core::metadata::HostMeta;
use sulafat_core::ssh_config::{validate_host, SshHost};

fn color_to_hex(rgba: &gdk::RGBA) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (rgba.red() * 255.0).round() as u8,
        (rgba.green() * 255.0).round() as u8,
        (rgba.blue() * 255.0).round() as u8,
    )
}

/// Show the host editor. `initial` is `None` when creating a new host. Calls `on_result` with
/// the edited `(SshHost, HostMeta)` on Save, or `None` if cancelled.
pub fn edit(
    parent: &impl IsA<gtk::Widget>,
    initial: Option<SshHost>,
    initial_meta: HostMeta,
    existing_groups: &[String],
    on_result: impl Fn((SshHost, HostMeta)) -> Result<(), String> + 'static,
) {
    let is_new = initial.is_none();
    let base = initial.unwrap_or_else(|| SshHost::new(""));

    let alias_row = adw::EntryRow::builder()
        .title(tr("Alias"))
        .text(&base.alias)
        .build();
    let host_name_row = adw::EntryRow::builder()
        .title("HostName")
        .text(base.host_name.clone().unwrap_or_default())
        .build();
    let user_row = adw::EntryRow::builder()
        .title(tr("User"))
        .text(base.user.clone().unwrap_or_default())
        .build();
    let port_row = adw::SpinRow::builder()
        .title(tr("Port"))
        .adjustment(&gtk::Adjustment::new(
            f64::from(base.port.unwrap_or(22)),
            1.0,
            65535.0,
            1.0,
            10.0,
            0.0,
        ))
        .build();
    let proxy_jump_row = adw::EntryRow::builder()
        .title("ProxyJump")
        .text(base.proxy_jump.clone().unwrap_or_default())
        .build();

    let identity_file_row = adw::EntryRow::builder()
        .title(tr("Identity file (private key)"))
        .text(base.identity_file.clone().unwrap_or_default())
        .build();
    let browse_btn = gtk::Button::builder()
        .icon_name("document-open-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    identity_file_row.add_suffix(&browse_btn);

    let mut group_options: Vec<String> = vec![tr("None")];
    group_options.extend(existing_groups.iter().cloned());
    group_options.push(tr("New group…"));
    let new_group_index = group_options.len() - 1;

    let group_row = adw::ComboRow::builder().title(tr("Group")).build();
    let group_model =
        gtk::StringList::new(&group_options.iter().map(String::as_str).collect::<Vec<_>>());
    group_row.set_model(Some(&group_model));
    let initial_group_index = initial_meta
        .group
        .as_ref()
        .and_then(|g| group_options.iter().position(|o| o == g))
        .unwrap_or(0);
    group_row.set_selected(initial_group_index as u32);

    let new_group_row = adw::EntryRow::builder()
        .title(tr("New group name"))
        .visible(initial_group_index == new_group_index)
        .build();
    group_row.connect_selected_notify(clone!(
        #[weak]
        new_group_row,
        move |row| new_group_row.set_visible(row.selected() as usize == new_group_index)
    ));

    let color_dialog_btn = gtk::ColorDialogButton::builder()
        .dialog(&gtk::ColorDialog::new())
        .valign(gtk::Align::Center)
        .build();
    if let Some(hex) = &initial_meta.color {
        if let Ok(rgba) = gdk::RGBA::parse(hex) {
            color_dialog_btn.set_rgba(&rgba);
        }
    }
    let color_row = adw::ActionRow::builder().title(tr("Color")).build();
    color_row.add_suffix(&color_dialog_btn);

    let notes_row = adw::EntryRow::builder()
        .title(tr("Notes"))
        .text(initial_meta.notes.clone().unwrap_or_default())
        .build();

    let advanced_view = gtk::TextView::builder()
        .monospace(true)
        .top_margin(6)
        .bottom_margin(6)
        .left_margin(6)
        .right_margin(6)
        .build();
    advanced_view.buffer().set_text(&base.extra);
    let advanced_scroller = gtk::ScrolledWindow::builder()
        .child(&advanced_view)
        .min_content_height(120)
        .build();
    let advanced_expander = adw::ExpanderRow::builder()
        .title(tr("Advanced options"))
        .subtitle(tr("Directives not mapped above, free-form text"))
        .build();
    advanced_expander.add_row(&advanced_scroller);

    let connection_group = adw::PreferencesGroup::builder()
        .title(tr("Connection"))
        .build();
    connection_group.add(&alias_row);
    connection_group.add(&host_name_row);
    connection_group.add(&user_row);
    connection_group.add(&port_row);
    connection_group.add(&proxy_jump_row);
    connection_group.add(&identity_file_row);

    let organization_group = adw::PreferencesGroup::builder()
        .title(tr("Organization"))
        .build();
    organization_group.add(&group_row);
    organization_group.add(&new_group_row);
    organization_group.add(&color_row);
    organization_group.add(&notes_row);

    let advanced_group = adw::PreferencesGroup::new();
    advanced_group.add(&advanced_expander);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&connection_group);
    content.append(&organization_group);
    content.append(&advanced_group);

    let scroller = gtk::ScrolledWindow::builder()
        .child(&content)
        .propagate_natural_height(true)
        .min_content_width(460)
        .build();

    let dialog = adw::Dialog::builder()
        .title(if is_new {
            tr("New connection")
        } else {
            tr("Edit connection")
        })
        .content_width(500)
        .content_height(620)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let save_btn = gtk::Button::builder()
        .label(tr("Save"))
        .css_classes(["suggested-action"])
        .build();
    let cancel_btn = gtk::Button::builder().label(tr("Cancel")).build();
    header.pack_start(&cancel_btn);
    header.pack_end(&save_btn);
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&scroller));
    dialog.set_child(Some(&toolbar_view));

    save_btn.set_sensitive(!alias_row.text().is_empty());
    alias_row.connect_changed(clone!(
        #[weak]
        save_btn,
        move |row| save_btn.set_sensitive(!row.text().is_empty())
    ));

    browse_btn.connect_clicked(clone!(
        #[weak]
        identity_file_row,
        #[weak]
        dialog,
        move |_| {
            let file_dialog = gtk::FileDialog::builder()
                .title(tr("Select private key"))
                .build();
            let root = dialog.root().and_downcast::<gtk::Window>();
            file_dialog.open(
                root.as_ref(),
                gio::Cancellable::NONE,
                clone!(
                    #[weak]
                    identity_file_row,
                    move |result| {
                        if let Ok(file) = result {
                            if let Some(path) = file.path() {
                                identity_file_row.set_text(&path.to_string_lossy());
                            }
                        }
                    }
                ),
            );
        }
    ));

    cancel_btn.connect_clicked(clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));

    let on_result = std::rc::Rc::new(on_result);

    save_btn.connect_clicked(clone!(
        #[weak]
        dialog,
        #[strong]
        on_result,
        #[strong]
        base,
        move |_| {
            let mut host = base.clone();
            host.alias = alias_row.text().to_string();
            host.host_name = non_empty(host_name_row.text());
            host.user = non_empty(user_row.text());
            host.port = {
                let p = port_row.value() as u16;
                (p != 22).then_some(p)
            };
            host.proxy_jump = non_empty(proxy_jump_row.text());
            host.identity_file = non_empty(identity_file_row.text());
            let buffer = advanced_view.buffer();
            host.extra = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), true)
                .to_string();

            let selected = group_row.selected() as usize;
            let group = if selected == 0 {
                None
            } else if selected == new_group_index {
                non_empty(new_group_row.text())
            } else {
                group_options.get(selected).cloned()
            };
            let meta = HostMeta {
                group,
                color: Some(color_to_hex(&color_dialog_btn.rgba())),
                notes: non_empty(notes_row.text()),
            };

            if let Err(error) = validate_host(&host)
                .map_err(|e| e.to_string())
                .and_then(|_| on_result((host, meta)))
            {
                let alert = adw::AlertDialog::new(Some(&tr("Could not save")), Some(&error));
                alert.add_response("ok", &tr("OK"));
                alert.present(Some(&dialog));
            } else {
                dialog.close();
            }
        }
    ));

    dialog.present(Some(parent));
}

fn non_empty(text: glib::GString) -> Option<String> {
    let text = text.to_string();
    (!text.is_empty()).then_some(text)
}
