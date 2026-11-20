#![windows_subsystem = "windows"]

mod app;
mod dock;
mod geometry;
mod integrations;
mod launchpad;
mod metrics;
mod platform;
mod render;
mod settings;
mod storage;
mod system;
mod ui;

fn main() -> std::process::ExitCode {
    match app::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            platform::report_error(None, &error);
            std::process::ExitCode::FAILURE
        }
    }
}
