#![windows_subsystem = "windows"]

mod app;
mod cards;
mod dock;
mod geometry;
mod integrations;
mod launchpad;
mod machine;
mod metrics;
mod platform;
mod plugins;
mod render;
mod settings;
mod storage;
mod system;
mod ui;
mod visibility;

fn main() -> std::process::ExitCode {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(std::ffi::OsStr::new("plugins")) {
        return match app::plugin_cli::run(args) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    match app::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            platform::report_error(None, &error);
            std::process::ExitCode::FAILURE
        }
    }
}
