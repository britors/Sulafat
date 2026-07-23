mod host_dialog;
mod host_list;
mod prefs;
mod quick_connect;
mod terminal_tab;
mod window_main;

use gtk::glib;
use gtk::prelude::*;

const APP_ID: &str = "org.lyraos.Sulafat";

fn main() -> glib::ExitCode {
    let filter = tracing_subscriber::EnvFilter::builder()
        .with_env_var("SULAFAT_LOG")
        .with_default_directive(tracing::level_filters::LevelFilter::WARN.into())
        .from_env_lossy();
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| {
        window_main::build(app, None);
    });
    app.run()
}
