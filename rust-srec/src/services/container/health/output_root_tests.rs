use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};

use super::*;
use crate::config::{ConfigEventBroadcaster, ConfigService};
use crate::database::models::StreamerDbModel;
use crate::database::repositories::ConfigRepository;
use crate::downloader::output_root_gate::RootHealthState;
use crate::streamer::{StreamerManager, StreamerMetadata};

fn gate(roots: Vec<PathBuf>) -> Arc<OutputRootGate> {
    OutputRootGate::new(
        Weak::new(),
        Arc::new(|_| {}),
        roots,
        Duration::from_secs(30),
    )
}

#[test]
fn inferred_probe_key_matches_runtime_and_respects_explicit_prefixes() {
    let gate = gate(vec![]);
    assert!(
        gate.probe_path_for_template("/rec/{platform}/{streamer}")
            .is_none()
    );
    let probe = gate
        .probe_path_for_template("/rec/huya/Alice/{title}")
        .unwrap();
    assert_eq!(probe, PathBuf::from("/rec/huya/Alice"));
    assert_eq!(
        gate.resolve_path(&probe),
        gate.resolve_path(Path::new("/rec/huya/Alice/actual-title"))
    );
    assert_eq!(
        gate.probe_path_for_template("relative/{streamer}"),
        Some(PathBuf::from("relative"))
    );
    assert_eq!(
        gate.probe_path_for_template("/var/lib/recordings/{title}"),
        Some(PathBuf::from("/var/lib/recordings"))
    );
    assert!(gate.probe_path_for_template("/{title}/output").is_none());
    assert!(gate.probe_path_for_template("/rec/%Y/output").is_none());
    let gate = self::gate(vec![
        PathBuf::from("/rec"),
        PathBuf::from("/rec/huya/special"),
    ]);
    assert_eq!(
        gate.resolve_path(Path::new("/rec/huya/special/Alice")),
        PathBuf::from("/rec/huya/special")
    );
    assert!(
        gate.probe_path_for_template("/rec/huya/{title}").is_none(),
        "unknown title could choose the longer configured prefix"
    );
    let paths = select_output_probe_paths(&gate, &["/rec/huya/special/Alice".to_string()]);
    assert_eq!(
        paths,
        HashSet::from([PathBuf::from("/rec"), PathBuf::from("/rec/huya/special")])
    );
}

#[cfg(windows)]
#[test]
fn windows_probe_paths_keep_native_drive_and_unc_identity() {
    let gate = gate(vec![
        PathBuf::from(r"D:\recordings"),
        PathBuf::from(r"D:\recordings\special"),
    ]);
    let probe = gate
        .probe_path_for_template(r"D:\recordings\special\Alice\{title}")
        .unwrap();
    assert_eq!(
        gate.resolve_path(&probe),
        PathBuf::from(r"D:\recordings\special")
    );
    assert_eq!(
        gate.resolve_path(&probe),
        gate.resolve_path(Path::new(r"D:\recordings\special\Alice\today"))
    );
    let unc = r"\\server\share\recordings\Alice\{title}";
    let probe = gate.probe_path_for_template(unc).unwrap();
    assert_eq!(
        gate.resolve_path(&probe),
        gate.resolve_path(Path::new(r"\\server\share\recordings\Alice\today"))
    );
}

#[test]
fn representative_paths_are_deterministic_and_explicit_roots_win_the_cap() {
    let explicit = PathBuf::from("/z-explicit/root");
    let gate = gate(vec![explicit.clone()]);
    let mut templates: Vec<_> = (0..MAX_OUTPUT_ROOT_PROBES + 5)
        .map(|i| format!("/root-{i}/output/Alice"))
        .collect();
    templates.extend([
        "/root-0/output".to_string(),
        "/root-0/output/Bob".to_string(),
    ]);
    let selected = select_output_probe_paths(&gate, &templates);
    assert!(selected.contains(Path::new("/root-0/output/Alice")));
    assert!(!selected.contains(Path::new("/root-0/output")));
    assert!(!selected.contains(Path::new("/root-0/output/Bob")));
    templates.reverse();
    assert_eq!(selected, select_output_probe_paths(&gate, &templates));
    let bounded = bounded_output_probe_paths(&gate, selected);
    assert_eq!(bounded.len(), MAX_OUTPUT_ROOT_PROBES);
    assert_eq!(bounded[0], explicit);
}

#[test]
fn writable_child_is_probed_instead_of_ancestor_gate_key() {
    let dir = tempfile::tempdir().unwrap();
    let child = dir.path().join("recordings/Alice");
    std::fs::create_dir_all(&child).unwrap();
    let gate = gate(vec![]);
    let probe = gate
        .probe_path_for_template(child.to_str().unwrap())
        .unwrap();
    assert_eq!(probe, child);
    assert_ne!(gate.resolve_path(&probe), probe);
    assert!(probe_root_writable(&probe, &gate).is_ok());
}

#[tokio::test]
async fn offline_streamer_missing_directory_does_not_fail_startup_probe() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("output");
    std::fs::create_dir(&output).unwrap();
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let configs = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
    let streamers = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
    let service = ConfigService::new(configs.clone(), streamers.clone());
    let manager = StreamerManager::new(streamers, ConfigEventBroadcaster::new());
    let platform = configs.get_platform_config_by_name("huya").await.unwrap();
    let streamer = StreamerDbModel::new("0601", "https://www.huya.com/0601", platform.id);
    let metadata = StreamerMetadata::from_db_model(&streamer);
    assert_eq!(metadata.state, crate::domain::StreamerState::NotLive);
    manager.create_streamer(metadata).await.unwrap();
    let mut global = service.get_global_config().await.unwrap();
    global.output_folder = output
        .join("{streamer}/%Y/%m/%d")
        .to_string_lossy()
        .into_owned();
    service.update_global_config(&global).await.unwrap();

    let gate = gate(vec![]);
    let paths = discover_output_probe_paths(&service, &manager, &gate).await;
    let probe = output.join("0601");
    assert_eq!(paths, HashSet::from([probe.clone()]));
    assert!(probe_root_writable(&probe, &gate).is_ok());
    assert_eq!(std::fs::read_dir(&output).unwrap().count(), 0);

    let recording_dir = probe.join("2026/09/10");
    crate::downloader::engine::utils::ensure_output_dir(&recording_dir)
        .await
        .unwrap();
    assert!(recording_dir.is_dir());
}

#[test]
fn missing_nested_directories_probe_existing_ancestor_without_creating_folders() {
    let dir = tempfile::tempdir().unwrap();
    let gate = gate(vec![dir.path().to_path_buf()]);
    let output = dir.path().join("0601/2026/09/10");
    assert!(probe_root_writable(&output, &gate).is_ok());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn missing_explicit_root_does_not_fall_back_to_writable_outer_root() {
    let dir = tempfile::tempdir().unwrap();
    let missing_root = dir.path().join("missing-mount");
    let gate = gate(vec![dir.path().to_path_buf(), missing_root.clone()]);
    for output in [&missing_root, &missing_root.join("0601/2026")] {
        assert_eq!(
            probe_root_writable(output, &gate).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn file_in_output_path_does_not_fall_back_to_writable_parent() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("0601");
    std::fs::write(&output, b"existing file").unwrap();
    let gate = gate(vec![dir.path().to_path_buf()]);
    assert!(probe_root_writable(&output, &gate).is_err());
    assert!(probe_root_writable(&output.join("2026"), &gate).is_err());
    assert_eq!(std::fs::read(&output).unwrap(), b"existing file");
}

#[cfg(unix)]
#[test]
fn dangling_output_symlinks_do_not_fall_back_to_writable_parent() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("0601");
    std::os::unix::fs::symlink(dir.path().join("missing-target"), &output).unwrap();
    let gate = gate(vec![dir.path().to_path_buf()]);
    for path in [&output, &output.join("2026/09/10")] {
        assert_eq!(
            probe_root_writable(path, &gate).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
    }
}

#[cfg(unix)]
#[test]
fn missing_subdirectories_under_valid_symlink_can_be_probed() {
    let dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let output = dir.path().join("0601");
    std::os::unix::fs::symlink(target.path(), &output).unwrap();
    let gate = gate(vec![dir.path().to_path_buf()]);
    assert!(probe_root_writable(&output.join("2026/09/10"), &gate).is_ok());
    assert_eq!(std::fs::read_dir(target.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn unwritable_destination_does_not_fall_back_to_writable_parent() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("0601");
    std::fs::create_dir(&output).unwrap();
    let previous = std::fs::metadata(&output).unwrap().permissions();
    std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o555)).unwrap();
    // Privileged users can bypass mode bits, so first check that this environment
    // can exercise permission failures. Restore permissions before any assertion.
    let permissions_enforced = tempfile::tempfile_in(&output).is_err();
    let gate = gate(vec![dir.path().to_path_buf()]);
    let existing_result = probe_root_writable(&output, &gate);
    let missing_result = probe_root_writable(&output.join("2026/09"), &gate);
    std::fs::set_permissions(&output, previous).unwrap();
    if permissions_enforced {
        assert_eq!(
            existing_result.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            missing_result.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }
}

#[cfg(unix)]
#[test]
fn read_only_parent_does_not_prevent_probing_writable_child() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let child = dir.path().join("recordings/Alice");
    std::fs::create_dir_all(&child).unwrap();
    let previous = std::fs::metadata(dir.path()).unwrap().permissions();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
    let gate = gate(vec![]);
    let probe = gate
        .probe_path_for_template(child.to_str().unwrap())
        .unwrap();
    let result = probe_root_writable(&probe, &gate);
    std::fs::set_permissions(dir.path(), previous).unwrap();
    assert!(
        result.is_ok(),
        "the probe must not create its temporary file in the read-only ancestor"
    );
    assert_eq!(probe, child);
}

#[tokio::test]
async fn discovered_startup_failure_blocks_actual_runtime_key_and_heals_once() {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let configs = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
    let streamers = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
    let service = ConfigService::new(configs.clone(), streamers.clone());
    let manager = StreamerManager::new(streamers.clone(), ConfigEventBroadcaster::new());
    let platform = configs.get_platform_config_by_name("huya").await.unwrap();
    let streamer = StreamerDbModel::new(
        "Alice/Name%Y%n%%",
        "https://www.huya.com/alice",
        platform.id,
    );
    let metadata = StreamerMetadata::from_db_model(&streamer);
    manager.create_streamer(metadata.clone()).await.unwrap();
    let mut global = service.get_global_config().await.unwrap();
    global.output_folder = "/rec/{platform}/{streamer}/{title}".to_string();
    service.update_global_config(&global).await.unwrap();
    let recoveries = Arc::new(AtomicUsize::new(0));
    let observed = recoveries.clone();
    let gate = OutputRootGate::new(
        Weak::new(),
        Arc::new(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
        }),
        vec![],
        Duration::from_secs(30),
    );
    let runtime = global
        .output_folder
        .replace(
            "{streamer}",
            &crate::utils::filename::sanitize_filename_for_template(&metadata.name),
        )
        .replace("{title}", "actual-title")
        .replace("{session_id}", "session")
        .replace("{platform}", metadata.platform());
    let runtime = pipeline_common::expand_path_template(&runtime);
    assert!(runtime.contains("Name%Y%n%%"));
    let paths = discover_output_probe_paths(&service, &manager, &gate).await;
    let probe = Path::new(&runtime).parent().unwrap();
    assert_eq!(paths, HashSet::from([probe.to_path_buf()]));
    gate.record_failure(
        probe,
        &std::io::Error::new(std::io::ErrorKind::PermissionDenied, "startup probe denied"),
    );
    let blocked = gate.check(Path::new(&runtime)).unwrap_err();
    assert_eq!(blocked.root, gate.resolve_path(Path::new(&runtime)));
    assert_ne!(blocked.root, PathBuf::from("/rec"));
    gate.mark_healthy(Path::new(&runtime));
    gate.mark_healthy(Path::new(&runtime));
    assert_eq!(recoveries.load(Ordering::SeqCst), 1);
    assert_eq!(gate.snapshot()[0].state, RootHealthState::Healthy);

    global.output_folder = "/new/{platform}/{streamer}/{title}".to_string();
    service.update_global_config(&global).await.unwrap();
    let updated = discover_output_probe_paths(&service, &manager, &gate).await;
    assert!(!updated.contains(probe));
    assert!(updated.iter().all(|path| path.starts_with("/new")));
    let mut platform = configs.get_platform_config_by_name("huya").await.unwrap();
    platform.output_folder = Some("relative/{streamer}/output".to_string());
    service.update_platform_config(&platform).await.unwrap();
    let updated = discover_output_probe_paths(&service, &manager, &gate).await;
    assert!(updated.contains(&PathBuf::from(format!(
        "relative/{}/output",
        crate::utils::filename::sanitize_filename(&metadata.name)
    ))));

    gate.record_failure(
        Path::new("/existing/root/path"),
        &std::io::Error::other("existing failure"),
    );
    pool.close().await;
    let _partial = discover_output_probe_paths(&service, &manager, &gate).await;
    assert!(
        gate.check(Path::new("/existing/root/path")).is_err(),
        "failed discovery must not remove prior gate state"
    );
}

#[test]
fn escaped_percent_metadata_keeps_probe_identity_without_expanding_real_time_tokens() {
    let gate = gate(vec![]);
    let template = "/rec/Alice%%Y%%n%%%%/%Y/{title}";
    let probe = gate.probe_path_for_template(template).unwrap();
    assert_eq!(probe, PathBuf::from("/rec/Alice%Y%n%%"));
    let runtime = pipeline_common::expand_path_template(&template.replace("{title}", "actual"));
    assert_eq!(
        gate.resolve_path(&probe),
        gate.resolve_path(Path::new(&runtime))
    );
    assert!(gate.probe_path_for_template("/rec/%Y/Alice%%n").is_none());
    let gate = self::gate(vec![
        PathBuf::from("/rec"),
        PathBuf::from("/rec/Alice%Y/special"),
    ]);
    assert!(
        gate.probe_path_for_template("/rec/Alice%%Y/{title}")
            .is_none(),
        "unknown title might select a deeper explicit root"
    );
    assert_eq!(
        gate.probe_path_for_template("/rec/Alice%%Y/special/{title}"),
        Some(PathBuf::from("/rec/Alice%Y/special"))
    );
}
