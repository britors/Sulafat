//! Terminal preferences (font, scrollback): a small settings file of our own — `sulafat-core`
//! only knows about `~/.ssh/config` and UI metadata, not terminal rendering choices — reusing
//! its XDG config-dir lookup so both files live side by side.

use std::fs;

use adw::prelude::*;
use gtk::glib;
use gtk::glib::clone;
use serde::{Deserialize, Serialize};

fn default_scrollback() -> u32 {
    10_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub font: Option<String>,
    #[serde(default = "default_scrollback")]
    pub scrollback_lines: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self { font: None, scrollback_lines: default_scrollback() }
    }
}

fn settings_path() -> Option<std::path::PathBuf> {
    sulafat_core::metadata::config_dir().ok().map(|dir| dir.join("settings.toml"))
}

impl Settings {
    pub fn load() -> Self {
        let Some(path) = settings_path() else { return Self::default() };
        let Ok(contents) = fs::read_to_string(path) else { return Self::default() };
        toml::from_str(&contents).unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = settings_path() else { return };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(contents) = toml::to_string_pretty(self) {
            let _ = fs::write(path, contents);
        }
    }
}

/// Show the preferences window. Changes are saved immediately and forwarded via `on_change` so
/// open terminals can be updated live.
pub fn show(parent: &impl IsA<gtk::Widget>, current: Settings, on_change: impl Fn(Settings) + 'static) {
    let dialog = adw::PreferencesDialog::builder().title("Configurações").build();
    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::builder().title("Terminal").build();

    let font_row = adw::ActionRow::builder().title("Fonte").build();
    let font_btn = gtk::FontDialogButton::builder().dialog(&gtk::FontDialog::new()).valign(gtk::Align::Center).build();
    if let Some(font) = &current.font {
        font_btn.set_font_desc(&gtk::pango::FontDescription::from_string(font));
    }
    font_row.add_suffix(&font_btn);

    let scrollback_row = adw::SpinRow::builder()
        .title("Scrollback (linhas)")
        .adjustment(&gtk::Adjustment::new(f64::from(current.scrollback_lines), 0.0, 1_000_000.0, 100.0, 1000.0, 0.0))
        .build();

    group.add(&font_row);
    group.add(&scrollback_row);
    page.add(&group);
    dialog.add(&page);

    let on_change = std::rc::Rc::new(on_change);
    let emit = clone!(
        #[weak]
        font_btn,
        #[weak]
        scrollback_row,
        #[strong]
        on_change,
        move || {
            let settings = Settings {
                font: Some(font_btn.font_desc().map(|d| d.to_string()).unwrap_or_default()).filter(|s| !s.is_empty()),
                scrollback_lines: scrollback_row.value() as u32,
            };
            settings.save();
            on_change(settings);
        }
    );
    font_btn.connect_font_desc_notify(clone!(
        #[strong]
        emit,
        move |_| emit()
    ));
    scrollback_row.connect_changed(move |_| emit());

    dialog.present(Some(parent));
}
