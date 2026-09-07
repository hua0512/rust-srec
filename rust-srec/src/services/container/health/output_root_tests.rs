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
    assert!(probe_root_writable(&probe).is_ok());
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
    let probe = gate(vec![])
        .probe_path_for_template(child.to_str().unwrap())
        .unwrap();
    let result = probe_root_writable(&probe);
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
    let streamer = StreamerDbModel::new("Alice/Name", "https://www.huya.com/alice", platform.id);
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
            &crate::utils::filename::sanitize_filename(&metadata.name),
        )
        .replace("{title}", "actual-title")
        .replace("{session_id}", "session")
        .replace("{platform}", metadata.platform());
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
