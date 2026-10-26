//! Isolated visible group undo acceptance harness. Never opens ordinary user data.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(windows)]
#[allow(dead_code)]
#[path = "../app_options.rs"]
mod app_options;
#[cfg(windows)]
#[allow(dead_code)]
#[path = "../platform/mod.rs"]
mod platform;

#[path = "undo_visible_acceptance/shared.rs"]
mod shared;

fn main() -> std::process::ExitCode {
    shared::entrypoint(true)
}
