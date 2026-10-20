// Public native API references (no volume/notional proxy or private endpoints):
// Binance inverse base OI: https://developers.binance.com/legacy-docs/derivatives/coin-margined-futures/market-data/rest-api/Open-Interest-Statistics
// OKX: https://www.okx.com/docs-v5/en/#trading-statistics-rest-api-get-contract-open-interest-history
// MEXC: https://www.mexc.com/api-docs/futures/market-endpoints/get-ticker-contract-market-data
// INTX: https://docs.cdp.coinbase.com/api-reference/international-exchange-api/rest-api/instruments/get-instrument-details
// MEXC contract size: https://www.mexc.com/api-docs/futures/market-endpoints/get-contract-info

use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use mexc_model::{
    Exchange, Instrument, Market, OpenInterestEvent, OpenInterestPage, OpenInterestSample,
    OpenInterestSupport, OpenInterestTimestamp, Product, QuantityMode,
};
use reqwest::Client;
use rust_decimal::{Decimal, prelude::FromPrimitive};
use serde_json::Value;
use tokio::{
    sync::{mpsc, watch},
    time::{Instant, MissedTickBehavior, interval, timeout},
};

use crate::{
    DataService, IO_TIMEOUT,
    providers::{get_json, integer, number},
};

fn receipt_time_ms() -> Result<i64> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("OI observation clock predates Unix epoch")?;
    i64::try_from(elapsed.as_millis()).context("OI observation timestamp overflow")
}

const HISTORY_INTERVAL_MS: i64 = 300_000;
const LIVE_INTERVAL_MS: i64 = 5_000;
const OKX: &str = "https://www.okx.com/api/v5";
const MEXC: &str = "https://api.mexc.com/api/v1/contract";
const INTX: &str = "https://api.international.coinbase.com/api/v1";

impl DataService {
    /// Native quantities in native timestamp order, independent of candle interval and CVD.
    pub async fn open_interest_history(
        &self,
        market: &Market,
        before_ms: Option<i64>,
    ) -> Result<OpenInterestPage> {
        let (catalog, index) = self.validate_market(market).await?;
        history(
            &self.state.http,
            &catalog.instruments[index],
            market,
            before_ms,
            QuantityMode::Native,
        )
        .await
    }
    pub async fn open_interest_history_in_base(
        &self,
        market: &Market,
        before_ms: Option<i64>,
    ) -> Result<OpenInterestPage> {
        let (catalog, index) = self.validate_instrument(market).await?;
        history(
            &self.state.http,
            &catalog.instruments[index],
            market,
            before_ms,
            QuantityMode::Base,
        )
        .await
    }

    /// An owned, cancellation-safe producer: bootstrap history, then public snapshots.
    /// A history failure is reported without preventing valid live observations.
    pub async fn open_interest_stream(
        &self,
        market: Market,
        sender: mpsc::Sender<OpenInterestEvent>,
        shutdown: watch::Receiver<bool>,
    ) -> Result<()> {
        self.open_interest_stream_with_quantity(market, sender, shutdown, QuantityMode::Native)
            .await
    }

    pub async fn open_interest_stream_in_base(
        &self,
        market: Market,
        sender: mpsc::Sender<OpenInterestEvent>,
        shutdown: watch::Receiver<bool>,
    ) -> Result<()> {
        self.open_interest_stream_with_quantity(market, sender, shutdown, QuantityMode::Base)
            .await
    }

    async fn open_interest_stream_with_quantity(
        &self,
        market: Market,
        sender: mpsc::Sender<OpenInterestEvent>,
        mut shutdown: watch::Receiver<bool>,
        quantity: QuantityMode,
    ) -> Result<()> {
        if *shutdown.borrow() || sender.is_closed() {
            return Ok(());
        }
        let session = async {
            let result = self.run_open_interest(&market, &sender, quantity).await;
            if let Err(error) = &result {
                publish(
                    &sender,
                    OpenInterestEvent::Status {
                        connected: false,
                        message: format!("Open interest: {error:#}"),
                    },
                )
                .await?;
            }
            result
        };
        // This select surrounds the entire session, including validation, every HTTP
        // body read, queue send, and timer wait. Dropping it cancels all owned work.
        tokio::select! {
            biased;
            _ = shutdown.changed() => Ok(()),
            _ = sender.closed() => Ok(()),
            result = session => result,
        }
    }

    async fn run_open_interest(
        &self,
        market: &Market,
        sender: &mpsc::Sender<OpenInterestEvent>,
        quantity: QuantityMode,
    ) -> Result<()> {
        let (catalog, index) = if quantity == QuantityMode::Base {
            self.validate_instrument(market).await?
        } else {
            self.validate_market(market).await?
        };
        let instrument = &catalog.instruments[index];
        let capability = support(instrument, quantity);
        publish(sender, OpenInterestEvent::Support(capability.clone())).await?;
        let mut history_error =
            match history(&self.state.http, instrument, market, None, quantity).await {
                Ok(page) => {
                    publish(sender, OpenInterestEvent::History(Arc::new(page))).await?;
                    None
                }
                Err(error) => {
                    let message = format!("Open interest history failed: {error:#}");
                    publish(
                        sender,
                        OpenInterestEvent::Status {
                            connected: false,
                            message: message.clone(),
                        },
                    )
                    .await?;
                    Some(message)
                }
            };
        if capability.live_interval_ms.is_none() {
            publish(
                sender,
                OpenInterestEvent::Status {
                    connected: false,
                    message: capability.description,
                },
            )
            .await?;
            return Ok(());
        }
        let mut ticks = interval(Duration::from_millis(
            capability.live_interval_ms.context("missing OI cadence")? as u64,
        ));
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut connected = false;
        let mut previous = None;
        let mut history_loaded_at = Instant::now();
        loop {
            ticks.tick().await;
            let live_error;
            match current(&self.state.http, instrument, quantity).await {
                Ok(sample) => {
                    if !connected {
                        let message = status_message(&capability, history_error.as_deref(), None);
                        publish(
                            sender,
                            OpenInterestEvent::Status {
                                connected: true,
                                message,
                            },
                        )
                        .await?;
                        connected = true;
                    }
                    live_error = None;
                    if previous != Some(sample) {
                        publish(sender, OpenInterestEvent::Sample(sample)).await?;
                        previous = Some(sample);
                    }
                }
                Err(error) => {
                    connected = false;
                    live_error = Some(format!("Open interest live failed: {error:#}"));
                    publish(
                        sender,
                        OpenInterestEvent::Status {
                            connected: false,
                            message: status_message(
                                &capability,
                                history_error.as_deref(),
                                live_error.as_deref(),
                            ),
                        },
                    )
                    .await?;
                }
            }
            // Refresh actual native history, not synthesized live buckets. Failed
            // bootstraps retry once a minute; healthy pages refresh at native 5m cadence.
            let refresh_after = if history_error.is_some() { 60 } else { 300 };
            if capability.history_interval_ms.is_some()
                && history_loaded_at.elapsed() >= Duration::from_secs(refresh_after)
            {
                match history(&self.state.http, instrument, market, None, quantity).await {
                    Ok(page) => {
                        publish(sender, OpenInterestEvent::History(Arc::new(page))).await?;
                        history_error = None;
                    }
                    Err(error) => {
                        history_error = Some(format!("Open interest history failed: {error:#}"));
                    }
                }
                history_loaded_at = Instant::now();
                publish(
                    sender,
                    OpenInterestEvent::Status {
                        connected,
                        message: status_message(
                            &capability,
                            history_error.as_deref(),
                            live_error.as_deref(),
                        ),
                    },
                )
                .await?;
            }
        }
    }
}

async fn publish(sender: &mpsc::Sender<OpenInterestEvent>, event: OpenInterestEvent) -> Result<()> {
    timeout(IO_TIMEOUT, sender.send(event))
        .await
        .context("open-interest event queue stalled")?
        .context("open-interest consumer closed")
}

fn status_message(
    support: &OpenInterestSupport,
    history_error: Option<&str>,
    live_error: Option<&str>,
) -> String {
    let mut message = support.description.clone();
    for error in [history_error, live_error].into_iter().flatten() {
        message.push_str(" · ");
        message.push_str(error);
    }
    message
}

fn support(instrument: &Instrument, quantity: QuantityMode) -> OpenInterestSupport {
    if instrument.product != Product::Perpetual {
        return OpenInterestSupport {
            unit: String::new(),
            history_interval_ms: None,
            live_interval_ms: None,
            description: "Open interest unavailable for spot markets; perpetuals only.".into(),
        };
    }
    if quantity == QuantityMode::Base {
        let (history_interval_ms, live_interval_ms, description) = match instrument.exchange {
            Exchange::Binance if instrument.inverse => (
                Some(HISTORY_INTERVAL_MS),
                Some(HISTORY_INTERVAL_MS),
                "Native COIN-M sumOpenInterestValue base quantity; latest 30 days, native 5m periodic observations and venue as-of timestamps; no authoritative public 5s base OI.",
            ),
            Exchange::Binance => (
                Some(HISTORY_INTERVAL_MS),
                Some(LIVE_INTERVAL_MS),
                "Native USD-M base quantity; 5m history (latest month), 5s REST polling; venue timestamps.",
            ),
            Exchange::Okx => (
                Some(HISTORY_INTERVAL_MS),
                Some(LIVE_INTERVAL_MS),
                "Native OKX oiCcy base quantity; 5m history (latest 1,440 entries), 5s REST polling; venue timestamps.",
            ),
            Exchange::Mexc if !instrument.inverse => (
                None,
                Some(LIVE_INTERVAL_MS),
                "Base quantity from native holdVol contracts × catalog contract size; history unavailable; 5s REST polling, observed receipt time; ticker timestamp is trade time, not OI as-of.",
            ),
            Exchange::Mexc => (
                None,
                None,
                "Base OI conversion unsupported for MEXC inverse contracts.",
            ),
            Exchange::Coinbase => (
                None,
                None,
                "Base OI unavailable: INTX documents open_interest as number of active positions without an authoritative base-quantity unit; native positions cannot be safely aggregated.",
            ),
        };
        return OpenInterestSupport {
            unit: instrument.base_coin.clone(),
            history_interval_ms,
            live_interval_ms,
            description: description.into(),
        };
    }
    let (unit, history_interval_ms, description) = match instrument.exchange {
        Exchange::Binance => (
            if instrument.inverse {
                "contracts".to_owned()
            } else {
                instrument.base_coin.clone()
            },
            Some(HISTORY_INTERVAL_MS),
            if instrument.inverse {
                "Native COIN-M perpetual contracts; 5m history (latest 30 days), 5s REST polling; venue timestamps."
            } else {
                "Native USD-M base quantity; 5m history (latest month), 5s REST polling; venue timestamps."
            },
        ),
        Exchange::Okx => (
            "contracts".into(),
            Some(HISTORY_INTERVAL_MS),
            "Native per-instrument contracts; 5m history (latest 1,440 entries), 5s REST polling; venue timestamps.",
        ),
        Exchange::Mexc => (
            "contracts".into(),
            None,
            "Native holdVol contracts; history unavailable (no documented public OI history); 5s REST polling, observed receipt time (ticker timestamp is last trade, not OI as-of).",
        ),
        // REST open_interest has no OI as-of timestamp. quote.timestamp is only a
        // quote update; the timestamped RISK socket requires API credentials.
        Exchange::Coinbase => (
            "native positions".into(),
            None,
            "Native INTX open_interest positions; history unavailable (no documented public OI history); 5s REST polling, observed receipt time (not venue OI as-of).",
        ),
    };
    OpenInterestSupport {
        unit,
        history_interval_ms,
        live_interval_ms: Some(LIVE_INTERVAL_MS),
        description: description.into(),
    }
}

fn binance_host(instrument: &Instrument) -> &'static str {
    if instrument.inverse {
        "https://dapi.binance.com"
    } else {
        "https://fapi.binance.com"
    }
}

fn binance_history_query(
    instrument: &Instrument,
    before_ms: Option<i64>,
) -> Result<Vec<(&'static str, String)>> {
    let mut query = vec![("period", "5m".into()), ("limit", "500".into())];
    if instrument.inverse {
        let pair = instrument
            .symbol
            .strip_suffix("_PERP")
            .context("Binance inverse OI requires a native perpetual symbol ending in _PERP")?;
        query.push(("pair", pair.into()));
        query.push(("contractType", "PERPETUAL".into()));
    } else {
        query.push(("symbol", instrument.symbol.clone()));
    }
    if let Some(before) = before_ms {
        query.push((
            "endTime",
            before
                .checked_sub(1)
                .context("OI cursor underflow")?
                .to_string(),
        ));
    }
    Ok(query)
}

fn binance_retention_floor(inverse: bool, now: chrono::DateTime<chrono::Utc>) -> Result<i64> {
    if inverse {
        Ok((now - chrono::Duration::days(30)).timestamp_millis())
    } else {
        Ok(now
            .checked_sub_months(chrono::Months::new(1))
            .context("computing Binance OI retention")?
            .timestamp_millis())
    }
}

async fn history(
    client: &Client,
    instrument: &Instrument,
    market: &Market,
    before_ms: Option<i64>,
    quantity: QuantityMode,
) -> Result<OpenInterestPage> {
    let capability = support(instrument, quantity);
    let mut samples = Vec::new();
    if capability.history_interval_ms.is_some() && !before_ms.is_some_and(|before| before <= 0) {
        match instrument.exchange {
            Exchange::Binance => {
                // Avoid querying beyond documented retention. These are query
                // boundaries, never fabricated observation timestamps.
                let now =
                    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(receipt_time_ms()?)
                        .context("OI retention timestamp out of range")?;
                let floor = binance_retention_floor(instrument.inverse, now)?;
                if !before_ms.is_some_and(|before| before <= floor) {
                    let rows: Vec<Value> = get_json(
                        client,
                        &format!("{}/futures/data/openInterestHist", binance_host(instrument)),
                        &binance_history_query(instrument, before_ms)?,
                    )
                    .await?;
                    samples.reserve(rows.len());
                    for row in rows {
                        samples.push(parse_binance_history(&row, instrument, quantity)?);
                    }
                }
            }
            Exchange::Okx => {
                let mut query = vec![
                    ("instId", instrument.symbol.clone()),
                    ("period", "5m".into()),
                    ("limit", "100".into()),
                ];
                if let Some(before) = before_ms {
                    // OKX end is already exclusive.
                    query.push(("end", before.to_string()));
                }
                let response: Value = get_json(
                    client,
                    &format!("{OKX}/rubik/stat/contracts/open-interest-history"),
                    &query,
                )
                .await?;
                let rows = okx_data(&response)?;
                samples.reserve(rows.len());
                for row in rows {
                    samples.push(parse_okx_history(row, quantity)?);
                }
            }
            Exchange::Mexc | Exchange::Coinbase => unreachable!("history capability checked"),
        }
    }
    normalize(&mut samples, before_ms)?;
    let next_before_ms = samples.first().map(|sample| sample.timestamp_ms);
    Ok(OpenInterestPage {
        market: market.clone(),
        support: capability,
        samples,
        next_before_ms,
    })
}

async fn current(
    client: &Client,
    instrument: &Instrument,
    quantity: QuantityMode,
) -> Result<OpenInterestSample> {
    ensure!(
        instrument.product == Product::Perpetual,
        "OI is only available for perpetual markets"
    );
    if quantity == QuantityMode::Base
        && instrument.exchange == Exchange::Binance
        && instrument.inverse
    {
        // Current COIN-M openInterest exposes contracts only. The periodic native
        // statistic is authoritative base quantity, with its actual 5m timestamp.
        let mut query = binance_history_query(instrument, None)?;
        query
            .iter_mut()
            .find(|(key, _)| *key == "limit")
            .context("COIN-M OI missing query limit")?
            .1 = "1".into();
        let rows: Vec<Value> = get_json(
            client,
            &format!("{}/futures/data/openInterestHist", binance_host(instrument)),
            &query,
        )
        .await?;
        ensure!(
            rows.len() == 1,
            "COIN-M base OI has no unique periodic observation"
        );
        return parse_binance_history(&rows[0], instrument, quantity);
    }
    match instrument.exchange {
        Exchange::Binance => {
            let api = if instrument.inverse { "dapi" } else { "fapi" };
            let response: Value = get_json(
                client,
                &format!("{}/{api}/v1/openInterest", binance_host(instrument)),
                &[("symbol", instrument.symbol.clone())],
            )
            .await?;
            ensure_symbol(&response, "symbol", &instrument.symbol)?;
            if instrument.inverse {
                ensure_symbol(&response, "contractType", "PERPETUAL")?;
            }
            sample(
                &response["time"],
                &response["openInterest"],
                LIVE_INTERVAL_MS,
            )
        }
        Exchange::Okx => {
            let response: Value = get_json(
                client,
                &format!("{OKX}/public/open-interest"),
                &[
                    ("instType", "SWAP".into()),
                    ("instId", instrument.symbol.clone()),
                ],
            )
            .await?;
            let rows = okx_data(&response)?;
            ensure!(
                rows.len() == 1,
                "OKX OI returned {} instruments instead of one",
                rows.len()
            );
            let row = &rows[0];
            ensure_symbol(row, "instId", &instrument.symbol)?;
            ensure_symbol(row, "instType", "SWAP")?;
            sample(
                &row["ts"],
                &row[if quantity == QuantityMode::Base {
                    "oiCcy"
                } else {
                    "oi"
                }],
                LIVE_INTERVAL_MS,
            )
        }
        Exchange::Mexc => {
            let response: Value = get_json(
                client,
                &format!("{MEXC}/ticker"),
                &[("symbol", instrument.symbol.clone())],
            )
            .await?;
            let factor = if quantity == QuantityMode::Base {
                Some(crate::base_quantity_factor(instrument)?)
            } else {
                None
            };
            parse_mexc_current(&response, &instrument.symbol, receipt_time_ms()?, factor)
        }
        Exchange::Coinbase => {
            ensure!(
                quantity == QuantityMode::Native,
                "INTX open_interest positions lack authoritative base quantity units"
            );
            let response: Value = get_json(
                client,
                &format!("{INTX}/instruments/{}", instrument.symbol),
                &[],
            )
            .await?;
            // Captured after the actual response arrives, not a claimed venue timestamp.
            parse_coinbase_current(&response, &instrument.symbol, receipt_time_ms()?)
        }
    }
}

fn ensure_symbol(row: &Value, field: &str, expected: &str) -> Result<()> {
    ensure!(
        row[field].as_str() == Some(expected),
        "OI {field} does not match requested {expected}"
    );
    Ok(())
}

fn sample(timestamp: &Value, quantity: &Value, interval_ms: i64) -> Result<OpenInterestSample> {
    let sample = OpenInterestSample {
        timestamp_ms: integer(timestamp).context("OI missing/invalid native timestamp")?,
        timestamp_source: OpenInterestTimestamp::Venue,
        quantity: number(quantity).context("OI missing/invalid native quantity")?,
        interval_ms,
    };
    validate(sample)?;
    Ok(sample)
}

fn validate(sample: OpenInterestSample) -> Result<()> {
    ensure!(sample.timestamp_ms >= 0, "negative OI timestamp");
    ensure!(
        sample.quantity.is_finite() && sample.quantity >= 0.0,
        "invalid native OI quantity"
    );
    ensure!(sample.interval_ms > 0, "invalid OI observation cadence");
    Ok(())
}

fn normalize(samples: &mut Vec<OpenInterestSample>, before_ms: Option<i64>) -> Result<()> {
    for &sample in samples.iter() {
        validate(sample)?;
    }
    samples.retain(|sample| before_ms.is_none_or(|before| sample.timestamp_ms < before));
    samples.sort_by_key(|sample| sample.timestamp_ms);
    // Repeated identical source observations are harmless, conflicting duplicates are not.
    for pair in samples.windows(2) {
        ensure!(
            pair[0].timestamp_ms != pair[1].timestamp_ms || pair[0] == pair[1],
            "conflicting native OI observations at one timestamp"
        );
    }
    samples.dedup_by_key(|sample| sample.timestamp_ms);
    Ok(())
}

fn parse_binance_history(
    row: &Value,
    instrument: &Instrument,
    quantity: QuantityMode,
) -> Result<OpenInterestSample> {
    if instrument.inverse {
        let pair = instrument
            .symbol
            .strip_suffix("_PERP")
            .context("invalid Binance inverse perpetual identity")?;
        ensure_symbol(row, "pair", pair)?;
        ensure_symbol(row, "contractType", "PERPETUAL")?;
    } else {
        ensure_symbol(row, "symbol", &instrument.symbol)?;
    }
    sample(
        &row["timestamp"],
        &row[if quantity == QuantityMode::Base && instrument.inverse {
            "sumOpenInterestValue"
        } else {
            "sumOpenInterest"
        }],
        HISTORY_INTERVAL_MS,
    )
}

fn okx_data(response: &Value) -> Result<&Vec<Value>> {
    ensure!(
        response["code"].as_str() == Some("0"),
        "OKX OI API error {}: {}",
        response["code"],
        response["msg"]
    );
    response["data"].as_array().context("OKX OI missing data")
}

fn parse_okx_history(row: &Value, quantity: QuantityMode) -> Result<OpenInterestSample> {
    let fields = row
        .as_array()
        .context("OKX OI history row is not an array")?;
    ensure!(
        fields.len() >= 4,
        "OKX OI history row missing native fields"
    );
    sample(
        &fields[0],
        &fields[if quantity == QuantityMode::Base { 2 } else { 1 }],
        HISTORY_INTERVAL_MS,
    )
}

fn parse_mexc_current(
    response: &Value,
    symbol: &str,
    observed_ms: i64,
    base_factor: Option<f64>,
) -> Result<OpenInterestSample> {
    ensure!(
        response["success"].as_bool() == Some(true) && response["code"].as_i64() == Some(0),
        "MEXC OI API error {}: {}",
        response["code"],
        response["message"]
    );
    let row = &response["data"];
    ensure_symbol(row, "symbol", symbol)?;
    if let Some(factor) = base_factor {
        let quantity = match &row["holdVol"] {
            Value::String(text) => crate::providers::flow::amount(text)?,
            Value::Number(value) => crate::providers::flow::amount(&value.to_string())?,
            _ => anyhow::bail!("MEXC OI missing native holdVol quantity"),
        };
        let factor = Decimal::from_f64(factor).context("unrepresentable MEXC contract size")?;
        ensure!(factor > Decimal::ZERO, "invalid MEXC base contract size");
        let base = quantity
            .checked_mul(factor)
            .context("MEXC base OI quantity overflow")?;
        ensure!(
            quantity == Decimal::ZERO || base > Decimal::ZERO,
            "MEXC base OI quantity underflow"
        );
        let observation = OpenInterestSample {
            timestamp_ms: observed_ms,
            timestamp_source: OpenInterestTimestamp::Observed,
            quantity: crate::providers::flow::rounded_float(base)?,
            interval_ms: LIVE_INTERVAL_MS,
        };
        validate(observation)?;
        Ok(observation)
    } else {
        observed_sample(&row["holdVol"], observed_ms)
    }
}

fn parse_coinbase_current(
    response: &Value,
    symbol: &str,
    observed_ms: i64,
) -> Result<OpenInterestSample> {
    ensure_symbol(response, "symbol", symbol)?;
    ensure_symbol(response, "type", "PERP")?;
    observed_sample(&response["open_interest"], observed_ms)
}

fn observed_sample(quantity: &Value, observed_ms: i64) -> Result<OpenInterestSample> {
    let sample = OpenInterestSample {
        timestamp_ms: observed_ms,
        timestamp_source: OpenInterestTimestamp::Observed,
        quantity: number(quantity).context("missing/invalid observed native OI quantity")?,
        interval_ms: LIVE_INTERVAL_MS,
    };
    validate(sample)?;
    Ok(sample)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mexc_model::Quote;
    use serde_json::json;

    fn instrument(exchange: Exchange, symbol: &str, inverse: bool) -> Instrument {
        Instrument {
            exchange,
            product: Product::Perpetual,
            symbol: symbol.into(),
            base_coin: "BTC".into(),
            quote_coin: Quote::USDT,
            settle_coin: "BTC".into(),
            price_scale: 1,
            tick_size: None,
            contract_size: 100.0,
            inverse,
            volume_unit: "contracts".into(),
        }
    }

    #[test]
    fn inverse_history_is_perpetual_only_and_cursor_is_exclusive() {
        let instrument = instrument(Exchange::Binance, "BTCUSD_PERP", true);
        let query = binance_history_query(&instrument, Some(600_000)).unwrap();
        assert!(query.contains(&("pair", "BTCUSD".into())));
        assert!(query.contains(&("contractType", "PERPETUAL".into())));
        assert!(query.contains(&("endTime", "599999".into())));
        assert!(binance_history_query(&instrument, Some(i64::MIN)).is_err());
        let row = json!({"pair":"BTCUSD", "contractType":"PERPETUAL", "timestamp":300000, "sumOpenInterest":"23", "sumOpenInterestValue":"999999"});
        assert_eq!(
            parse_binance_history(&row, &instrument, QuantityMode::Native)
                .unwrap()
                .quantity,
            23.0
        );
        let mut delivery = row;
        delivery["contractType"] = json!("CURRENT_QUARTER");
        assert!(parse_binance_history(&delivery, &instrument, QuantityMode::Native).is_err());
    }

    #[test]
    fn binance_retention_keeps_calendar_month_and_thirty_day_boundaries_distinct() {
        let utc = |text: &str| {
            chrono::DateTime::parse_from_rfc3339(text)
                .unwrap()
                .with_timezone(&chrono::Utc)
        };
        let now = utc("2024-03-31T12:00:00Z");
        assert_eq!(
            binance_retention_floor(false, now).unwrap(),
            utc("2024-02-29T12:00:00Z").timestamp_millis()
        );
        assert_eq!(
            binance_retention_floor(true, now).unwrap(),
            utc("2024-03-01T12:00:00Z").timestamp_millis()
        );
    }

    #[test]
    fn native_history_sorts_deduplicates_and_keeps_zero_without_resampling() {
        let make = |time, value| sample(&json!(time), &json!(value), HISTORY_INTERVAL_MS).unwrap();
        let mut samples = vec![
            make(900_001, 9),
            make(300_017, 0),
            make(300_017, 0),
            make(600_010, 6),
        ];
        normalize(&mut samples, Some(900_001)).unwrap();
        assert_eq!(samples, vec![make(300_017, 0), make(600_010, 6)]);
        let mut conflict = vec![make(1, 0), make(1, 1)];
        assert!(normalize(&mut conflict, None).is_err());
        assert!(sample(&json!(1), &json!("NaN"), HISTORY_INTERVAL_MS).is_err());
        assert!(sample(&json!(1), &json!(-1), HISTORY_INTERVAL_MS).is_err());
    }

    #[test]
    fn okx_and_mexc_preserve_contracts_and_distinguish_timestamp_provenance() {
        let okx = parse_okx_history(
            &json!(["1701417600017", "12.5", "999", "8888888"]),
            QuantityMode::Native,
        )
        .unwrap();
        assert_eq!(okx.quantity, 12.5);
        assert_eq!(okx.timestamp_ms, 1_701_417_600_017);
        let mexc = json!({"success":true,"code":0,"data":{"symbol":"BTC_USDT","holdVol":0,"timestamp":1701417600041_i64}});
        let parsed = parse_mexc_current(&mexc, "BTC_USDT", 1_701_417_600_099, None).unwrap();
        assert_eq!(parsed.quantity, 0.0);
        assert_eq!(parsed.timestamp_source, OpenInterestTimestamp::Observed);
        assert_eq!(parsed.timestamp_ms, 1_701_417_600_099);
        assert!(parse_mexc_current(&mexc, "ETH_USDT", 1, None).is_err());
        assert!(
            parse_mexc_current(&json!({"success":false,"code":500}), "BTC_USDT", 1, None).is_err()
        );
        assert!(parse_okx_history(&json!([1, 2]), QuantityMode::Native).is_err());
    }

    #[test]
    fn intx_untimestamped_native_quantity_is_observed_not_quote_time() {
        let response = json!({"symbol":"BTC-PERP","type":"PERP","open_interest":"32.5","quote":{"timestamp":"2020-01-01T00:00:00Z"}});
        let parsed = parse_coinbase_current(&response, "BTC-PERP", 1701417600099).unwrap();
        assert_eq!(parsed.quantity, 32.5);
        assert_eq!(parsed.timestamp_ms, 1_701_417_600_099);
        assert_eq!(parsed.timestamp_source, OpenInterestTimestamp::Observed);
        assert!(parse_coinbase_current(&response, "ETH-PERP", 1).is_err());
    }
    #[test]
    fn base_interest_uses_authoritative_fields_and_declared_cadence() {
        let inverse = instrument(Exchange::Binance, "BTCUSD_PERP", true);
        let row = json!({
            "pair":"BTCUSD", "contractType":"PERPETUAL",
            "timestamp":300017, "sumOpenInterest":"23", "sumOpenInterestValue":"0.12345678"
        });
        let base = parse_binance_history(&row, &inverse, QuantityMode::Base).unwrap();
        assert_eq!(base.quantity, 0.12345678);
        assert_eq!(base.timestamp_ms, 300017);
        assert_eq!(base.timestamp_source, OpenInterestTimestamp::Venue);
        assert_eq!(base.interval_ms, HISTORY_INTERVAL_MS);
        assert_eq!(
            support(&inverse, QuantityMode::Base).live_interval_ms,
            Some(HISTORY_INTERVAL_MS)
        );
        let mut missing = row;
        missing
            .as_object_mut()
            .unwrap()
            .remove("sumOpenInterestValue");
        assert!(parse_binance_history(&missing, &inverse, QuantityMode::Base).is_err());
        let okx = parse_okx_history(
            &json!(["300017", "123", "0.0042", "888"]),
            QuantityMode::Base,
        )
        .unwrap();
        assert_eq!(okx.quantity, 0.0042);
        assert_eq!(okx.timestamp_ms, 300017);
    }

    #[test]
    fn mexc_base_interest_multiplies_decimal_contracts_and_preserves_zero() {
        let mut response = json!({"success":true,"code":0,"data":{"symbol":"BTC_USDT","holdVol":"23","timestamp":1}});
        let base = parse_mexc_current(&response, "BTC_USDT", 300017, Some(0.0001)).unwrap();
        assert_eq!(base.quantity, 0.0023);
        assert_eq!(base.timestamp_ms, 300017);
        assert_eq!(base.timestamp_source, OpenInterestTimestamp::Observed);
        response["data"]["holdVol"] = json!(0);
        assert_eq!(
            parse_mexc_current(&response, "BTC_USDT", 300017, Some(0.0001))
                .unwrap()
                .quantity,
            0.0
        );
        response["data"]["holdVol"] = json!(-1);
        assert!(parse_mexc_current(&response, "BTC_USDT", 300017, Some(0.0001)).is_err());
        response["data"]["holdVol"] = json!("1e-28");
        assert!(parse_mexc_current(&response, "BTC_USDT", 300017, Some(0.0001)).is_err());
    }

    #[test]
    fn undocumented_intx_positions_are_not_base_interest() {
        let native = instrument(Exchange::Coinbase, "BTC-PERP", false);
        assert!(
            support(&native, QuantityMode::Native)
                .live_interval_ms
                .is_some()
        );
        let base = support(&native, QuantityMode::Base);
        assert!(base.history_interval_ms.is_none());
        assert!(base.live_interval_ms.is_none());
        assert!(crate::base_quantity_factor(&native).is_err());
    }
}
