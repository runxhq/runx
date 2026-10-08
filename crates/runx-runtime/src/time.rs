use std::time::{SystemTime, UNIX_EPOCH};

/// Deterministic fixture timestamp for harness and oracle callers that must
/// keep receipt content stable. Live runtime paths should call [`now_iso8601`].
pub(crate) const DEFAULT_CREATED_AT: &str = "2026-05-18T00:00:00Z";

pub fn now_iso8601() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = i64::try_from(duration.as_secs()).unwrap_or(i64::MAX);
    let millis = duration.subsec_millis();
    let (year, month, day, hour, minute, second) = civil_from_unix_seconds(seconds);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

pub(crate) fn iso8601_from_unix_seconds(seconds: i64) -> String {
    let (year, month, day, hour, minute, second) = civil_from_unix_seconds(seconds);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_unix_seconds(seconds: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = u32::try_from(day_seconds / 3_600).unwrap_or(0);
    let minute = u32::try_from((day_seconds % 3_600) / 60).unwrap_or(0);
    let second = u32::try_from(day_seconds % 60).unwrap_or(0);
    (year, month, day, hour, minute, second)
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096).div_euclid(365);
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2).div_euclid(153);
    let day = doy - (153 * mp + 2).div_euclid(5) + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + i64::from(month <= 2);
    (
        i32::try_from(year).unwrap_or(i32::MAX),
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

/// The instant a relative lookback such as `7d` or `24h` starts, counted back
/// from `as_of` (an RFC 3339 instant) or from now.
pub(crate) fn lookback_since(period: &str, as_of: Option<&str>) -> Result<String, String> {
    let period = period.trim();
    let (amount, unit_seconds) = if let Some(amount) = period.strip_suffix(['d', 'D']) {
        (amount, 86_400)
    } else if let Some(amount) = period.strip_suffix(['h', 'H']) {
        (amount, 3_600)
    } else {
        return Err("period must use d or h units".to_owned());
    };
    let amount = amount
        .parse::<i64>()
        .ok()
        .filter(|amount| *amount > 0)
        .ok_or("period must be a positive duration such as 7d or 24h")?;
    let as_of = as_of.map_or_else(now_iso8601, str::to_owned);
    let (days, seconds, _) = runx_core::policy::parse_rfc3339_moment(&as_of)
        .ok_or("as_of must be an RFC3339 timestamp")?;
    let epoch_seconds = days
        .checked_mul(86_400)
        .and_then(|value| value.checked_add(seconds))
        .and_then(|value| value.checked_sub(amount.saturating_mul(unit_seconds)))
        .ok_or("period is outside the supported timestamp range")?;
    Ok(iso8601_from_unix_seconds(epoch_seconds))
}
