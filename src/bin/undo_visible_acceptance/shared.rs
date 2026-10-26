fn acceptance_passed(
    seeded: bool,
    restored: bool,
    source_unchanged: bool,
    disk_restored_exact: bool,
) -> bool {
    seeded && restored && source_unchanged && disk_restored_exact
}

pub(crate) fn entrypoint(group: bool) -> std::process::ExitCode {
    if std::env::args_os().len() != 1 {
        eprintln!("This visible acceptance binary does not accept arguments.");
        return std::process::ExitCode::from(2);
    }

    #[cfg(windows)]
    {
        match run(group) {
            Ok(true) => std::process::ExitCode::SUCCESS,
            Ok(false) => {
                eprintln!(
                    "Undo visible acceptance was not restored; inspect undo-visible-report.json."
                );
                std::process::ExitCode::FAILURE
            }
            Err(error) => {
                eprintln!("Undo visible acceptance failed: {error}");
                std::process::ExitCode::FAILURE
            }
        }
    }

    #[cfg(not(windows))]
    {
        let _ = group;
        eprintln!("Undo visible acceptance requires Windows.");
        std::process::ExitCode::FAILURE
    }
}

#[cfg(windows)]
fn run(group: bool) -> std::io::Result<bool> {
    use crate::{app_options, platform};
    use litefences::{
        core::{
            group_ops::{add_references, create_group},
            layout::RectDip,
            model::{Config, FileReference, NativePath, ReferenceKind},
        },
        storage::codec,
    };

    let artifacts = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("artifacts");
    std::fs::create_dir_all(&artifacts)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(std::io::Error::other)?
        .as_nanos();
    let prefix = if group {
        format!("undo-group-visible-{}-{stamp}", std::process::id())
    } else {
        format!("undo-visible-{}-{stamp}", std::process::id())
    };
    let root = artifacts.join(prefix);
    // A collision fails instead of reopening or overwriting an existing fixture.
    std::fs::create_dir(&root)?;

    let source_path = root.join("source.txt");
    const SOURCE_CONTENT: &[u8] =
        b"LiteFences undo visible acceptance fixture.\r\nThis file must remain unchanged.\r\n";
    std::fs::write(&source_path, SOURCE_CONTENT)?;
    let source_before = std::fs::read(&source_path)?;

    let mut original_config = Config::default();
    let group_id = create_group(
        &mut original_config,
        "Undo acceptance fixture".into(),
        RectDip {
            x: 48,
            y: 130,
            width: 320,
            height: 220,
        },
    )
    .map_err(|error| std::io::Error::other(format!("fixture group: {error:?}")))?;
    let path = NativePath::from_utf16(source_path.as_os_str().encode_wide().collect::<Vec<u16>>())
        .map_err(|error| std::io::Error::other(format!("fixture path: {error}")))?;
    add_references(
        &mut original_config,
        group_id,
        vec![FileReference {
            kind: ReferenceKind::File,
            path,
        }],
    )
    .map_err(|error| std::io::Error::other(format!("fixture reference: {error:?}")))?;

    let seed_bytes = codec::encode(&original_config)
        .map_err(|error| std::io::Error::other(format!("fixture codec: {error}")))?;
    std::fs::write(root.join("config.json"), &seed_bytes)?;
    // Preserve the exact disk seed as an artifact as well as the in-memory model.
    std::fs::write(root.join("seed-config.json"), seed_bytes)?;

    let options = app_options::Options {
        smoke: false,
        data_dir: Some(root.clone()),
        storage_smoke: None,
    };
    let loop_report = if group {
        platform::run_undo_group_acceptance(options, 60)?
    } else {
        platform::run_undo_acceptance(options, 60)?
    };

    let source_unchanged = std::fs::read(&source_path)? == source_before;
    let disk_restored_exact = std::fs::read(root.join("config.json"))
        .ok()
        .and_then(|bytes| codec::decode(&bytes).ok())
        .is_some_and(|config| config == original_config);
    let passed = acceptance_passed(
        loop_report.undo_fixture_seeded,
        loop_report.undo_fixture_restored,
        source_unchanged,
        disk_restored_exact,
    );
    let report = serde_json::json!({
        "status": if passed { "passed" } else { "failed" },
        "undo_kind": if group { "group" } else { "reference" },
        "scope": "programmatically seeded history; requires separately observed real Ctrl+Z/button input; not Explorer drag acceptance",
        "process_id": std::process::id(),
        "data_dir": root,
        "undo_fixture_seeded": loop_report.undo_fixture_seeded,
        "undo_fixture_restored": loop_report.undo_fixture_restored,
        "source_unchanged": source_unchanged,
        "disk_restored_exact": disk_restored_exact,
        "timeout_fired": loop_report.timeout_fired,
    });
    std::fs::write(
        root.join("undo-visible-report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(passed)
}

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

#[cfg(test)]
mod tests {
    use super::acceptance_passed;

    #[test]
    fn only_all_true_combination_passes() {
        for bits in 0..16 {
            let values = [bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0];
            assert_eq!(
                acceptance_passed(values[0], values[1], values[2], values[3]),
                bits == 15,
                "unexpected result for bit combination {bits:04b}"
            );
        }
    }
}
