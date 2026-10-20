use super::*;

struct Temp {
    directory: PathBuf,
    path: PathBuf,
}

impl Temp {
    fn new() -> Self {
        let directory = env::temp_dir().join(format!("mexc-workspace-test-{}", unique_suffix()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("workspace.json");
        Self { directory, path }
    }

    fn open(&self) -> Opened {
        Store::open_at(self.path.clone()).unwrap()
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        // Tests never share directories or touch the user's workspace.
        #[cfg(unix)]
        fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700)).ok();
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

fn snapshot(magnet: bool) -> Snapshot {
    Snapshot {
        magnet,
        ..Snapshot::default()
    }
}

fn configured_snapshot() -> Snapshot {
    use crate::workspace::{
        SavedAnchor, SavedBook, SavedDrawing, SavedInstrument, SavedShape, SavedStyle,
    };
    use mexc_indicators::profiles::{Instance, Kind, Range, Spec};
    use mexc_indicators::{IndicatorSource, IndicatorSpec};
    use mexc_model::executions::NativeInstrument;
    use mexc_model::{Exchange, Market, Product};

    let mut chart = crate::state::ChartState::default();
    chart
        .indicators
        .restore_instance(
            41,
            IndicatorSpec::Rsi { period: 7 },
            IndicatorSource::default(),
            false,
        )
        .unwrap();
    chart
        .indicators
        .restore_instance(
            9,
            IndicatorSpec::Sma { period: 3 },
            IndicatorSource::default(),
            true,
        )
        .unwrap();
    let current = NativeInstrument::from_market(&chart.market);
    let other = NativeInstrument::from_market(&Market {
        exchange: Exchange::Coinbase,
        product: Product::Spot,
        symbol: "Case-sensitive_native".into(),
        interval: chart.market.interval,
    });
    let mut tpo = Spec::default_for(Kind::Tpo);
    tpo.ticks_per_row = 7;
    tpo.value_area_percent = 68;
    tpo.display.letters = true;
    let mut visible = Spec::default_for(Kind::Volume);
    visible.range = Range::Visible;
    let mut fixed = Spec::default_for(Kind::Volume);
    fixed.range = Range::Fixed {
        start_ms: 1_704_067_200_000,
        end_ms: 1_704_153_600_000,
    };
    chart.profiles = vec![
        Instance {
            id: 13,
            spec: tpo,
            visible: true,
            instrument: None,
        },
        Instance {
            id: 2,
            spec: visible,
            visible: false,
            instrument: None,
        },
        Instance {
            id: 91,
            spec: fixed.clone(),
            visible: true,
            instrument: Some(current.clone()),
        },
        Instance {
            id: 8,
            spec: fixed,
            visible: false,
            instrument: Some(other),
        },
    ];
    let mut saved = Snapshot {
        chart: chart.workspace_snapshot(),
        magnet: true,
        ..Snapshot::default()
    };
    saved.chart.active_pane = Some(41);
    saved.chart.panes[0].height = 277.0;
    saved.chart.panes[0].collapsed = true;
    saved.chart.panes[0].scale = crate::workspace::SavedScale {
        auto_fit: false,
        low: Some(-25.0),
        high: Some(125.0),
    };
    saved.recent_markets = vec![saved.chart.market.clone()];
    saved.favorite_markets = saved.recent_markets.clone();
    saved.books = vec![SavedBook {
        instrument: SavedInstrument::from(&current),
        drawings: vec![SavedDrawing {
            id: 73,
            shape: SavedShape::Horizontal {
                at: SavedAnchor {
                    utc_ms: 1_704_067_200_000,
                    price: 123.45,
                },
            },
            style: SavedStyle::from(crate::drawing_types::Style::default()),
            visible: false,
            locked: true,
        }],
    }];
    saved.validate().unwrap();
    saved
}

fn checkpoint(path: &Path) -> Snapshot {
    let loaded = load(path).unwrap();
    assert!(matches!(loaded.status, LoadStatus::Valid));
    loaded.snapshot.expect("expected validated checkpoint")
}

fn names(directory: &Path) -> Vec<String> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

fn absolute_test_root() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(r"C:\isolated")
    }
    #[cfg(unix)]
    {
        PathBuf::from("/isolated")
    }
}

#[test]
fn default_paths_are_absolute_and_ignore_relative_environment_values() {
    let root = absolute_test_root();
    let state = root.join("state");
    let home = root.join("home");
    let local = root.join("local");
    assert_eq!(
        default_path_from(
            Some(state.clone().into_os_string()),
            Some(home.clone().into_os_string()),
            Some(local.clone().into_os_string()),
        )
        .unwrap(),
        state.join("mexc-chart/workspace.json"),
    );
    #[cfg(windows)]
    let home_state = home.join("AppData/Local");
    #[cfg(target_os = "macos")]
    let home_state = home.join("Library/Application Support");
    #[cfg(all(unix, not(target_os = "macos")))]
    let home_state = home.join(".local/state");
    assert_eq!(
        default_path_from(
            Some("relative/state".into()),
            Some(home.into_os_string()),
            Some("relative/local".into()),
        )
        .unwrap(),
        home_state.join("mexc-chart/workspace.json"),
    );
    for invalid in ["", "relative", "."] {
        assert!(
            default_path_from(
                Some(invalid.into()),
                Some(invalid.into()),
                Some(invalid.into()),
            )
            .is_err()
        );
    }
    #[cfg(windows)]
    {
        assert_eq!(
            default_path_from(None, None, Some(local.into_os_string())).unwrap(),
            root.join("local/mexc-chart/workspace.json"),
        );
        for invalid in [r"C:relative", r"\rooted-without-drive"] {
            assert!(default_path_from(Some(invalid.into()), None, None).is_err());
        }
    }
}

#[test]
fn relative_checkpoint_paths_are_rejected_before_creating_files() {
    assert!(Store::open_at(PathBuf::from("relative-workspace/workspace.json")).is_err());
    assert!(Store::open_at(PathBuf::from("workspace.json")).is_err());
}

#[test]
fn independent_directories_lock_and_private_permissions() {
    let first = Temp::new();
    let second = Temp::new();
    let opened = first.open();
    let other = second.open();
    assert!(opened.snapshot.is_none());
    assert!(opened.warning.is_none());
    assert!(opened.writable);
    assert!(Store::open_at(first.path.clone()).is_err());
    opened.store.save(1, &snapshot(true)).unwrap();
    other.store.save(1, &snapshot(false)).unwrap();
    #[cfg(unix)]
    {
        assert_eq!(
            fs::metadata(&first.directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for path in [&first.path, &sibling(&first.path, ".lock")] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    drop(opened);
    let reopened = first.open();
    assert_eq!(reopened.snapshot, Some(snapshot(true)));
    assert_eq!(checkpoint(&second.path), snapshot(false));
    // Revision numbers are process-local and restart at zero.
    assert!(reopened.store.save(0, &snapshot(false)).unwrap().written);
}

#[test]
fn corrupt_primary_recovers_validated_last_good_backup() {
    let temp = Temp::new();
    let opened = temp.open();
    opened.store.save(1, &snapshot(true)).unwrap();
    opened.store.save(2, &snapshot(false)).unwrap();
    drop(opened);
    let broken = b"{ interrupted checkpoint";
    fs::write(&temp.path, broken).unwrap();
    let recovered = temp.open();
    assert_eq!(recovered.snapshot, Some(snapshot(true)));
    assert!(recovered.warning.is_some());
    let copies = names(&temp.directory);
    let preserved = copies
        .iter()
        .find(|name| name.starts_with("workspace.json.corrupt-"))
        .unwrap();
    assert_eq!(fs::read(temp.directory.join(preserved)).unwrap(), broken);
    assert_eq!(checkpoint(&temp.path), snapshot(true));
    assert!(recovered.store.save(1, &snapshot(false)).unwrap().written);
}

#[test]
fn structural_corruption_preserves_both_invalid_copies_and_starts_empty() {
    let temp = Temp::new();
    let mut invalid = snapshot(true);
    invalid.chart.price_height = -1.0;
    assert!(invalid.validate().is_err());
    let bytes = serde_json::to_vec(&invalid).unwrap();
    fs::write(&temp.path, &bytes).unwrap();
    let backup = sibling(&temp.path, ".backup");
    fs::write(&backup, b"not json").unwrap();
    let opened = temp.open();
    assert!(opened.snapshot.is_none());
    assert!(opened.warning.is_some());
    assert!(!temp.path.exists());
    assert!(!backup.exists());
    let copies = names(&temp.directory);
    let primary = copies
        .iter()
        .find(|name| name.starts_with("workspace.json.corrupt-"))
        .unwrap();
    let backup = copies
        .iter()
        .find(|name| name.starts_with("workspace.json.backup.corrupt-"))
        .unwrap();
    assert_eq!(fs::read(temp.directory.join(primary)).unwrap(), bytes);
    assert_eq!(fs::read(temp.directory.join(backup)).unwrap(), b"not json");
    assert!(opened.store.save(1, &Snapshot::default()).unwrap().written);
}

#[test]
fn future_primary_and_backup_are_byte_preserved_until_explicit_reset() {
    for future_primary in [true, false] {
        let temp = Temp::new();
        let mut value = serde_json::to_value(snapshot(true)).unwrap();
        value["schema_version"] = serde_json::json!(u64::from(SCHEMA_VERSION) + 10);
        value["unknown_future_payload"] = serde_json::json!({"keep": [1, 2, 3]});
        let future = serde_json::to_vec_pretty(&value).unwrap();
        let backup = sibling(&temp.path, ".backup");
        if future_primary {
            fs::write(&temp.path, &future).unwrap();
            atomic_json(&backup, &snapshot(false)).unwrap();
        } else {
            atomic_json(&temp.path, &snapshot(false)).unwrap();
            fs::write(&backup, &future).unwrap();
        }
        let before_main = fs::read(&temp.path).unwrap();
        let before_backup = fs::read(&backup).unwrap();
        let opened = temp.open();
        assert!(!opened.writable);
        assert!(!opened.store.writable());
        assert!(opened.store.save(1, &Snapshot::default()).is_err());
        let partial = Snapshot::default().cleared(ResetKind::Current).unwrap();
        assert!(opened.store.reset(2, &partial).is_err());
        assert!(!sibling(&temp.path, ".reset").exists());
        assert_eq!(fs::read(&temp.path).unwrap(), before_main);
        assert_eq!(fs::read(&backup).unwrap(), before_backup);
        let reset = Snapshot::default().cleared(ResetKind::All).unwrap();
        assert!(
            opened
                .store
                .reset_with_kind(2, &reset, ResetKind::All)
                .unwrap()
                .written
        );
        assert!(opened.store.writable());
        assert_eq!(checkpoint(&temp.path), reset);
        assert_eq!(checkpoint(&backup), reset);
    }
}

#[test]
fn stale_revisions_and_pre_reset_epochs_cannot_resurrect_state() {
    let temp = Temp::new();
    let opened = temp.open();
    let original = configured_snapshot();
    assert!(opened.store.save(5, &original).unwrap().written);
    assert!(!opened.store.save(4, &snapshot(false)).unwrap().written);
    let reset = original.cleared(ResetKind::Current).unwrap();
    assert!(opened.store.reset(6, &reset).unwrap().written);
    assert!(opened.store.reset_is_authoritative(reset.reset_epoch));
    assert!(!opened.store.save(100, &original).unwrap().written);
    assert!(!opened.store.save(5, &reset).unwrap().written);
    assert!(!opened.store.reset(6, &reset).unwrap().written);
    assert_eq!(checkpoint(&temp.path), reset);
    assert_eq!(checkpoint(&sibling(&temp.path, ".backup")), reset);
}

#[test]
fn interrupted_reset_replays_at_every_checkpoint_boundary_idempotently() {
    for (phase, kind) in
        (0..3).flat_map(|phase| [ResetKind::Current, ResetKind::All].map(|kind| (phase, kind)))
    {
        let temp = Temp::new();
        let original = configured_snapshot();
        atomic_json(&temp.path, &original).unwrap();
        atomic_json(&sibling(&temp.path, ".backup"), &original).unwrap();
        let reset = original.cleared(kind).unwrap();
        let intent = ResetIntent {
            snapshot: reset.clone(),
            kind,
        };
        atomic_json(&sibling(&temp.path, ".reset"), &intent).unwrap();
        if phase >= 1 {
            atomic_json(&sibling(&temp.path, ".backup"), &reset).unwrap();
        }
        if phase >= 2 {
            atomic_json(&temp.path, &reset).unwrap();
        }
        fs::write(sibling(&temp.path, ".corrupt-old"), b"old private data").unwrap();
        fs::write(sibling(&temp.path, ".backup.tmp-abandoned"), b"incomplete").unwrap();
        let opened = temp.open();
        assert_eq!(opened.snapshot, Some(reset.clone()));
        assert!(opened.warning.is_some());
        assert_eq!(checkpoint(&temp.path), reset);
        assert_eq!(checkpoint(&sibling(&temp.path, ".backup")), reset);
        assert!(!sibling(&temp.path, ".reset").exists());
        assert_eq!(
            sibling(&temp.path, ".corrupt-old").exists(),
            kind == ResetKind::Current
        );
        assert_eq!(
            sibling(&temp.path, ".backup.tmp-abandoned").exists(),
            kind == ResetKind::Current
        );
        drop(opened);
        let reopened = temp.open();
        assert_eq!(reopened.snapshot, Some(reset));
        assert!(reopened.warning.is_none());
    }
}

#[test]
fn interrupted_ordinary_write_never_becomes_a_checkpoint() {
    let temp = Temp::new();
    atomic_json(&temp.path, &snapshot(true)).unwrap();
    fs::write(sibling(&temp.path, ".tmp-interrupted"), b"{half written").unwrap();
    let opened = temp.open();
    assert_eq!(opened.snapshot, Some(snapshot(true)));
    assert!(opened.warning.is_none());
    assert!(opened.store.save(1, &snapshot(false)).unwrap().written);
    assert_eq!(checkpoint(&temp.path), snapshot(false));
    assert_eq!(checkpoint(&sibling(&temp.path, ".backup")), snapshot(true));
}

#[test]
fn failed_checkpoint_after_reset_intent_stays_authoritative_and_retries() {
    let temp = Temp::new();
    let opened = temp.open();
    let original = configured_snapshot();
    opened.store.save(1, &original).unwrap();
    let backup = sibling(&temp.path, ".backup");
    // An actual filesystem rename failure, not a mocked writer.
    fs::create_dir(&backup).unwrap();
    let reset = original.cleared(ResetKind::All).unwrap();
    assert!(
        opened
            .store
            .reset_with_kind(2, &reset, ResetKind::All)
            .is_err()
    );
    assert!(opened.store.reset_is_authoritative(reset.reset_epoch));
    assert!(sibling(&temp.path, ".reset").is_file());
    assert_eq!(checkpoint(&temp.path), original);
    assert!(!opened.store.save(100, &original).unwrap().written);
    fs::remove_dir(&backup).unwrap();
    assert!(
        opened
            .store
            .reset_with_kind(2, &reset, ResetKind::All)
            .unwrap()
            .written
    );
    assert_eq!(checkpoint(&temp.path), reset);
    assert_eq!(checkpoint(&backup), reset);
    assert!(!sibling(&temp.path, ".reset").exists());
}

#[test]
fn failed_reset_checkpoint_replays_on_restart_instead_of_old_main() {
    let temp = Temp::new();
    let opened = temp.open();
    let original = snapshot(true);
    opened.store.save(1, &original).unwrap();
    let backup = sibling(&temp.path, ".backup");
    fs::create_dir(&backup).unwrap();
    let reset = original.cleared(ResetKind::All).unwrap();
    assert!(
        opened
            .store
            .reset_with_kind(2, &reset, ResetKind::All)
            .is_err()
    );
    drop(opened);
    let interrupted = temp.open();
    assert_eq!(interrupted.snapshot, Some(reset.clone()));
    assert!(interrupted.warning.is_some());
    assert!(interrupted.store.reset_is_authoritative(reset.reset_epoch));
    fs::remove_dir(&backup).unwrap();
    // Saves of the already-cleared state can also complete startup replay.
    assert!(interrupted.store.save(1, &reset).unwrap().written);
    assert_eq!(checkpoint(&temp.path), reset);
    assert!(!sibling(&temp.path, ".reset").exists());
}

#[test]
fn all_reset_deletes_owned_recovery_artifacts_but_keeps_lock_and_neighbors() {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    let temp = Temp::new();
    let opened = temp.open();
    opened.store.save(1, &snapshot(true)).unwrap();
    let lock = sibling(&temp.path, ".lock");
    #[cfg(unix)]
    let lock_inode = fs::metadata(&lock).unwrap().ino();
    for suffix in [
        ".corrupt-old",
        ".backup.corrupt-old",
        ".tmp-old",
        ".backup.tmp-old",
        ".reset.tmp-old",
    ] {
        fs::write(sibling(&temp.path, suffix), b"private old workspace").unwrap();
    }
    let neighbor = temp.directory.join("other-program.json");
    fs::write(&neighbor, b"unrelated").unwrap();
    let reset = snapshot(true).cleared(ResetKind::All).unwrap();
    opened
        .store
        .reset_with_kind(2, &reset, ResetKind::All)
        .unwrap();
    #[cfg(unix)]
    assert_eq!(fs::metadata(&lock).unwrap().ino(), lock_inode);
    assert!(lock.is_file());
    assert_eq!(fs::read(&neighbor).unwrap(), b"unrelated");
    assert_eq!(names(&temp.directory).len(), 4);
    assert!(Store::open_at(temp.path.clone()).is_err());
    // A later corrupt primary can recover only the cleared backup.
    drop(opened);
    fs::write(&temp.path, b"broken").unwrap();
    assert_eq!(temp.open().snapshot, Some(reset));
}

#[test]
fn invalid_authoritative_marker_refuses_older_state_and_preserves_bytes() {
    let temp = Temp::new();
    atomic_json(&temp.path, &snapshot(true)).unwrap();
    let intent = sibling(&temp.path, ".reset");
    fs::write(&intent, b"partial intent").unwrap();
    assert!(Store::open_at(temp.path.clone()).is_err());
    assert_eq!(checkpoint(&temp.path), snapshot(true));
    assert_eq!(fs::read(&intent).unwrap(), b"partial intent");
}

#[test]
fn future_reset_intent_is_not_bypassed_by_an_old_main() {
    let temp = Temp::new();
    atomic_json(&temp.path, &snapshot(true)).unwrap();
    let intent = sibling(&temp.path, ".reset");
    let mut value = serde_json::to_value(ResetIntent {
        snapshot: snapshot(true).cleared(ResetKind::All).unwrap(),
        kind: ResetKind::All,
    })
    .unwrap();
    value["snapshot"]["schema_version"] = serde_json::json!(u64::from(SCHEMA_VERSION) + 1);
    let bytes = serde_json::to_vec(&value).unwrap();
    fs::write(&intent, &bytes).unwrap();
    let opened = temp.open();
    assert!(opened.snapshot.is_none());
    assert!(!opened.writable);
    assert!(opened.store.save(1, &Snapshot::default()).is_err());
    let partial = Snapshot::default().cleared(ResetKind::Current).unwrap();
    assert!(opened.store.reset(2, &partial).is_err());
    assert_eq!(fs::read(&intent).unwrap(), bytes);
    let reset = Snapshot::default().cleared(ResetKind::All).unwrap();
    opened
        .store
        .reset_with_kind(2, &reset, ResetKind::All)
        .unwrap();
    assert_eq!(checkpoint(&temp.path), reset);
    assert!(!intent.exists());
}

#[test]
fn ordinary_saves_do_not_destroy_new_unknown_or_unreadable_files() {
    for suffix in ["", ".backup"] {
        let temp = Temp::new();
        let opened = temp.open();
        let target = sibling(&temp.path, suffix);
        fs::write(&target, b"externally replaced invalid data").unwrap();
        assert!(opened.store.save(1, &snapshot(true)).is_err());
        assert_eq!(
            fs::read(&target).unwrap(),
            b"externally replaced invalid data"
        );
        fs::remove_file(&target).unwrap();
        fs::create_dir(&target).unwrap();
        assert!(opened.store.save(1, &snapshot(true)).is_err());
        assert!(target.is_dir());
        fs::remove_dir(&target).unwrap();
        assert!(opened.store.save(1, &snapshot(true)).unwrap().written);
    }
}

#[test]
fn write_failures_fence_old_revisions_and_allow_same_revision_retry() {
    let temp = Temp::new();
    let opened = temp.open();
    opened.store.save(1, &snapshot(true)).unwrap();
    let backup = sibling(&temp.path, ".backup");
    fs::create_dir(&backup).unwrap();
    assert!(opened.store.save(3, &snapshot(false)).is_err());
    assert!(!opened.store.save(2, &snapshot(true)).unwrap().written);
    assert_eq!(checkpoint(&temp.path), snapshot(true));
    fs::remove_dir(&backup).unwrap();
    assert!(opened.store.save(3, &snapshot(false)).unwrap().written);
    assert!(!opened.store.save(3, &snapshot(true)).unwrap().written);
    assert_eq!(checkpoint(&temp.path), snapshot(false));
}

#[cfg(unix)]
#[test]
fn permission_denied_write_reports_error_without_replacing_checkpoint() {
    let temp = Temp::new();
    let opened = temp.open();
    opened.store.save(1, &snapshot(true)).unwrap();
    fs::set_permissions(&temp.directory, fs::Permissions::from_mode(0o500)).unwrap();
    let probe = temp.directory.join("permission-probe");
    // Privileged test runners bypass DAC. Deterministic rename/read errors are
    // covered separately; exercise DAC when the OS actually enforces it.
    if fs::write(&probe, b"probe").is_err() {
        assert!(opened.store.save(2, &snapshot(false)).is_err());
        assert_eq!(checkpoint(&temp.path), snapshot(true));
    }
    fs::set_permissions(&temp.directory, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(opened.store.save(2, &snapshot(false)).unwrap().written);
}

#[test]
fn reset_failure_before_intent_does_not_clear_or_claim_authority() {
    let temp = Temp::new();
    let opened = temp.open();
    let original = snapshot(true);
    opened.store.save(1, &original).unwrap();
    let intent_path = sibling(&temp.path, ".reset");
    fs::create_dir(&intent_path).unwrap();
    let reset = original.cleared(ResetKind::All).unwrap();
    assert!(
        opened
            .store
            .reset_with_kind(2, &reset, ResetKind::All)
            .is_err()
    );
    assert!(!opened.store.reset_is_authoritative(reset.reset_epoch));
    assert_eq!(checkpoint(&temp.path), original);
    assert!(opened.store.save(2, &original).unwrap().written);
    fs::remove_dir(&intent_path).unwrap();
    assert!(
        opened
            .store
            .reset_with_kind(3, &reset, ResetKind::All)
            .unwrap()
            .written
    );
}

#[test]
fn older_schema_is_preserved_without_guessing_a_migration() {
    let temp = Temp::new();
    let bytes = br#"{"schema_version":0,"unrecognized_legacy_state":"keep"}"#;
    fs::write(&temp.path, bytes).unwrap();
    let opened = temp.open();
    assert!(opened.snapshot.is_none());
    assert!(opened.warning.is_some());
    assert!(!temp.path.exists());
    let copies = names(&temp.directory);
    let preserved = copies
        .iter()
        .find(|name| name.starts_with("workspace.json.corrupt-"))
        .unwrap();
    assert_eq!(fs::read(temp.directory.join(preserved)).unwrap(), bytes);
}

fn schema_one_bytes(snapshot: &Snapshot) -> Vec<u8> {
    let mut value = serde_json::to_value(snapshot).unwrap();
    assert!(snapshot.chart.profiles.is_empty());
    value["schema_version"] = serde_json::json!(1);
    value["chart"].as_object_mut().unwrap().remove("profiles");
    serde_json::to_vec_pretty(&value).unwrap()
}

#[test]
fn schema_one_primary_and_backup_migrate_without_losing_configuration() {
    for from_backup in [false, true] {
        let temp = Temp::new();
        let mut expected = configured_snapshot();
        expected.chart.profiles.clear();
        expected.reset_epoch = 17;
        expected.chart.market.symbol = "Case-sensitive_native".into();
        expected.recent_markets = vec![expected.chart.market.clone()];
        expected.favorite_markets = expected.recent_markets.clone();
        expected.chart.view.live = false;
        expected.chart.view.left = Some(1_704_067_200.0);
        expected.chart.price_height = 321.0;
        let bytes = schema_one_bytes(&expected);
        let source = if from_backup {
            sibling(&temp.path, ".backup")
        } else {
            temp.path.clone()
        };
        fs::write(&source, &bytes).unwrap();
        let opened = temp.open();
        assert!(opened.writable);
        assert_eq!(opened.snapshot.as_ref(), Some(&expected));
        assert!(opened.snapshot.as_ref().unwrap().chart.profiles.is_empty());
        if !from_backup {
            // Loading never rewrites a valid primary just to upgrade its version.
            assert_eq!(fs::read(&source).unwrap(), bytes);
        }
        assert!(opened.store.save(1, &expected).unwrap().written);
        let encoded: serde_json::Value =
            serde_json::from_slice(&fs::read(&temp.path).unwrap()).unwrap();
        assert_eq!(encoded["schema_version"], serde_json::json!(SCHEMA_VERSION));
        assert_eq!(encoded["chart"]["profiles"], serde_json::json!([]));
        drop(opened);
        let reopened = temp.open();
        assert_eq!(reopened.snapshot, Some(expected));
    }
}

#[test]
fn invalid_schema_one_is_quarantined_before_valid_backup_is_restored() {
    let temp = Temp::new();
    let expected = snapshot(true);
    fs::write(sibling(&temp.path, ".backup"), schema_one_bytes(&expected)).unwrap();
    let mut invalid: serde_json::Value =
        serde_json::from_slice(&schema_one_bytes(&expected)).unwrap();
    invalid["chart"]["price_height"] = serde_json::json!(-1);
    let bytes = serde_json::to_vec(&invalid).unwrap();
    fs::write(&temp.path, &bytes).unwrap();
    let opened = temp.open();
    assert_eq!(opened.snapshot, Some(expected.clone()));
    assert_eq!(checkpoint(&temp.path), expected);
    let copies = names(&temp.directory);
    let preserved = copies
        .iter()
        .find(|name| name.starts_with("workspace.json.corrupt-"))
        .unwrap();
    assert_eq!(fs::read(temp.directory.join(preserved)).unwrap(), bytes);
}

#[test]
fn future_checkpoints_protect_schema_one_companion_bytes_from_migration_writes() {
    for future_primary in [false, true] {
        let temp = Temp::new();
        let expected = snapshot(true);
        let legacy = schema_one_bytes(&expected);
        let future = br#"{"schema_version":999,"future":{"profiles":["preserve"]}}"#;
        let backup = sibling(&temp.path, ".backup");
        fs::write(
            &temp.path,
            if future_primary {
                future.as_slice()
            } else {
                &legacy
            },
        )
        .unwrap();
        fs::write(
            &backup,
            if future_primary {
                &legacy
            } else {
                future.as_slice()
            },
        )
        .unwrap();
        let before_main = fs::read(&temp.path).unwrap();
        let before_backup = fs::read(&backup).unwrap();
        let opened = temp.open();
        assert!(!opened.writable);
        assert_eq!(opened.snapshot, (!future_primary).then_some(expected));
        assert!(opened.store.save(1, &snapshot(false)).is_err());
        assert_eq!(fs::read(&temp.path).unwrap(), before_main);
        assert_eq!(fs::read(&backup).unwrap(), before_backup);
    }
}

#[test]
fn schema_one_reset_intent_migrates_before_replaying_authoritative_epoch() {
    let temp = Temp::new();
    let mut original = configured_snapshot();
    original.chart.profiles.clear();
    atomic_json(&temp.path, &original).unwrap();
    let expected = original.cleared(ResetKind::Current).unwrap();
    let legacy: serde_json::Value = serde_json::from_slice(&schema_one_bytes(&expected)).unwrap();
    let intent = serde_json::json!({ "snapshot": legacy, "kind": "current" });
    fs::write(
        sibling(&temp.path, ".reset"),
        serde_json::to_vec(&intent).unwrap(),
    )
    .unwrap();
    let opened = temp.open();
    assert_eq!(opened.snapshot, Some(expected.clone()));
    assert!(opened.store.reset_is_authoritative(expected.reset_epoch));
    assert!(!opened.store.save(100, &original).unwrap().written);
    assert_eq!(checkpoint(&temp.path), expected);
    assert_eq!(checkpoint(&sibling(&temp.path, ".backup")), expected);
    assert!(!sibling(&temp.path, ".reset").exists());
}

#[test]
fn schema_two_persists_only_complete_profile_configuration_and_recovers_last_good_copy() {
    let temp = Temp::new();
    let original = configured_snapshot();
    let opened = temp.open();
    assert!(opened.store.save(1, &original).unwrap().written);
    let mut updated = original.clone();
    updated.chart.profiles[0].spec.ticks_per_row = 11;
    updated.chart.profiles[1].visible = true;
    assert!(opened.store.save(2, &updated).unwrap().written);
    assert_eq!(checkpoint(&temp.path), updated);
    assert_eq!(checkpoint(&sibling(&temp.path, ".backup")), original);
    drop(opened);

    let reopened = temp.open();
    assert_eq!(reopened.snapshot, Some(updated));
    assert!(reopened.warning.is_none());
    drop(reopened);
    fs::write(&temp.path, b"interrupted profile checkpoint").unwrap();
    let recovered = temp.open();
    assert_eq!(recovered.snapshot, Some(original.clone()));
    assert_eq!(checkpoint(&temp.path), original);
    assert!(recovered.warning.is_some());
}

#[test]
fn invalid_schema_two_profiles_recover_the_complete_validated_backup() {
    let invalidations: &[fn(&mut serde_json::Value)] = &[
        |value| {
            value["chart"].as_object_mut().unwrap().remove("profiles");
        },
        |value| value["chart"]["profiles"] = serde_json::Value::Null,
        |value| value["chart"]["profiles"][1]["id"] = value["chart"]["profiles"][0]["id"].clone(),
        |value| value["chart"]["profiles"][0]["spec"]["ticks_per_row"] = serde_json::json!(0),
        |value| {
            value["chart"]["profiles"][0]["spec"]["display"]["width_percent"] =
                serde_json::json!(101)
        },
        |value| value["chart"]["profiles"][2]["instrument"] = serde_json::Value::Null,
        |value| value["chart"]["profiles"][0]["rows"] = serde_json::json!([]),
        |value| {
            value["chart"]["profiles"][0]["spec"]["display"]["coverage"] = serde_json::json!([])
        },
        |value| {
            value["chart"]["profiles"][2]["spec"]["range"]["fixed"]["engine"] =
                serde_json::json!({})
        },
    ];
    for invalidate in invalidations {
        let temp = Temp::new();
        let expected = configured_snapshot();
        atomic_json(&sibling(&temp.path, ".backup"), &expected).unwrap();
        let mut value = serde_json::to_value(&expected).unwrap();
        invalidate(&mut value);
        let bytes = serde_json::to_vec(&value).unwrap();
        fs::write(&temp.path, &bytes).unwrap();
        let opened = temp.open();
        assert!(opened.writable);
        assert_eq!(opened.snapshot, Some(expected.clone()));
        assert_eq!(checkpoint(&temp.path), expected);
        let copies = names(&temp.directory);
        let preserved = copies
            .iter()
            .find(|name| name.starts_with("workspace.json.corrupt-"))
            .unwrap();
        assert_eq!(fs::read(temp.directory.join(preserved)).unwrap(), bytes);
    }
}

#[test]
fn invalid_reset_intents_never_replay_or_restore_pre_reset_profile_configuration() {
    let invalidations: &[fn(&mut serde_json::Value)] = &[
        |value| value["unknown"] = serde_json::json!(true),
        |value| {
            value.as_object_mut().unwrap().remove("kind");
        },
        |value| value["kind"] = serde_json::json!("unknown"),
        |value| value["snapshot"]["chart"]["price_height"] = serde_json::json!(-1),
        |value| value["snapshot"]["chart"]["unexpected"] = serde_json::json!({}),
    ];
    for version in [1, 2, 3, SCHEMA_VERSION] {
        for invalidate in invalidations {
            let temp = Temp::new();
            let original = configured_snapshot();
            atomic_json(&temp.path, &original).unwrap();
            atomic_json(&sibling(&temp.path, ".backup"), &original).unwrap();
            let reset = original.cleared(ResetKind::All).unwrap();
            let saved = if version == 1 {
                serde_json::from_slice(&schema_one_bytes(&reset)).unwrap()
            } else {
                serde_json::to_value(&reset).unwrap()
            };
            let mut intent = serde_json::json!({ "snapshot": saved, "kind": "all" });
            invalidate(&mut intent);
            let bytes = serde_json::to_vec(&intent).unwrap();
            let intent_path = sibling(&temp.path, ".reset");
            fs::write(&intent_path, &bytes).unwrap();
            assert!(Store::open_at(temp.path.clone()).is_err());
            assert_eq!(checkpoint(&temp.path), original);
            assert_eq!(checkpoint(&sibling(&temp.path, ".backup")), original);
            assert_eq!(fs::read(&intent_path).unwrap(), bytes);
        }
    }
}

#[test]
fn future_schema_discovered_during_save_remains_protected_from_partial_reset() {
    for suffix in ["", ".backup"] {
        let temp = Temp::new();
        let opened = temp.open();
        let original = configured_snapshot();
        assert!(opened.store.save(1, &original).unwrap().written);
        let future_path = sibling(&temp.path, suffix);
        let future = br#"{"schema_version":999,"future_profiles":["preserve"]}"#;
        fs::write(&future_path, future).unwrap();
        assert!(opened.store.save(2, &original).is_err());
        assert!(!opened.store.writable());
        let partial = original.cleared(ResetKind::Current).unwrap();
        assert!(opened.store.reset(3, &partial).is_err());
        assert_eq!(fs::read(&future_path).unwrap(), future);
        assert!(!sibling(&temp.path, ".reset").exists());
        let all = original.cleared(ResetKind::All).unwrap();
        assert!(
            opened
                .store
                .reset_with_kind(4, &all, ResetKind::All)
                .unwrap()
                .written
        );
        assert_eq!(checkpoint(&temp.path), all);
        assert_eq!(checkpoint(&sibling(&temp.path, ".backup")), all);
    }
}

#[test]
fn partial_reset_rechecks_future_schemas_that_appeared_after_open() {
    for suffix in ["", ".backup"] {
        let temp = Temp::new();
        let opened = temp.open();
        let original = configured_snapshot();
        assert!(opened.store.save(1, &original).unwrap().written);
        let future_path = sibling(&temp.path, suffix);
        let future = br#"{"schema_version":999,"future_profiles":["preserve"]}"#;
        fs::write(&future_path, future).unwrap();
        let partial = original.cleared(ResetKind::Current).unwrap();
        assert!(opened.store.reset(2, &partial).is_err());
        assert!(!opened.store.writable());
        assert_eq!(fs::read(&future_path).unwrap(), future);
        assert!(!sibling(&temp.path, ".reset").exists());
        assert!(!opened.store.reset_is_authoritative(partial.reset_epoch));
    }
}

#[test]
fn custom_sessions_roundtrip_and_participate_in_authoritative_resets() {
    use crate::workspace::{SavedProfilePeriod, SavedProfileRange};
    use mexc_indicators::profiles::{Period, Range};
    let custom = Period::custom("America/New_York", "18:00", "09:30").unwrap();
    let mut original = configured_snapshot();
    original.chart.profiles[0].spec.range = SavedProfileRange::Session(custom.into());
    let value = serde_json::to_value(&original).unwrap();
    assert_eq!(
        value["chart"]["profiles"][0]["spec"]["range"],
        serde_json::json!({"session":{"custom":{"timezone":"America/New_York","start":"18:00","end":"09:30"}}})
    );
    for (period, expected) in [
        (SavedProfilePeriod::Day, "day"),
        (SavedProfilePeriod::Week, "week"),
        (SavedProfilePeriod::Month, "month"),
    ] {
        assert_eq!(
            serde_json::to_value(period).unwrap(),
            serde_json::json!(expected)
        );
    }
    assert_eq!(Snapshot::from_value(value).unwrap(), original);
    assert_eq!(
        original.chart.profiles[0].to_instance().unwrap().spec.range,
        Range::Session(custom)
    );
    let temp = Temp::new();
    let opened = temp.open();
    opened.store.save(1, &original).unwrap();
    drop(opened);
    let reopened = temp.open();
    assert_eq!(reopened.snapshot, Some(original.clone()));
    for kind in [ResetKind::Current, ResetKind::All] {
        let reset = original.cleared(kind).unwrap();
        assert!(
            !reset
                .chart
                .profiles
                .iter()
                .any(|profile| profile.id == original.chart.profiles[0].id)
        );
        if kind == ResetKind::All {
            assert!(reset.chart.profiles.is_empty());
        }
        let reset_temp = Temp::new();
        let reset_opened = reset_temp.open();
        reset_opened.store.save(1, &original).unwrap();
        assert!(reset_opened.store.reset(2, &reset).unwrap().written);
        assert!(reset_opened.store.reset_is_authoritative(reset.reset_epoch));
        assert!(!reset_opened.store.save(3, &original).unwrap().written);
        assert_eq!(checkpoint(&reset_temp.path), reset);
        assert_eq!(checkpoint(&sibling(&reset_temp.path, ".backup")), reset);
        drop(reset_opened);
        let reset_reopened = reset_temp.open();
        assert_eq!(reset_reopened.snapshot, Some(reset));
    }
}

#[test]
fn invalid_custom_session_dtos_recover_valid_backup_without_serializing_outputs() {
    let custom = serde_json::json!({"session":{"custom":{
        "timezone":"America/New_York","start":"09:30","end":"16:00"
    }}});
    for (field, invalid) in [
        ("timezone", serde_json::json!("Unknown/Zone")),
        ("start", serde_json::json!("9:30")),
        ("end", serde_json::json!("24:00")),
        ("rows", serde_json::json!([])),
    ] {
        let temp = Temp::new();
        let original = configured_snapshot();
        atomic_json(&sibling(&temp.path, ".backup"), &original).unwrap();
        let mut value = serde_json::to_value(&original).unwrap();
        value["chart"]["profiles"][0]["spec"]["range"] = custom.clone();
        value["chart"]["profiles"][0]["spec"]["range"]["session"]["custom"][field] = invalid;
        fs::write(&temp.path, serde_json::to_vec(&value).unwrap()).unwrap();
        let opened = temp.open();
        assert_eq!(opened.snapshot, Some(original));
        assert!(opened.warning.is_some());
    }
}

#[test]
fn unicode_checkpoint_names_survive_replacement_backup_and_reset() {
    let mut temp = Temp::new();
    temp.path = temp.directory.join("workspace-日本語-é.json");
    let opened = temp.open();
    opened.store.save(1, &snapshot(true)).unwrap();
    opened.store.save(2, &snapshot(false)).unwrap();
    assert_eq!(checkpoint(&temp.path), snapshot(false));
    assert_eq!(checkpoint(&sibling(&temp.path, ".backup")), snapshot(true));
    let reset = snapshot(false).cleared(ResetKind::All).unwrap();
    opened
        .store
        .reset_with_kind(3, &reset, ResetKind::All)
        .unwrap();
    assert_eq!(checkpoint(&temp.path), reset);
    assert_eq!(checkpoint(&sibling(&temp.path, ".backup")), reset);
    assert!(!sibling(&temp.path, ".reset").exists());
    assert!(Store::open_at(temp.path.clone()).is_err());
    drop(opened);
    assert_eq!(temp.open().snapshot, Some(reset));
}

#[test]
fn failed_atomic_replacement_preserves_destination_and_removes_temporary_file() {
    let temp = Temp::new();
    fs::create_dir(&temp.path).unwrap();
    let neighbor = temp.path.join("keep");
    fs::write(&neighbor, b"destination contents").unwrap();
    let failure = atomic_bytes(&temp.path, b"new checkpoint").unwrap_err();
    assert!(!failure.renamed);
    assert_eq!(fs::read(&neighbor).unwrap(), b"destination contents");
    assert_eq!(names(&temp.directory), ["workspace.json"]);
}

#[cfg(windows)]
#[test]
fn long_windows_checkpoint_paths_use_native_replacement() {
    let mut temp = Temp::new();
    let mut directory = temp.directory.clone();
    for _ in 0..8 {
        directory = directory.join("long-workspace-parent-directory");
    }
    temp.path = directory.join("workspace.json");
    let opened = temp.open();
    opened.store.save(1, &snapshot(true)).unwrap();
    opened.store.save(2, &snapshot(false)).unwrap();
    drop(opened);
    assert_eq!(temp.open().snapshot, Some(snapshot(false)));
}

#[test]
fn native_lock_excludes_another_process() {
    const PROBE_PATH: &str = "MEXC_CHART_WORKSPACE_LOCK_TEST_PATH";
    if let Some(path) = env::var_os(PROBE_PATH) {
        assert!(Store::open_at(PathBuf::from(path)).is_err());
        return;
    }
    let temp = Temp::new();
    let opened = temp.open();
    let status = std::process::Command::new(env::current_exe().unwrap())
        .args([
            "--exact",
            "workspace_store::tests::native_lock_excludes_another_process",
            "--nocapture",
        ])
        .env(PROBE_PATH, &temp.path)
        .status()
        .unwrap();
    assert!(status.success());
    drop(opened);
    assert!(temp.open().writable);
}

#[test]
fn retired_reset_intents_cannot_override_later_checkpoints() {
    let temp = Temp::new();
    let cleared = snapshot(false).cleared(ResetKind::All).unwrap();
    atomic_json(
        &sibling(&temp.path, ".reset.tmp-completed-interrupted-unlink"),
        &ResetIntent {
            snapshot: cleared.clone(),
            kind: ResetKind::All,
        },
    )
    .unwrap();
    let later = Snapshot {
        magnet: true,
        ..cleared
    };
    atomic_json(&temp.path, &later).unwrap();
    let opened = temp.open();
    assert_eq!(opened.snapshot, Some(later.clone()));
    assert_eq!(checkpoint(&temp.path), later);
    let reset = later.cleared(ResetKind::All).unwrap();
    opened
        .store
        .reset_with_kind(1, &reset, ResetKind::All)
        .unwrap();
    assert!(!sibling(&temp.path, ".reset.tmp-completed-interrupted-unlink").exists());
}
