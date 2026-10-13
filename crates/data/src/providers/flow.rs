use std::{
    fmt::Write,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use mexc_model::{FlowQuality, VolumeFlow};
use rust_decimal::Decimal;

/// Parse native quantity without first rounding it through binary floating point.
pub fn amount(text: &str) -> Result<Decimal> {
    let text = text.trim();
    let value = if text.contains(['e', 'E']) {
        Decimal::from_scientific(text)
    } else {
        Decimal::from_str_exact(text)
    }
    .context("unrepresentable native flow quantity")?;
    ensure!(value >= Decimal::ZERO, "negative native flow quantity");
    Ok(value)
}

pub(crate) fn rounded_float(value: Decimal) -> Result<f64> {
    // Decimal::to_f64 can round its mantissa and division separately.
    // Decimal text + the standard parser rounds once, with a stack-only buffer.
    let mut text = arrayvec::ArrayString::<64>::new();
    write!(&mut text, "{value}").context("formatting native decimal")?;
    text.parse().context("unrepresentable native flow")
}

/// Convert only the final decimal difference/total, not each execution operand.
pub fn snapshot(
    buy: Decimal,
    sell: Decimal,
    quality: FlowQuality,
    covered_from: i64,
    through_ms: i64,
) -> Result<VolumeFlow> {
    ensure!(
        quality != FlowQuality::Missing,
        "missing flow has no numeric snapshot"
    );
    ensure!(
        buy >= Decimal::ZERO && sell >= Decimal::ZERO,
        "negative native flow quantity"
    );
    let delta = rounded_float(buy.checked_sub(sell).context("native delta overflow")?)?;
    let volume = rounded_float(
        buy.checked_add(sell)
            .context("native flow volume overflow")?,
    )?;
    ensure!(
        delta.is_finite() && volume.is_finite(),
        "non-finite native flow"
    );
    ensure!(
        covered_from >= 0 && through_ms >= covered_from.saturating_mul(1000),
        "invalid native flow coverage"
    );
    Ok(VolumeFlow {
        delta: Some(delta),
        volume: Some(volume),
        quality,
        covered_from,
        through_ms,
    })
}

pub fn missing(covered_from: i64, through_ms: i64) -> VolumeFlow {
    VolumeFlow {
        delta: None,
        volume: None,
        quality: FlowQuality::Missing,
        covered_from,
        through_ms,
    }
}

pub fn unix_millis() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock predates Unix epoch")
            .as_millis(),
    )
    .expect("clock exceeds timestamp range")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_subtraction_preserves_small_native_imbalance() {
        let flow = snapshot(
            amount("1000000000000000.00000001").unwrap(),
            amount("1000000000000000.00000000").unwrap(),
            FlowQuality::Exact,
            300,
            600_000,
        )
        .unwrap();
        assert_eq!(flow.delta, Some(0.00000001));
        assert_eq!(flow.volume, Some(2_000_000_000_000_000.0));
    }

    #[test]
    fn native_quantity_rejects_negative_or_unrepresentable_precision() {
        assert!(amount("-0.1").is_err());
        assert!(amount("0.00000000000000000000000000001").is_err());
        assert_eq!(amount("1e-8").unwrap(), amount("0.00000001").unwrap());
    }
}
