//! When a clock trigger's times fall (§5.3). `every` counts from its
//! anchor; a time of day and a cron schedule resolve in their IANA zone. A
//! local time that doesn't exist, when clocks go forward, runs at the first
//! instant after the gap; one that happens twice, when they go back, runs at
//! the first.

use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

use crate::file::Trigger;

/// The most occurrences one catch-up counts. Past it, the count says "at
/// least" this many.
pub const COUNT_BOUND: u32 = 10_000;

/// The first time `trigger` falls strictly after `after`, counting `every`
/// from `anchor`. None for a trigger that isn't a time, or a schedule with
/// no time left.
#[must_use]
pub fn next_after(
    trigger: &Trigger,
    anchor: DateTime<Utc>,
    after: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    match trigger {
        Trigger::Every(span) => {
            let step = span.0.num_milliseconds();
            if step <= 0 {
                return None;
            }
            let elapsed = (after - anchor).num_milliseconds();
            let steps = if elapsed < 0 { 0 } else { elapsed / step + 1 };
            anchor.checked_add_signed(Duration::milliseconds(steps.checked_mul(step)?))
        }
        Trigger::At { at, days, tz } => {
            let local_after = after.with_timezone(tz).date_naive();
            // A week and a day covers every weekday and a time a gap moved.
            (0..9).find_map(|offset| {
                let date = local_after.checked_add_signed(Duration::days(offset))?;
                if !days.is_empty() && !days.contains(&date.weekday()) {
                    return None;
                }
                resolve(tz, date.and_time(*at)).filter(|instant| *instant > after)
            })
        }
        Trigger::Cron { cron, tz } => {
            let schedule = croner::Cron::new(cron).parse().ok()?;
            schedule
                .find_next_occurrence(&after.with_timezone(tz), false)
                .ok()
                .map(|instant| instant.with_timezone(&Utc))
        }
        Trigger::Feed { .. } | Trigger::Webhook { .. } => None,
    }
}

/// `local` in `zone`: the first of two when it happens twice, and the first
/// instant after the gap when it doesn't happen at all.
fn resolve(zone: &Tz, local: NaiveDateTime) -> Option<DateTime<Utc>> {
    match zone.from_local_datetime(&local) {
        LocalResult::Single(instant) => Some(instant.with_timezone(&Utc)),
        LocalResult::Ambiguous(first, _) => Some(first.with_timezone(&Utc)),
        LocalResult::None => (1..=24 * 60).find_map(|minutes| {
            let later = local.checked_add_signed(Duration::minutes(minutes))?;
            match zone.from_local_datetime(&later) {
                LocalResult::Single(instant) | LocalResult::Ambiguous(instant, _) => {
                    // The gap's end: the first whole minute that exists.
                    Some(instant.with_timezone(&Utc))
                }
                LocalResult::None => None,
            }
        }),
    }
}

/// The times of a trigger that came due: those after its frontier and at or
/// before now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Due {
    /// The latest of them.
    pub latest: DateTime<Utc>,
    /// How many there were, at most [`COUNT_BOUND`].
    pub count: u32,
}

/// What of `trigger` came due between `covered_until` and `now`.
#[must_use]
pub fn due(
    trigger: &Trigger,
    anchor: DateTime<Utc>,
    covered_until: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Option<Due> {
    let mut at = next_after(trigger, anchor, covered_until)?;
    if at > now {
        return None;
    }
    let mut count = 1;
    while count < COUNT_BOUND {
        match next_after(trigger, anchor, at) {
            Some(next) if next <= now => {
                at = next;
                count += 1;
            }
            _ => break,
        }
    }
    if count == COUNT_BOUND {
        // Past the bound only the latest matters, found from just before now.
        let mut probe = now - Duration::days(1);
        while let Some(next) = next_after(trigger, anchor, probe).filter(|next| *next <= now) {
            at = next;
            probe = next;
        }
    }
    Some(Due { latest: at, count })
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::{due, next_after};
    use crate::file::{LoopFile, Trigger};

    fn utc(text: &str) -> Result<DateTime<Utc>, chrono::ParseError> {
        Ok(DateTime::parse_from_rfc3339(text)?.with_timezone(&Utc))
    }

    fn trigger(on: &str) -> Result<Trigger, Box<dyn std::error::Error>> {
        let file = LoopFile::parse(format!("calls: a#b\nbrief: x\non: [{on}]\n").as_bytes())?;
        Ok(file.on.into_iter().next().ok_or("no trigger")?)
    }

    #[test]
    fn every_counts_from_its_anchor_and_catch_up_counts_what_it_missed()
    -> Result<(), Box<dyn std::error::Error>> {
        let every = trigger("{every: 1h}")?;
        let anchor = utc("2026-11-17T00:00:00Z")?;
        assert_eq!(
            next_after(&every, anchor, utc("2026-11-17T00:30:00Z")?),
            Some(utc("2026-11-17T01:00:00Z")?)
        );
        assert_eq!(
            next_after(&every, anchor, utc("2026-11-17T01:00:00Z")?),
            Some(utc("2026-11-17T02:00:00Z")?)
        );
        let missed = due(&every, anchor, anchor, utc("2026-11-17T05:10:00Z")?).ok_or("none due")?;
        assert_eq!(
            (missed.latest, missed.count),
            (utc("2026-11-17T05:00:00Z")?, 5)
        );
        assert!(
            due(
                &every,
                anchor,
                utc("2026-11-17T05:00:00Z")?,
                utc("2026-11-17T05:10:00Z")?
            )
            .is_none()
        );
        Ok(())
    }

    #[test]
    fn a_time_of_day_runs_once_on_the_days_clocks_change() -> Result<(), Box<dyn std::error::Error>>
    {
        // Hobart: clocks go forward at 02:00 on 4 October 2026 and back at
        // 03:00 on 5 April 2026.
        let forward = trigger("{at: '02:30', tz: Australia/Hobart}")?;
        let anchor = utc("2026-10-01T00:00:00Z")?;
        let gap = next_after(&forward, anchor, utc("2026-10-03T15:00:00Z")?).ok_or("none")?;
        // 02:30 doesn't exist that day; it runs at 03:00 AEDT, 16:00 UTC.
        assert_eq!(gap, utc("2026-10-03T16:00:00Z")?);
        let back = trigger("{at: '02:30', tz: Australia/Hobart}")?;
        let twice = next_after(&back, anchor, utc("2026-04-04T14:00:00Z")?).ok_or("none")?;
        // 02:30 happens twice; it runs at the first, AEDT, 15:30 UTC.
        assert_eq!(twice, utc("2026-04-04T15:30:00Z")?);
        let after = next_after(&back, anchor, twice).ok_or("none")?;
        assert_eq!(after, utc("2026-04-05T16:30:00Z")?, "and once only");
        let weekdays = trigger("{at: '08:00', days: [mon], tz: UTC}")?;
        assert_eq!(
            next_after(&weekdays, anchor, utc("2026-11-17T09:00:00Z")?),
            Some(utc("2026-11-23T08:00:00Z")?)
        );
        Ok(())
    }

    #[test]
    fn a_cron_schedule_resolves_in_its_zone() -> Result<(), Box<dyn std::error::Error>> {
        let cron = trigger("{cron: '0 9 * * 1-5', tz: Australia/Hobart}")?;
        let anchor = utc("2026-11-01T00:00:00Z")?;
        // Tuesday 17 November, 10:00 in Hobart: next is Wednesday 09:00 AEDT.
        assert_eq!(
            next_after(&cron, anchor, utc("2026-11-16T23:00:00Z")?),
            Some(utc("2026-11-17T22:00:00Z")?)
        );
        Ok(())
    }
}
