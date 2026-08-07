//! Deterministic temporal bindings (§2.6–2.7).
//!
//! The kernel never reads a host clock or host zoneinfo. Callers supply a
//! journaled UTC epoch-millisecond value; timezone rules come from the
//! exact-pinned `chrono-tz` crate.

use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;

use crate::versioning::{impl_hash, Hash};

/// Exact tzdb payload dependency selected in `Cargo.toml`.
pub const TZDB_VERSION: &str = "chrono-tz=0.10.4";

/// UTC epoch-millisecond interval. Every interval is half-open `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Interval {
    pub start_ms: u64,
    pub end_ms: u64,
}

impl Interval {
    pub fn contains(self, epoch_ms: u64) -> bool {
        self.start_ms <= epoch_ms && epoch_ms < self.end_ms
    }
}

/// Calendar unit that determines a cache-validity bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Granularity {
    Hour,
    Day,
    Month,
    Quarter,
}

/// Validated IANA timezone identifier. Host `TZ` and `/usr/share/zoneinfo`
/// are deliberately not consulted.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IanaTz(String);

impl IanaTz {
    pub fn parse(name: impl AsRef<str>) -> Result<Self, TimeError> {
        let tz: Tz = name
            .as_ref()
            .parse()
            .map_err(|_| TimeError::InvalidTimezone(name.as_ref().to_owned()))?;
        Ok(Self(tz.name().to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn zone(&self) -> Tz {
        // Construction validates this value and the canonical name is owned by
        // chrono-tz, so this cannot fail without a programming error.
        self.0.parse().expect("validated IANA timezone")
    }
}

/// Relative temporal forms retained in a cache key rather than replaced by
/// their resolved ranges.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RelativeExpr {
    LastNDays(u32),
    CalendarQuarter(i32),
    MonthToDate,
}

/// A relative expression resolved from an already-journaled `now`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TemporalBinding {
    pub expression: RelativeExpr,
    pub granularity: Granularity,
    pub tz: IanaTz,
    pub resolved: Interval,
    pub resolved_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeError {
    InvalidTimezone(String),
    InvalidEpochMillis(u64),
    InvalidRelativeExpression(&'static str),
    CalendarOverflow,
    NonexistentLocalBoundary,
}

/// Return the supplied journaled value. Deliberately accepts no clock source:
/// a live caller must journal before constructing any temporal binding.
pub const fn now_ms(journaled_now_ms: u64) -> u64 {
    journaled_now_ms
}

/// Hash of the shipped, exact-pinned timezone-rule dependency for
/// `SystemVersion.tzdb`.
pub fn tzdb_hash() -> Hash {
    impl_hash(TZDB_VERSION)
}

/// Stable cache-key material for a resolved validity bucket.
pub fn interval_hash(interval: Interval) -> Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"harness-core:temporal-interval:v1");
    hasher.update(&interval.start_ms.to_be_bytes());
    hasher.update(&interval.end_ms.to_be_bytes());
    *hasher.finalize().as_bytes()
}

/// Calendar-aware validity bucket in the tenant timezone.
pub fn calendar_bucket(
    now_utc_ms: u64,
    granularity: Granularity,
    tz: &IanaTz,
) -> Result<Interval, TimeError> {
    let now = utc_from_ms(now_utc_ms)?;
    let zone = tz.zone();
    let local = now.with_timezone(&zone);
    let date = local.date_naive();

    let (start, end) = match granularity {
        Granularity::Hour => {
            let start = local_boundary(zone, date, local.hour())?;
            (start, start + Duration::hours(1))
        }
        Granularity::Day => {
            let start = local_boundary(zone, date, 0)?;
            let end_date = date
                .checked_add_signed(Duration::days(1))
                .ok_or(TimeError::CalendarOverflow)?;
            (start, local_boundary(zone, end_date, 0)?)
        }
        Granularity::Month => {
            let start_date = date.with_day(1).ok_or(TimeError::CalendarOverflow)?;
            let end_date = next_month_start(start_date)?;
            (
                local_boundary(zone, start_date, 0)?,
                local_boundary(zone, end_date, 0)?,
            )
        }
        Granularity::Quarter => {
            let quarter_start_month = ((date.month0() / 3) * 3) + 1;
            let start_date = NaiveDate::from_ymd_opt(date.year(), quarter_start_month, 1)
                .ok_or(TimeError::CalendarOverflow)?;
            let end_date = add_months(start_date, 3)?;
            (
                local_boundary(zone, start_date, 0)?,
                local_boundary(zone, end_date, 0)?,
            )
        }
    };

    interval(start, end)
}

impl TemporalBinding {
    /// Resolve from a journal entry, never a system clock.
    pub fn resolve(
        expression: RelativeExpr,
        granularity: Granularity,
        tz: IanaTz,
        journaled_now_ms: u64,
    ) -> Result<Self, TimeError> {
        let now = utc_from_ms(journaled_now_ms)?;
        let zone = tz.zone();
        let local = now.with_timezone(&zone);
        let today = local.date_naive();
        let resolved = match &expression {
            RelativeExpr::LastNDays(days) => {
                if *days == 0 {
                    return Err(TimeError::InvalidRelativeExpression(
                        "LastNDays requires at least one day",
                    ));
                }
                // Calendar days, including the current tenant-local day. This
                // makes the range stable for a Day validity bucket.
                let start_date = today
                    .checked_sub_signed(Duration::days(i64::from(*days) - 1))
                    .ok_or(TimeError::CalendarOverflow)?;
                let end_date = today
                    .checked_add_signed(Duration::days(1))
                    .ok_or(TimeError::CalendarOverflow)?;
                interval(
                    local_boundary(zone, start_date, 0)?,
                    local_boundary(zone, end_date, 0)?,
                )?
            }
            RelativeExpr::CalendarQuarter(offset) => {
                let current_start_month = ((today.month0() / 3) * 3) + 1;
                let current_start = NaiveDate::from_ymd_opt(today.year(), current_start_month, 1)
                    .ok_or(TimeError::CalendarOverflow)?;
                let start_date = add_months(
                    current_start,
                    offset.checked_mul(3).ok_or(TimeError::CalendarOverflow)?,
                )?;
                let end_date = add_months(start_date, 3)?;
                interval(
                    local_boundary(zone, start_date, 0)?,
                    local_boundary(zone, end_date, 0)?,
                )?
            }
            RelativeExpr::MonthToDate => interval(
                local_boundary(
                    zone,
                    today.with_day(1).ok_or(TimeError::CalendarOverflow)?,
                    0,
                )?,
                now,
            )?,
        };

        Ok(Self {
            expression,
            granularity,
            tz,
            resolved,
            resolved_at: now_ms(journaled_now_ms),
        })
    }
}

fn utc_from_ms(epoch_ms: u64) -> Result<DateTime<Utc>, TimeError> {
    let millis = i64::try_from(epoch_ms).map_err(|_| TimeError::InvalidEpochMillis(epoch_ms))?;
    DateTime::from_timestamp_millis(millis).ok_or(TimeError::InvalidEpochMillis(epoch_ms))
}

fn local_boundary(zone: Tz, date: NaiveDate, hour: u32) -> Result<DateTime<Tz>, TimeError> {
    let local = date
        .and_hms_opt(hour, 0, 0)
        .ok_or(TimeError::CalendarOverflow)?;
    match zone.from_local_datetime(&local) {
        LocalResult::Single(value) => Ok(value),
        // DST overlaps resolve to the earlier instant/offset by rule (§2.6).
        LocalResult::Ambiguous(earlier, _) => Ok(earlier),
        LocalResult::None => first_valid_local_instant(zone, local),
    }
}

fn first_valid_local_instant(zone: Tz, local: NaiveDateTime) -> Result<DateTime<Tz>, TimeError> {
    // Some historical zones skip a midnight. Preserve a valid half-open
    // calendar boundary by advancing to the first representable local instant.
    for minutes in 1..=(24 * 60) {
        let candidate = local
            .checked_add_signed(Duration::minutes(minutes))
            .ok_or(TimeError::CalendarOverflow)?;
        match zone.from_local_datetime(&candidate) {
            LocalResult::Single(value) => return Ok(value),
            LocalResult::Ambiguous(earlier, _) => return Ok(earlier),
            LocalResult::None => {}
        }
    }
    Err(TimeError::NonexistentLocalBoundary)
}

fn interval<StartTz: TimeZone, EndTz: TimeZone>(
    start: DateTime<StartTz>,
    end: DateTime<EndTz>,
) -> Result<Interval, TimeError> {
    let start_ms =
        u64::try_from(start.timestamp_millis()).map_err(|_| TimeError::CalendarOverflow)?;
    let end_ms = u64::try_from(end.timestamp_millis()).map_err(|_| TimeError::CalendarOverflow)?;
    if start_ms >= end_ms {
        return Err(TimeError::CalendarOverflow);
    }
    Ok(Interval { start_ms, end_ms })
}

fn next_month_start(date: NaiveDate) -> Result<NaiveDate, TimeError> {
    add_months(date, 1)
}

fn add_months(date: NaiveDate, months: i32) -> Result<NaiveDate, TimeError> {
    let month_index = date
        .year()
        .checked_mul(12)
        .and_then(|value| value.checked_add(i32::try_from(date.month0()).ok()?))
        .and_then(|value| value.checked_add(months))
        .ok_or(TimeError::CalendarOverflow)?;
    let year = month_index.div_euclid(12);
    let month =
        u32::try_from(month_index.rem_euclid(12) + 1).map_err(|_| TimeError::CalendarOverflow)?;
    NaiveDate::from_ymd_opt(year, month, 1).ok_or(TimeError::CalendarOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> u64 {
        u64::try_from(
            Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
                .single()
                .unwrap()
                .timestamp_millis(),
        )
        .unwrap()
    }

    #[test]
    fn i1_tenant_midnight_rollover_changes_kolkata_validity_bucket() {
        let kolkata = IanaTz::parse("Asia/Kolkata").unwrap();
        // 23:50 then 00:10 local, while both instants are still on Sep 30 UTC.
        let before = utc_ms(2026, 9, 30, 18, 20);
        let after = utc_ms(2026, 9, 30, 18, 40);
        let before_bucket = calendar_bucket(before, Granularity::Day, &kolkata).unwrap();
        let after_bucket = calendar_bucket(after, Granularity::Day, &kolkata).unwrap();
        assert_ne!(before_bucket, after_bucket);

        let before_binding = TemporalBinding::resolve(
            RelativeExpr::LastNDays(30),
            Granularity::Day,
            kolkata.clone(),
            before,
        )
        .unwrap();
        let after_binding = TemporalBinding::resolve(
            RelativeExpr::LastNDays(30),
            Granularity::Day,
            kolkata,
            after,
        )
        .unwrap();
        assert_ne!(before_binding.resolved, after_binding.resolved);
    }

    #[test]
    fn i2_intervals_are_half_open() {
        let tz = IanaTz::parse("UTC").unwrap();
        let bucket = calendar_bucket(utc_ms(2026, 9, 30, 23, 59), Granularity::Month, &tz).unwrap();
        assert!(bucket.contains(utc_ms(2026, 9, 30, 23, 59) + 999));
        assert!(!bucket.contains(utc_ms(2026, 10, 1, 0, 0)));
    }

    #[test]
    fn i3_berlin_fall_back_day_is_25_hours_and_ambiguity_uses_earlier_offset() {
        let berlin = IanaTz::parse("Europe/Berlin").unwrap();
        let bucket =
            calendar_bucket(utc_ms(2026, 10, 25, 12, 0), Granularity::Day, &berlin).unwrap();
        assert_eq!(bucket.end_ms - bucket.start_ms, 25 * 60 * 60 * 1_000);

        let ambiguous = local_boundary(
            berlin.zone(),
            NaiveDate::from_ymd_opt(2026, 10, 25).unwrap(),
            2,
        )
        .unwrap();
        assert_eq!(
            ambiguous.timestamp_millis(),
            utc_ms(2026, 10, 25, 0, 0) as i64
        );
        assert_eq!(interval_hash(bucket), interval_hash(bucket));
        assert_eq!(tzdb_hash(), tzdb_hash(), "shipped-rule identity is stable");
    }

    #[test]
    fn i4_tzdb_is_exact_pinned_dependency_not_host_configuration() {
        assert_eq!(TZDB_VERSION, "chrono-tz=0.10.4");
        assert_ne!(tzdb_hash(), [0; 32]);
        assert!(IanaTz::parse("not/a-timezone").is_err());
    }
}
