use super::*;

const DAY: i64 = 86_400_000;

fn decimal(value: &str) -> Decimal {
    value.parse().unwrap()
}
fn bar(time_ms: i64, low: f64, high: f64) -> Candle {
    Candle {
        time: time_ms / 1000,
        open: low,
        high,
        low,
        close: high,
        volume: 999_999.0,
        flow: None,
    }
}
fn execution(id: &str, time_ms: i64, price: &str, quantity: &str) -> Execution {
    Execution {
        id: id.into(),
        time_ms,
        price: decimal(price),
        quantity: decimal(quantity),
    }
}
fn coverage(start_ms: i64, end_ms: i64) -> Coverage {
    Coverage {
        start_ms,
        end_ms,
        complete: true,
        reason: None,
    }
}
fn fixed(kind: Kind, end: i64) -> Spec {
    Spec {
        range: Range::Fixed {
            start_ms: 0,
            end_ms: end,
        },
        block_minutes: 1,
        ..Spec::default_for(kind)
    }
}
fn calculate(engine: &mut Engine, spec: &Spec, now: i64) -> Vec<Snapshot> {
    engine.calculate(spec, Decimal::ONE, (0, DAY), now).unwrap()
}
fn one(engine: &mut Engine, spec: &Spec, now: i64) -> Snapshot {
    calculate(engine, spec, now).remove(0)
}
fn structure() -> Vec<Candle> {
    // Blocks 0/1 leave a bounded single-print run [2,4], owned by 0.
    vec![
        bar(0, 1.0, 5.0),
        bar(MINUTE, 1.0, 1.0),
        bar(2 * MINUTE, 5.0, 5.0),
    ]
}

#[test]
fn defaults_and_all_validation_constraints_are_public() {
    let tpo = Spec::default_for(Kind::Tpo);
    assert_eq!(tpo.range, Range::Session(Period::Day));
    assert_eq!(tpo.block_minutes, 30);
    assert_eq!(tpo.value_area_percent, 70);
    assert!(tpo.estimate_missing && Spec::default_for(Kind::Volume).estimate_missing);
    assert!(tpo.display.single_prints && tpo.display.initial_balance && tpo.display.poor_extremes);
    assert!(!Spec::default_for(Kind::Volume).display.single_prints);
    assert!(tpo.validate().is_ok());
    for invalid in [
        Spec {
            ticks_per_row: 0,
            ..tpo.clone()
        },
        Spec {
            block_minutes: 0,
            ..tpo.clone()
        },
        Spec {
            block_minutes: 44_641,
            ..tpo.clone()
        },
        Spec {
            value_area_percent: 0,
            ..tpo.clone()
        },
        Spec {
            value_area_percent: 101,
            ..tpo.clone()
        },
        Spec {
            display: Display {
                width_percent: 0,
                ..tpo.display.clone()
            },
            ..tpo.clone()
        },
        Spec {
            range: Range::Fixed {
                start_ms: 1,
                end_ms: MINUTE,
            },
            ..tpo.clone()
        },
        Spec {
            range: Range::Fixed {
                start_ms: MINUTE,
                end_ms: MINUTE,
            },
            ..tpo.clone()
        },
    ] {
        assert!(invalid.validate().is_err(), "{invalid:?}");
    }
}

#[test]
fn utc_periods_use_monday_weeks_and_real_calendar_months() {
    let stamp = |year, month, day| {
        Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis()
    };
    let leap = stamp(2024, 2, 29);
    assert_eq!(
        period_bounds(Period::Month, leap),
        (stamp(2024, 2, 1), stamp(2024, 3, 1))
    );
    assert_eq!(
        period_bounds(Period::Week, stamp(2024, 3, 3)),
        (stamp(2024, 2, 26), stamp(2024, 3, 4))
    );
    assert_eq!(
        period_bounds(Period::Month, stamp(2024, 12, 31)),
        (stamp(2024, 12, 1), stamp(2025, 1, 1))
    );
    assert_eq!(period_bounds(Period::Day, -1), (-DAY, 0));
    assert_eq!(period_bounds(Period::Day, i64::MAX), (i64::MAX, i64::MAX));
}

#[test]
fn tpo_unions_each_block_without_bridging_disjoint_candles() {
    let mut engine = Engine::default();
    engine.upsert_bars(&[
        bar(0, 1.0, 2.0),
        bar(MINUTE, 5.0, 6.0),
        bar(2 * MINUTE, 1.0, 1.0),
    ]);
    let mut spec = fixed(Kind::Tpo, 3 * MINUTE);
    spec.block_minutes = 2;
    let snapshot = one(&mut engine, &spec, 3 * MINUTE);
    assert_eq!(
        snapshot.rows.iter().map(|r| r.index).collect::<Vec<_>>(),
        [1, 2, 5, 6]
    );
    assert_eq!(snapshot.rows[0].blocks, [0, 1]);
    assert_eq!(snapshot.rows[0].weight, 2.0);
    assert_eq!(snapshot.rows[1].weight, 1.0);
    assert_eq!(snapshot.poc, Some(1.0));
}

#[test]
fn forming_bar_replacement_removes_old_block_contributions() {
    let mut engine = Engine::default();
    let spec = fixed(Kind::Tpo, 2 * MINUTE);
    engine.upsert_bars(&[bar(0, 1.0, 4.0)]);
    assert_eq!(one(&mut engine, &spec, MINUTE).rows.len(), 4);
    engine.upsert_bars(&[bar(0, 2.0, 2.0)]);
    let corrected = one(&mut engine, &spec, MINUTE);
    assert_eq!(
        corrected.rows.iter().map(|r| r.index).collect::<Vec<_>>(),
        [2]
    );
    assert_eq!(corrected.rows[0].weight, 1.0);
    assert_eq!(one(&mut engine, &spec, MINUTE), corrected);
}

#[test]
fn exact_only_volume_uses_native_decimal_quantity_not_bar_volume_or_notional() {
    let mut engine = Engine::default();
    engine.upsert_bars(&[bar(0, 1.0, 1000.0)]);
    engine
        .ingest_executions(&[
            execution("raw:a", 0, "10.01", "0.125"),
            execution("raw:b", 1, "10.99", "0.375"),
            execution("raw:c", 2, "11", "0.25"),
        ])
        .unwrap();
    let mut spec = fixed(Kind::Volume, MINUTE);
    spec.estimate_missing = false;
    let snapshot = one(&mut engine, &spec, MINUTE);
    assert_eq!(snapshot.rows.len(), 2);
    assert_eq!(snapshot.rows[0].weight, 0.5);
    assert_eq!(snapshot.rows[1].weight, 0.25);
    assert_eq!(snapshot.poc, Some(10.0));
    assert!(snapshot.rows.iter().all(|r| r.blocks.is_empty()));
    assert!(snapshot.initial_balance.is_none());
    assert_eq!(snapshot.volume_method, VolumeMethod::Exact);
}

#[test]
fn native_identity_dedup_corrections_and_authoritative_replacement_replay() {
    let mut engine = Engine::default();
    let spec = fixed(Kind::Volume, MINUTE);
    let original = execution("raw:a", 1, "10", "2");
    engine
        .ingest_executions(&[original.clone(), original])
        .unwrap();
    assert_eq!(one(&mut engine, &spec, MINUTE).rows[0].weight, 2.0);
    engine
        .ingest_executions(&[execution("raw:a", 2, "20", "3")])
        .unwrap();
    let corrected = one(&mut engine, &spec, MINUTE);
    assert_eq!(corrected.rows.len(), 1);
    assert_eq!(
        (corrected.rows[0].index, corrected.rows[0].weight),
        (20, 3.0)
    );
    engine
        .replace_executions(0, MINUTE, &[execution("raw:b", 3, "30", "4")])
        .unwrap();
    let replaced = one(&mut engine, &spec, MINUTE);
    assert_eq!((replaced.rows[0].index, replaced.rows[0].weight), (30, 4.0));
    engine.replace_executions(0, MINUTE, &[]).unwrap();
    assert!(one(&mut engine, &spec, MINUTE).rows.is_empty());
}

#[test]
fn invalid_execution_batches_are_atomic_and_ranges_are_half_open() {
    let mut engine = Engine::default();
    engine
        .ingest_executions(&[execution("raw:a", 1, "1", "1")])
        .unwrap();
    assert!(
        engine
            .ingest_executions(&[execution("raw:b", 2, "2", "1"), execution("", 3, "3", "1")])
            .is_err()
    );
    assert!(
        engine
            .ingest_executions(&[execution("raw:b", 2, "2", "0")])
            .is_err()
    );
    assert!(
        engine
            .replace_executions(0, MINUTE, &[execution("raw:b", MINUTE, "2", "1")])
            .is_err()
    );
    let snapshot = one(&mut engine, &fixed(Kind::Volume, MINUTE), MINUTE);
    assert_eq!(snapshot.rows.len(), 1);
    assert_eq!(snapshot.rows[0].index, 1);
}

#[test]
fn half_open_boundaries_exclude_next_range_and_block_ownership_is_exact() {
    let mut engine = Engine::default();
    engine.upsert_bars(&[
        bar(0, 1.0, 1.0),
        bar(MINUTE, 2.0, 2.0),
        bar(2 * MINUTE, 3.0, 3.0),
    ]);
    let snapshot = one(&mut engine, &fixed(Kind::Tpo, 2 * MINUTE), 3 * MINUTE);
    assert_eq!(snapshot.rows.len(), 2);
    assert_eq!(snapshot.rows[0].blocks, [0]);
    assert_eq!(snapshot.rows[1].blocks, [1]);
    engine
        .ingest_executions(&[
            execution("raw:a", 0, "1", "1"),
            execution("raw:b", MINUTE, "2", "1"),
        ])
        .unwrap();
    assert_eq!(
        one(&mut engine, &fixed(Kind::Volume, MINUTE), MINUTE)
            .rows
            .len(),
        1
    );
}

#[test]
fn explicit_tick_grid_handles_negative_prices_and_lower_central_poc_tick() {
    let mut engine = Engine::default();
    engine
        .ingest_executions(&[
            execution("raw:a", 0, "-0.01", "1"),
            execution("raw:b", 1, "0.39", "3"),
        ])
        .unwrap();
    let spec = Spec {
        ticks_per_row: 4,
        ..fixed(Kind::Volume, MINUTE)
    };
    let snapshot = engine
        .calculate(&spec, decimal("0.1"), (0, MINUTE), MINUTE)
        .unwrap()
        .remove(0);
    assert_eq!(
        (
            snapshot.rows[0].index,
            snapshot.rows[0].low,
            snapshot.rows[0].high
        ),
        (-1, -0.4, -0.1)
    );
    assert_eq!(
        (
            snapshot.rows[1].index,
            snapshot.rows[1].low,
            snapshot.rows[1].high
        ),
        (0, 0.0, 0.3)
    );
    assert_eq!(snapshot.poc, Some(0.1));
    assert_eq!((snapshot.val, snapshot.vah), (Some(0.0), Some(0.3)));
}

#[test]
fn poc_ties_use_midpoint_distance_then_lower_row() {
    let mut engine = Engine::default();
    engine
        .ingest_executions(&[
            execution("raw:a", 0, "0", "2"),
            execution("raw:b", 1, "2", "2"),
            execution("raw:c", 2, "4", "1"),
        ])
        .unwrap();
    assert_eq!(
        one(&mut engine, &fixed(Kind::Volume, MINUTE), MINUTE).poc,
        Some(2.0)
    );
    engine
        .replace_executions(
            0,
            MINUTE,
            &[
                execution("raw:a", 0, "1", "2"),
                execution("raw:b", 1, "2", "2"),
            ],
        )
        .unwrap();
    assert_eq!(
        one(&mut engine, &fixed(Kind::Volume, MINUTE), MINUTE).poc,
        Some(1.0)
    );
}

#[test]
fn value_area_expands_contiguous_whole_rows_and_chooses_lower_on_ties() {
    let mut engine = Engine::default();
    engine
        .ingest_executions(&[
            execution("raw:a", 0, "0", "1"),
            execution("raw:b", 1, "1", "2"),
            execution("raw:c", 2, "2", "1"),
        ])
        .unwrap();
    let mut spec = fixed(Kind::Volume, MINUTE);
    let snapshot = one(&mut engine, &spec, MINUTE);
    assert_eq!((snapshot.val, snapshot.vah), (Some(0.0), Some(1.0)));
    spec.value_area_percent = 100;
    assert_eq!(one(&mut engine, &spec, MINUTE).vah, Some(2.0));
    engine
        .replace_executions(
            0,
            MINUTE,
            &[
                execution("raw:a", 0, "0", "2"),
                execution("raw:b", 1, "3", "1"),
            ],
        )
        .unwrap();
    let gap = one(&mut engine, &spec, MINUTE);
    assert_eq!((gap.val, gap.vah), (Some(0.0), Some(3.0)));
    assert_eq!(gap.rows.len(), 2);
}

#[test]
fn tiny_native_quantities_do_not_round_value_area_target_down_to_zero() {
    let mut engine = Engine::default();
    let tiny = "0.0000000000000000000000000001";
    engine
        .ingest_executions(&[
            execution("raw:a", 0, "1", tiny),
            execution("raw:b", 1, "2", tiny),
        ])
        .unwrap();
    let snapshot = one(&mut engine, &fixed(Kind::Volume, MINUTE), MINUTE);
    assert_eq!((snapshot.val, snapshot.vah), (Some(1.0), Some(2.0)));
}

#[test]
fn empty_data_has_no_levels_and_requires_explicit_coverage_proof() {
    let mut engine = Engine::default();
    let spec = fixed(Kind::Volume, MINUTE);
    let unknown = one(&mut engine, &spec, MINUTE);
    assert!(!unknown.complete);
    assert!(unknown.reason.is_some());
    assert!(unknown.poc.is_none() && unknown.val.is_none() && unknown.vah.is_none());
    engine.set_execution_coverage(vec![coverage(0, MINUTE)]);
    let empty = one(&mut engine, &spec, MINUTE);
    assert!(empty.complete && empty.rows.is_empty());
    assert!(empty.poc.is_none());
}

#[test]
fn closure_and_coverage_are_independent_and_gaps_override_overlapping_proof() {
    let mut engine = Engine::default();
    let spec = fixed(Kind::Tpo, 3 * MINUTE);
    engine.upsert_bars(&structure());
    engine.set_bar_coverage(vec![coverage(0, 3 * MINUTE)]);
    let open = one(&mut engine, &spec, MINUTE);
    assert!(!open.complete);
    assert!(open.reason.as_deref().unwrap().contains("open"));
    engine.set_bar_coverage(vec![
        coverage(0, 3 * MINUTE),
        Coverage {
            start_ms: MINUTE,
            end_ms: 2 * MINUTE,
            complete: false,
            reason: Some("lost minute".into()),
        },
    ]);
    let partial = one(&mut engine, &spec, 3 * MINUTE);
    assert!(!partial.complete);
    assert!(partial.reason.as_deref().unwrap().contains("lost minute"));
    assert!(
        partial
            .single_prints
            .iter()
            .all(|zone| zone.touch == Touch::Unknown)
    );
    engine.set_bar_coverage(vec![coverage(0, MINUTE), coverage(MINUTE, 3 * MINUTE)]);
    assert!(one(&mut engine, &spec, 3 * MINUTE).complete);
}

#[test]
fn execution_invalidation_splits_complete_coverage_and_recovery_replaces_it() {
    let mut engine = Engine::default();
    let spec = fixed(Kind::Volume, 3 * MINUTE);
    engine.set_execution_coverage(vec![coverage(0, 3 * MINUTE)]);
    assert!(one(&mut engine, &spec, 3 * MINUTE).complete);
    engine.invalidate_executions(MINUTE, 2 * MINUTE, "disconnected".into());
    let partial = one(&mut engine, &spec, 3 * MINUTE);
    assert!(!partial.complete);
    assert!(partial.reason.unwrap().contains("disconnected"));
    engine.set_execution_coverage(vec![coverage(0, 3 * MINUTE)]);
    assert!(one(&mut engine, &spec, 3 * MINUTE).complete);
}

#[test]
fn future_source_data_is_not_used_before_the_observation_horizon() {
    let mut engine = Engine::default();
    engine.upsert_bars(&[bar(0, 1.0, 1.0), bar(MINUTE, 2.0, 2.0)]);
    let spec = fixed(Kind::Tpo, 2 * MINUTE);
    assert_eq!(one(&mut engine, &spec, MINUTE).rows.len(), 1);
    assert_eq!(one(&mut engine, &spec, MINUTE + 1).rows.len(), 2);
    assert_eq!(one(&mut engine, &spec, MINUTE).rows.len(), 1);
    engine
        .ingest_executions(&[
            execution("raw:a", 999, "1", "1"),
            execution("raw:b", 1000, "2", "1"),
        ])
        .unwrap();
    let mut volume = fixed(Kind::Volume, MINUTE);
    volume.estimate_missing = false;
    assert_eq!(one(&mut engine, &volume, 1000).rows.len(), 1);
    assert_eq!(one(&mut engine, &volume, 1001).rows.len(), 2);
    let estimated = fixed(Kind::Volume, 2 * MINUTE);
    assert_eq!(one(&mut engine, &estimated, MINUTE).rows.len(), 1);
    assert_eq!(one(&mut engine, &estimated, MINUTE + 1).rows.len(), 2);
    assert_eq!(one(&mut engine, &estimated, MINUTE).rows.len(), 1);
}

#[test]
fn sessions_return_visible_periods_and_exactly_the_preceding_reference() {
    let mut engine = Engine::default();
    let spec = Spec::default_for(Kind::Tpo);
    let snapshots = engine
        .calculate(&spec, Decimal::ONE, (DAY, 2 * DAY), 2 * DAY)
        .unwrap();
    assert_eq!(
        snapshots
            .iter()
            .map(|s| (s.start_ms, s.end_ms))
            .collect::<Vec<_>>(),
        [(0, DAY), (DAY, 2 * DAY)]
    );
    assert!(
        engine
            .calculate(&spec, Decimal::ONE, (0, 33 * DAY), 33 * DAY)
            .is_err()
    );
}

#[test]
fn naked_poc_touch_uses_the_whole_row_and_corrections_replay_dependents() {
    let mut engine = Engine::default();
    engine
        .ingest_executions(&[
            execution("raw:origin", 0, "10", "2"),
            execution("raw:later", DAY, "30", "1"),
        ])
        .unwrap();
    engine.set_execution_coverage(vec![coverage(0, 2 * DAY)]);
    let spec = Spec {
        ticks_per_row: 4,
        ..Spec::default_for(Kind::Volume)
    };
    let calculate = |engine: &mut Engine| {
        engine
            .calculate(&spec, Decimal::ONE, (DAY, 2 * DAY), 2 * DAY)
            .unwrap()
    };
    let original = calculate(&mut engine);
    let level = original[1]
        .naked_pocs
        .iter()
        .find(|level| level.origin_ms == 0)
        .unwrap();
    assert_eq!(
        (level.low, level.high, level.price, level.touch),
        (8.0, 11.0, 9.0, Touch::Untouched)
    );
    engine
        .ingest_executions(&[execution("raw:later", DAY, "11", "1")])
        .unwrap();
    assert!(calculate(&mut engine)[1].naked_pocs.is_empty());
    engine
        .replace_executions(
            DAY,
            2 * DAY,
            &[execution("raw:replacement", DAY, "30", "1")],
        )
        .unwrap();
    assert_eq!(
        calculate(&mut engine)[1].naked_pocs[0].touch,
        Touch::Untouched
    );
    engine.set_execution_coverage(vec![coverage(0, DAY)]);
    assert_eq!(
        calculate(&mut engine)[1].naked_pocs[0].touch,
        Touch::Unknown
    );
    engine.set_execution_coverage(vec![coverage(DAY, 2 * DAY)]);
    assert!(calculate(&mut engine)[1].naked_pocs.is_empty());
}

#[test]
fn fixed_and_visible_profiles_never_mint_previous_or_naked_references() {
    let mut engine = Engine::default();
    engine
        .ingest_executions(&[
            execution("raw:old", -DAY, "10", "1"),
            execution("raw:current", 0, "20", "1"),
        ])
        .unwrap();
    engine.set_execution_coverage(vec![coverage(-DAY, DAY)]);
    let mut spec = fixed(Kind::Volume, MINUTE);
    spec.display.naked = true;
    assert!(one(&mut engine, &spec, DAY).naked_pocs.is_empty());
    spec.range = Range::Visible;
    assert!(one(&mut engine, &spec, DAY).naked_pocs.is_empty());
}

#[test]
fn single_prints_are_bounded_row_sets_tails_are_separate_and_ib_uses_two_blocks() {
    let mut engine = Engine::default();
    engine.upsert_bars(&[
        bar(0, 0.0, 6.0),
        bar(MINUTE, 1.0, 1.0),
        bar(2 * MINUTE, 5.0, 5.0),
    ]);
    engine.set_bar_coverage(vec![coverage(0, 3 * MINUTE)]);
    let snapshot = one(&mut engine, &fixed(Kind::Tpo, 3 * MINUTE), 3 * MINUTE);
    assert_eq!(
        snapshot.single_prints,
        [Zone {
            low: 2.0,
            high: 4.0,
            block: Some(0),
            touch: Touch::Untouched
        }]
    );
    assert_eq!(
        snapshot
            .tails
            .iter()
            .map(|z| (z.low, z.high))
            .collect::<Vec<_>>(),
        [(0.0, 0.0), (6.0, 6.0)]
    );
    assert_eq!(snapshot.initial_balance, Some((0.0, 6.0)));
    assert!(!snapshot.poor_low && !snapshot.poor_high);
    engine.upsert_bars(&[bar(MINUTE, 0.0, 0.0), bar(2 * MINUTE, 6.0, 6.0)]);
    let poor = one(&mut engine, &fixed(Kind::Tpo, 3 * MINUTE), 3 * MINUTE);
    assert!(poor.poor_low && poor.poor_high);
}

#[test]
fn singleton_owner_changes_keep_ownership_without_breaking_interior_classification() {
    let mut engine = Engine::default();
    engine.upsert_bars(&[
        bar(0, 0.0, 2.0),
        bar(MINUTE, 3.0, 5.0),
        bar(2 * MINUTE, 0.0, 0.0),
        bar(3 * MINUTE, 5.0, 5.0),
    ]);
    let snapshot = one(&mut engine, &fixed(Kind::Tpo, 4 * MINUTE), 4 * MINUTE);
    assert_eq!(
        snapshot
            .single_prints
            .iter()
            .map(|z| (z.low, z.high, z.block))
            .collect::<Vec<_>>(),
        [(1.0, 2.0, Some(0)), (3.0, 4.0, Some(1))]
    );
    engine.upsert_bars(&[bar(MINUTE, 4.0, 5.0)]);
    assert!(
        one(&mut engine, &fixed(Kind::Tpo, 4 * MINUTE), 4 * MINUTE)
            .single_prints
            .is_empty()
    );
}

#[test]
fn sp_mitigation_tracks_rows_not_envelope_and_preserves_positive_touches_through_gaps() {
    let mut engine = Engine::default();
    engine.upsert_bars(&structure());
    engine.set_bar_coverage(vec![coverage(0, 3 * MINUTE)]);
    let spec = fixed(Kind::Tpo, 3 * MINUTE);
    assert_eq!(
        one(&mut engine, &spec, 4 * MINUTE).single_prints[0].touch,
        Touch::Unknown
    );
    engine.upsert_bars(&[bar(3 * MINUTE, 2.0, 2.0), bar(4 * MINUTE, 4.0, 4.0)]);
    assert_eq!(
        one(&mut engine, &spec, 5 * MINUTE).single_prints[0].touch,
        Touch::Partial
    );
    engine.upsert_bars(&[bar(5 * MINUTE, 3.0, 3.0)]);
    assert_eq!(
        one(&mut engine, &spec, 6 * MINUTE).single_prints[0].touch,
        Touch::Filled
    );
    engine.upsert_bars(&[bar(5 * MINUTE, 10.0, 10.0)]);
    assert_eq!(
        one(&mut engine, &spec, 6 * MINUTE).single_prints[0].touch,
        Touch::Partial
    );
    engine.upsert_bars(&[bar(3 * MINUTE, 10.0, 10.0), bar(4 * MINUTE, 10.0, 10.0)]);
    assert_eq!(
        one(&mut engine, &spec, 6 * MINUTE).single_prints[0].touch,
        Touch::Unknown
    );
    engine.set_bar_coverage(vec![coverage(0, 6 * MINUTE)]);
    assert_eq!(
        one(&mut engine, &spec, 6 * MINUTE).single_prints[0].touch,
        Touch::Untouched
    );
}

#[test]
fn incomplete_origins_cannot_mint_certified_sp_or_naked_levels() {
    let mut engine = Engine::default();
    engine.upsert_bars(&structure());
    let spec = Spec {
        block_minutes: 1,
        ..Spec::default_for(Kind::Tpo)
    };
    let snapshots = engine
        .calculate(&spec, Decimal::ONE, (DAY, 2 * DAY), 2 * DAY)
        .unwrap();
    assert!(!snapshots[0].complete);
    assert_eq!(snapshots[0].single_prints[0].touch, Touch::Unknown);
    assert!(snapshots[1].naked_pocs.is_empty());
}

#[test]
fn initial_balance_excludes_third_and_later_blocks() {
    let mut engine = Engine::default();
    engine.upsert_bars(&[
        bar(0, 5.0, 6.0),
        bar(MINUTE, 4.0, 7.0),
        bar(2 * MINUTE, 0.0, 20.0),
    ]);
    let snapshot = one(&mut engine, &fixed(Kind::Tpo, 3 * MINUTE), 3 * MINUTE);
    assert_eq!(snapshot.initial_balance, Some((4.0, 7.0)));
    assert_eq!((snapshot.low, snapshot.high), (Some(0.0), Some(20.0)));
}

#[test]
fn explicit_resource_and_invalid_source_errors_never_coarsen_or_fake_output() {
    let mut engine = Engine::default();
    let spec = fixed(Kind::Tpo, MINUTE);
    assert!(
        engine
            .calculate(&spec, Decimal::ZERO, (0, MINUTE), MINUTE)
            .is_err()
    );
    assert!(
        engine
            .calculate(&spec, Decimal::ONE, (0, 0), MINUTE)
            .is_err()
    );
    engine.upsert_bars(&[bar(0, 0.0, MAX_ROWS as f64)]);
    assert!(
        engine
            .calculate(&spec, Decimal::ONE, (0, MINUTE), MINUTE)
            .is_err()
    );
    engine.upsert_bars(&[bar(0, 1.0, 1.0)]);
    assert_eq!(one(&mut engine, &spec, MINUTE).rows.len(), 1);
    engine.upsert_bars(&[bar(1000, 1.0, 1.0)]);
    assert!(
        engine
            .calculate(&spec, Decimal::ONE, (0, MINUTE), MINUTE)
            .is_err()
    );
}

#[test]
fn cached_incremental_results_match_fresh_engine_after_corrections() {
    let spec = fixed(Kind::Tpo, 3 * MINUTE);
    let mut cached = Engine::default();
    let mut bars = structure();
    cached.upsert_bars(&bars);
    let _ = one(&mut cached, &spec, 6 * MINUTE);
    bars[0] = bar(0, 0.0, 6.0);
    bars.push(bar(3 * MINUTE, 2.0, 3.0));
    cached.upsert_bars(&bars);
    let mut fresh = Engine::default();
    fresh.upsert_bars(&bars);
    assert_eq!(
        one(&mut cached, &spec, 6 * MINUTE),
        one(&mut fresh, &spec, 6 * MINUTE)
    );
    let volume = fixed(Kind::Volume, MINUTE);
    cached
        .ingest_executions(&[execution("raw:a", 0, "10", "1")])
        .unwrap();
    let _ = one(&mut cached, &volume, MINUTE);
    cached
        .ingest_executions(&[execution("raw:a", 1, "20", "3")])
        .unwrap();
    fresh
        .ingest_executions(&[execution("raw:a", 1, "20", "3")])
        .unwrap();
    assert_eq!(
        one(&mut cached, &volume, MINUTE),
        one(&mut fresh, &volume, MINUTE)
    );
}

#[test]
fn unchanged_calculations_reuse_contributions_and_do_not_journal_duplicate_inputs() {
    let mut engine = Engine::default();
    engine.upsert_bars(&structure());
    let spec = fixed(Kind::Tpo, 3 * MINUTE);
    let first = one(&mut engine, &spec, 3 * MINUTE);
    let revision = engine.revision;
    let storage = engine.caches[0].rows.first_key_value().unwrap().1 as *const Bin;
    engine.upsert_bars(&structure());
    assert_eq!(one(&mut engine, &spec, 3 * MINUTE), first);
    assert_eq!(engine.revision, revision);
    assert_eq!(
        engine.caches[0].rows.first_key_value().unwrap().1 as *const Bin,
        storage
    );
}

#[test]
fn decimal_prices_just_below_a_row_boundary_never_round_into_the_next_row() {
    let mut engine = Engine::default();
    engine
        .ingest_executions(&[
            execution("raw:positive", 0, "2.9999999999999999999999999999", "1"),
            execution("raw:negative", 1, "-0.0000000000000000000000000001", "1"),
        ])
        .unwrap();
    let spec = Spec {
        ticks_per_row: 3,
        ..fixed(Kind::Volume, MINUTE)
    };
    let snapshot = one(&mut engine, &spec, MINUTE);
    assert_eq!(
        snapshot
            .rows
            .iter()
            .map(|row| row.index)
            .collect::<Vec<_>>(),
        [-1, 0]
    );
}

#[test]
fn unrepresentable_quantity_sums_report_errors_instead_of_losing_native_quantity() {
    let mut engine = Engine::default();
    engine
        .ingest_executions(&[
            execution("raw:large", 0, "1", "79228162514264337593543950335"),
            execution("raw:tiny", 1, "1", "0.0000000000000000000000000001"),
        ])
        .unwrap();
    assert!(
        engine
            .calculate(
                &fixed(Kind::Volume, MINUTE),
                Decimal::ONE,
                (0, MINUTE),
                MINUTE
            )
            .is_err()
    );
}

#[test]
fn failed_incremental_cache_updates_are_discarded_and_can_recover_after_correction() {
    let mut engine = Engine::default();
    let spec = fixed(Kind::Tpo, MINUTE);
    engine.upsert_bars(&[bar(0, 1.0, 1.0)]);
    let _ = one(&mut engine, &spec, MINUTE);
    engine.upsert_bars(&[bar(0, 0.0, MAX_ROWS as f64)]);
    assert!(
        engine
            .calculate(&spec, Decimal::ONE, (0, MINUTE), MINUTE)
            .is_err()
    );
    assert_eq!(
        engine.cached_rows,
        engine
            .caches
            .iter()
            .map(|formation| formation.rows.len())
            .sum::<usize>()
    );
    engine.upsert_bars(&[bar(0, 2.0, 2.0)]);
    assert_eq!(one(&mut engine, &spec, MINUTE).poc, Some(2.0));
    assert_eq!(
        engine.cached_rows,
        engine
            .caches
            .iter()
            .map(|formation| formation.rows.len())
            .sum::<usize>()
    );
}

fn utc(value: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(value)
        .unwrap()
        .timestamp_millis()
}

#[test]
fn custom_period_validation_rejects_invalid_zone_and_non_hhmm_times() {
    for (zone, start, end) in [
        ("Not/A_Zone", "09:30", "16:00"),
        ("America/New_York", "9:30", "16:00"),
        ("UTC", "24:00", "16:00"),
        ("UTC", "09:60", "16:00"),
        ("UTC", "09:30", "16:00:00"),
        ("UTC", " 09:30", "16:00"),
    ] {
        assert!(Period::custom(zone, start, end).is_err());
    }
    assert!(Period::custom_minutes("UTC", 1440, 0).is_err());
    assert_eq!(parse_session_time("23:59").unwrap(), 1439);
    assert_eq!(Period::ALL.len(), 4);
}

#[test]
fn custom_overnight_and_gaps_return_latest_open_and_exact_previous_session() {
    let overnight = Period::custom("America/New_York", "18:00", "09:30").unwrap();
    let expected = (utc("2024-01-02T23:00:00Z"), utc("2024-01-03T14:30:00Z"));
    assert_eq!(
        period_bounds(overnight, utc("2024-01-03T08:00:00Z")),
        expected
    );
    assert_eq!(
        period_bounds(overnight, utc("2024-01-03T18:00:00Z")),
        expected
    );
    assert_eq!(
        period_bounds(overnight, expected.0 - 1),
        (utc("2024-01-01T23:00:00Z"), utc("2024-01-02T14:30:00Z"))
    );
    let daytime = Period::custom("UTC", "09:30", "16:00").unwrap();
    let today = (utc("2024-01-02T09:30:00Z"), utc("2024-01-02T16:00:00Z"));
    assert_eq!(period_bounds(daytime, today.0), today);
    assert_eq!(period_bounds(daytime, today.1), today);
    assert_eq!(period_bounds(daytime, today.1 + 12 * 60 * MINUTE), today);
    assert_eq!(
        period_bounds(daytime, today.0 - 1),
        (today.0 - DAY, today.1 - DAY)
    );
}

#[test]
fn custom_new_york_full_days_follow_spring_and_fall_dst_calendar() {
    let period = Period::custom("America/New_York", "00:00", "00:00").unwrap();
    let spring = period_bounds(period, utc("2024-03-10T12:00:00Z"));
    assert_eq!(
        spring,
        (utc("2024-03-10T05:00:00Z"), utc("2024-03-11T04:00:00Z"))
    );
    assert_eq!(spring.1 - spring.0, 23 * 60 * MINUTE);
    let fall = period_bounds(period, utc("2024-11-03T12:00:00Z"));
    assert_eq!(
        fall,
        (utc("2024-11-03T04:00:00Z"), utc("2024-11-04T05:00:00Z"))
    );
    assert_eq!(fall.1 - fall.0, 25 * 60 * MINUTE);
    assert_eq!(period_bounds(period, fall.0 - 1).1, fall.0);
}

#[test]
fn custom_ambiguous_opens_are_earliest_closes_latest_and_missing_minutes_advance() {
    let ambiguous = Period::custom("America/New_York", "01:15", "01:45").unwrap();
    assert_eq!(
        period_bounds(ambiguous, utc("2024-11-03T06:30:00Z")),
        (utc("2024-11-03T05:15:00Z"), utc("2024-11-03T06:45:00Z"))
    );
    let missing_open = Period::custom("America/New_York", "02:15", "04:00").unwrap();
    assert_eq!(
        period_bounds(missing_open, utc("2024-03-10T08:00:00Z")),
        (utc("2024-03-10T07:00:00Z"), utc("2024-03-10T08:00:00Z"))
    );
    let missing_close = Period::custom("America/New_York", "01:00", "02:15").unwrap();
    assert_eq!(
        period_bounds(missing_close, utc("2024-03-10T08:00:00Z")),
        (utc("2024-03-10T06:00:00Z"), utc("2024-03-10T07:00:00Z"))
    );
    // Both boundaries resolve to 03:00: skip this zero-length formation safely.
    let missing_window = Period::custom("America/New_York", "02:10", "02:40").unwrap();
    assert_eq!(
        period_bounds(missing_window, utc("2024-03-10T08:00:00Z")),
        (utc("2024-03-09T07:10:00Z"), utc("2024-03-09T07:40:00Z"))
    );
}

#[test]
fn custom_profiles_preserve_gaps_previous_empty_sessions_and_native_numerics() {
    let start = utc("2024-01-02T09:30:00Z");
    let end = utc("2024-01-02T16:00:00Z");
    let period = Period::custom("UTC", "09:30", "16:00").unwrap();
    let mut engine = Engine::default();
    engine.upsert_bars(&[
        bar(start, 10.0, 12.0),
        bar(start + 30 * MINUTE, 11.0, 12.0),
        bar(end, 99.0, 99.0),
    ]);
    engine
        .ingest_executions(&[
            execution("raw:session", start, "11", "5"),
            execution("raw:close-gap", end, "99", "900"),
        ])
        .unwrap();
    engine.set_bar_coverage(vec![coverage(start - DAY, end + DAY)]);
    engine.set_execution_coverage(vec![coverage(start - DAY, end + DAY)]);
    for kind in Kind::ALL {
        let spec = Spec {
            range: Range::Session(period),
            ..Spec::default_for(kind)
        };
        let narrow = engine
            .calculate(&spec, Decimal::ONE, (start, end), end)
            .unwrap();
        let wider = engine
            .calculate(
                &spec,
                Decimal::ONE,
                (start - MINUTE, end + 6 * 60 * MINUTE),
                end,
            )
            .unwrap();
        let current = narrow.iter().find(|s| s.start_ms == start).unwrap();
        assert_eq!(current, wider.iter().find(|s| s.start_ms == start).unwrap());
        assert!(current.complete);
        assert!(current.rows.iter().all(|row| row.high < 99.0));
        assert_eq!(current.poc, Some(11.0));
        let previous = narrow.iter().find(|s| s.start_ms == start - DAY).unwrap();
        assert_eq!(previous.end_ms, end - DAY);
        assert!(previous.complete && previous.rows.is_empty() && previous.poc.is_none());
        let following = engine
            .calculate(&spec, Decimal::ONE, (start + DAY, end + DAY), end + DAY)
            .unwrap();
        assert_eq!(following[0].start_ms, start);
        assert_eq!(following[0].rows, current.rows);
        assert!(following.last().unwrap().rows.is_empty());
    }
}

#[test]
fn custom_naked_references_and_single_prints_monitor_between_sessions() {
    let start = utc("2024-01-02T09:30:00Z");
    let end = utc("2024-01-02T16:00:00Z");
    let period = Period::custom("UTC", "09:30", "16:00").unwrap();
    let mut engine = Engine::default();
    engine.upsert_bars(&[
        bar(start - DAY, 3.0, 3.0),
        bar(start, 1.0, 5.0),
        bar(start + MINUTE, 1.0, 1.0),
        bar(start + 2 * MINUTE, 5.0, 5.0),
    ]);
    engine.set_bar_coverage(vec![coverage(start - DAY, end + 2 * MINUTE)]);
    let spec = Spec {
        range: Range::Session(period),
        block_minutes: 1,
        ..Spec::default_for(Kind::Tpo)
    };
    let before = engine
        .calculate(&spec, Decimal::ONE, (start, end), end)
        .unwrap();
    assert!(before.last().unwrap().naked_pocs.is_empty());
    assert_eq!(
        before.last().unwrap().single_prints[0].touch,
        Touch::Untouched
    );
    engine.upsert_bars(&[bar(end + MINUTE, 2.0, 4.0)]);
    let after = engine
        .calculate(
            &spec,
            Decimal::ONE,
            (start, end + 2 * MINUTE),
            end + 2 * MINUTE,
        )
        .unwrap();
    assert_eq!(after.last().unwrap().single_prints[0].touch, Touch::Filled);

    let mut volume = Engine::default();
    volume
        .ingest_executions(&[
            execution("raw:previous", start - DAY, "7", "1"),
            execution("raw:today", start, "11", "2"),
        ])
        .unwrap();
    volume.set_execution_coverage(vec![coverage(start - DAY, end + 2 * MINUTE)]);
    let spec = Spec {
        kind: Kind::Volume,
        ..spec
    };
    let before = volume
        .calculate(&spec, Decimal::ONE, (start, end), end)
        .unwrap();
    assert_eq!(before.last().unwrap().naked_pocs[0].price, 7.0);
    volume
        .ingest_executions(&[execution("raw:gap-retest", end + MINUTE, "7", "1")])
        .unwrap();
    let after = volume
        .calculate(
            &spec,
            Decimal::ONE,
            (start, end + 2 * MINUTE),
            end + 2 * MINUTE,
        )
        .unwrap();
    assert!(after.last().unwrap().naked_pocs.is_empty());
    assert_eq!(after.last().unwrap().rows.len(), 1);
    assert_eq!(after.last().unwrap().poc, Some(11.0));
}

#[test]
fn custom_dst_sessions_accumulate_native_inputs_across_both_repeated_hours() {
    let period = Period::custom("America/New_York", "01:15", "01:45").unwrap();
    let start = utc("2024-11-03T05:15:00Z");
    let end = utc("2024-11-03T06:45:00Z");
    let mut engine = Engine::default();
    engine.upsert_bars(&[bar(start, 10.0, 10.0), bar(start + 60 * MINUTE, 10.0, 10.0)]);
    engine
        .ingest_executions(&[
            execution("raw:first", start, "10", "2"),
            execution("raw:second", start + 60 * MINUTE, "10", "3"),
        ])
        .unwrap();
    engine.set_bar_coverage(vec![coverage(start, end)]);
    engine.set_execution_coverage(vec![coverage(start, end)]);
    for (kind, expected) in [(Kind::Tpo, 2.0), (Kind::Volume, 5.0)] {
        let spec = Spec {
            range: Range::Session(period),
            ..Spec::default_for(kind)
        };
        let snapshots = engine
            .calculate(&spec, Decimal::ONE, (start, end), end)
            .unwrap();
        let current = snapshots.last().unwrap();
        assert_eq!((current.start_ms, current.end_ms), (start, end));
        assert_eq!(current.rows[0].weight, expected);
        assert_eq!(current.poc, Some(10.0));
    }
}

#[test]
fn manually_constructed_custom_minutes_are_validated_before_calculation() {
    for (start_minute, end_minute) in [(1440, 0), (0, 1440), (u16::MAX, u16::MAX)] {
        let spec = Spec {
            range: Range::Session(Period::Custom {
                timezone: chrono_tz::UTC,
                start_minute,
                end_minute,
            }),
            ..Spec::default_for(Kind::Tpo)
        };
        assert!(spec.validate().is_err());
        assert!(
            Engine::default()
                .calculate(&spec, Decimal::ONE, (0, DAY), DAY)
                .is_err()
        );
    }
    for time in ["", ":", "00:0", "00:000", "０:00", "00:00 "] {
        assert!(parse_session_time(time).is_err(), "{time}");
    }
    assert!(Period::custom_minutes("UTC", 0, 1440).is_err());
}

#[test]
fn viewport_entirely_in_custom_gap_keeps_latest_and_exact_previous_window() {
    let start = utc("2024-01-02T09:30:00Z");
    let end = utc("2024-01-02T16:00:00Z");
    let spec = Spec {
        range: Range::Session(Period::custom("UTC", "09:30", "16:00").unwrap()),
        ..Spec::default_for(Kind::Tpo)
    };
    let snapshots = Engine::default()
        .calculate(
            &spec,
            Decimal::ONE,
            (end + MINUTE, start + DAY),
            start + DAY,
        )
        .unwrap();
    assert_eq!(
        snapshots
            .iter()
            .map(|s| (s.start_ms, s.end_ms))
            .collect::<Vec<_>>(),
        [(start - DAY, end - DAY), (start, end)]
    );
}

#[test]
fn custom_enumeration_skips_collapsed_dst_windows_and_whole_skipped_dates() {
    let period = Period::custom("America/New_York", "02:10", "02:40").unwrap();
    let first = utc("2024-03-09T07:10:00Z");
    let last = utc("2024-03-12T06:40:00Z");
    let spec = Spec {
        range: Range::Session(period),
        ..Spec::default_for(Kind::Tpo)
    };
    let snapshots = Engine::default()
        .calculate(&spec, Decimal::ONE, (first, last), last)
        .unwrap();
    assert_eq!(
        snapshots
            .iter()
            .map(|s| (s.start_ms, s.end_ms))
            .collect::<Vec<_>>(),
        [
            (utc("2024-03-08T07:10:00Z"), utc("2024-03-08T07:40:00Z")),
            (first, utc("2024-03-09T07:40:00Z")),
            (utc("2024-03-11T06:10:00Z"), utc("2024-03-11T06:40:00Z")),
            (utc("2024-03-12T06:10:00Z"), last),
        ]
    );
    let apia = Period::custom("Pacific/Apia", "09:00", "10:00").unwrap();
    let current = period_bounds(apia, utc("2011-12-30T21:00:00Z"));
    assert_eq!(
        current,
        (utc("2011-12-30T19:00:00Z"), utc("2011-12-30T20:00:00Z"))
    );
    let previous = period_bounds(apia, current.0 - 1);
    assert_eq!(
        previous,
        (utc("2011-12-29T19:00:00Z"), utc("2011-12-29T20:00:00Z"))
    );
    assert!(previous.0 < current.0 && previous.1 < current.0);
}

#[test]
fn custom_full_day_ambiguous_boundary_preserves_overlap_and_advances_backward() {
    let period = Period::custom("America/New_York", "01:30", "01:30").unwrap();
    let current = (utc("2024-11-03T05:30:00Z"), utc("2024-11-04T06:30:00Z"));
    let previous = (utc("2024-11-02T05:30:00Z"), utc("2024-11-03T06:30:00Z"));
    assert_eq!(period_bounds(period, utc("2024-11-03T06:00:00Z")), current);
    assert_eq!(period_bounds(period, current.0 - 1), previous);
    let spec = Spec {
        range: Range::Session(period),
        ..Spec::default_for(Kind::Tpo)
    };
    let snapshots = Engine::default()
        .calculate(&spec, Decimal::ONE, current, current.1)
        .unwrap();
    assert_eq!(
        snapshots
            .iter()
            .map(|s| (s.start_ms, s.end_ms))
            .collect::<Vec<_>>(),
        [previous, current]
    );
}

#[test]
fn session_coverage_excludes_gaps_but_naked_lifecycle_requires_gap_coverage() {
    let start = utc("2024-01-02T09:30:00Z");
    let end = utc("2024-01-02T16:00:00Z");
    let mut engine = Engine::default();
    engine
        .ingest_executions(&[
            execution("raw:previous", start - DAY, "7", "1"),
            execution("raw:current", start, "11", "2"),
        ])
        .unwrap();
    engine.set_execution_coverage(vec![coverage(start - DAY, end - DAY), coverage(start, end)]);
    let spec = Spec {
        range: Range::Session(Period::custom("UTC", "09:30", "16:00").unwrap()),
        ..Spec::default_for(Kind::Volume)
    };
    let snapshots = engine
        .calculate(&spec, Decimal::ONE, (start, end), end)
        .unwrap();
    assert!(
        snapshots
            .iter()
            .all(|snapshot| snapshot.covered && snapshot.complete)
    );
    let current = snapshots.last().unwrap();
    assert_eq!(current.naked_pocs.len(), 1);
    assert_eq!(current.naked_pocs[0].price, 7.0);
    assert_eq!(current.naked_pocs[0].touch, Touch::Unknown);
    assert_eq!(current.rows[0].weight, 2.0);
}

#[test]
fn initial_balance_keeps_native_extremes_when_rows_are_coarse_and_bars_are_corrected() {
    let mut engine = Engine::default();
    let mut spec = fixed(Kind::Tpo, 3 * MINUTE);
    spec.ticks_per_row = 100;
    engine.upsert_bars(&[
        bar(0, 101.0, 109.0),
        bar(MINUTE, 102.0, 108.0),
        bar(2 * MINUTE, 90.0, 150.0),
    ]);
    assert_eq!(
        one(&mut engine, &spec, 3 * MINUTE).initial_balance,
        Some((101.0, 109.0))
    );
    engine.upsert_bars(&[bar(0, 103.0, 107.0)]);
    assert_eq!(
        one(&mut engine, &spec, 3 * MINUTE).initial_balance,
        Some((102.0, 108.0))
    );
    assert_eq!(
        one(&mut engine, &spec, MINUTE).initial_balance,
        Some((103.0, 107.0))
    );
}

#[test]
fn developing_tpo_distinguishes_provisional_current_minute_from_missing_history() {
    let spec = fixed(Kind::Tpo, 2 * MINUTE);
    let mut engine = Engine::default();
    engine.upsert_bars(&[bar(0, 100.0, 101.0), bar(MINUTE, 101.0, 102.0)]);
    engine.set_bar_coverage(vec![coverage(0, MINUTE)]);
    let developing = one(&mut engine, &spec, MINUTE + 1);
    assert!(developing.covered && !developing.complete);
    assert_eq!(developing.rows.last().unwrap().high, 102.0);
    assert!(!one(&mut engine, &spec, 2 * MINUTE).complete);
    engine.set_bar_coverage(vec![coverage(0, 2 * MINUTE)]);
    assert!(one(&mut engine, &spec, 2 * MINUTE).complete);
    engine.set_bar_coverage(Vec::new());
    assert!(!one(&mut engine, &spec, MINUTE + 1).covered);
    let mut awaiting_live = Engine::default();
    awaiting_live.upsert_bars(&[bar(0, 100.0, 101.0)]);
    awaiting_live.set_bar_coverage(vec![coverage(0, MINUTE)]);
    assert!(!one(&mut awaiting_live, &spec, MINUTE + 1).covered);
}

#[test]
fn future_formation_never_reports_a_future_as_of_or_certifies_empty_source_coverage() {
    let mut engine = Engine::default();
    let snapshot = one(&mut engine, &fixed(Kind::Volume, MINUTE), -1);
    assert_eq!(snapshot.as_of_ms, -1);
    assert!(!snapshot.covered && !snapshot.complete);
    assert!(snapshot.rows.is_empty());
}

#[test]
fn native_quantity_disclosure_preserves_decimal_precision_across_price_rows_and_corrections() {
    let mut engine = Engine::default();
    engine
        .ingest_executions(&[
            execution("a", 1, "10", "0.1"),
            execution("b", 2, "11", "0.2"),
        ])
        .unwrap();
    let spec = fixed(Kind::Volume, MINUTE);
    assert_eq!(
        one(&mut engine, &spec, MINUTE).native_volume,
        Some(decimal("0.3"))
    );
    engine
        .ingest_executions(&[execution("a", 1, "10", "0.15")])
        .unwrap();
    assert_eq!(
        one(&mut engine, &spec, MINUTE).native_volume,
        Some(decimal("0.35"))
    );
    engine.upsert_bars(&[bar(0, 10.0, 11.0)]);
    assert_eq!(
        one(&mut engine, &fixed(Kind::Tpo, MINUTE), MINUTE).native_volume,
        None
    );
}

fn volume_bar(time: i64, low: f64, high: f64, volume: f64) -> Candle {
    Candle {
        volume,
        ..bar(time, low, high)
    }
}

#[test]
fn candle_estimate_conserves_decimal_remainder_and_has_deterministic_levels() {
    let mut engine = Engine::default();
    engine.upsert_bars(&[volume_bar(0, 10.0, 12.0, 1.0)]);
    let spec = fixed(Kind::Volume, MINUTE);
    let snapshot = one(&mut engine, &spec, MINUTE);
    assert_eq!(snapshot.volume_method, VolumeMethod::Estimated);
    assert_eq!(snapshot.native_volume, Some(Decimal::ONE));
    assert!(!snapshot.covered && !snapshot.complete);
    assert!(
        snapshot
            .reason
            .as_deref()
            .unwrap()
            .contains("Candle-volume estimate")
    );
    let bins = &engine.caches[0].rows;
    assert_eq!(
        bins.values()
            .fold(Decimal::ZERO, |sum, b| exact_add(sum, b.quantity).unwrap()),
        Decimal::ONE
    );
    assert_eq!(
        bins[&10].quantity - bins[&11].quantity,
        decimal("0.00000001")
    );
    assert_eq!(snapshot.poc, Some(10.0));
    assert_eq!((snapshot.val, snapshot.vah), (Some(10.0), Some(12.0)));
    assert_eq!(one(&mut engine, &spec, MINUTE), snapshot);
}

#[test]
fn partial_prints_are_replaced_not_added_and_corrections_wait_for_exact_recovery() {
    let mut engine = Engine::default();
    engine.upsert_bars(&[volume_bar(0, 10.0, 10.0, 12.0)]);
    engine
        .ingest_executions(&[execution("a", 1, "99", "2")])
        .unwrap();
    let spec = fixed(Kind::Volume, MINUTE);
    let estimated = one(&mut engine, &spec, MINUTE);
    assert_eq!(estimated.native_volume, Some(decimal("12")));
    assert_eq!(estimated.rows.len(), 1);
    assert_eq!(estimated.poc, Some(10.0));
    engine
        .ingest_executions(&[execution("a", 1, "98", "5"), execution("b", 2, "97", "3")])
        .unwrap();
    assert_eq!(one(&mut engine, &spec, MINUTE), estimated);
    engine.set_execution_coverage(vec![coverage(0, MINUTE)]);
    let exact = one(&mut engine, &spec, MINUTE);
    assert_eq!(exact.native_volume, Some(decimal("8")));
    assert_eq!(exact.volume_method, VolumeMethod::Exact);
    assert!(exact.covered && exact.complete);
    assert_eq!(
        exact.rows.iter().map(|r| r.index).collect::<Vec<_>>(),
        [97, 98]
    );
    engine.invalidate_executions(0, MINUTE, "transport loss".into());
    assert_eq!(
        one(&mut engine, &spec, MINUTE).native_volume,
        Some(decimal("12"))
    );
}

#[test]
fn proof_only_recovery_changes_mixed_estimates_to_exact_without_bar_or_print_changes() {
    let mut engine = Engine::default();
    engine.upsert_bars(&[
        volume_bar(0, 10.0, 10.0, 10.0),
        volume_bar(MINUTE, 20.0, 20.0, 20.0),
    ]);
    engine
        .ingest_executions(&[
            execution("a", 1, "11", "2"),
            execution("b", MINUTE + 1, "21", "3"),
        ])
        .unwrap();
    let spec = fixed(Kind::Volume, 2 * MINUTE);
    assert_eq!(
        one(&mut engine, &spec, 2 * MINUTE).native_volume,
        Some(decimal("30"))
    );
    engine.set_execution_coverage(vec![coverage(0, MINUTE)]);
    let mixed = one(&mut engine, &spec, 2 * MINUTE);
    assert_eq!(mixed.volume_method, VolumeMethod::Mixed);
    assert_eq!(mixed.native_volume, Some(decimal("22")));
    assert!(!mixed.covered);
    engine.set_execution_coverage(vec![coverage(0, 2 * MINUTE)]);
    let exact = one(&mut engine, &spec, 2 * MINUTE);
    assert_eq!(exact.volume_method, VolumeMethod::Exact);
    assert_eq!(exact.native_volume, Some(decimal("5")));
    assert_eq!(exact.poc, Some(21.0));
    assert!(exact.covered && exact.complete);
}

#[test]
fn bar_revisions_update_estimated_rows_quantity_and_value_area_at_same_horizon() {
    let mut engine = Engine::default();
    let spec = fixed(Kind::Volume, MINUTE);
    engine.upsert_bars(&[volume_bar(0, 1.0, 3.0, 9.0)]);
    assert_eq!(
        one(&mut engine, &spec, MINUTE).native_volume,
        Some(decimal("9"))
    );
    engine.upsert_bars(&[volume_bar(0, 5.0, 6.0, 4.0)]);
    let revised = one(&mut engine, &spec, MINUTE);
    assert_eq!(revised.native_volume, Some(decimal("4")));
    assert_eq!(
        revised
            .rows
            .iter()
            .map(|r| (r.index, r.weight))
            .collect::<Vec<_>>(),
        [(5, 2.0), (6, 2.0)]
    );
    assert_eq!(revised.poc, Some(5.0));
    assert_eq!((revised.val, revised.vah), (Some(5.0), Some(6.0)));
    assert_eq!(one(&mut engine, &spec, MINUTE), revised);
}

#[test]
fn exact_only_setting_has_separate_cache_and_never_uses_available_candles() {
    let mut engine = Engine::default();
    engine.upsert_bars(&[volume_bar(0, 10.0, 10.0, 30.0)]);
    engine
        .ingest_executions(&[execution("a", 1, "20", "2")])
        .unwrap();
    let mut spec = fixed(Kind::Volume, MINUTE);
    assert_eq!(
        one(&mut engine, &spec, MINUTE).native_volume,
        Some(decimal("30"))
    );
    spec.estimate_missing = false;
    let exact = one(&mut engine, &spec, MINUTE);
    assert_eq!(exact.volume_method, VolumeMethod::Exact);
    assert_eq!(exact.native_volume, Some(decimal("2")));
    assert_eq!(exact.poc, Some(20.0));
    assert!(!exact.covered);
    spec.estimate_missing = true;
    assert_eq!(
        one(&mut engine, &spec, MINUTE).native_volume,
        Some(decimal("30"))
    );
}

#[test]
fn missing_or_invalid_candles_retain_only_observed_prints_and_honest_gaps() {
    let mut engine = Engine::default();
    engine
        .ingest_executions(&[execution("a", 1, "10", "2")])
        .unwrap();
    let spec = fixed(Kind::Volume, 2 * MINUTE);
    let missing = one(&mut engine, &spec, 2 * MINUTE);
    assert_eq!(missing.native_volume, Some(decimal("2")));
    assert_eq!(missing.volume_method, VolumeMethod::Exact);
    assert!(!missing.covered && !missing.complete);
    engine.upsert_bars(&[volume_bar(0, 1.0, 1.0, 8.0)]);
    assert_eq!(
        one(&mut engine, &spec, 2 * MINUTE).native_volume,
        Some(decimal("8"))
    );
    for invalid in [
        volume_bar(0, 1.0, 1.0, f64::NAN),
        volume_bar(0, 1.0, 1.0, -1.0),
        volume_bar(0, 2.0, 1.0, 8.0),
    ] {
        engine.upsert_bars(&[invalid]);
        let snapshot = one(&mut engine, &spec, 2 * MINUTE);
        assert_eq!(snapshot.native_volume, Some(decimal("2")));
        assert_eq!(snapshot.volume_method, VolumeMethod::Exact);
        assert!(!snapshot.covered);
    }
}

#[test]
fn closed_partial_edges_are_prorated_and_forming_volume_is_already_elapsed() {
    let mut engine = Engine::default();
    engine.upsert_bars(&[volume_bar(0, 10.0, 10.0, 60.0)]);
    let mut spec = fixed(Kind::Volume, 45_000);
    spec.range = Range::Fixed {
        start_ms: 15_000,
        end_ms: 45_000,
    };
    assert_eq!(
        one(&mut engine, &spec, MINUTE).native_volume,
        Some(decimal("30"))
    );
    let mut forming = Engine::default();
    forming.upsert_bars(&[volume_bar(0, 10.0, 10.0, 18.0)]);
    let whole = fixed(Kind::Volume, MINUTE);
    assert_eq!(
        one(&mut forming, &whole, 30_000).native_volume,
        Some(decimal("18"))
    );
    assert_eq!(
        one(&mut forming, &spec, 30_000).native_volume,
        Some(decimal("9"))
    );
    forming.upsert_bars(&[volume_bar(0, 10.0, 10.0, 24.0)]);
    assert_eq!(
        one(&mut forming, &whole, 40_000).native_volume,
        Some(decimal("24"))
    );
    assert_eq!(
        one(&mut forming, &spec, 40_000).native_volume,
        Some(decimal("15"))
    );
    // A backwards inspection rebuilds the contribution rather than retaining
    // the later minute's quantity or applying it twice.
    assert_eq!(
        one(&mut forming, &whole, 20_000).native_volume,
        Some(decimal("24"))
    );
}

#[test]
fn partial_bucket_proof_uses_exact_requested_interval_not_unrequested_candle_volume() {
    let mut engine = Engine::default();
    engine.upsert_bars(&[volume_bar(0, 10.0, 10.0, 60.0)]);
    engine
        .ingest_executions(&[
            execution("outside", 1, "90", "100"),
            execution("inside", 20_000, "20", "2"),
        ])
        .unwrap();
    engine.set_execution_coverage(vec![coverage(15_000, 45_000)]);
    let mut spec = fixed(Kind::Volume, 45_000);
    spec.range = Range::Fixed {
        start_ms: 15_000,
        end_ms: 45_000,
    };
    let snapshot = one(&mut engine, &spec, MINUTE);
    assert_eq!(snapshot.native_volume, Some(decimal("2")));
    assert_eq!(snapshot.volume_method, VolumeMethod::Exact);
    assert!(snapshot.complete);
    assert_eq!(snapshot.poc, Some(20.0));
}

#[test]
fn estimated_sessions_never_mint_naked_origins_or_estimate_native_level_touches() {
    let mut engine = Engine::default();
    engine.upsert_bars(&[
        volume_bar(0, 10.0, 10.0, 5.0),
        volume_bar(DAY, 10.0, 10.0, 5.0),
    ]);
    let spec = Spec::default_for(Kind::Volume);
    let snapshots = engine
        .calculate(&spec, Decimal::ONE, (DAY, 2 * DAY), 2 * DAY)
        .unwrap();
    assert!(snapshots.iter().all(|s| s.naked_pocs.is_empty()));
    engine
        .ingest_executions(&[execution("origin", 1, "10", "2")])
        .unwrap();
    engine.set_execution_coverage(vec![coverage(0, DAY)]);
    let snapshots = engine
        .calculate(&spec, Decimal::ONE, (DAY, 2 * DAY), 2 * DAY)
        .unwrap();
    let reference = snapshots
        .last()
        .unwrap()
        .naked_pocs
        .iter()
        .find(|l| l.origin_ms == 0)
        .unwrap();
    assert_eq!(reference.touch, Touch::Unknown);
}

#[test]
fn estimates_preserve_native_row_budget_and_recover_after_invalid_revision() {
    let mut engine = Engine::default();
    let spec = fixed(Kind::Volume, MINUTE);
    engine.upsert_bars(&[volume_bar(0, 1.0, 1.0, 5.0)]);
    assert_eq!(
        one(&mut engine, &spec, MINUTE).native_volume,
        Some(decimal("5"))
    );
    engine.upsert_bars(&[volume_bar(0, 0.0, MAX_ROWS as f64, 5.0)]);
    assert!(
        engine
            .calculate(&spec, Decimal::ONE, (0, MINUTE), MINUTE)
            .is_err()
    );
    engine.upsert_bars(&[volume_bar(0, 2.0, 2.0, 3.0)]);
    let recovered = one(&mut engine, &spec, MINUTE);
    assert_eq!(recovered.native_volume, Some(decimal("3")));
    assert_eq!(recovered.poc, Some(2.0));
}

#[test]
fn large_partial_quantities_do_not_overflow_an_unneeded_decimal_intermediate() {
    let quantity = decimal("60000000000000000000000000000");
    assert_eq!(
        prorated_quantity(quantity, 30_000, 60_000),
        decimal("30000000000000000000000000000")
    );
    assert_eq!(
        prorated_quantity(decimal("0.0000000000000000000000000001"), 30_000, 60_000),
        Decimal::ZERO
    );
}

#[test]
fn certified_empty_bucket_replaces_estimate_and_zero_volume_candle_adds_no_prints() {
    let mut engine = Engine::default();
    let spec = fixed(Kind::Volume, MINUTE);
    engine.upsert_bars(&[volume_bar(0, 10.0, 10.0, 5.0)]);
    assert_eq!(
        one(&mut engine, &spec, MINUTE).native_volume,
        Some(decimal("5"))
    );
    engine.set_execution_coverage(vec![coverage(0, MINUTE)]);
    let empty = one(&mut engine, &spec, MINUTE);
    assert!(empty.rows.is_empty() && empty.complete);
    assert_eq!(empty.native_volume, Some(Decimal::ZERO));
    assert_eq!(empty.volume_method, VolumeMethod::Exact);
    engine.set_execution_coverage(Vec::new());
    engine
        .ingest_executions(&[execution("partial", 1, "99", "2")])
        .unwrap();
    engine.upsert_bars(&[volume_bar(0, 10.0, 10.0, 0.0)]);
    let zero = one(&mut engine, &spec, MINUTE);
    assert!(zero.rows.is_empty() && !zero.covered);
    assert_eq!(zero.native_volume, Some(Decimal::ZERO));
    assert_eq!(zero.volume_method, VolumeMethod::Estimated);
    assert!(
        zero.reason
            .as_deref()
            .unwrap()
            .contains("Candle-volume estimate")
    );
}

#[test]
fn repeated_non_divisible_estimates_and_exact_prints_keep_decimal_accumulation_headroom() {
    let mut engine = Engine::default();
    let count = 1024;
    let bars: Vec<_> = (0..count)
        .map(|i| volume_bar(i * MINUTE, 10.0, 22.0, 100.0))
        .collect();
    engine.upsert_bars(&bars);
    engine
        .ingest_executions(&[execution("exact", count * MINUTE + 1, "10", "0.123456789")])
        .unwrap();
    let spec = fixed(Kind::Volume, (count + 1) * MINUTE);
    let snapshot = one(&mut engine, &spec, (count + 1) * MINUTE);
    assert_eq!(snapshot.volume_method, VolumeMethod::Mixed);
    assert_eq!(snapshot.native_volume, Some(decimal("102400.123456789")));
    assert_eq!(snapshot.rows.len(), 13);
    assert_eq!(snapshot.poc, Some(10.0));
    let rows = &engine.caches[0].rows;
    assert_eq!(
        rows.values()
            .fold(Decimal::ZERO, |sum, bin| exact_add(sum, bin.quantity)
                .unwrap()),
        decimal("102400.123456789"),
    );
    // A revised candle removes precisely its prior allocation before adding
    // the replacement, even after many contributions have accumulated.
    engine.upsert_bars(&[volume_bar(0, 10.0, 22.0, 101.0)]);
    let revised = one(&mut engine, &spec, (count + 1) * MINUTE);
    assert_eq!(revised.native_volume, Some(decimal("102401.123456789")));
    let mut fresh = Engine::default();
    fresh.upsert_bars(&bars);
    fresh.upsert_bars(&[volume_bar(0, 10.0, 22.0, 101.0)]);
    fresh
        .ingest_executions(&[execution("exact", count * MINUTE + 1, "10", "0.123456789")])
        .unwrap();
    assert_eq!(one(&mut fresh, &spec, (count + 1) * MINUTE), revised);
}

#[test]
fn zero_estimate_with_observed_prints_from_another_bucket_is_mixed() {
    let mut engine = Engine::default();
    engine.upsert_bars(&[volume_bar(0, 10.0, 10.0, 0.0)]);
    engine
        .ingest_executions(&[execution("observed", MINUTE + 1, "20", "2")])
        .unwrap();
    let snapshot = one(&mut engine, &fixed(Kind::Volume, 2 * MINUTE), 2 * MINUTE);
    assert_eq!(snapshot.native_volume, Some(decimal("2")));
    assert_eq!(snapshot.volume_method, VolumeMethod::Mixed);
    assert!(!snapshot.covered);
    assert!(
        snapshot
            .reason
            .as_deref()
            .unwrap()
            .contains("Candle-volume estimate")
    );
}

#[test]
fn zero_volume_fallback_contributions_are_evicted_before_configuration_exhaustion() {
    let mut engine = Engine::default();
    let minutes = MAX_CACHED_ROWS / MAX_CACHES + 2;
    let end = minutes as i64 * MINUTE;
    engine.upsert_bars(
        &(0..minutes)
            .map(|index| volume_bar(index as i64 * MINUTE, 1.0, 1.0, 0.0))
            .collect::<Vec<_>>(),
    );
    let mut spec = fixed(Kind::Volume, end);
    for width in 1..=MAX_CACHES + 1 {
        spec.ticks_per_row = width as u32;
        engine.calculate(&spec, Decimal::ONE, (0, end), end).expect(
            "inactive candle contributions must be evicted before exhausting configurations",
        );
    }
    let snapshot = one(&mut engine, &spec, end);
    assert_eq!(snapshot.native_volume, Some(Decimal::ZERO));
    assert_eq!(snapshot.volume_method, VolumeMethod::Estimated);
    assert!(!snapshot.covered);
}
