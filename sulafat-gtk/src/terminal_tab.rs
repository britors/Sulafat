//! One session tab: a VTE terminal running `ssh`, with a "session ended" overlay and a
//! generated color-dot icon for the tab strip.
//!
//! `ssh` itself handles every interactive prompt (password, host-key confirmation) directly in
//! the terminal — this module never intercepts or automates them.

use std::cell::Cell;
use std::rc::Rc;

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::glib::clone;
use vte4::prelude::*;

use crate::prefs::Settings;

/// A conservative "looks like a URL" pattern — good enough for clickable links in shell output,
/// not a full URI grammar.
const URL_PATTERN: &str = r#"(https?|ftp)://[^\s<>"']+"#;

pub struct TerminalTab {
    overlay: gtk::Overlay,
    terminal: vte4::Terminal,
    running: Rc<Cell<bool>>,
}

fn parse_hex_color_bytes(color: Option<&str>) -> (u8, u8, u8) {
    const DEFAULT: (u8, u8, u8) = (140, 140, 140);
    let Some(color) = color else { return DEFAULT };
    let hex = color.trim_start_matches('#');
    if hex.len() != 6 {
        return DEFAULT;
    }
    let byte = |range: std::ops::Range<usize>| u8::from_str_radix(&hex[range], 16).ok();
    match (byte(0..2), byte(2..4), byte(4..6)) {
        (Some(r), Some(g), Some(b)) => (r, g, b),
        _ => DEFAULT,
    }
}

/// A small solid-color dot texture for [`adw::TabPage::set_icon`], built from raw pixels — no
/// external image asset or rendering dependency needed.
pub fn color_dot_texture(color: Option<&str>) -> gdk::MemoryTexture {
    let (r, g, b) = parse_hex_color_bytes(color);
    const SIZE: i32 = 16;
    let radius = f64::from(SIZE) / 2.0;
    let mut data = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = f64::from(x) + 0.5 - radius;
            let dy = f64::from(y) + 0.5 - radius;
            let idx = ((y * SIZE + x) * 4) as usize;
            if (dx * dx + dy * dy).sqrt() <= radius - 0.5 {
                data[idx] = r;
                data[idx + 1] = g;
                data[idx + 2] = b;
                data[idx + 3] = 255;
            }
        }
    }
    let bytes = glib::Bytes::from_owned(data);
    gdk::MemoryTexture::new(SIZE, SIZE, gdk::MemoryFormat::R8g8b8a8Premultiplied, &bytes, (SIZE * 4) as usize)
}

impl TerminalTab {
    /// Builds the tab and spawns `argv` immediately. `on_close_requested` is called when the
    /// user presses "Fechar" on the session-ended overlay (closing the tab itself is
    /// `window_main`'s job, via `AdwTabView`). `on_running_changed` fires whenever the child
    /// process starts or exits, so callers can keep other UI (e.g. a sidebar "connected"
    /// indicator) in sync without polling.
    pub fn new(
        argv: Vec<String>,
        settings: &Settings,
        on_close_requested: impl Fn() + 'static,
        on_running_changed: impl Fn(bool) + 'static,
    ) -> Self {
        let terminal = vte4::Terminal::new();
        terminal.set_scrollback_lines(i64::from(settings.scrollback_lines));
        if let Some(font) = &settings.font {
            terminal.set_font(Some(&gtk::pango::FontDescription::from_string(font)));
        }

        let url_regex = vte4::Regex::for_match(URL_PATTERN, 0).ok();
        if let Some(regex) = &url_regex {
            let tag = terminal.match_add_regex(regex, 0);
            terminal.match_set_cursor_name(tag, "pointer");
        }

        let status_label = gtk::Label::builder().wrap(true).justify(gtk::Justification::Center).css_classes(["title-3"]).build();
        let reconnect_btn = gtk::Button::builder().label("Reconectar").css_classes(["suggested-action", "pill"]).build();
        let close_btn = gtk::Button::builder().label("Fechar").css_classes(["pill"]).build();
        let button_box = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(6).halign(gtk::Align::Center).build();
        button_box.append(&reconnect_btn);
        button_box.append(&close_btn);
        let status_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .css_classes(["osd", "card"])
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .visible(false)
            .build();
        status_box.append(&status_label);
        status_box.append(&button_box);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&terminal));
        overlay.add_overlay(&status_box);

        let argv = Rc::new(argv);
        let running = Rc::new(Cell::new(false));
        let on_running_changed: Rc<dyn Fn(bool)> = Rc::new(on_running_changed);

        let spawn_now = clone!(
            #[strong]
            terminal,
            #[strong]
            argv,
            #[strong]
            running,
            #[strong]
            status_box,
            #[strong]
            on_running_changed,
            move || {
                status_box.set_visible(false);
                let cwd = glib::home_dir();
                let cwd_str = cwd.to_string_lossy().into_owned();
                let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
                let envv: Vec<String> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
                let envv_refs: Vec<&str> = envv.iter().map(String::as_str).collect();
                let running_for_cb = running.clone();
                let on_running_changed = on_running_changed.clone();
                terminal.spawn_async(
                    vte4::PtyFlags::DEFAULT,
                    Some(cwd_str.as_str()),
                    &argv_refs,
                    &envv_refs,
                    glib::SpawnFlags::SEARCH_PATH,
                    || {},
                    -1,
                    None::<&gio::Cancellable>,
                    move |result| {
                        if result.is_ok() {
                            running_for_cb.set(true);
                            on_running_changed(true);
                        }
                    },
                );
            }
        );

        spawn_now();

        terminal.connect_child_exited(clone!(
            #[strong]
            running,
            #[strong]
            on_running_changed,
            #[weak]
            status_box,
            #[weak]
            status_label,
            move |_terminal, status| {
                running.set(false);
                on_running_changed(false);
                if status == 0 {
                    status_label.set_label("Sessão encerrada");
                } else {
                    status_label.set_label(&format!("Sessão encerrada (código {status})"));
                }
                status_box.set_visible(true);
            }
        ));

        reconnect_btn.connect_clicked(clone!(
            #[strong]
            spawn_now,
            move |_| spawn_now()
        ));
        close_btn.connect_clicked(move |_| on_close_requested());

        install_copy_paste_shortcuts(&terminal);
        install_url_click(&terminal, url_regex);

        Self { overlay, terminal, running }
    }

    pub fn widget(&self) -> &gtk::Overlay {
        &self.overlay
    }

    pub fn terminal(&self) -> &vte4::Terminal {
        &self.terminal
    }

    pub fn is_running(&self) -> bool {
        self.running.get()
    }
}

fn install_copy_paste_shortcuts(terminal: &vte4::Terminal) {
    let controller = gtk::ShortcutController::new();
    controller.set_scope(gtk::ShortcutScope::Local);

    let copy_action = gtk::CallbackAction::new(clone!(
        #[weak]
        terminal,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, _| {
            terminal.copy_clipboard_format(vte4::Format::Text);
            glib::Propagation::Stop
        }
    ));
    controller.add_shortcut(gtk::Shortcut::new(gtk::ShortcutTrigger::parse_string("<Ctrl><Shift>c"), Some(copy_action)));

    let paste_action = gtk::CallbackAction::new(clone!(
        #[weak]
        terminal,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, _| {
            terminal.paste_clipboard();
            glib::Propagation::Stop
        }
    ));
    controller.add_shortcut(gtk::Shortcut::new(gtk::ShortcutTrigger::parse_string("<Ctrl><Shift>v"), Some(paste_action)));

    terminal.add_controller(controller);
}

fn install_url_click(terminal: &vte4::Terminal, url_regex: Option<vte4::Regex>) {
    let Some(url_regex) = url_regex else { return };
    let click = gtk::GestureClick::new();
    click.connect_released(clone!(
        #[weak]
        terminal,
        move |_gesture, _n_press, x, y| {
            let matches = terminal.check_regex_simple_at(x, y, &[&url_regex], 0);
            if let Some(url) = matches.first() {
                gtk::UriLauncher::new(url).launch(None::<&gtk::Window>, None::<&gio::Cancellable>, |_| {});
            }
        }
    ));
    terminal.add_controller(click);
}
