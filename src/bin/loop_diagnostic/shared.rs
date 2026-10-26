//! Shared timed diagnostic logic for the CLI and the interactive test launcher.
#[cfg(windows)]
use crate::{app_options, platform};

#[derive(Debug, PartialEq)]
struct DiagnosticOptions {
    seconds: u32,
    hidden: bool,
}

fn parse(args: impl IntoIterator<Item = String>) -> Option<DiagnosticOptions> {
    let mut args = args.into_iter();
    let mut seconds = None;
    let mut visible = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seconds" if seconds.is_none() => {
                let value = args.next()?.parse::<u32>().ok()?;
                if !(1..=60).contains(&value) {
                    return None;
                }
                seconds = Some(value);
            }
            "--visible" if !visible => visible = true,
            _ => return None,
        }
    }
    Some(DiagnosticOptions {
        seconds: seconds.unwrap_or(10),
        hidden: !visible,
    })
}

#[cfg(windows)]
fn cpu_seconds(thread: bool) -> std::io::Result<f64> {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentProcess, GetCurrentThread, GetProcessTimes, GetThreadTimes},
    };
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: pseudo handles refer to our own process/current UI thread and must not be closed;
    // all four FILETIME outputs are live, distinct and correctly aligned.
    if unsafe {
        if thread {
            GetThreadTimes(
                GetCurrentThread(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        } else {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        }
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    Ok((ticks(kernel) as f64 + ticks(user) as f64) / 10_000_000.0)
}

#[cfg(windows)]
fn execute(options: DiagnosticOptions) -> std::io::Result<serde_json::Value> {
    use litefences::core::{group_ops::create_group, layout::RectDip, model::Config};
    let artifacts = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("artifacts");
    std::fs::create_dir_all(&artifacts)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(std::io::Error::other)?
        .as_nanos();
    let root = artifacts.join(format!("loop-diagnostic-{}-{stamp}", std::process::id()));
    std::fs::create_dir(&root)?;
    let mut config = Config::default();
    create_group(
        &mut config,
        "诊断空分组".into(),
        RectDip {
            x: 48,
            y: 130,
            width: 320,
            height: 220,
        },
    )
    .map_err(|error| std::io::Error::other(format!("fixture: {error:?}")))?;
    let bytes = litefences::storage::codec::encode(&config)
        .map_err(|error| std::io::Error::other(format!("fixture codec: {error:?}")))?;
    std::fs::write(root.join("config.json"), bytes)?;
    let before = cpu_seconds(false)?;
    let ui_before = cpu_seconds(true).ok();
    let environment = platform::loop_environment::read();
    let started = std::time::Instant::now();
    let loop_report = platform::run_diagnostic(
        app_options::Options {
            smoke: false,
            data_dir: Some(root.clone()),
            storage_smoke: None,
        },
        options.seconds,
        options.hidden,
    )?;
    let snapshot = loop_report.counters;
    let mut message_histogram = loop_report.message_histogram;
    message_histogram.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let message_kind_count = message_histogram.len();
    message_histogram.truncate(32);
    let elapsed = started.elapsed().as_secs_f64();
    let cpu_delta = cpu_seconds(false)? - before;
    let ui_delta = ui_before
        .zip(cpu_seconds(true).ok())
        .map(|(before, after)| after - before);
    // SAFETY: read-only query across all processor groups, no process handles opened.
    let logical =
        unsafe { windows_sys::Win32::System::Threading::GetActiveProcessorCount(u16::MAX) };
    let report = serde_json::json!({
        "schema_version":1,"process_id":std::process::id(),"data_dir":root,
        "status":if loop_report.timeout_fired {"completed"} else {"closed_early"},
        "timeout_fired":loop_report.timeout_fired,"seconds_requested":options.seconds,"hidden_requested":options.hidden,
        "elapsed_seconds":elapsed,"cpu_seconds_delta":cpu_delta,"logical_processors":logical,
        "ui_thread_cpu_seconds_delta":ui_delta,
        "cpu_normalized_percent":if logical>0 {Some(cpu_delta/elapsed/f64::from(logical)*100.0)} else {None},
        "initial_window":loop_report.initial_visibility.map(|(visible,minimized)|serde_json::json!({"visible":visible,"minimized":minimized})),
        "closing_window":loop_report.closing_visibility.map(|(visible,minimized)|serde_json::json!({"visible":visible,"minimized":minimized})),
        "environment": {"station_name":environment.station_name,"desktop_name":environment.desktop_name,"receives_input":environment.receives_input},
        "visibility_note":"IsWindowVisible reports WS_VISIBLE, not whether the user can see the window or whether computer-use can capture it",
        "wait_strategy":"MWMO_INPUTAVAILABLE; queue drained on every wake",
        "top_parent_messages":message_histogram.iter().map(|(id,count)|serde_json::json!({"message_id":id,"message_id_hex":format!("0x{id:04X}"),"count":count})).collect::<Vec<_>>(),
        "parent_message_kinds":message_kind_count,"parent_message_list_truncated":message_kind_count>32,
        "high_parent_message_ids":loop_report.high_message_ids,
        "counters":{
            "wait_worker":snapshot.wait_worker,"wait_input":snapshot.wait_input,"wait_other":snapshot.wait_other,
            "session_changes":snapshot.session_changes,"icon_changes":snapshot.icon_changes,
            "window_messages":snapshot.messages,"paints":snapshot.paints,"refresh_timers":snapshot.refresh_timers,
            "cross_thread_messages":snapshot.cross_thread_messages,"cross_thread_getobject":snapshot.cross_thread_getobject,
        },
        "scope":"one empty group; startup through graceful close included; no program-generated physical input; caller must record UIA/manual interactions; not S1",
    });
    // The UI and storage worker have stopped. Only this newly-created test root
    // receives a bounded report; nothing is written from paint/input callbacks.
    std::fs::write(
        root.join("loop-report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(report)
}

#[cfg(windows)]
pub(crate) fn entrypoint(force_visible: bool) -> std::process::ExitCode {
    if force_visible && std::env::args_os().len() != 1 {
        return std::process::ExitCode::from(2);
    }
    let Some(mut options) = parse(std::env::args().skip(1)) else {
        eprintln!("Usage: loop-diagnostic [--seconds 1..60] [--visible]");
        return std::process::ExitCode::from(2);
    };
    if force_visible {
        options.hidden = false;
        options.seconds = 60;
    }
    match execute(options) {
        Ok(report) => {
            println!("{report}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Diagnostic failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn entrypoint(_force_visible: bool) -> std::process::ExitCode {
    eprintln!("loop-diagnostic requires Windows");
    std::process::ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Option<DiagnosticOptions> {
        parse(values.iter().map(|s| (*s).into()))
    }
    #[test]
    fn defaults_are_hidden_and_bounded() {
        assert_eq!(
            args(&[]),
            Some(DiagnosticOptions {
                seconds: 10,
                hidden: true
            })
        );
        assert_eq!(
            args(&["--visible", "--seconds", "60"]),
            Some(DiagnosticOptions {
                seconds: 60,
                hidden: false
            })
        );
    }
    #[test]
    fn invalid_flags_cannot_select_user_data_or_disable_timeout() {
        for input in [
            vec!["--seconds", "0"],
            vec!["--seconds", "61"],
            vec!["--seconds"],
            vec!["--seconds", "1", "--seconds", "2"],
            vec!["--visible", "--visible"],
            vec!["--data-dir", "E:\\user"],
            vec!["--seconds", "oops"],
        ] {
            assert!(args(&input).is_none(), "{input:?}");
        }
    }
}
