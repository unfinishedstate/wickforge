use super::*;
use crate::{
    drawing_types::{Anchor, Command, Shape, Style},
    feeds,
};
use mexc_indicators::IndicatorSpec;
use mexc_model::{Candle, MarketEvent};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    directory: PathBuf,
    path: PathBuf,
    desktop: Desktop,
}
impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "mexc-workspace-runtime-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("workspace.json");
        let opened = Store::open_at(path.clone()).unwrap();
        let (mut desktop, startup) = Desktop::new();
        drop(startup);
        desktop.workspace.store = Some(Arc::new(opened.store));
        desktop.workspace.enabled = true;
        Self {
            directory,
            path,
            desktop,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.desktop.workspace.store.take();
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[test]
fn committed_reset_rejects_old_native_callbacks_and_stale_save_failures() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.desktop;
    let _ = app.update(Message::Action(Action::AddIndicator(IndicatorSpec::Rsi {
        period: 14,
    })));
    app.commit_drawing(Command::Create {
        shape: Shape::Horizontal(Anchor {
            utc_ms: 1_000,
            price: 12.0,
        }),
        style: Style::default(),
    });
    let old = Message::MarketData(feeds::Event {
        scope: Arc::new(feeds::Scope {
            market: app.chart.market.clone(),
            generation: app.market_generation,
            flow_generation: app.flow_generation,
            cvd: false,
        }),
        events: vec![MarketEvent::Candle {
            candle: Candle {
                time: 300,
                open: 1.0,
                high: 1.0,
                low: 1.0,
                close: 1.0,
                volume: 1.0,
                flow: None,
            },
        }]
        .into_boxed_slice(),
    });
    let cleared = Arc::new(
        app.workspace_snapshot()
            .cleared(ResetKind::Current)
            .unwrap(),
    );
    let result = app.workspace.store.as_ref().unwrap().reset(10, &cleared);
    let _ = app.workspace_reset_done(10, ResetKind::Current, cleared, result);
    let _ = app.update(old);
    let _ = app.workspace_saved(2, Err("old writer callback".into()));
    assert!(app.chart.candles().is_empty());
    assert!(app.chart.indicators.instances.is_empty());
    assert!(app.drawings.objects().is_empty());
    assert!(app.workspace.error.is_none());
    let saved: Snapshot = serde_json::from_slice(&fs::read(&fixture.path).unwrap()).unwrap();
    assert!(saved.chart.indicators.is_empty());
    assert!(saved.books.iter().all(|book| book.drawings.is_empty()));
    assert_eq!(saved.reset_epoch, 1);
}

#[test]
fn failed_close_save_keeps_changes_and_retry_commits_them() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.desktop;
    let _ = app.update(Message::Action(Action::AddIndicator(IndicatorSpec::Ema {
        period: 50,
    })));
    assert!(app.workspace.dirty_at.is_some());
    let snapshot = app.workspace_snapshot();
    let revision = app.workspace.revision;
    // An existing directory at the checkpoint is a genuine I/O failure even as root.
    fs::create_dir(&fixture.path).unwrap();
    app.workspace.closing = Some(iced::window::Id::unique());
    let failed = app
        .workspace
        .store
        .as_ref()
        .unwrap()
        .save(revision, &snapshot);
    assert!(failed.is_err());
    let _ = app.workspace_saved(revision, failed);
    assert!(app.workspace.controls.is_open());
    assert!(app.workspace.error.is_some());
    assert!(app.workspace.dirty_at.is_some());
    assert_eq!(
        app.chart.indicators.instances[0].spec,
        IndicatorSpec::Ema { period: 50 }
    );
    fs::remove_dir(&fixture.path).unwrap();
    let _ = app.workspace_event(Event::RetrySave);
    let revision = app.workspace.revision;
    let snapshot = app.workspace_snapshot();
    let result = app
        .workspace
        .store
        .as_ref()
        .unwrap()
        .save(revision, &snapshot);
    let _ = app.workspace_saved(revision, result);
    assert!(app.workspace.error.is_none());
    assert!(app.workspace.dirty_at.is_none());
    let saved: Snapshot = serde_json::from_slice(&fs::read(&fixture.path).unwrap()).unwrap();
    assert_eq!(
        saved.chart.indicators[0].spec.to_spec().unwrap(),
        IndicatorSpec::Ema { period: 50 }
    );
}

#[test]
fn confirmation_cancel_and_live_ticks_do_not_commit_drafts_or_change_workspace() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.desktop;
    let _ = app.update(Message::Action(Action::AddIndicator(IndicatorSpec::Macd {
        fast: 12,
        slow: 26,
        signal: 9,
    })));
    let snapshot = app.workspace_snapshot();
    let revision = app.workspace.revision;
    let _ = app.workspace_event(Event::Open(ResetKind::All));
    let _ = app.update(Message::Action(Action::RemoveIndicator(
        snapshot.chart.indicators[0].id,
    )));
    let _ = app.update(Message::Tick);
    let _ = app.workspace_event(Event::Cancel);
    assert_eq!(app.workspace_snapshot(), snapshot);
    assert_eq!(app.workspace.revision, revision);
    assert!(!app.workspace.controls.is_open());
    assert!(!fixture.path.exists());
}

#[test]
fn partial_clear_cannot_destroy_unknown_newer_workspace_books() {
    let mut fixture = Fixture::new();
    fixture.desktop.workspace.store.take();
    let future = br#"{"schema_version":999,"unknown_workspace":{"books":["keep"]}}"#;
    fs::write(&fixture.path, future).unwrap();
    let opened = Store::open_at(fixture.path.clone()).unwrap();
    fixture.desktop.workspace.enabled = opened.writable;
    fixture.desktop.workspace.store = Some(Arc::new(opened.store));
    let _ = fixture
        .desktop
        .workspace_event(Event::Open(ResetKind::Current));
    let _ = fixture.desktop.workspace_event(Event::Confirm);
    assert!(!fixture.desktop.workspace.reset_pending);
    assert!(fixture.desktop.workspace.error.is_some());
    assert_eq!(fs::read(&fixture.path).unwrap(), future);
}

#[test]
fn protected_workspace_only_prompts_on_close_after_configuration_edits() {
    let mut fixture = Fixture::new();
    fixture.desktop.workspace.store.take();
    let future = br#"{"schema_version":999,"future_workspace":{"books":["keep"]}}"#;
    fs::write(&fixture.path, future).unwrap();
    let opened = Store::open_at(fixture.path.clone()).unwrap();
    let app = &mut fixture.desktop;
    app.workspace.enabled = opened.writable;
    app.workspace.track_changes = true;
    app.workspace.store = Some(Arc::new(opened.store));
    let _ = app.workspace_event(Event::Open(ResetKind::Current));
    let _ = app.workspace_event(Event::Confirm);
    let _ = app.workspace_event(Event::Cancel);
    assert!(app.workspace.error.is_some());
    app.workspace.closing = Some(iced::window::Id::unique());
    let _ = app.poll_workspace_save(true);
    assert!(!app.workspace.controls.is_open());
    assert!(app.workspace.dirty_at.is_none());

    app.workspace.closing = None;
    let _ = app.update(Message::Action(Action::AddIndicator(IndicatorSpec::Ema {
        period: 50,
    })));
    assert!(app.workspace.dirty_at.is_some());
    app.workspace.closing = Some(iced::window::Id::unique());
    let _ = app.poll_workspace_save(true);
    assert!(app.workspace.controls.is_open());
    assert_eq!(
        app.workspace_snapshot().chart.indicators[0]
            .spec
            .to_spec()
            .unwrap(),
        IndicatorSpec::Ema { period: 50 }
    );
    assert_eq!(fs::read(&fixture.path).unwrap(), future);
}
