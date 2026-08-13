//! Badges the app's dock/taskbar icon with the number of active SSH sessions across every
//! open window, via the `com.canonical.Unity.LauncherEntry` D-Bus signal honored by
//! dash-to-dock and similar docks. Docks that don't understand the signal simply never see
//! it — this is a bare broadcast on the session bus, no reply expected.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::gio;
use gtk::glib;
use gtk::glib::prelude::*;

const APP_URI: &str = "application://org.lyraos.Sulafat.desktop";
const OBJECT_PATH: &str = "/org/lyraos/Sulafat";

thread_local! {
    static CONNECTION: RefCell<Option<gio::DBusConnection>> = const { RefCell::new(None) };
    // Per-window active-session counts, summed into the single app-wide badge.
    static COUNTS: RefCell<HashMap<usize, u32>> = RefCell::new(HashMap::new());
}

/// Reports how many sessions are active in the window identified by `window_id` (a stable
/// per-window key, e.g. `Rc::as_ptr` of that window's state). Call [`forget`] once the window
/// closes so its sessions stop counting toward the badge.
pub fn report(window_id: usize, count: u32) {
    let total = COUNTS.with(|counts| {
        let mut counts = counts.borrow_mut();
        counts.insert(window_id, count);
        counts.values().sum::<u32>()
    });
    emit(total);
}

/// Drops a closed window's contribution to the badge count.
pub fn forget(window_id: usize) {
    let total = COUNTS.with(|counts| {
        let mut counts = counts.borrow_mut();
        counts.remove(&window_id);
        counts.values().sum::<u32>()
    });
    emit(total);
}

fn emit(count: u32) {
    CONNECTION.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>).ok();
        }
        let Some(connection) = slot.as_ref() else {
            return;
        };

        let properties = glib::VariantDict::new(None);
        properties.insert("count", count as i64);
        properties.insert("count-visible", count > 0);
        let params = (APP_URI, properties.end()).to_variant();

        let _ = connection.emit_signal(
            None,
            OBJECT_PATH,
            "com.canonical.Unity.LauncherEntry",
            "Update",
            Some(&params),
        );
    });
}
