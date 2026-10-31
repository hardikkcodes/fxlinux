mod audio;
mod install;
mod presets;
mod ui;

use anyhow::{Result, bail};
use audio::pw_worker::PwWorker;
use gtk::prelude::*;
use std::rc::Rc;

const USAGE: &str = "Usage: fxlinux-app [install [--output NODE_NAME] | uninstall]";

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Launch,
    Install { output: Option<String> },
    Uninstall,
}

fn parse_args<I, S>(arguments: I) -> Result<Action>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = arguments.into_iter().map(Into::into).collect();
    match args.as_slice() {
        [] => Ok(Action::Launch),
        [command] if command == "install" => Ok(Action::Install { output: None }),
        [command, flag, output] if command == "install" && flag == "--output" => {
            if output.is_empty() {
                bail!("NODE_NAME must not be empty");
            }
            Ok(Action::Install {
                output: Some(output.clone()),
            })
        }
        [command] if command == "uninstall" => Ok(Action::Uninstall),
        _ => bail!("invalid arguments"),
    }
}

fn main() -> gtk::glib::ExitCode {
    let action = match parse_args(std::env::args().skip(1)) {
        Ok(action) => action,
        Err(error) => {
            eprintln!("fxlinux-app: {error}\n{USAGE}");
            return gtk::glib::ExitCode::FAILURE;
        }
    };

    match action {
        Action::Install { output } => {
            return command_result("install", install::run(output.as_deref()));
        }
        Action::Uninstall => return command_result("uninstall", install::uninstall()),
        Action::Launch => {}
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

fn command_result(command: &str, result: Result<()>) -> gtk::glib::ExitCode {
    match result {
        Ok(()) => gtk::glib::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fxlinux-app {command} failed: {error:#}");
            gtk::glib::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_commands() {
        assert_eq!(parse_args(Vec::<String>::new()).unwrap(), Action::Launch);
        assert_eq!(
            parse_args(["install"]).unwrap(),
            Action::Install { output: None }
        );
        assert_eq!(
            parse_args(["install", "--output", "alsa_output.test"]).unwrap(),
            Action::Install {
                output: Some("alsa_output.test".into())
            }
        );
        assert_eq!(parse_args(["uninstall"]).unwrap(), Action::Uninstall);
    }

    #[test]
    fn rejects_invalid_arguments() {
        assert!(parse_args(["install", "--output"]).is_err());
        assert!(parse_args(["install", "--wrong", "sink"]).is_err());
        assert!(parse_args(["uninstall", "extra"]).is_err());
        assert!(parse_args(["unknown"]).is_err());
    }
}
