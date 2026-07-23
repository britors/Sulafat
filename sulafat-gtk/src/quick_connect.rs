//! "Conexão rápida": connect to `[usuário@]host[:porta]` without creating a saved host.

use adw::prelude::*;
use gtk::glib;
use gtk::glib::clone;
use sulafat_core::command::parse_quick_target;

/// A popover with a single entry; pressing Enter or the "Conectar" button calls `on_connect`
/// with the parsed target and closes the popover. Invalid/empty input just disables the button.
pub fn popover(on_connect: impl Fn(sulafat_core::command::ConnectTarget) + 'static) -> gtk::Popover {
    let entry = gtk::Entry::builder().placeholder_text("usuário@host[:porta]").width_chars(28).build();
    let connect_btn = gtk::Button::builder().label("Conectar").css_classes(["suggested-action"]).sensitive(false).build();

    let content = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(6).margin_top(6).margin_bottom(6).margin_start(6).margin_end(6).build();
    content.append(&entry);
    content.append(&connect_btn);

    let popover = gtk::Popover::builder().child(&content).build();

    entry.connect_changed(clone!(
        #[weak]
        connect_btn,
        move |entry| connect_btn.set_sensitive(parse_quick_target(&entry.text()).is_some())
    ));

    let on_connect = std::rc::Rc::new(on_connect);
    let try_connect = clone!(
        #[weak]
        entry,
        #[weak]
        popover,
        #[strong]
        on_connect,
        move || {
            if let Some(target) = parse_quick_target(&entry.text()) {
                popover.popdown();
                entry.set_text("");
                on_connect(target);
            }
        }
    );
    entry.connect_activate(clone!(
        #[strong]
        try_connect,
        move |_| try_connect()
    ));
    connect_btn.connect_clicked(move |_| try_connect());

    popover.connect_show(clone!(
        #[weak]
        entry,
        move |_| {
            entry.grab_focus();
        }
    ));

    popover
}
