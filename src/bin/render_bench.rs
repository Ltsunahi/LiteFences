#![windows_subsystem = "windows"]
#![deny(unsafe_op_in_unsafe_fn)]

#[path = "../diagnostics/render_bench.rs"]
mod benchmark;

fn main() -> std::process::ExitCode {
    match benchmark::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
