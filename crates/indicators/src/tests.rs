use super::*;
use mexc_model::VolumeFlow;

#[test]
fn vwap_preserves_extreme_weighted_products_without_intermediate_underflow() {
    let bars = vec![vwap_bar(0, 1e-300, 1e300), vwap_bar(300, 1e300, 1e-300)];
    let mut state = VwapState::default();
    let mut points = Vec::new();
    state.update(
        &bars,
        &mut points,
        0,
        Timeframe::Min5,
        VwapReset::Anchored { start_ms: 0 },
    );
    let result = points[1].values[0].unwrap();
    assert!(result.is_finite() && (result / 2e-300 - 1.0).abs() < 1e-12);
    let mut bar = vwap_bar(0, 3e-300, 1.0);
    bar.low = -f64::MAX;
    bar.high = f64::MAX;
    state.update(
        &[bar],
        &mut points,
        0,
        Timeframe::Min5,
        VwapReset::Anchored { start_ms: 0 },
    );
    let result = points[0].values[0].unwrap();
    assert!((result / 1e-300 - 1.0).abs() < 1e-12);
}

#[test]
fn vwap_truncation_empty_history_and_timeframe_changes_do_not_reuse_stale_context() {
    let bars = vec![vwap_bar(0, 10.0, 1.0), vwap_bar(300, 30.0, 1.0)];
    let mut state = VwapState::default();
    let mut points = Vec::new();
    let reset = VwapReset::Session(profiles::Period::Day);
    state.update(&bars, &mut points, 0, Timeframe::Min5, reset);
    assert!(state.context(1).unwrap().covered);
    state.update(&bars, &mut points, 2, Timeframe::Min1, reset);
    assert!(!state.context(1).unwrap().covered);
    state.update(&bars[..1], &mut points, 1, Timeframe::Min1, reset);
    assert_eq!(points.len(), 1);
    assert!(state.context(1).is_none());
    state.update(&[], &mut points, 1, Timeframe::Min1, reset);
    assert!(points.is_empty());
    assert!(state.context(0).is_none());
}

fn vwap_bar(time: i64, price: f64, volume: f64) -> Candle {
    Candle {
        time,
        open: price,
        high: price,
        low: price,
        close: price,
        volume,
        flow: None,
    }
}

fn vwap_ms(timestamp: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .unwrap()
        .timestamp_millis()
}

#[test]
fn vwap_weights_native_hlc3_and_integrates_as_a_native_overlay() {
    let mut bars = vec![vwap_bar(0, 10.0, 2.0), vwap_bar(300, 20.0, 1.0)];
    bars[0].high = 14.0;
    bars[0].low = 6.0;
    bars[0].close = 13.0; // HLC3 = 11, not the close or OHLC4.
    let spec = IndicatorKind::Vwap.default_spec();
    assert_eq!(
        spec,
        IndicatorSpec::Vwap {
            reset: profiles::Period::Day
        }
    );
    assert!(spec.is_overlay());
    assert_eq!(spec.series_names(), &["VWAP"]);
    let mut set = IndicatorSet::default();
    set.add(spec, &bars, Timeframe::Min5).unwrap();
    series(&set.instances[0].points, 0, &[Some(11.0), Some(14.0)]);
    assert_eq!(
        set.instances[0].vwap_context(1),
        Some(VwapContext {
            origin_ms: 0,
            covered: true
        })
    );
    assert!(
        validate_source(
            &IndicatorSource {
                mode: SourceMode::All,
                markets: Vec::new()
            },
            spec
        )
        .is_err()
    );
}

#[test]
fn vwap_zero_and_invalid_bars_never_manufacture_values_or_coverage() {
    let mut bars = vec![
        vwap_bar(0, 8.0, 0.0),
        vwap_bar(300, 10.0, 2.0),
        vwap_bar(600, 999.0, 0.0),
        vwap_bar(900, 20.0, -1.0),
        vwap_bar(1200, 20.0, 2.0),
        vwap_bar(1500, 20.0, f64::NAN),
        vwap_bar(1800, 20.0, 1.0),
        vwap_bar(2100, 20.0, f64::INFINITY),
        vwap_bar(2400, 20.0, 1.0),
        vwap_bar(2700, 20.0, 1.0),
    ];
    bars[6].high = f64::INFINITY;
    bars[8].close = f64::NAN;
    bars[9].low = 21.0;
    let mut state = VwapState::default();
    let mut points = Vec::new();
    state.update(
        &bars,
        &mut points,
        0,
        Timeframe::Min5,
        VwapReset::Session(profiles::Period::Day),
    );
    series(
        &points,
        0,
        &[
            None,
            Some(10.0),
            Some(10.0),
            None,
            Some(15.0),
            None,
            None,
            None,
            None,
            None,
        ],
    );
    assert!(state.context(0).unwrap().covered); // zero is valid native volume
    assert!(state.context(2).unwrap().covered);
    assert!(!state.context(3).unwrap().covered);
    assert!(!state.context(4).unwrap().covered);
}

#[test]
fn vwap_calendar_resets_follow_utc_bounds_not_the_chart_timeframe() {
    for (period, boundary) in [
        (profiles::Period::Day, "2024-01-02T00:00:00Z"),
        (profiles::Period::Week, "2024-01-08T00:00:00Z"),
        (profiles::Period::Month, "2024-02-01T00:00:00Z"),
    ] {
        let boundary = vwap_ms(boundary);
        let bars = vec![
            vwap_bar(boundary / 1000 - 300, 10.0, 3.0),
            vwap_bar(boundary / 1000, 30.0, 1.0),
            vwap_bar(boundary / 1000 + 300, 50.0, 1.0),
        ];
        let mut state = VwapState::default();
        let mut points = Vec::new();
        state.update(
            &bars,
            &mut points,
            0,
            Timeframe::Min5,
            VwapReset::Session(period),
        );
        series(&points, 0, &[Some(10.0), Some(30.0), Some(40.0)]);
        assert!(!state.context(0).unwrap().covered);
        assert_eq!(
            state.context(1),
            Some(VwapContext {
                origin_ms: boundary,
                covered: true
            })
        );
        assert!(state.context(2).unwrap().covered);
    }
    let origin = vwap_ms("2024-02-01T00:00:00Z");
    let bars = vec![
        vwap_bar(origin / 1000, 10.0, 1.0),
        vwap_bar(origin / 1000 + 86400, 30.0, 1.0),
    ];
    let mut state = VwapState::default();
    let mut points = Vec::new();
    state.update(
        &bars,
        &mut points,
        0,
        Timeframe::Day1,
        VwapReset::Session(profiles::Period::Month),
    );
    series(&points, 0, &[Some(10.0), Some(20.0)]);
    assert!(state.context(1).unwrap().covered);
}

#[test]
fn vwap_partial_origins_gaps_and_new_sessions_report_honest_coverage() {
    let bars = vec![
        vwap_bar(300, 10.0, 1.0),
        vwap_bar(600, 20.0, 1.0),
        vwap_bar(1200, 30.0, 1.0),
        vwap_bar(86400, 40.0, 1.0),
    ];
    let mut state = VwapState::default();
    let mut points = Vec::new();
    state.update(
        &bars,
        &mut points,
        0,
        Timeframe::Min5,
        VwapReset::Session(profiles::Period::Day),
    );
    series(
        &points,
        0,
        &[Some(10.0), Some(15.0), Some(20.0), Some(40.0)],
    );
    for i in 0..3 {
        assert_eq!(
            state.context(i),
            Some(VwapContext {
                origin_ms: 0,
                covered: false
            })
        );
    }
    assert_eq!(
        state.context(3),
        Some(VwapContext {
            origin_ms: 86_400_000,
            covered: true
        })
    );
    let mut with_origin = vec![vwap_bar(0, 1.0, 1.0)];
    with_origin.extend_from_slice(&bars[..3]);
    state.update(
        &with_origin,
        &mut points,
        3,
        Timeframe::Min5,
        VwapReset::Session(profiles::Period::Day),
    );
    assert!(state.context(2).unwrap().covered); // changed first candle forces reseed
    assert!(!state.context(3).unwrap().covered); // native 900-second candle is absent
}

#[test]
fn vwap_custom_gaps_overnight_and_straddling_candles_are_not_invented() {
    let reset = VwapReset::Session(profiles::Period::custom("UTC", "09:30", "10:00").unwrap());
    let bars = vec![
        vwap_bar(9 * 3600, 10.0, 1.0),  // straddles open on an hourly chart
        vwap_bar(10 * 3600, 20.0, 1.0), // outside session
    ];
    let mut state = VwapState::default();
    let mut points = Vec::new();
    state.update(&bars, &mut points, 0, Timeframe::Min60, reset);
    series(&points, 0, &[None, None]);
    assert!(state.context(0).is_none());
    let bars = vec![vwap_bar(9 * 3600 + 1800, 10.0, 1.0)];
    state.update(&bars, &mut points, 0, Timeframe::Min60, reset);
    assert_eq!(points[0].values[0], None); // straddles close
    assert!(!state.context(0).unwrap().covered);
    let reset = VwapReset::Session(profiles::Period::custom("UTC", "22:00", "02:00").unwrap());
    let bars = vec![
        vwap_bar(22 * 3600, 10.0, 1.0),
        vwap_bar(23 * 3600, 20.0, 1.0),
        vwap_bar(24 * 3600, 30.0, 1.0),
        vwap_bar(25 * 3600, 40.0, 1.0),
        vwap_bar(26 * 3600, 50.0, 1.0),
        vwap_bar(46 * 3600, 60.0, 1.0),
    ];
    state.update(&bars, &mut points, 0, Timeframe::Min60, reset);
    series(
        &points,
        0,
        &[
            Some(10.0),
            Some(15.0),
            Some(20.0),
            Some(25.0),
            None,
            Some(60.0),
        ],
    );
    assert!(state.context(3).unwrap().covered);
    assert!(state.context(4).is_none());
    assert!(state.context(5).unwrap().covered);
}

#[test]
fn vwap_custom_iana_sessions_share_spring_and_fall_dst_calendar_rules() {
    for (date, open, last, duration_hours) in [
        (
            "2024-03-10",
            "2024-03-10T05:00:00Z",
            "2024-03-11T03:00:00Z",
            23,
        ),
        (
            "2024-11-03",
            "2024-11-03T04:00:00Z",
            "2024-11-04T04:00:00Z",
            25,
        ),
    ] {
        let period = profiles::Period::custom("America/New_York", "00:00", "00:00").unwrap();
        let open = vwap_ms(open);
        let bounds = profiles::period_bounds(period, open);
        assert_eq!(bounds, (open, open + duration_hours * 3_600_000), "{date}");
        let bars: Vec<_> = (0..duration_hours)
            .map(|i| vwap_bar(open / 1000 + i * 3600, (i + 1) as f64, 1.0))
            .collect();
        let mut state = VwapState::default();
        let mut points = Vec::new();
        state.update(
            &bars,
            &mut points,
            0,
            Timeframe::Min60,
            VwapReset::Session(period),
        );
        assert_eq!(points.last().unwrap().time * 1000, vwap_ms(last));
        close(
            points.last().unwrap().values[0],
            Some((duration_hours + 1) as f64 / 2.0),
        );
        assert!(state.context(bars.len() - 1).unwrap().covered);
    }
    let period = profiles::Period::custom("America/New_York", "02:30", "04:00").unwrap();
    let origin = vwap_ms("2024-03-10T07:00:00Z"); // missing 02:30 advances to 03:00
    let mut state = VwapState::default();
    let mut points = Vec::new();
    state.update(
        &[vwap_bar(origin / 1000, 7.0, 1.0)],
        &mut points,
        0,
        Timeframe::Min60,
        VwapReset::Session(period),
    );
    assert_eq!(
        state.context(0),
        Some(VwapContext {
            origin_ms: origin,
            covered: true
        })
    );
}

#[test]
fn vwap_incremental_replacements_corrections_appends_and_prepends_match_reseed() {
    let reset = VwapReset::Session(profiles::Period::Day);
    let mut bars: Vec<_> = (0..20)
        .map(|i| vwap_bar(i * 300, i as f64 + 2.0, i as f64 + 1.0))
        .collect();
    let mut state = VwapState::default();
    let mut points = Vec::new();
    state.update(&bars, &mut points, 0, Timeframe::Min5, reset);
    for from in [19, 19, 8, 0] {
        bars[from] = vwap_bar(bars[from].time, 35.0 + from as f64, 7.0);
        state.update(&bars, &mut points, from, Timeframe::Min5, reset);
        let mut rebuilt = Vec::new();
        let mut fresh = VwapState::default();
        fresh.update(&bars, &mut rebuilt, 0, Timeframe::Min5, reset);
        assert_eq!(points, rebuilt);
        for i in 0..bars.len() {
            assert_eq!(state.context(i), fresh.context(i));
        }
    }
    bars.push(vwap_bar(6000, 123.0, 4.0));
    state.update(&bars, &mut points, 20, Timeframe::Min5, reset);
    let mut rebuilt = Vec::new();
    VwapState::default().update(&bars, &mut rebuilt, 0, Timeframe::Min5, reset);
    assert_eq!(points, rebuilt);
    bars.insert(0, vwap_bar(-300, 8.0, 2.0));
    state.update(&bars, &mut points, 0, Timeframe::Min5, reset);
    VwapState::default().update(&bars, &mut rebuilt, 0, Timeframe::Min5, reset);
    assert_eq!(points, rebuilt);
    bars.reverse();
    state.update(&bars, &mut points, 20, Timeframe::Min5, reset);
    VwapState::default().update(&bars, &mut rebuilt, 0, Timeframe::Min5, reset);
    assert_eq!(points, rebuilt);
    assert!(points.iter().skip(1).all(|point| point.values[0].is_none()));
}

#[test]
fn vwap_anchored_exact_midbar_absent_and_future_origins_are_explicit() {
    let bars = vec![
        vwap_bar(0, 10.0, 1.0),
        vwap_bar(300, 20.0, 3.0),
        vwap_bar(600, 40.0, 1.0),
    ];
    let mut state = VwapState::default();
    let mut points = Vec::new();
    state.update(
        &bars,
        &mut points,
        0,
        Timeframe::Min5,
        VwapReset::Anchored { start_ms: 300_000 },
    );
    series(&points, 0, &[None, Some(20.0), Some(25.0)]);
    assert!(state.context(0).is_none());
    assert_eq!(
        state.context(2),
        Some(VwapContext {
            origin_ms: 300_000,
            covered: true
        })
    );
    state.update(
        &bars,
        &mut points,
        2,
        Timeframe::Min5,
        VwapReset::Anchored { start_ms: 150_000 },
    );
    series(&points, 0, &[None, Some(20.0), Some(25.0)]);
    assert_eq!(
        state.context(1),
        Some(VwapContext {
            origin_ms: 150_000,
            covered: false
        })
    );
    state.update(
        &bars[1..],
        &mut points,
        2,
        Timeframe::Min5,
        VwapReset::Anchored { start_ms: 0 },
    );
    series(&points, 0, &[Some(20.0), Some(25.0)]);
    assert_eq!(
        state.context(0),
        Some(VwapContext {
            origin_ms: 0,
            covered: false
        })
    );
    state.update(
        &bars,
        &mut points,
        0,
        Timeframe::Min5,
        VwapReset::Anchored { start_ms: 900_000 },
    );
    assert!(points.iter().all(|point| point.values[0].is_none()));
    assert!((0..bars.len()).all(|i| state.context(i).is_none()));
}

#[test]
fn vwap_extreme_finite_prices_and_weights_remain_bounded_and_never_nan() {
    let reset = VwapReset::Anchored { start_ms: 0 };
    let mut state = VwapState::default();
    let mut points = Vec::new();
    let bars = vec![
        vwap_bar(0, f64::MAX, f64::MAX),
        vwap_bar(300, f64::MAX, f64::MAX),
        vwap_bar(600, -f64::MAX, f64::MAX),
        vwap_bar(900, 0.0, f64::MAX),
    ];
    state.update(&bars, &mut points, 0, Timeframe::Min5, reset);
    close(points[0].values[0], Some(f64::MAX));
    close(points[1].values[0], Some(f64::MAX));
    close(points[2].values[0], Some(f64::MAX / 3.0));
    close(points[3].values[0], Some(f64::MAX / 4.0));
    assert!(
        points
            .iter()
            .all(|point| point.values[0].unwrap().is_finite())
    );
    let bars = vec![
        vwap_bar(0, 3.0, f64::from_bits(1)),
        vwap_bar(300, 9.0, f64::from_bits(1)),
    ];
    state.update(&bars, &mut points, 0, Timeframe::Min5, reset);
    series(&points, 0, &[Some(3.0), Some(6.0)]);
    state.update(
        &[vwap_bar(i64::MAX, 3.0, 1.0)],
        &mut points,
        0,
        Timeframe::Min5,
        reset,
    );
    assert_eq!(points[0].values[0], None);
    assert!(state.context(0).is_none());
}

#[test]
fn vwap_public_reset_validation_and_custom_labels_are_safe() {
    for start_ms in [i64::MIN, i64::MAX] {
        assert!(VwapReset::Anchored { start_ms }.validate().is_err());
    }
    for (start_minute, end_minute) in [(1440, 0), (0, 1440), (u16::MAX, u16::MAX)] {
        let reset = profiles::Period::Custom {
            timezone: chrono_tz::UTC,
            start_minute,
            end_minute,
        };
        assert!(VwapReset::Session(reset).validate().is_err());
        assert!(IndicatorSpec::Vwap { reset }.validate().is_err());
        let mut points = Vec::new();
        VwapState::default().update(
            &[vwap_bar(0, 1.0, 1.0)],
            &mut points,
            0,
            Timeframe::Min5,
            VwapReset::Session(reset),
        );
        assert_eq!(points[0].values[0], None);
    }
    let period = profiles::Period::custom("America/New_York", "09:30", "16:00").unwrap();
    assert!(
        IndicatorSpec::Vwap { reset: period }
            .label()
            .contains("America/New_York 09:30–16:00")
    );
}

#[test]
fn native_open_interest_never_allocates_candle_aligned_outputs_across_configuration() {
    let bars = candles(&[10.0, 20.0, 30.0]);
    let mut set = IndicatorSet::default();
    let id = set
        .add(IndicatorSpec::OpenInterest, &bars, Timeframe::Min5)
        .unwrap();
    assert!(set.instances[0].points.is_empty());
    set.recalculate(&bars, 0, Timeframe::Min5);
    set.toggle(id, &bars).unwrap();
    set.recalculate(&bars, 2, Timeframe::Min5);
    assert!(set.instances[0].points.is_empty());
    set.configure(id, IndicatorSpec::Sma { period: 1 }, &bars, Timeframe::Min5)
        .unwrap();
    assert_eq!(set.instances[0].points.len(), bars.len());
    set.configure(id, IndicatorSpec::OpenInterest, &bars, Timeframe::Min5)
        .unwrap();
    assert!(set.instances[0].points.is_empty());
    assert_eq!(set.instances[0].id, id);
    assert!(!set.instances[0].visible);
}

fn candles(closes: &[f64]) -> Vec<Candle> {
    closes
        .iter()
        .enumerate()
        .map(|(i, &close)| Candle {
            time: i as i64 * 300,
            open: close,
            high: close + 1.0,
            low: close - 1.0,
            close,
            volume: 1.0,
            flow: None,
        })
        .collect()
}

fn calculate(spec: IndicatorSpec, candles: &[Candle]) -> Vec<IndicatorPoint> {
    let mut set = IndicatorSet::default();
    set.add(spec, candles, Timeframe::Min5).unwrap();
    set.instances.remove(0).points
}
fn covered_flow(time: i64, delta: f64, quality: FlowQuality, covered_from: i64) -> VolumeFlow {
    VolumeFlow {
        delta: Some(delta),
        volume: Some(delta.abs()),
        quality,
        covered_from,
        through_ms: (time + 300) * 1000,
    }
}

fn with_flow(mut bars: Vec<Candle>, deltas: &[Option<(f64, FlowQuality)>]) -> Vec<Candle> {
    for (bar, delta) in bars.iter_mut().zip(deltas) {
        bar.flow = delta.map(|(value, quality)| covered_flow(bar.time, value, quality, bar.time));
    }
    bars
}

#[test]
fn cvd_uses_signed_flow_and_compensated_replacement_checkpoints() {
    let bars = with_flow(
        candles(&[10.0, 9.0, 9.0, 20.0]),
        &[
            Some((1e16, FlowQuality::Exact)),
            Some((1.0, FlowQuality::Reported)),
            Some((-1e16, FlowQuality::Exact)),
            Some((-0.25, FlowQuality::Exact)),
        ],
    );
    let spec = IndicatorSpec::Cvd {
        reset: CvdReset::Continuous,
    };
    let mut set = IndicatorSet::default();
    set.add(spec, &bars, Timeframe::Min5).unwrap();
    series(
        &set.instances[0].points,
        0,
        &[Some(1e16), Some(1e16), Some(1.0), Some(0.75)],
    );
    assert_eq!(set.instances[0].cvd_context(0).unwrap().origin, 0);
    assert_eq!(
        set.instances[0].cvd_context(3).unwrap().quality,
        FlowQuality::Reported
    );

    let mut replacement = bars.clone();
    replacement[3].flow = Some(covered_flow(900, 2.5, FlowQuality::Reported, 900));
    set.recalculate_flow(&replacement, 3, Timeframe::Min5);
    close(set.instances[0].points[3].values[0], Some(3.5));
    assert_eq!(
        set.instances[0].cvd_context(3).unwrap().quality,
        FlowQuality::Reported
    );
}

#[test]
fn cvd_missing_revocation_repairs_gaps_and_requires_utc_midnight_anchor() {
    let mut bars = with_flow(
        candles(&[4.0, 1.0, 7.0, 2.0]),
        &[
            Some((2.0, FlowQuality::Exact)),
            None,
            Some((3.0, FlowQuality::Exact)),
            Some((1.0, FlowQuality::Exact)),
        ],
    );
    let spec = IndicatorSpec::Cvd {
        reset: CvdReset::Continuous,
    };
    let mut set = IndicatorSet::default();
    set.add(spec, &bars, Timeframe::Min5).unwrap();
    series(&set.instances[0].points, 0, &[Some(2.0), None, None, None]);
    assert!(set.instances[0].cvd_context(1).is_none());

    bars[1].flow = Some(covered_flow(300, -0.5, FlowQuality::Exact, 0));
    set.recalculate_flow(&bars, 1, Timeframe::Min5);
    series(
        &set.instances[0].points,
        0,
        &[Some(2.0), Some(1.5), Some(4.5), Some(5.5)],
    );
    bars[1].flow = None;
    set.recalculate_flow(&bars, 1, Timeframe::Min5);
    series(&set.instances[0].points, 0, &[Some(2.0), None, None, None]);

    let day = 86_400;
    let mut daily = candles(&[1.0, 2.0, 3.0]);
    for (i, bar) in daily.iter_mut().enumerate() {
        bar.time = day - 600 + i as i64 * 300;
        bar.flow = Some(covered_flow(bar.time, 1.0, FlowQuality::Exact, bar.time));
    }
    daily[2].time = day;
    daily[2].flow = Some(covered_flow(day, 1.0, FlowQuality::Exact, day));
    let mut daily_set = IndicatorSet::default();
    daily_set
        .add(
            IndicatorSpec::Cvd {
                reset: CvdReset::UtcDay,
            },
            &daily,
            Timeframe::Min5,
        )
        .unwrap();
    series(&daily_set.instances[0].points, 0, &[None, None, Some(1.0)]);
    assert_eq!(daily_set.instances[0].cvd_context(2).unwrap().origin, day);
}

#[test]
fn daily_cvd_accepts_proven_empty_midnight_prefix_but_not_unknown_loaded_flow() {
    let day = 86_400;
    let mut bars = candles(&[1.0, 2.0]);
    bars[0].time = day + 300;
    bars[1].time = day + 600;
    bars[0].flow = Some(covered_flow(bars[0].time, 1.25, FlowQuality::Exact, day));
    bars[1].flow = Some(covered_flow(
        bars[1].time,
        -0.5,
        FlowQuality::Exact,
        bars[1].time,
    ));
    let mut set = IndicatorSet::default();
    set.add(
        IndicatorSpec::Cvd {
            reset: CvdReset::UtcDay,
        },
        &bars,
        Timeframe::Min5,
    )
    .unwrap();
    series(&set.instances[0].points, 0, &[Some(1.25), Some(0.75)]);
    assert_eq!(set.instances[0].cvd_context(0).unwrap().origin, day);

    bars[0].flow.as_mut().unwrap().covered_from = bars[0].time;
    set.recalculate_flow(&bars, 0, Timeframe::Min5);
    series(&set.instances[0].points, 0, &[None, None]);
    bars[1].time = 2 * day + 300;
    bars[1].flow = Some(covered_flow(
        bars[1].time,
        -0.5,
        FlowQuality::Exact,
        2 * day,
    ));
    set.recalculate_flow(&bars, 1, Timeframe::Min5);
    series(&set.instances[0].points, 0, &[None, Some(-0.5)]);
    assert_eq!(set.instances[0].cvd_context(1).unwrap().origin, 2 * day);
}

#[test]
fn cvd_bridges_only_proven_empty_timeframe_gaps() {
    let mut bars = candles(&[1.0, 2.0, 3.0]);
    bars[2].time = 900;
    bars[0].flow = Some(covered_flow(0, 2.0, FlowQuality::Exact, 0));
    bars[1].flow = Some(covered_flow(300, 4.0, FlowQuality::Exact, 300));
    bars[2].flow = Some(covered_flow(900, 1.0, FlowQuality::Exact, 0));
    let mut set = IndicatorSet::default();
    set.add(
        IndicatorSpec::Cvd {
            reset: CvdReset::Continuous,
        },
        &bars,
        Timeframe::Min5,
    )
    .unwrap();
    series(
        &set.instances[0].points,
        0,
        &[Some(2.0), Some(6.0), Some(7.0)],
    );
    bars[2].flow = Some(covered_flow(900, 1.0, FlowQuality::Exact, 900));
    set.recalculate_flow(&bars, 2, Timeframe::Min5);
    series(&set.instances[0].points, 0, &[Some(2.0), Some(6.0), None]);
}
#[test]
fn cvd_instances_keep_independent_hidden_state_and_reset_modes() {
    let mut bars = with_flow(
        candles(&[1.0, 3.0, 2.0]),
        &[
            Some((2.0, FlowQuality::Exact)),
            Some((-1.0, FlowQuality::Exact)),
            Some((4.0, FlowQuality::Reported)),
        ],
    );
    for bar in &mut bars {
        bar.time += 300;
        let flow = bar.flow.as_mut().unwrap();
        flow.covered_from = bar.time;
        flow.through_ms = (bar.time + 300) * 1000;
    }
    let mut set = IndicatorSet::default();
    let continuous = set
        .add(
            IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            &bars,
            Timeframe::Min5,
        )
        .unwrap();
    let daily = set
        .add(
            IndicatorSpec::Cvd {
                reset: CvdReset::UtcDay,
            },
            &bars,
            Timeframe::Min5,
        )
        .unwrap();
    assert_ne!(continuous, daily);
    set.toggle(continuous, &bars).unwrap();
    bars[2].flow = Some(covered_flow(
        bars[2].time,
        5.0,
        FlowQuality::Exact,
        bars[2].time,
    ));
    set.recalculate_flow(&bars, 2, Timeframe::Min5);
    assert_eq!(set.instances[0].points[2].values[0], Some(6.0));
    assert_eq!(set.instances[1].points[2].values[0], None);
    assert_eq!(
        set.instances[0].cvd_context(2).unwrap().quality,
        FlowQuality::Exact
    );
}
#[test]
fn cvd_prepend_rebases_continuous_origin_and_uses_native_sign_only() {
    let bars = with_flow(
        candles(&[100.0, 50.0]),
        &[
            Some((-0.75, FlowQuality::Exact)),
            Some((0.125, FlowQuality::Exact)),
        ],
    );
    let spec = IndicatorSpec::Cvd {
        reset: CvdReset::Continuous,
    };
    let mut set = IndicatorSet::default();
    set.add(spec, &bars, Timeframe::Min5).unwrap();
    series(&set.instances[0].points, 0, &[Some(-0.75), Some(-0.625)]);

    let mut earlier = candles(&[500.0]);
    earlier[0].time = -300;
    earlier[0].flow = Some(covered_flow(-300, 2.25, FlowQuality::Exact, -300));
    earlier.extend_from_slice(&bars);
    set.recalculate(&earlier, 0, Timeframe::Min5);
    series(
        &set.instances[0].points,
        0,
        &[Some(2.25), Some(1.5), Some(1.625)],
    );
    assert_eq!(set.instances[0].cvd_context(2).unwrap().origin, -300);
}
#[test]
fn cvd_partial_tail_is_not_carried_into_next_bar_until_closed_repair() {
    let mut bars = candles(&[1.0, 2.0]);
    bars[0].flow = Some(VolumeFlow {
        delta: Some(2.0),
        volume: Some(2.0),
        quality: FlowQuality::Exact,
        covered_from: 0,
        through_ms: 120_000,
    });
    bars[1].flow = Some(covered_flow(300, 3.0, FlowQuality::Exact, 0));
    let mut set = IndicatorSet::default();
    set.add(
        IndicatorSpec::Cvd {
            reset: CvdReset::Continuous,
        },
        &bars,
        Timeframe::Min5,
    )
    .unwrap();
    series(&set.instances[0].points, 0, &[Some(2.0), None]);

    bars[0].flow = Some(covered_flow(0, 2.0, FlowQuality::Exact, 0));
    set.recalculate_flow(&bars, 0, Timeframe::Min5);
    series(&set.instances[0].points, 0, &[Some(2.0), Some(5.0)]);
}

fn close(actual: Option<f64>, expected: Option<f64>) {
    match (actual, expected) {
        (Some(actual), Some(expected)) => assert!(
            actual.is_finite() && (actual - expected).abs() <= 1e-9 * expected.abs().max(1.0),
            "expected {expected}, got {actual}",
        ),
        (None, None) => {}
        _ => panic!("expected {expected:?}, got {actual:?}"),
    }
}

fn series(points: &[IndicatorPoint], slot: usize, expected: &[Option<f64>]) {
    assert_eq!(points.len(), expected.len());
    for (point, expected) in points.iter().zip(expected) {
        close(point.values[slot], *expected);
    }
}

fn same_points(actual: &[IndicatorPoint], expected: &[IndicatorPoint]) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual.time, expected.time);
        for slot in 0..3 {
            close(actual.values[slot], expected.values[slot]);
        }
    }
}

#[test]
fn arithmetic_and_exponential_averages_have_distinct_sma_seeds() {
    let bars = candles(&[1.0, 2.0, 4.0, 8.0, 16.0]);
    series(
        &calculate(IndicatorSpec::Sma { period: 3 }, &bars),
        0,
        &[
            None,
            None,
            Some(7.0 / 3.0),
            Some(14.0 / 3.0),
            Some(28.0 / 3.0),
        ],
    );
    // SMA seed 7/3, then alpha 1/2, not an EMA seeded from the first close.
    series(
        &calculate(IndicatorSpec::Ema { period: 3 }, &bars),
        0,
        &[
            None,
            None,
            Some(7.0 / 3.0),
            Some(31.0 / 6.0),
            Some(127.0 / 12.0),
        ],
    );
}

#[test]
fn bollinger_uses_population_variance_and_three_aligned_outputs() {
    let bars = candles(&[1.0, 2.0, 4.0, 8.0, 16.0]);
    let points = calculate(
        IndicatorSpec::Bollinger {
            period: 3,
            deviations: 2.0,
        },
        &bars,
    );
    let means = [7.0 / 3.0, 14.0 / 3.0, 28.0 / 3.0];
    // For [1,2,4], E[x²] - E[x]² = 7 - 49/9 = 14/9.
    let widths = [
        2.0 * 14.0_f64.sqrt() / 3.0,
        4.0 * 14.0_f64.sqrt() / 3.0,
        8.0 * 14.0_f64.sqrt() / 3.0,
    ];
    assert_eq!(points[0].values, [None; 3]);
    assert_eq!(points[1].values, [None; 3]);
    for i in 0..3 {
        close(points[i + 2].values[0], Some(means[i]));
        close(points[i + 2].values[1], Some(means[i] + widths[i]));
        close(points[i + 2].values[2], Some(means[i] - widths[i]));
    }
}

#[test]
fn bollinger_preserves_small_variance_on_large_price_baseline() {
    let bars = candles(&[1e12 + 1.0, 1e12 + 2.0, 1e12 + 3.0, 1e12 + 4.0]);
    let points = calculate(
        IndicatorSpec::Bollinger {
            period: 3,
            deviations: 2.0,
        },
        &bars,
    );
    for (i, point) in points.iter().enumerate().skip(2) {
        let middle = 1e12 + i as f64;
        assert_eq!(point.values[0], Some(middle));
        let width = point.values[1].unwrap() - middle;
        assert!((width - 2.0 * (2.0_f64 / 3.0).sqrt()).abs() < 0.0002);
    }
}

#[test]
fn rsi_seeds_from_changes_and_wilder_smooths_gains_and_losses() {
    let bars = candles(&[10.0, 11.0, 10.0, 12.0, 12.0, 9.0, 13.0]);
    // Initial gain=1, loss=1/3. After the flat bar: 2/3 and 2/9.
    // After -3: 4/9 and 31/27. After +4: 44/27 and 62/81.
    series(
        &calculate(IndicatorSpec::Rsi { period: 3 }, &bars),
        0,
        &[
            None,
            None,
            None,
            Some(75.0),
            Some(75.0),
            Some(1200.0 / 43.0),
            Some(6600.0 / 97.0),
        ],
    );
    series(
        &calculate(
            IndicatorSpec::Rsi { period: 2 },
            &candles(&[1.0, 2.0, 3.0, 4.0]),
        ),
        0,
        &[None, None, Some(100.0), Some(100.0)],
    );
    series(
        &calculate(
            IndicatorSpec::Rsi { period: 2 },
            &candles(&[4.0, 3.0, 2.0, 1.0]),
        ),
        0,
        &[None, None, Some(0.0), Some(0.0)],
    );
}

#[test]
fn atr_includes_first_range_and_previous_close_gaps() {
    let mut bars = candles(&[10.0, 12.0, 9.0, 11.0]);
    bars[1].high = 14.0;
    // TRs = [2,4,4,3]; the first ATR is 10/3, then Wilder gives 29/9.
    series(
        &calculate(IndicatorSpec::Atr { period: 3 }, &bars),
        0,
        &[None, None, Some(10.0 / 3.0), Some(29.0 / 9.0)],
    );
}

#[test]
fn macd_seeds_independent_emas_and_signal_only_from_valid_macd() {
    let points = calculate(
        IndicatorSpec::Macd {
            fast: 2,
            slow: 3,
            signal: 2,
        },
        &candles(&[1.0, 2.0, 4.0, 8.0, 16.0]),
    );
    // Fast EMA: 3/2,19/6,115/18,691/54; slow: 7/3,31/6,127/12.
    series(
        &points,
        0,
        &[
            None,
            None,
            Some(5.0 / 6.0),
            Some(11.0 / 9.0),
            Some(239.0 / 108.0),
        ],
    );
    series(
        &points,
        1,
        &[None, None, None, Some(37.0 / 36.0), Some(589.0 / 324.0)],
    );
    series(
        &points,
        2,
        &[None, None, None, Some(7.0 / 36.0), Some(32.0 / 81.0)],
    );
}

#[test]
fn stochastic_smooths_valid_raw_k_then_valid_smoothed_k() {
    let points = calculate(
        IndicatorSpec::Stochastic {
            period: 3,
            smooth_k: 2,
            smooth_d: 2,
        },
        &candles(&[1.0, 2.0, 3.0, 2.0, 4.0, 3.0]),
    );
    // Raw %K = [75,100/3,75,50]; both smoothers are arithmetic means.
    series(
        &points,
        0,
        &[
            None,
            None,
            None,
            Some(325.0 / 6.0),
            Some(325.0 / 6.0),
            Some(62.5),
        ],
    );
    series(
        &points,
        1,
        &[None, None, None, None, Some(325.0 / 6.0), Some(175.0 / 3.0)],
    );
    assert!(points.iter().all(|point| point.values[2].is_none()));
}

#[test]
fn flat_prices_produce_defined_values_after_each_series_warmup() {
    let mut bars = candles(&[42.0; 9]);
    for bar in &mut bars {
        bar.high = 42.0;
        bar.low = 42.0;
    }
    let cases = [
        (
            IndicatorSpec::Sma { period: 3 },
            [Some((2, 42.0)), None, None],
        ),
        (
            IndicatorSpec::Ema { period: 3 },
            [Some((2, 42.0)), None, None],
        ),
        (
            IndicatorSpec::Bollinger {
                period: 3,
                deviations: 2.0,
            },
            [Some((2, 42.0)); 3],
        ),
        (
            IndicatorSpec::Rsi { period: 3 },
            [Some((3, 50.0)), None, None],
        ),
        (
            IndicatorSpec::Macd {
                fast: 2,
                slow: 3,
                signal: 2,
            },
            [Some((2, 0.0)), Some((3, 0.0)), Some((3, 0.0))],
        ),
        (
            IndicatorSpec::Atr { period: 3 },
            [Some((2, 0.0)), None, None],
        ),
        (
            IndicatorSpec::Stochastic {
                period: 3,
                smooth_k: 2,
                smooth_d: 2,
            },
            [Some((3, 50.0)), Some((4, 50.0)), None],
        ),
    ];
    for (spec, outputs) in cases {
        let points = calculate(spec, &bars);
        for (i, point) in points.iter().enumerate() {
            assert_eq!(point.time, bars[i].time);
            for (slot, output) in outputs.iter().enumerate() {
                let expected = output.and_then(|(start, value)| (i >= start).then_some(value));
                close(point.values[slot], expected);
            }
        }
    }
}

#[test]
fn period_one_has_no_extra_warmup_and_rsi_still_requires_a_change() {
    let bars = candles(&[3.0, 1.0, 1.0, 4.0]);
    for spec in [
        IndicatorSpec::Sma { period: 1 },
        IndicatorSpec::Ema { period: 1 },
        IndicatorSpec::Bollinger {
            period: 1,
            deviations: 2.0,
        },
    ] {
        let points = calculate(spec, &bars);
        series(&points, 0, &[Some(3.0), Some(1.0), Some(1.0), Some(4.0)]);
        if matches!(spec, IndicatorSpec::Bollinger { .. }) {
            series(&points, 1, &[Some(3.0), Some(1.0), Some(1.0), Some(4.0)]);
            series(&points, 2, &[Some(3.0), Some(1.0), Some(1.0), Some(4.0)]);
        }
    }
    series(
        &calculate(IndicatorSpec::Rsi { period: 1 }, &bars),
        0,
        &[None, Some(0.0), Some(50.0), Some(100.0)],
    );
    series(
        &calculate(IndicatorSpec::Atr { period: 1 }, &bars),
        0,
        &[Some(2.0), Some(3.0), Some(2.0), Some(4.0)],
    );
    let stoch = calculate(
        IndicatorSpec::Stochastic {
            period: 1,
            smooth_k: 1,
            smooth_d: 1,
        },
        &bars,
    );
    series(&stoch, 0, &[Some(50.0); 4]);
    series(&stoch, 1, &[Some(50.0); 4]);
    let macd = calculate(
        IndicatorSpec::Macd {
            fast: 1,
            slow: 2,
            signal: 1,
        },
        &bars,
    );
    series(
        &macd,
        0,
        &[None, Some(-1.0), Some(-1.0 / 3.0), Some(8.0 / 9.0)],
    );
    series(
        &macd,
        1,
        &[None, Some(-1.0), Some(-1.0 / 3.0), Some(8.0 / 9.0)],
    );
    series(&macd, 2, &[None, Some(0.0), Some(0.0), Some(0.0)]);
}

fn mutation_specs() -> [IndicatorSpec; 9] {
    [
        IndicatorSpec::Sma { period: 5 },
        IndicatorSpec::Ema { period: 5 },
        IndicatorSpec::Bollinger {
            period: 5,
            deviations: 2.5,
        },
        IndicatorSpec::Rsi { period: 5 },
        IndicatorSpec::Macd {
            fast: 3,
            slow: 7,
            signal: 4,
        },
        IndicatorSpec::Atr { period: 5 },
        IndicatorSpec::Stochastic {
            period: 5,
            smooth_k: 3,
            smooth_d: 2,
        },
        IndicatorSpec::Stochastic {
            period: 1,
            smooth_k: 1,
            smooth_d: 1,
        },
        IndicatorSpec::Macd {
            fast: 1,
            slow: 2,
            signal: 1,
        },
    ]
}

#[test]
fn incremental_mutations_match_batch_without_changing_the_unchanged_prefix() {
    let fixture = candles(
        &(0..60)
            .map(|i| 100.0 + ((i * 17) % 23) as f64 + i as f64 / 10.0)
            .collect::<Vec<_>>(),
    );
    for spec in mutation_specs() {
        let mut set = IndicatorSet::default();
        set.add(spec, &[], Timeframe::Min5).unwrap();
        let mut bars = Vec::new();
        for bar in &fixture[10..40] {
            let from = bars.len();
            bars.push(*bar);
            set.recalculate(&bars, from, Timeframe::Min5);
            same_points(&set.instances[0].points, &calculate(spec, &bars));
            let prefix = set.instances[0].points[..from].to_vec();
            // Each replacement is a fresh provisional bar, not an additional EMA/Wilder step.
            for delta in [3.0, -2.0, 0.0, 1.5] {
                let current = bars.last_mut().unwrap();
                *current = *bar;
                current.close += delta;
                current.high = current.high.max(current.close);
                current.low = current.low.min(current.close);
                set.recalculate(&bars, from, Timeframe::Min5);
                assert_eq!(&set.instances[0].points[..from], prefix.as_slice());
                same_points(&set.instances[0].points, &calculate(spec, &bars));
            }
        }
        for correction in [17, 3, 0] {
            let prefix = set.instances[0].points[..correction].to_vec();
            bars[correction].close += 7.0;
            bars[correction].high = bars[correction].high.max(bars[correction].close);
            set.recalculate(&bars, correction, Timeframe::Min5);
            assert_eq!(&set.instances[0].points[..correction], prefix.as_slice());
            same_points(&set.instances[0].points, &calculate(spec, &bars));
        }
        let mut prepended = fixture[..10].to_vec();
        prepended.extend_from_slice(&bars);
        set.recalculate(&prepended, 0, Timeframe::Min5);
        same_points(&set.instances[0].points, &calculate(spec, &prepended));
        prepended.truncate(8);
        set.recalculate(&prepended, 8, Timeframe::Min5);
        same_points(&set.instances[0].points, &calculate(spec, &prepended));
        set.recalculate(&[], 0, Timeframe::Min5);
        assert!(set.instances[0].points.is_empty());
        set.recalculate(&fixture, 0, Timeframe::Min5);
        same_points(&set.instances[0].points, &calculate(spec, &fixture));
    }
}

#[test]
fn instances_remain_independent_across_configuration_visibility_and_removal() {
    let mut bars = candles(&[1.0, 2.0, 4.0, 8.0, 16.0]);
    let mut set = IndicatorSet::default();
    let first = set
        .add(IndicatorSpec::Sma { period: 2 }, &bars, Timeframe::Min5)
        .unwrap();
    let second = set
        .add(IndicatorSpec::Sma { period: 3 }, &bars, Timeframe::Min5)
        .unwrap();
    assert_ne!(first, second);
    let second_points = set.instances[1].points.clone();
    set.configure(
        first,
        IndicatorSpec::Rsi { period: 2 },
        &bars,
        Timeframe::Min5,
    )
    .unwrap();
    same_points(
        &set.instances[0].points,
        &calculate(IndicatorSpec::Rsi { period: 2 }, &bars),
    );
    assert_eq!(set.instances[1].points, second_points);
    set.toggle(first, &bars).unwrap();
    assert!(!set.instances[0].visible);
    bars[4].close = 3.0;
    bars[4].low = 2.0;
    set.recalculate(&bars, 4, Timeframe::Min5);
    set.toggle(first, &bars).unwrap();
    assert!(set.instances[0].visible);
    same_points(
        &set.instances[0].points,
        &calculate(IndicatorSpec::Rsi { period: 2 }, &bars),
    );
    let preserved = set.instances[0].points.clone();
    assert!(
        set.configure(
            first,
            IndicatorSpec::Ema { period: 0 },
            &bars,
            Timeframe::Min5
        )
        .is_err()
    );
    assert_eq!(set.instances[0].points, preserved);
    assert_eq!(set.instances[0].spec, IndicatorSpec::Rsi { period: 2 });
    assert!(set.remove(first));
    assert!(!set.remove(first));
    assert_eq!(set.instances[0].id, second);
    assert!(set.toggle(first, &bars).is_err());
    assert!(
        set.configure(
            first,
            IndicatorSpec::Sma { period: 2 },
            &bars,
            Timeframe::Min5
        )
        .is_err()
    );
    assert!(
        set.add(IndicatorSpec::Sma { period: 0 }, &bars, Timeframe::Min5)
            .is_err()
    );
    let third = set
        .add(IndicatorSpec::Sma { period: 2 }, &bars, Timeframe::Min5)
        .unwrap();
    assert!(third > second);
}

#[test]
fn invalid_parameters_are_rejected_at_all_numerical_boundaries() {
    for period in [0, 10_001, usize::MAX] {
        for spec in [
            IndicatorSpec::Sma { period },
            IndicatorSpec::Ema { period },
            IndicatorSpec::Bollinger {
                period,
                deviations: 2.0,
            },
            IndicatorSpec::Rsi { period },
            IndicatorSpec::Atr { period },
            IndicatorSpec::Macd {
                fast: period,
                slow: 10,
                signal: 2,
            },
            IndicatorSpec::Macd {
                fast: 1,
                slow: period,
                signal: 2,
            },
            IndicatorSpec::Macd {
                fast: 1,
                slow: 2,
                signal: period,
            },
            IndicatorSpec::Stochastic {
                period,
                smooth_k: 2,
                smooth_d: 2,
            },
            IndicatorSpec::Stochastic {
                period: 2,
                smooth_k: period,
                smooth_d: 2,
            },
            IndicatorSpec::Stochastic {
                period: 2,
                smooth_k: 2,
                smooth_d: period,
            },
        ] {
            assert!(spec.validate().is_err(), "{spec:?}");
        }
    }
    for deviations in [
        0.0,
        -1.0,
        100.01,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        assert!(
            IndicatorSpec::Bollinger {
                period: 2,
                deviations
            }
            .validate()
            .is_err()
        );
    }
    assert!(
        IndicatorSpec::Macd {
            fast: 2,
            slow: 2,
            signal: 1
        }
        .validate()
        .is_err()
    );
    assert!(
        IndicatorSpec::Macd {
            fast: 3,
            slow: 2,
            signal: 1
        }
        .validate()
        .is_err()
    );
    assert!(
        IndicatorSpec::Bollinger {
            period: 10_000,
            deviations: 100.0
        }
        .validate()
        .is_ok()
    );
    assert!(
        IndicatorSpec::Macd {
            fast: 9_999,
            slow: 10_000,
            signal: 10_000
        }
        .validate()
        .is_ok()
    );
    assert!(
        IndicatorSpec::Stochastic {
            period: 10_000,
            smooth_k: 10_000,
            smooth_d: 10_000
        }
        .validate()
        .is_ok()
    );
}

mod external_sources {
    use super::*;
    use mexc_model::{
        Exchange, FlowBar, Market, OpenInterestSample, OpenInterestTimestamp, Product,
    };
    use std::collections::HashMap;

    fn market(exchange: Exchange, symbol: &str) -> Market {
        Market {
            exchange,
            product: Product::Perpetual,
            symbol: symbol.into(),
            interval: Timeframe::Min5,
        }
    }

    fn flow(time: i64, delta: f64, from: i64, through_ms: i64, quality: FlowQuality) -> FlowBar {
        FlowBar {
            time,
            flow: VolumeFlow {
                delta: Some(delta),
                volume: Some(delta.abs()),
                quality,
                covered_from: from,
                through_ms,
            },
        }
    }

    fn sample(timestamp_ms: i64, quantity: f64, interval_ms: i64) -> OpenInterestSample {
        OpenInterestSample {
            timestamp_ms,
            quantity,
            interval_ms,
            timestamp_source: OpenInterestTimestamp::Venue,
        }
    }

    fn source(market: Market) -> SourceData {
        let mut data = SourceData::new(market, "BTC".into());
        data.flow_connected = true;
        data.oi_connected = true;
        data
    }

    fn set(spec: IndicatorSpec, markets: &[Market]) -> IndicatorSet {
        let mut set = IndicatorSet::default();
        let id = set.add(spec, &[], Timeframe::Min5).unwrap();
        set.configure_source(
            id,
            IndicatorSource {
                mode: SourceMode::Selected,
                markets: markets.to_vec(),
            },
        )
        .unwrap();
        set
    }

    fn values(set: &IndicatorSet) -> Vec<Option<f64>> {
        set.instances[0]
            .points
            .iter()
            .map(|p| p.values[0])
            .collect()
    }

    #[test]
    fn source_validation_rejects_duplicate_instruments_even_with_different_intervals() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let mut duplicate = a.clone();
        duplicate.interval = Timeframe::Min1;
        let mut set = IndicatorSet::default();
        let id = set
            .add(
                IndicatorSpec::Cvd {
                    reset: CvdReset::Continuous,
                },
                &[],
                Timeframe::Min5,
            )
            .unwrap();
        assert!(
            set.configure_source(
                id,
                IndicatorSource {
                    mode: SourceMode::Selected,
                    markets: vec![a.clone(), duplicate]
                }
            )
            .is_err()
        );
        assert!(
            set.configure_source(
                id,
                IndicatorSource {
                    mode: SourceMode::Single,
                    markets: vec![]
                }
            )
            .is_err()
        );
        assert!(
            set.configure_source(
                id,
                IndicatorSource {
                    mode: SourceMode::Chart,
                    markets: vec![a.clone()]
                }
            )
            .is_err()
        );
        let price = set
            .add(IndicatorSpec::Sma { period: 1 }, &[], Timeframe::Min5)
            .unwrap();
        assert!(
            set.configure_source(
                price,
                IndicatorSource {
                    mode: SourceMode::Single,
                    markets: vec![a]
                }
            )
            .is_err()
        );
        assert!(set.instances[0].source.is_chart());
    }

    #[test]
    fn cvd_sums_raw_deltas_at_a_shared_origin_and_reports_asynchronous_coverage() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let b = market(Exchange::Okx, "BTC-USD-SWAP");
        let mut first = source(a.clone());
        first.merge_flows(
            &[
                flow(-300, 100.0, -300, 0, FlowQuality::Exact),
                flow(0, 2.0, 0, 300_000, FlowQuality::Exact),
                flow(300, -1.0, 300, 420_000, FlowQuality::Exact),
            ],
            true,
        );
        let mut second = source(b.clone());
        second.merge_flows(
            &[
                flow(0, 3.0, 0, 300_000, FlowQuality::Reported),
                flow(300, 4.0, 300, 450_000, FlowQuality::Exact),
            ],
            true,
        );
        let inputs = HashMap::from([(a.clone(), first), (b.clone(), second)]);
        let mut set = set(
            IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            &[a.clone(), b],
        );
        assert_eq!(set.refresh_sources(&inputs, &a, "BTC", 460_000), vec![1]);
        assert_eq!(values(&set), vec![None, Some(5.0), Some(8.0)]);
        let instance = &set.instances[0];
        assert_eq!(
            instance.cvd_context(2),
            Some(CvdContext {
                origin: 0,
                quality: FlowQuality::Reported,
                through_ms: 450_000
            })
        );
        let output = instance.external.as_ref().unwrap();
        assert!(output.complete);
        assert_eq!(output.asof_range_ms, Some((420_000, 450_000)));
        assert_eq!(
            output.context_asof_ranges_ms,
            vec![None, Some((300_000, 300_000)), Some((420_000, 450_000))]
        );
        assert_eq!(output.unit, "BTC");
        assert_eq!(output.contributors.len(), 2);
        assert!(set.refresh_sources(&inputs, &a, "BTC", 470_000).is_empty());
        // Expiring the native bucket changes the current readout, not its proven curve.
        assert_eq!(set.refresh_sources(&inputs, &a, "BTC", 600_000), vec![1]);
        assert!(!set.instances[0].external.as_ref().unwrap().complete);
        assert_eq!(values(&set), vec![None, Some(5.0), Some(8.0)]);
    }

    #[test]
    fn cvd_retains_bucket_compensation_across_source_cancellation() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let b = market(Exchange::Okx, "BTC-USDT-SWAP");
        let mut first = source(a.clone());
        first.merge_flows(
            &[
                flow(0, 1e16, 0, 300_000, FlowQuality::Exact),
                flow(300, -1e16, 300, 600_000, FlowQuality::Exact),
            ],
            false,
        );
        let mut second = source(b.clone());
        second.merge_flows(
            &[
                flow(0, 1.0, 0, 300_000, FlowQuality::Exact),
                flow(300, 0.0, 300, 600_000, FlowQuality::Exact),
            ],
            false,
        );
        let inputs = HashMap::from([(a.clone(), first), (b.clone(), second)]);
        let mut set = set(
            IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            &[a.clone(), b],
        );
        set.refresh_sources(&inputs, &a, "BTC", 600_000);
        assert_eq!(values(&set), vec![Some(1e16), Some(1.0)]);
    }

    #[test]
    fn cvd_missing_source_bucket_is_not_zero_and_a_gap_invalidates_the_suffix() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let b = market(Exchange::Okx, "BTC-USDT-SWAP");
        let mut first = source(a.clone());
        first.merge_flows(
            &[
                flow(0, 2.0, 0, 300_000, FlowQuality::Exact),
                flow(300, 3.0, 300, 600_000, FlowQuality::Exact),
                flow(600, 5.0, 600, 900_000, FlowQuality::Exact),
            ],
            false,
        );
        let mut second = source(b.clone());
        second.merge_flows(
            &[
                flow(0, 7.0, 0, 300_000, FlowQuality::Exact),
                flow(600, 11.0, 600, 900_000, FlowQuality::Exact),
            ],
            false,
        );
        let mut inputs = HashMap::from([(a.clone(), first), (b.clone(), second)]);
        let mut set = set(
            IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            &[a.clone(), b.clone()],
        );
        set.refresh_sources(&inputs, &a, "BTC", 900_000);
        assert_eq!(values(&set), vec![Some(9.0), None, None]);
        // A real later coverage proof establishes the absent 300 bucket as zero.
        inputs
            .get_mut(&b)
            .unwrap()
            .merge_flows(&[flow(600, 11.0, 300, 900_000, FlowQuality::Exact)], false);
        set.refresh_sources(&inputs, &a, "BTC", 900_000);
        assert_eq!(values(&set), vec![Some(9.0), Some(12.0), Some(28.0)]);
        assert_eq!(set.instances[0].cvd_context(2).unwrap().origin, 0);
    }

    #[test]
    fn cvd_utc_day_requires_midnight_proof_and_recovers_only_at_a_proven_new_day() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let mut data = source(a.clone());
        data.merge_flows(
            &[
                flow(300, 1.0, 300, 600_000, FlowQuality::Exact),
                flow(600, 2.0, 600, 900_000, FlowQuality::Exact),
                flow(86_400, 3.0, 86_400, 86_700_000, FlowQuality::Exact),
            ],
            false,
        );
        let inputs = HashMap::from([(a.clone(), data)]);
        let mut set = set(
            IndicatorSpec::Cvd {
                reset: CvdReset::UtcDay,
            },
            std::slice::from_ref(&a),
        );
        set.refresh_sources(&inputs, &a, "BTC", 86_700_000);
        assert_eq!(values(&set), vec![None, None, Some(3.0)]);
        assert_eq!(set.instances[0].cvd_context(2).unwrap().origin, 86_400);
    }

    #[test]
    fn flow_revocation_preserves_final_history_and_blocks_late_unfinished_history() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let mut data = source(a.clone());
        data.merge_flows(
            &[
                flow(0, 1.0, 0, 300_000, FlowQuality::Exact),
                flow(300, 2.0, 300, 420_000, FlowQuality::Exact),
            ],
            true,
        );
        let mut inputs = HashMap::from([(a.clone(), data)]);
        let mut set = set(
            IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            std::slice::from_ref(&a),
        );
        set.refresh_sources(&inputs, &a, "BTC", 420_000);
        assert_eq!(values(&set), vec![Some(1.0), Some(3.0)]);
        let data = inputs.get_mut(&a).unwrap();
        assert!(data.revoke_unfinished(430_000));
        data.flow_connected = false;
        assert!(!data.merge_flows(&[flow(300, 2.0, 300, 420_000, FlowQuality::Exact)], false));
        set.refresh_sources(&inputs, &a, "BTC", 430_000);
        assert_eq!(values(&set), vec![Some(1.0), None]);
        let data = inputs.get_mut(&a).unwrap();
        data.flow_connected = true;
        data.merge_flows(&[flow(300, 4.0, 300, 600_000, FlowQuality::Exact)], false);
        set.refresh_sources(&inputs, &a, "BTC", 600_000);
        assert_eq!(values(&set), vec![Some(1.0), Some(5.0)]);
    }

    #[test]
    fn disconnected_unfinished_source_does_not_revoke_preceding_proven_zero_buckets() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let b = market(Exchange::Okx, "BTC-USDT-SWAP");
        let mut first = source(a.clone());
        first.merge_flows(
            &[
                flow(0, 2.0, 0, 300_000, FlowQuality::Exact),
                flow(300, 3.0, 300, 600_000, FlowQuality::Exact),
                flow(600, 4.0, 600, 720_000, FlowQuality::Exact),
            ],
            true,
        );
        let mut second = source(b.clone());
        second.merge_flows(&[flow(600, 1.0, 0, 750_000, FlowQuality::Exact)], true);
        let mut inputs = HashMap::from([(a.clone(), first), (b.clone(), second)]);
        let mut set = set(
            IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            &[a.clone(), b.clone()],
        );
        set.refresh_sources(&inputs, &a, "BTC", 750_000);
        assert_eq!(values(&set), vec![Some(2.0), Some(5.0), Some(10.0)]);
        let second = inputs.get_mut(&b).unwrap();
        second.flow_connected = false;
        second.revoke_unfinished(750_000);
        set.refresh_sources(&inputs, &a, "BTC", 750_000);
        assert_eq!(values(&set), vec![Some(2.0), Some(5.0), None]);
    }

    #[test]
    fn live_flow_precedence_rejects_older_snapshots_and_history_replacements() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let mut data = source(a);
        let current = flow(0, 3.0, 0, 200_000, FlowQuality::Exact);
        assert!(data.merge_flows(&[current], true));
        assert!(!data.merge_flows(&[flow(0, 2.0, 0, 100_000, FlowQuality::Exact)], true));
        assert!(!data.merge_flows(&[flow(0, 9.0, 0, 200_000, FlowQuality::Reported)], false));
        assert_eq!(data.flows[&0], current.flow);
        assert!(data.merge_flows(&[flow(0, 4.0, 0, 300_000, FlowQuality::Exact)], false));
        assert_eq!(data.revision, 2);
    }

    #[test]
    fn oi_uses_actual_native_timeline_zero_and_weakest_timestamp_provenance() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let b = market(Exchange::Okx, "BTC-USDT-SWAP");
        let mut first = source(a.clone());
        first.merge_oi(&[sample(1_000, 0.0, 1_000), sample(2_000, 2.0, 1_000)]);
        let mut second = source(b.clone());
        let mut observed = sample(1_500, 4.0, 2_000);
        observed.timestamp_source = OpenInterestTimestamp::Observed;
        second.merge_oi(&[observed]);
        let inputs = HashMap::from([(a.clone(), first), (b.clone(), second)]);
        let mut set = set(IndicatorSpec::OpenInterest, &[a.clone(), b]);
        set.refresh_sources(&inputs, &a, "BTC", 2_500);
        assert!(set.instances[0].points.is_empty());
        let output = set.instances[0].external.as_ref().unwrap();
        assert_eq!(
            output
                .oi
                .iter()
                .map(|s| (s.timestamp_ms, s.quantity))
                .collect::<Vec<_>>(),
            vec![(1_500, 4.0), (2_000, 6.0)]
        );
        assert!(
            output
                .oi
                .iter()
                .all(|s| s.timestamp_source == OpenInterestTimestamp::Observed)
        );
        assert_eq!(output.asof_range_ms, Some((1_500, 2_000)));
        assert_eq!(
            output.oi_asof_ranges_ms,
            vec![(1_000, 1_500), (1_500, 2_000)]
        );
        assert!(output.complete);
        assert!(set.refresh_sources(&inputs, &a, "BTC", 2_600).is_empty());
        assert_eq!(set.refresh_sources(&inputs, &a, "BTC", 5_001), vec![1]);
        let output = set.instances[0].external.as_ref().unwrap();
        assert!(!output.complete);
        assert_eq!(output.oi.len(), 2);
    }

    #[test]
    fn oi_current_freshness_uses_declared_cadence_and_disconnect_hides_only_current() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let mut data = source(a.clone());
        data.oi_cadence_ms = Some(100);
        data.merge_oi(&[sample(1_000, 0.0, 10_000)]);
        let mut inputs = HashMap::from([(a.clone(), data)]);
        let mut set = set(IndicatorSpec::OpenInterest, std::slice::from_ref(&a));
        set.refresh_sources(&inputs, &a, "BTC", 1_300);
        assert!(set.instances[0].external.as_ref().unwrap().complete);
        set.refresh_sources(&inputs, &a, "BTC", 1_301);
        assert!(!set.instances[0].external.as_ref().unwrap().complete);
        let data = inputs.get_mut(&a).unwrap();
        data.merge_oi(&[sample(1_400, 3.0, 100)]);
        data.oi_connected = false;
        set.refresh_sources(&inputs, &a, "BTC", 1_400);
        let output = set.instances[0].external.as_ref().unwrap();
        assert!(!output.complete);
        assert_eq!(output.oi.last().unwrap().quantity, 3.0);
    }

    #[test]
    fn oi_recovery_marks_a_new_proven_segment_without_synthetic_samples() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let b = market(Exchange::Okx, "BTC-USDT-SWAP");
        let mut first = source(a.clone());
        first.merge_oi(&[sample(0, 1.0, 100), sample(1_000, 3.0, 100)]);
        let mut second = source(b.clone());
        second.merge_oi(&[
            sample(0, 2.0, 100),
            sample(500, 4.0, 100),
            sample(1_000, 5.0, 100),
        ]);
        let mut inputs = HashMap::from([(a.clone(), first), (b.clone(), second)]);
        let mut set = set(IndicatorSpec::OpenInterest, &[a.clone(), b.clone()]);
        set.refresh_sources(&inputs, &a, "BTC", 1_000);
        let output = set.instances[0].external.as_ref().unwrap();
        assert_eq!(
            output
                .oi
                .iter()
                .map(|s| (s.timestamp_ms, s.quantity, s.interval_ms))
                .collect::<Vec<_>>(),
            vec![(0, 3.0, 0), (1_000, 8.0, 0)]
        );
        // Late history fills the entire gap and suffix rebuilding restores continuity.
        inputs.get_mut(&a).unwrap().merge_oi(&[
            sample(250, 1.0, 100),
            sample(500, 2.0, 100),
            sample(750, 2.0, 100),
        ]);
        inputs
            .get_mut(&b)
            .unwrap()
            .merge_oi(&[sample(250, 2.0, 100), sample(750, 4.0, 100)]);
        set.refresh_sources(&inputs, &a, "BTC", 1_000);
        let output = set.instances[0].external.as_ref().unwrap();
        assert_eq!(output.oi.len(), 5);
        assert!(output.oi.iter().skip(1).all(|s| s.interval_ms == 100));
    }

    #[test]
    fn oi_merges_sorted_pages_collisions_and_preserves_venue_time_over_receipt_time() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let mut data = source(a);
        data.merge_oi(&[sample(20, 2.0, 10), sample(40, 4.0, 10)]);
        let mut collision = sample(20, 99.0, 10);
        collision.timestamp_source = OpenInterestTimestamp::Observed;
        assert!(data.merge_oi(&[sample(30, 3.0, 10), collision, sample(10, 1.0, 10)]));
        assert_eq!(
            data.oi
                .iter()
                .map(|s| (s.timestamp_ms, s.quantity))
                .collect::<Vec<_>>(),
            vec![(10, 1.0), (20, 2.0), (30, 3.0), (40, 4.0)]
        );
        assert!(!data.merge_oi(&[collision]));
        assert!(!data.merge_oi(&[
            sample(50, -1.0, 10),
            sample(60, f64::NAN, 10),
            sample(70, 1.0, 0)
        ]));
    }

    #[test]
    fn source_membership_and_feed_identity_reset_origins_without_touching_chart_instances() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let b = market(Exchange::Binance, "BTCUSDC");
        let mut first = source(a.clone());
        first.merge_flows(
            &[
                flow(0, 1.0, 0, 300_000, FlowQuality::Exact),
                flow(300, 2.0, 300, 600_000, FlowQuality::Exact),
            ],
            false,
        );
        let mut second = source(b.clone());
        second.merge_flows(&[flow(300, 4.0, 300, 600_000, FlowQuality::Exact)], false);
        let mut inputs = HashMap::from([(a.clone(), first), (b.clone(), second)]);
        let mut set = set(
            IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            std::slice::from_ref(&a),
        );
        let chart_id = set
            .add(
                IndicatorSpec::Sma { period: 1 },
                &candles(&[10.0]),
                Timeframe::Min5,
            )
            .unwrap();
        set.refresh_sources(&inputs, &a, "BTC", 600_000);
        assert_eq!(values(&set), vec![Some(1.0), Some(3.0)]);
        set.configure_source(
            1,
            IndicatorSource {
                mode: SourceMode::All,
                markets: vec![a.clone(), b.clone()],
            },
        )
        .unwrap();
        set.refresh_sources(&inputs, &a, "BTC", 600_000);
        assert!(set.instances[0].points.is_empty());
        // All membership is not an assertion that the native universe is complete.
        set.set_all_source_error(1, None).unwrap();
        set.refresh_sources(&inputs, &a, "BTC", 600_000);
        assert_eq!(values(&set), vec![None, Some(6.0)]);
        assert_eq!(set.instances[0].cvd_context(1).unwrap().origin, 300);
        assert_eq!(set.instances[1].id, chart_id);
        assert_eq!(set.instances[1].points[0].values[0], Some(10.0));
        // Replacing a feed can collide in public revision, but not its internal identity.
        let mut replacement = source(b.clone());
        replacement.merge_flows(
            &[
                flow(0, 10.0, 0, 300_000, FlowQuality::Exact),
                flow(300, 20.0, 300, 600_000, FlowQuality::Exact),
            ],
            false,
        );
        inputs.insert(b, replacement);
        set.refresh_sources(&inputs, &a, "BTC", 600_000);
        assert_eq!(values(&set), vec![Some(11.0), Some(33.0)]);
        assert_eq!(set.instances[0].cvd_context(1).unwrap().origin, 0);
    }

    #[test]
    fn required_sources_fail_closed_for_missing_underlying_product_and_interval() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let b = market(Exchange::Okx, "BTC-USDT-SWAP");
        let mut first = source(a.clone());
        first.merge_oi(&[sample(1_000, 4.0, 1_000)]);
        let mut inputs = HashMap::from([(a.clone(), first)]);
        let mut set = set(IndicatorSpec::OpenInterest, &[a.clone(), b.clone()]);
        set.refresh_sources(&inputs, &a, "BTC", 1_000);
        assert!(set.instances[0].external.as_ref().unwrap().oi.is_empty());
        let mut wrong = source(b.clone());
        wrong.base_asset = "ETH".into();
        wrong.merge_oi(&[sample(1_000, 10.0, 1_000)]);
        inputs.insert(b.clone(), wrong);
        set.refresh_sources(&inputs, &a, "BTC", 1_000);
        assert!(!set.instances[0].external.as_ref().unwrap().complete);
        let mut matching = source(b.clone());
        matching.merge_oi(&[sample(1_000, 10.0, 1_000)]);
        inputs.insert(b.clone(), matching);
        set.refresh_sources(&inputs, &a, "BTC", 1_000);
        assert_eq!(
            set.instances[0].external.as_ref().unwrap().oi[0].quantity,
            14.0
        );
        let mut chart = a.clone();
        chart.product = Product::Spot;
        set.refresh_sources(&inputs, &chart, "BTC", 1_000);
        assert!(set.instances[0].external.as_ref().unwrap().oi.is_empty());
        chart = a.clone();
        chart.interval = Timeframe::Min1;
        set.refresh_sources(&inputs, &chart, "BTC", 1_000);
        assert!(!set.instances[0].external.as_ref().unwrap().complete);
    }

    #[test]
    fn parameter_changes_preserve_sources_and_price_conversion_clears_obsolete_config() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let mut data = source(a.clone());
        data.merge_flows(&[flow(300, 2.0, 300, 600_000, FlowQuality::Exact)], false);
        let inputs = HashMap::from([(a.clone(), data)]);
        let mut set = set(
            IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            std::slice::from_ref(&a),
        );
        set.refresh_sources(&inputs, &a, "BTC", 600_000);
        assert_eq!(values(&set), vec![Some(2.0)]);
        set.configure(
            1,
            IndicatorSpec::Cvd {
                reset: CvdReset::UtcDay,
            },
            &[],
            Timeframe::Min5,
        )
        .unwrap();
        assert_eq!(set.instances[0].source.markets, vec![a.clone()]);
        set.refresh_sources(&inputs, &a, "BTC", 600_000);
        assert_eq!(values(&set), vec![None]);
        assert_eq!(inputs[&a].flows.len(), 1);
        set.configure(
            1,
            IndicatorSpec::Sma { period: 1 },
            &candles(&[7.0]),
            Timeframe::Min5,
        )
        .unwrap();
        assert!(set.instances[0].source.is_chart());
        assert!(set.instances[0].external.is_none());
        assert_eq!(set.instances[0].points[0].values[0], Some(7.0));
    }

    #[test]
    fn all_cvd_universe_transitions_clear_history_and_recover_retained_native_inputs() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let b = market(Exchange::Okx, "BTC-USDT-SWAP");
        let mut first = source(a.clone());
        first.merge_flows(
            &[
                flow(0, 2.0, 0, 300_000, FlowQuality::Exact),
                flow(300, 1.0, 300, 420_000, FlowQuality::Exact),
            ],
            true,
        );
        let mut second = source(b.clone());
        second.merge_flows(
            &[
                flow(0, 3.0, 0, 300_000, FlowQuality::Reported),
                flow(300, 4.0, 300, 450_000, FlowQuality::Exact),
            ],
            true,
        );
        let mut inputs = HashMap::from([(a.clone(), first), (b.clone(), second)]);
        let mut set = set(
            IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            &[a.clone(), b.clone()],
        );
        set.configure_source(
            1,
            IndicatorSource {
                mode: SourceMode::All,
                markets: vec![a.clone(), b.clone()],
            },
        )
        .unwrap();
        set.toggle(1, &[]).unwrap();
        set.refresh_sources(&inputs, &a, "BTC", 460_000);
        assert!(set.instances[0].points.is_empty());
        let output = set.instances[0].external.as_ref().unwrap();
        assert!(!output.complete);
        assert!(output.contexts.is_empty());
        assert!(output.context_asof_ranges_ms.is_empty());
        assert_eq!(output.asof_range_ms, None);
        assert_eq!(output.contributors.len(), 2);
        assert!(output.contributors.iter().all(|source| source.complete));
        assert!(set.refresh_sources(&inputs, &a, "BTC", 470_000).is_empty());

        set.set_all_source_error(1, None).unwrap();
        assert_eq!(set.refresh_sources(&inputs, &a, "BTC", 470_000), vec![1]);
        assert_eq!(values(&set), vec![Some(5.0), Some(10.0)]);
        assert_eq!(set.instances[0].cvd_context(1).unwrap().origin, 0);
        assert!(set.instances[0].external.as_ref().unwrap().complete);
        let points = set.instances[0].points.as_ptr();
        let contexts = set.instances[0]
            .external
            .as_ref()
            .unwrap()
            .contexts
            .as_ptr();
        set.set_all_source_error(1, None).unwrap();
        assert!(set.refresh_sources(&inputs, &a, "BTC", 470_000).is_empty());
        assert_eq!(set.instances[0].points.as_ptr(), points);
        assert_eq!(
            set.instances[0]
                .external
                .as_ref()
                .unwrap()
                .contexts
                .as_ptr(),
            contexts
        );

        set.set_all_source_error(1, Some("Native discovery failed".into()))
            .unwrap();
        assert!(set.instances[0].points.is_empty());
        let output = set.instances[0].external.as_ref().unwrap();
        assert!(!output.complete);
        assert!(output.contexts.is_empty());
        assert!(output.context_asof_ranges_ms.is_empty());
        assert_eq!(output.asof_range_ms, None);
        assert_eq!(output.contributors.len(), 2);
        assert_eq!(set.refresh_sources(&inputs, &a, "BTC", 470_000), vec![1]);
        set.set_all_source_error(1, Some("Awaiting native rediscovery".into()))
            .unwrap();
        set.refresh_sources(&inputs, &a, "BTC", 470_000);
        inputs
            .get_mut(&a)
            .unwrap()
            .merge_flows(&[flow(300, 2.0, 300, 475_000, FlowQuality::Exact)], true);
        set.refresh_sources(&inputs, &a, "BTC", 480_000);
        assert!(set.instances[0].points.is_empty());
        assert!(
            set.instances[0]
                .external
                .as_ref()
                .unwrap()
                .contexts
                .is_empty()
        );
        assert_eq!(inputs[&a].flows.len(), 2);
        assert_eq!(inputs[&b].flows.len(), 2);

        set.set_all_source_error(1, None).unwrap();
        assert_eq!(set.refresh_sources(&inputs, &a, "BTC", 480_000), vec![1]);
        assert_eq!(values(&set), vec![Some(5.0), Some(11.0)]);
        assert_eq!(set.instances[0].cvd_context(1).unwrap().origin, 0);
        assert!(set.instances[0].external.as_ref().unwrap().complete);
        assert_eq!(set.instances[0].id, 1);
        assert!(!set.instances[0].visible);
        assert!(set.refresh_sources(&inputs, &a, "BTC", 480_000).is_empty());
    }

    #[test]
    fn all_membership_and_kind_changes_revoke_validation_without_changing_raw_producers() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let b = market(Exchange::Okx, "BTC-USDT-SWAP");
        let mut first = source(a.clone());
        first.merge_flows(&[flow(0, 1.0, 0, 100_000, FlowQuality::Exact)], true);
        first.merge_oi(&[sample(100_000, 3.0, 10_000)]);
        let mut second = source(b.clone());
        second.merge_flows(&[flow(0, 2.0, 0, 110_000, FlowQuality::Exact)], true);
        second.merge_oi(&[sample(100_000, 4.0, 10_000)]);
        let inputs = HashMap::from([(a.clone(), first), (b.clone(), second)]);
        let mut set = set(
            IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            std::slice::from_ref(&a),
        );
        set.configure_source(
            1,
            IndicatorSource {
                mode: SourceMode::All,
                markets: vec![a.clone()],
            },
        )
        .unwrap();
        set.set_all_source_error(1, None).unwrap();
        set.refresh_sources(&inputs, &a, "BTC", 110_000);
        assert_eq!(values(&set), vec![Some(1.0)]);

        set.configure_source(
            1,
            IndicatorSource {
                mode: SourceMode::All,
                markets: vec![a.clone(), b.clone()],
            },
        )
        .unwrap();
        set.refresh_sources(&inputs, &a, "BTC", 110_000);
        assert!(set.instances[0].points.is_empty());
        set.set_all_source_error(1, None).unwrap();
        set.refresh_sources(&inputs, &a, "BTC", 110_000);
        assert_eq!(values(&set), vec![Some(3.0)]);
        set.configure(1, IndicatorSpec::OpenInterest, &[], Timeframe::Min5)
            .unwrap();
        set.refresh_sources(&inputs, &a, "BTC", 110_000);
        assert!(set.instances[0].external.as_ref().unwrap().oi.is_empty());
        assert!(!set.instances[0].external.as_ref().unwrap().complete);
        set.set_all_source_error(1, None).unwrap();
        set.refresh_sources(&inputs, &a, "BTC", 110_000);
        assert_eq!(
            set.instances[0].external.as_ref().unwrap().oi[0].quantity,
            7.0
        );
        assert!(set.instances[0].external.as_ref().unwrap().complete);
        assert_eq!(inputs[&a].flows.len(), 1);
        assert_eq!(inputs[&b].oi.len(), 1);

        set.configure_source(
            1,
            IndicatorSource {
                mode: SourceMode::Selected,
                markets: vec![a.clone(), b],
            },
        )
        .unwrap();
        set.set_all_source_error(1, Some("All discovery is unavailable".into()))
            .unwrap();
        set.refresh_sources(&inputs, &a, "BTC", 110_000);
        assert_eq!(
            set.instances[0].external.as_ref().unwrap().oi[0].quantity,
            7.0
        );
        assert!(set.instances[0].external.as_ref().unwrap().complete);
    }

    #[test]
    fn all_oi_recovery_requires_validated_scope_and_every_raw_contributor() {
        let a = market(Exchange::Binance, "BTCUSDT");
        let b = market(Exchange::Okx, "BTC-USDT-SWAP");
        let mut first = source(a.clone());
        first.merge_oi(&[sample(0, 1.0, 1_000), sample(1_000, 3.0, 1_000)]);
        let mut inputs = HashMap::from([(a.clone(), first)]);
        let mut set = set(IndicatorSpec::OpenInterest, &[a.clone(), b.clone()]);
        set.configure_source(
            1,
            IndicatorSource {
                mode: SourceMode::All,
                markets: vec![a.clone(), b.clone()],
            },
        )
        .unwrap();
        set.set_all_source_error(1, None).unwrap();
        set.refresh_sources(&inputs, &a, "BTC", 1_000);
        assert!(set.instances[0].external.as_ref().unwrap().oi.is_empty());
        assert!(!set.instances[0].external.as_ref().unwrap().complete);

        let mut second = source(b.clone());
        second.merge_oi(&[sample(0, 2.0, 1_000), sample(1_000, 4.0, 1_000)]);
        inputs.insert(b, second);
        set.set_all_source_error(1, Some("Discovery was revoked".into()))
            .unwrap();
        set.refresh_sources(&inputs, &a, "BTC", 1_000);
        assert!(set.instances[0].external.as_ref().unwrap().oi.is_empty());
        assert_eq!(
            set.instances[0]
                .external
                .as_ref()
                .unwrap()
                .contributors
                .len(),
            2
        );
        assert_eq!(inputs[&a].oi.len(), 2);
        set.set_all_source_error(1, None).unwrap();
        set.refresh_sources(&inputs, &a, "BTC", 1_000);
        let output = set.instances[0].external.as_ref().unwrap();
        assert_eq!(
            output.oi.iter().map(|s| s.quantity).collect::<Vec<_>>(),
            vec![3.0, 7.0]
        );
        assert_eq!(output.oi_asof_ranges_ms, vec![(0, 0), (1_000, 1_000)]);
        assert!(output.complete);
        let observations = output.oi.as_ptr();
        assert!(set.refresh_sources(&inputs, &a, "BTC", 1_000).is_empty());
        assert_eq!(
            set.instances[0].external.as_ref().unwrap().oi.as_ptr(),
            observations
        );

        set.set_all_source_error(1, Some("Awaiting native rediscovery".into()))
            .unwrap();
        let output = set.instances[0].external.as_ref().unwrap();
        assert!(output.oi.is_empty());
        assert!(output.oi_asof_ranges_ms.is_empty());
        assert_eq!(output.asof_range_ms, None);
        assert!(!output.complete);
        set.set_all_source_error(1, None).unwrap();
        set.refresh_sources(&inputs, &a, "BTC", 1_000);
        assert_eq!(set.instances[0].external.as_ref().unwrap().oi.len(), 2);
        assert!(set.instances[0].external.as_ref().unwrap().complete);
    }
}
