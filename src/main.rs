#![windows_subsystem = "windows"]

mod app;
mod dock;
mod geometry;
mod platform;
mod render;
mod settings;
mod storage;
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
