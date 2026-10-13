use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use mexc_model::{
    Candle, Catalog, Exchange, FlowQuality, Instrument, Market, MarketEvent, PAGE_SIZE, Product,
    QuantityMode, Quote, Timeframe, VolumeFlow,
};
use reqwest::Client;
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use super::{flow, get_json, history_end, history_page, integer, number, unix_seconds};

const SPOT_REST: &str = "https://api.binance.com/api/v3";
const UM_REST: &str = "https://fapi.binance.com/fapi/v1";
const CM_REST: &str = "https://dapi.binance.com/dapi/v1";
const SPOT_WS: &str = "wss://stream.binance.com:9443/ws";
const UM_WS: &str = "wss://fstream.binance.com/market/ws";
const CM_WS: &str = "wss://dstream.binance.com/ws";
const IO_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn catalog(client: &Client, product: Product) -> Result<Catalog> {
    let (url, inverse, spot_query) = match product {
        Product::Spot => (
            format!("{SPOT_REST}/exchangeInfo"),
            false,
            vec![
                ("symbolStatus", "TRADING".to_owned()),
                ("showPermissionSets", "false".to_owned()),
            ],
        ),
        Product::Perpetual => (format!("{UM_REST}/exchangeInfo"), false, Vec::new()),
    };
    let mut instruments = Vec::new();
    let value: Value = get_json(client, &url, &spot_query)
        .await
        .context("fetching Binance exchange metadata")?;
    append_symbols(&mut instruments, &value, product, inverse)?;
    if product == Product::Perpetual {
        let value: Value = get_json(client, &format!("{CM_REST}/exchangeInfo"), &[])
            .await
            .context("fetching Binance COIN-M exchange metadata")?;
        append_symbols(&mut instruments, &value, product, true)?;
    }
    Ok(Catalog {
        exchange: Exchange::Binance,
        product,
        intervals: Timeframe::ALL.to_vec(),
        instruments,
        flow_support: super::flow_support(Exchange::Binance, product),
    })
}

fn append_symbols(
    out: &mut Vec<Instrument>,
    response: &Value,
    product: Product,
    inverse: bool,
) -> Result<()> {
    let symbols = response
        .get("symbols")
        .and_then(Value::as_array)
        .context("Binance exchangeInfo missing symbols")?;
    for row in symbols {
        let status = if inverse {
            row.get("contractStatus")
        } else {
            row.get("status")
        }
        .and_then(Value::as_str);
        if status != Some("TRADING") {
            continue;
        }
        if product == Product::Perpetual
            && row.get("contractType").and_then(Value::as_str) != Some("PERPETUAL")
        {
            continue;
        }
        let symbol = match row.get("symbol").and_then(Value::as_str) {
            Some(s) => s,
            None => continue,
        };
        let base = match row.get("baseAsset").and_then(Value::as_str) {
            Some(s) => s,
            None => continue,
        };
        let quote = match row
            .get("quoteAsset")
            .and_then(Value::as_str)
            .and_then(Quote::parse)
        {
            Some(q) => q,
            None => continue,
        };
        let settle = if product == Product::Spot {
            quote.to_string()
        } else {
            match row.get("marginAsset").and_then(Value::as_str) {
                Some(s) => s.to_owned(),
                None => continue,
            }
        };
        let tick = row
            .get("filters")
            .and_then(Value::as_array)
            .and_then(|fs| {
                fs.iter()
                    .find(|f| f.get("filterType").and_then(Value::as_str) == Some("PRICE_FILTER"))
            })
            .and_then(|f| f.get("tickSize"))
            .and_then(Value::as_str)
            .context("Binance PRICE_FILTER missing tickSize")?;
        let price_scale = tick
            .split_once('.')
            .map(|(_, decimals)| decimals.trim_end_matches('0').len() as u32)
            .unwrap_or(0);
        let contract_size = if inverse {
            row.get("contractSize")
                .and_then(Value::as_f64)
                .or_else(|| {
                    row.get("contractSize")
                        .and_then(Value::as_str)
                        .and_then(|s| s.parse().ok())
                })
                .context("Binance COIN-M symbol missing contractSize")?
        } else {
            1.0
        };
        if !contract_size.is_finite() || contract_size <= 0.0 {
            bail!("Binance symbol has invalid contract size");
        }
        out.push(Instrument {
            exchange: Exchange::Binance,
            product,
            symbol: symbol.to_owned(),
            base_coin: base.to_owned(),
            quote_coin: quote,
            settle_coin: settle,
            price_scale,
            contract_size,
            inverse,
            volume_unit: if inverse {
                "contracts".to_owned()
            } else {
                base.to_owned()
            },
        });
    }
    Ok(())
}

fn endpoint(instrument: &Instrument) -> Result<&'static str> {
    if instrument.exchange != Exchange::Binance {
        bail!("instrument is not a Binance market");
    }
    Ok(match (instrument.product, instrument.inverse) {
        (Product::Spot, false) => SPOT_REST,
        (Product::Perpetual, false) => UM_REST,
        (Product::Perpetual, true) => CM_REST,
        (Product::Spot, true) => bail!("Binance spot cannot be inverse"),
    })
}

fn interval_code(interval: Timeframe) -> &'static str {
    match interval {
        Timeframe::Min1 => "1m",
        Timeframe::Min5 => "5m",
        Timeframe::Min15 => "15m",
        Timeframe::Min30 => "30m",
        Timeframe::Min60 => "1h",
        Timeframe::Hour4 => "4h",
        Timeframe::Hour6 => "6h",
        Timeframe::Hour8 => "8h",
        Timeframe::Day1 => "1d",
    }
}

fn native_flow(total: &Value, taker_buy: &Value, time: i64, through_ms: i64) -> Result<VolumeFlow> {
    let total = flow::amount(
        total
            .as_str()
            .context("Binance volume is not a decimal string")?,
    )?;
    let buy = flow::amount(
        taker_buy
            .as_str()
            .context("Binance taker-buy volume is not a decimal string")?,
    )?;
    ensure!(
        buy <= total,
        "Binance taker-buy volume exceeds total volume"
    );
    let sell = total
        .checked_sub(buy)
        .context("Binance sell-volume overflow")?;
    flow::snapshot(buy, sell, FlowQuality::Exact, time, through_ms)
}

fn candle_from_history(
    row: &Value,
    inverse: bool,
    observed_ms: i64,
    quantity: QuantityMode,
) -> Result<Candle> {
    let values = row
        .as_array()
        .context("Binance kline row is not an array")?;
    // COIN-M exposes authoritative base totals alongside contract totals:
    // https://developers.binance.com/legacy-docs/derivatives/coin-margined-futures/market-data/rest-api/Kline-Candlestick-Data
    let (volume_index, buy_index) = if inverse && quantity == QuantityMode::Base {
        (7, 10)
    } else {
        (5, 9)
    };
    if values.len() <= buy_index {
        bail!("Binance kline row is missing requested taker-buy volume");
    }
    let millis = integer(&values[0])?;
    if millis < 0 || millis % 1000 != 0 {
        bail!("Binance kline has invalid millisecond open time");
    }
    Ok(Candle {
        time: millis / 1000,
        open: number(&values[1])?,
        high: number(&values[2])?,
        low: number(&values[3])?,
        close: number(&values[4])?,
        volume: number(&values[5])?,
        flow: Some(native_flow(
            &values[volume_index],
            &values[buy_index],
            millis / 1000,
            observed_ms.min(integer(&values[6])?.saturating_add(1)),
        )?),
    })
}

pub async fn history(
    client: &Client,
    instrument: &Instrument,
    market: &Market,
    before: Option<i64>,
    quantity: QuantityMode,
) -> Result<mexc_model::HistoryPage> {
    if instrument.symbol != market.symbol
        || instrument.product != market.product
        || market.exchange != Exchange::Binance
    {
        bail!("Binance history instrument and market do not match");
    }
    let end = history_end(before, unix_seconds(), market.interval)?;
    let url = format!("{}/klines", endpoint(instrument)?);
    let query = [
        ("symbol", instrument.symbol.clone()),
        ("interval", interval_code(market.interval).to_owned()),
        (
            "endTime",
            end.saturating_mul(1000).saturating_add(999).to_string(),
        ),
        ("limit", PAGE_SIZE.to_string()),
    ];
    let observed_ms = flow::unix_millis();
    let response: Value = get_json(client, &url, &query)
        .await
        .context("fetching Binance klines")?;
    let rows = response
        .as_array()
        .context("Binance klines response is not an array")?;
    let mut candles = Vec::with_capacity(rows.len());
    for row in rows {
        candles.push(candle_from_history(
            row,
            instrument.inverse,
            observed_ms,
            quantity,
        )?);
    }
    history_page(candles, end, market.clone())
}

fn candle_from_ws(
    value: &Value,
    market: &Market,
    inverse: bool,
    quantity: QuantityMode,
) -> Result<Candle> {
    let kline = value
        .get("k")
        .context("Binance stream event missing kline")?;
    let millis = integer(kline.get("t").context("Binance kline missing open time")?)?;
    if millis < 0 || millis % 1000 != 0 {
        bail!("Binance WebSocket kline has invalid millisecond open time");
    }
    // COIN-M q/Q are base amounts, not the quote amounts used by spot/USD-M:
    // https://developers.binance.com/legacy-docs/derivatives/coin-margined-futures/websocket-market-streams/Kline-Candlestick-Streams
    let (volume_key, buy_key) = if inverse && quantity == QuantityMode::Base {
        ("q", "Q")
    } else {
        ("v", "V")
    };
    let candle = Candle {
        time: millis / 1000,
        open: number(kline.get("o").context("Binance kline missing open")?)?,
        high: number(kline.get("h").context("Binance kline missing high")?)?,
        low: number(kline.get("l").context("Binance kline missing low")?)?,
        close: number(kline.get("c").context("Binance kline missing close")?)?,
        volume: number(kline.get("v").context("Binance kline missing volume")?)?,
        flow: Some(native_flow(
            kline
                .get(volume_key)
                .context("Binance kline missing requested flow volume")?,
            kline
                .get(buy_key)
                .context("Binance kline missing requested taker-buy volume")?,
            millis / 1000,
            integer(value.get("E").context("Binance kline missing event time")?)?
                .min(millis.saturating_add(market.interval.seconds().saturating_mul(1000))),
        )?),
    };
    super::validate(candle, market.interval)?;
    Ok(candle)
}

pub async fn stream(
    _client: &Client,
    instrument: &Instrument,
    market: &Market,
    _cvd: bool,
    sender: &mpsc::Sender<MarketEvent>,
    mut shutdown: watch::Receiver<bool>,
    quantity: QuantityMode,
) -> Result<()> {
    let rest = endpoint(instrument)?;
    if instrument.symbol != market.symbol
        || instrument.product != market.product
        || market.exchange != Exchange::Binance
    {
        bail!("Binance stream instrument and market do not match");
    }
    let ws_base = match rest {
        SPOT_REST => SPOT_WS,
        UM_REST => UM_WS,
        CM_REST => CM_WS,
        _ => unreachable!(),
    };
    let stream_url = format!(
        "{}/{}@kline_{}",
        ws_base,
        instrument.symbol.to_lowercase(),
        interval_code(market.interval)
    );
    if *shutdown.borrow() {
        return Ok(());
    }
    let (mut socket, _) = tokio::select! {
        _ = shutdown.changed() => return Ok(()),
        result = tokio::time::timeout(IO_TIMEOUT, connect_async(&stream_url)) => result.context("Binance WebSocket connect timed out")??,
    };
    sender
        .send(MarketEvent::Status {
            connected: true,
            message: "Connected to Binance native kline stream".into(),
        })
        .await
        .map_err(|_| anyhow!("Binance stream consumer closed"))?;
    loop {
        tokio::select! {
            changed = shutdown.changed() => { if changed.is_err() || *shutdown.borrow() { let _ = socket.close(None).await; return Ok(()); } }
            message = tokio::time::timeout(IO_TIMEOUT, socket.next()) => {
                let message = message.context("Binance stream heartbeat timed out")?.context("Binance WebSocket disconnected")??;
                match message {
                    Message::Text(text) => {
                        let value: Value = serde_json::from_str(&text).context("invalid Binance WebSocket JSON")?;
                        match value.get("e").and_then(Value::as_str) {
                            Some("kline") => {
                                let candle = candle_from_ws(&value, market, instrument.inverse, quantity)?;
                                sender.send(MarketEvent::Candle { candle }).await.map_err(|_| anyhow!("Binance stream consumer closed"))?;
                            }
                            Some("serverShutdown") => bail!("Binance WebSocket server is shutting down"),
                            _ if value.get("code").is_some() || value.get("msg").is_some() => bail!("Binance WebSocket error: {value}"),
                            _ => {}
                        }
                    }
                    Message::Ping(payload) => socket.send(Message::Pong(payload)).await.context("replying to Binance WebSocket ping")?,
                    Message::Pong(_) => {},
                    Message::Close(frame) => bail!("Binance WebSocket closed: {frame:?}"),
                    Message::Binary(_) => bail!("unexpected binary Binance WebSocket message"),
                    _ => {},
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inverse_market() -> Market {
        Market {
            exchange: Exchange::Binance,
            product: Product::Perpetual,
            symbol: "BTCUSD_PERP".into(),
            interval: Timeframe::Min5,
        }
    }

    fn inverse_ws_packet() -> Value {
        serde_json::json!({
            "e": "kline", "E": 600_000,
            "k": {
                "t": 300_000, "T": 599_999,
                "o": "10", "h": "12", "l": "9", "c": "11",
                "v": "4755", "V": "2633",
                "q": "5.69558002", "Q": "3.15385326"
            }
        })
    }

    fn inverse_history_row() -> Value {
        serde_json::json!([
            300_000,
            "10",
            "12",
            "9",
            "11",
            "206",
            599_999,
            "2.13660389",
            48,
            "119",
            "1.23424865",
            "0"
        ])
    }

    fn assert_same_candle_fields(native: &Candle, base: &Candle) {
        assert_eq!(
            (
                native.time,
                native.open,
                native.high,
                native.low,
                native.close,
                native.volume
            ),
            (
                base.time,
                base.open,
                base.high,
                base.low,
                base.close,
                base.volume
            ),
        );
        let native_flow = native.flow.unwrap();
        let base_flow = base.flow.unwrap();
        assert_eq!(native_flow.quality, base_flow.quality);
        assert_eq!(native_flow.covered_from, base_flow.covered_from);
        assert_eq!(native_flow.through_ms, base_flow.through_ms);
    }

    #[test]
    fn inverse_kline_delta_uses_contract_quantity_not_base_amount() {
        let market = inverse_market();
        let packet = inverse_ws_packet();
        let candle = candle_from_ws(&packet, &market, true, QuantityMode::Native).unwrap();
        assert_eq!(candle.flow.unwrap().delta, Some(511.0));
        assert_eq!(candle.flow.unwrap().volume, Some(4755.0));
    }

    #[test]
    fn inverse_history_base_uses_authoritative_totals_without_changing_candle() {
        let row = inverse_history_row();
        let native = candle_from_history(&row, true, 650_000, QuantityMode::Native).unwrap();
        let base = candle_from_history(&row, true, 650_000, QuantityMode::Base).unwrap();
        assert_same_candle_fields(&native, &base);
        assert_eq!(native.volume, 206.0);
        assert_eq!(native.flow.unwrap().delta, Some(32.0));
        assert_eq!(native.flow.unwrap().volume, Some(206.0));
        let base_flow = base.flow.unwrap();
        assert_eq!(base_flow.delta, Some(0.33189341));
        assert_eq!(base_flow.volume, Some(2.13660389));
        assert_eq!(base_flow.quality, FlowQuality::Exact);
        assert_eq!(base_flow.covered_from, 300);
        assert_eq!(base_flow.through_ms, 600_000);

        let partial_native =
            candle_from_history(&row, true, 450_123, QuantityMode::Native).unwrap();
        let partial_base = candle_from_history(&row, true, 450_123, QuantityMode::Base).unwrap();
        assert_same_candle_fields(&partial_native, &partial_base);
        assert_eq!(partial_base.flow.unwrap().through_ms, 450_123);
    }

    #[test]
    fn inverse_ws_base_uses_authoritative_totals_without_changing_candle() {
        let market = inverse_market();
        let packet = inverse_ws_packet();
        let native = candle_from_ws(&packet, &market, true, QuantityMode::Native).unwrap();
        let base = candle_from_ws(&packet, &market, true, QuantityMode::Base).unwrap();
        assert_same_candle_fields(&native, &base);
        assert_eq!(base.volume, 4755.0);
        let base_flow = base.flow.unwrap();
        assert_eq!(base_flow.delta, Some(0.6121265));
        assert_eq!(base_flow.volume, Some(5.69558002));
        assert_eq!(base_flow.quality, FlowQuality::Exact);
        assert_eq!(base_flow.covered_from, 300);
        assert_eq!(base_flow.through_ms, 600_000);

        let mut partial = packet.clone();
        partial["E"] = serde_json::json!(450_123);
        let partial_native = candle_from_ws(&partial, &market, true, QuantityMode::Native).unwrap();
        let partial_base = candle_from_ws(&partial, &market, true, QuantityMode::Base).unwrap();
        assert_same_candle_fields(&partial_native, &partial_base);
        assert_eq!(partial_base.flow.unwrap().through_ms, 450_123);
        partial["E"] = serde_json::json!(650_000);
        assert_eq!(
            candle_from_ws(&partial, &market, true, QuantityMode::Base)
                .unwrap()
                .flow
                .unwrap()
                .through_ms,
            600_000,
        );
    }

    #[test]
    fn inverse_base_delta_is_signed_and_preserves_decimal_boundary_imbalance() {
        let market = inverse_market();
        for (total, buy, delta, volume) in [
            ("0", "0", 0.0, 0.0),
            ("2", "0", -2.0, 2.0),
            ("2", "2", 2.0, 2.0),
            ("2", "0.5", -1.0, 2.0),
            (
                "2000000000000000.00000001",
                "1000000000000000.00000001",
                0.00000001,
                2_000_000_000_000_000.0,
            ),
        ] {
            let mut row = inverse_history_row();
            row[7] = Value::String(total.into());
            row[10] = Value::String(buy.into());
            let mut packet = inverse_ws_packet();
            packet["k"]["q"] = Value::String(total.into());
            packet["k"]["Q"] = Value::String(buy.into());
            let history = candle_from_history(&row, true, 600_000, QuantityMode::Base).unwrap();
            let ws = candle_from_ws(&packet, &market, true, QuantityMode::Base).unwrap();
            for candle in [history, ws] {
                let flow = candle.flow.unwrap();
                assert_eq!(flow.delta, Some(delta));
                assert_eq!(flow.volume, Some(volume));
                assert_eq!(flow.quality, FlowQuality::Exact);
                assert_eq!(flow.covered_from, 300);
                assert_eq!(flow.through_ms, 600_000);
            }
            assert_eq!(
                candle_from_history(&row, true, 600_000, QuantityMode::Native)
                    .unwrap()
                    .flow
                    .unwrap()
                    .delta,
                Some(32.0),
            );
            assert_eq!(
                candle_from_ws(&packet, &market, true, QuantityMode::Native)
                    .unwrap()
                    .flow
                    .unwrap()
                    .delta,
                Some(511.0),
            );
        }
    }

    #[test]
    fn inverse_base_requires_valid_base_fields_without_falling_back_to_contracts() {
        let market = inverse_market();
        for (total, buy) in [
            (Value::Null, Value::String("1".into())),
            (Value::String("2".into()), Value::Null),
            (Value::String("2".into()), Value::String("3".into())),
            (Value::String("-1".into()), Value::String("0".into())),
            (Value::String("2".into()), Value::String("-1".into())),
            (Value::String("2".into()), Value::String("NaN".into())),
            (
                Value::String("2".into()),
                Value::String("0.00000000000000000000000000001".into()),
            ),
        ] {
            let mut row = inverse_history_row();
            row[7] = total.clone();
            row[10] = buy.clone();
            let mut packet = inverse_ws_packet();
            packet["k"]["q"] = total;
            packet["k"]["Q"] = buy;
            assert!(candle_from_history(&row, true, 600_000, QuantityMode::Base).is_err());
            assert!(candle_from_ws(&packet, &market, true, QuantityMode::Base).is_err());
            assert!(candle_from_history(&row, true, 600_000, QuantityMode::Native).is_ok());
            assert!(candle_from_ws(&packet, &market, true, QuantityMode::Native).is_ok());
        }
        let mut row = inverse_history_row();
        row.as_array_mut().unwrap().truncate(10);
        assert!(candle_from_history(&row, true, 600_000, QuantityMode::Base).is_err());
        assert!(candle_from_history(&row, true, 600_000, QuantityMode::Native).is_ok());
        let mut packet = inverse_ws_packet();
        packet["k"].as_object_mut().unwrap().remove("Q");
        assert!(candle_from_ws(&packet, &market, true, QuantityMode::Base).is_err());
        assert!(candle_from_ws(&packet, &market, true, QuantityMode::Native).is_ok());
    }

    #[test]
    fn spot_and_linear_native_quantities_are_already_base() {
        let mut row = inverse_history_row();
        row[7] = Value::Null;
        row[10] = Value::Null;
        let mut packet = inverse_ws_packet();
        packet["k"]["q"] = Value::Null;
        packet["k"]["Q"] = Value::Null;
        for product in [Product::Spot, Product::Perpetual] {
            let market = Market {
                product,
                symbol: "BTCUSDT".into(),
                ..inverse_market()
            };
            let native = candle_from_history(&row, false, 600_000, QuantityMode::Native).unwrap();
            let base = candle_from_history(&row, false, 600_000, QuantityMode::Base).unwrap();
            assert_same_candle_fields(&native, &base);
            assert_eq!(base.flow.unwrap().delta, native.flow.unwrap().delta);
            assert_eq!(base.flow.unwrap().volume, native.flow.unwrap().volume);
            let native = candle_from_ws(&packet, &market, false, QuantityMode::Native).unwrap();
            let base = candle_from_ws(&packet, &market, false, QuantityMode::Base).unwrap();
            assert_same_candle_fields(&native, &base);
            assert_eq!(base.flow.unwrap().delta, native.flow.unwrap().delta);
            assert_eq!(base.flow.unwrap().volume, native.flow.unwrap().volume);
        }
    }

    #[test]
    fn malformed_taker_volume_cannot_be_replaced_with_unsigned_volume() {
        assert!(
            native_flow(
                &Value::String("10".into()),
                &Value::String("11".into()),
                300,
                600_000,
            )
            .is_err()
        );
        assert!(native_flow(&Value::String("10".into()), &Value::Null, 300, 600_000,).is_err());
    }
}
