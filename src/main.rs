#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(windows)]
mod app_options;
#[cfg(windows)]
mod platform;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    let Some(options) = app_options::Options::parse(std::env::args_os().skip(1)) else {
        return std::process::ExitCode::from(2);
    };
    let quiet = options.hidden();
    match platform::run(options) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            if !quiet {
                platform::show_error(&error.to_string());
            }
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("LiteFences P0-01 requires Windows 11 x64.");
    std::process::exit(1);
}
