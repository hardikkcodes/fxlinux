mod audio;
mod install;
mod presets;
mod ui;

use audio::pw_worker::PwWorker;
use gtk::prelude::*;
use std::rc::Rc;

fn main() -> gtk::glib::ExitCode {
    if std::env::args().nth(1).as_deref() == Some("install") {
        return match install::run() {
            Ok(()) => gtk::glib::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("fxlinux-app install failed: {e:#}");
                gtk::glib::ExitCode::FAILURE
            }
        };
    }

    let app = adw::Application::builder()
        .application_id("io.github.fxlinux.FxLinux")
        .build();

    app.connect_activate(|app| {
        let (worker, status_rx) = PwWorker::spawn();
        ui::window::build(app, Rc::new(worker), status_rx);
    });

    app.run()
}
