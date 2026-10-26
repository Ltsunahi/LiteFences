//! P0-03 diagnostics only. Never modifies Explorer, files, icons or global input.
#[path = "../diagnostics/desktop_probe.rs"]
mod desktop_probe;
#[path = "../platform/host_recovery.rs"]
mod host_recovery;

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [arg] if arg == "--snapshot" => desktop_probe::snapshot(),
        [arg] if arg == "--self-test" => desktop_probe::self_test(),
        _ => Err(std::io::Error::other(
            "Usage: desktop-probe --snapshot | --self-test",
        )),
    };
    match result {
        Ok(json) => println!("{json}"),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
