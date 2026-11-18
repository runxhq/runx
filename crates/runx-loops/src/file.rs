//! A loop's file, `loops/<name>.yaml` in the workspace (the loop contract
//! §4): which method it calls, its brief and inputs, what wakes it, the
//! authority it runs under and its policies.

use std::fmt;
use std::path::{Path, PathBuf};

use chrono::{Duration, NaiveTime, Weekday};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

use crate::LoopError;

/// The directory beside `skills/` that holds a workspace's loops.
pub const LOOPS_DIR: &str = "loops";

/// A loop's name, its file's: lowercase letters, digits and hyphens,
/// starting with a letter, at most 64 characters. It names the loop's
/// streams and the scope its runs keep their skill's records under.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LoopName(String);

impl LoopName {
    pub fn new(name: &str) -> Result<Self, LoopError> {
        let named = !name.is_empty()
            && name.len() <= 64
            && name.starts_with(|first: char| first.is_ascii_lowercase())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if named {
            Ok(Self(name.to_owned()))
        } else {
            Err(LoopError::Invalid(format!(
                "{name} isn't a loop name: lowercase letters, digits and hyphens, starting with a letter, at most 64 characters"
            )))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The loop's file in the workspace at `cwd`.
    #[must_use]
    pub fn path(&self, cwd: &Path) -> PathBuf {
        cwd.join(LOOPS_DIR).join(format!("{}.yaml", self.0))
    }
}

impl fmt::Display for LoopName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for LoopName {
    type Error = LoopError;
    fn try_from(name: String) -> Result<Self, Self::Error> {
        Self::new(&name)
    }
}

impl From<LoopName> for String {
    fn from(name: LoopName) -> Self {
        name.0
    }
}

/// The method a loop calls: a skill and one of its entry methods, written
/// `<skill>#<method>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Calls {
    pub skill: String,
    pub method: String,
}

impl Calls {
    pub fn parse(calls: &str) -> Result<Self, LoopError> {
        match calls.split_once('#') {
            Some((skill, method)) if !skill.is_empty() && !method.is_empty() => Ok(Self {
                skill: skill.to_owned(),
                method: method.to_owned(),
            }),
            _ => Err(LoopError::Invalid(format!(
                "calls: {calls} names no method: write it <skill>#<method>, such as acme/outreach#cycle"
            ))),
        }
    }
}

impl fmt::Display for Calls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.skill, self.method)
    }
}

/// A length of time, written as a whole number and a unit: `30s`, `10m`,
/// `2h`, `1d`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Span(pub Duration);

impl Span {
    pub fn parse(text: &str) -> Result<Self, LoopError> {
        let invalid = || {
            LoopError::Invalid(format!(
                "{text} isn't a length of time: write a whole number and s, m, h or d, such as 10m or 1d"
            ))
        };
        let split = text
            .find(|c: char| !c.is_ascii_digit())
            .ok_or_else(invalid)?;
        let (number, unit) = text.split_at(split);
        let number: i64 = number.parse().map_err(|_| invalid())?;
        let duration = match unit {
            "s" => Duration::try_seconds(number),
            "m" => Duration::try_minutes(number),
            "h" => Duration::try_hours(number),
            "d" => Duration::try_days(number),
            _ => None,
        }
        .filter(|duration| *duration > Duration::zero())
        .ok_or_else(invalid)?;
        Ok(Self(duration))
    }

    fn minutes(minutes: i64) -> Self {
        Self(Duration::minutes(minutes))
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let seconds = self.0.num_seconds();
        let (number, unit) = [(86_400, "d"), (3_600, "h"), (60, "m")]
            .into_iter()
            .find(|(size, _)| seconds % size == 0)
            .map_or((seconds, "s"), |(size, unit)| (seconds / size, unit));
        write!(f, "{number}{unit}")
    }
}

impl<'de> Deserialize<'de> for Span {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

impl Serialize for Span {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// What wakes a loop, besides the person's messages and "run now", which
/// always do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// A time every `every`, counted from the revision that activated it.
    Every(Span),
    /// A time of day on the days given, every day when none are, in an IANA
    /// zone.
    At {
        at: NaiveTime,
        days: Vec<Weekday>,
        tz: Tz,
    },
    /// A cron schedule, five fields, in an IANA zone.
    Cron { cron: String, tz: Tz },
    /// A connector's feed, `<connector>/<feed>` (the loop contract's E4).
    Feed { connector: String, feed: String },
    /// A request to the loop's address with its token (§5.2), named.
    Webhook { name: String },
}

impl Trigger {
    /// The trigger's identity, which names its occurrences and its frontier.
    #[must_use]
    pub fn id(&self) -> String {
        digest(self.to_string().as_bytes())[7..23].to_owned()
    }

    /// Whether the trigger is a time, which runx appends when it comes.
    #[must_use]
    pub const fn is_clock(&self) -> bool {
        matches!(self, Self::Every(_) | Self::At { .. } | Self::Cron { .. })
    }

    fn parse(value: &Value) -> Result<Self, LoopError> {
        let invalid = |why: &str| LoopError::Invalid(format!("on: {why}"));
        let entry = value.as_object().ok_or_else(|| {
            invalid("each trigger is a map, such as {every: 1d} or {at: \"08:00\", tz: Australia/Hobart}")
        })?;
        let text = |key: &str| -> Result<&str, LoopError> {
            entry
                .get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| invalid(&format!("{key} takes a string")))
        };
        let only = |keys: &[&str]| -> Result<(), LoopError> {
            match entry.keys().find(|key| !keys.contains(&key.as_str())) {
                Some(key) => Err(invalid(&format!("{key} doesn't belong with {}", keys[0]))),
                None => Ok(()),
            }
        };
        let tz = || -> Result<Tz, LoopError> {
            let zone = text("tz")?;
            zone.parse::<Tz>().map_err(|_| {
                invalid(&format!(
                    "tz: {zone} isn't an IANA time zone, such as Australia/Hobart"
                ))
            })
        };
        if entry.contains_key("every") {
            only(&["every"])?;
            Ok(Self::Every(Span::parse(text("every")?)?))
        } else if entry.contains_key("at") {
            only(&["at", "days", "tz"])?;
            let written = text("at")?;
            let at = NaiveTime::parse_from_str(written, "%H:%M").map_err(|_| {
                invalid(&format!(
                    "at: {written} isn't a time of day: write it HH:MM, such as 08:00"
                ))
            })?;
            let mut days = Vec::new();
            if let Some(listed) = entry.get("days") {
                let listed = listed
                    .as_array()
                    .ok_or_else(|| invalid("days takes a list, such as [mon, tue]"))?;
                for day in listed {
                    let day = day
                        .as_str()
                        .and_then(|day| day.parse::<Weekday>().ok())
                        .ok_or_else(|| invalid("days: each is a day, mon to sun"))?;
                    if !days.contains(&day) {
                        days.push(day);
                    }
                }
                days.sort_by_key(Weekday::num_days_from_monday);
            }
            Ok(Self::At {
                at,
                days,
                tz: tz()?,
            })
        } else if entry.contains_key("cron") {
            only(&["cron", "tz"])?;
            let cron = text("cron")?;
            if cron.split_whitespace().count() != 5 {
                return Err(invalid(&format!(
                    "cron: {cron} isn't five fields: minute, hour, day of month, month, day of week"
                )));
            }
            croner::Cron::new(cron)
                .parse()
                .map_err(|error| invalid(&format!("cron: {cron}: {error}")))?;
            Ok(Self::Cron {
                cron: cron.to_owned(),
                tz: tz()?,
            })
        } else if entry.contains_key("feed") {
            only(&["feed"])?;
            let written = text("feed")?;
            // A connector's name may hold a slash; a feed's never does.
            let (connector, feed) = written
                .rsplit_once('/')
                .filter(|(connector, feed)| !connector.is_empty() && !feed.is_empty())
                .ok_or_else(|| {
                    invalid(&format!(
                        "feed: {written} names no feed: write it <connector>/<feed>, the connector's name and then one of the feeds its manifest declares"
                    ))
                })?;
            Ok(Self::Feed {
                connector: connector.to_owned(),
                feed: feed.to_owned(),
            })
        } else if entry.contains_key("webhook") {
            only(&["webhook"])?;
            let name = text("webhook")?;
            LoopName::new(name).map_err(|_| {
                invalid(&format!(
                    "webhook: {name} isn't a name: lowercase letters, digits and hyphens"
                ))
            })?;
            Ok(Self::Webhook {
                name: name.to_owned(),
            })
        } else {
            Err(invalid(
                "each trigger is one of every, at, cron, feed or webhook",
            ))
        }
    }
}

impl fmt::Display for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Every(span) => write!(f, "every {span}"),
            Self::At { at, days, tz } => {
                write!(f, "at {}", at.format("%H:%M"))?;
                if !days.is_empty() {
                    let days = days.iter().map(ToString::to_string).collect::<Vec<_>>();
                    write!(f, " on {}", days.join(", ").to_lowercase())?;
                }
                write!(f, " in {tz}")
            }
            Self::Cron { cron, tz } => write!(f, "cron {cron} in {tz}"),
            Self::Feed { connector, feed } => write!(f, "feed {connector}/{feed}"),
            Self::Webhook { name } => write!(f, "webhook {name}"),
        }
    }
}

/// What three failures in a row do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnFailure {
    /// How many failures in a row pause the loop; absent, it never pauses.
    #[serde(default)]
    pub pause_after: Option<u32>,
}

/// A clock time missed while runx was off: run once, marked late, or skip
/// it when it's past `grace`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Late {
    #[default]
    Run,
    Skip,
}

/// What the loop may spend in a day.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spend {
    /// In US dollars; zero when absent.
    #[serde(default)]
    pub per_day_usd: f64,
}

impl Spend {
    /// The day's budget in millionths of a dollar.
    #[must_use]
    pub fn per_day_micro_usd(&self) -> u64 {
        // A budget is a count of millionths; a fraction of one is no budget.
        let micros = (self.per_day_usd * 1_000_000.0).floor();
        if micros.is_finite() && micros > 0.0 {
            micros as u64
        } else {
            0
        }
    }
}

/// A loop's policies (the loop contract §12), each with its default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Policy {
    pub on_failure: OnFailure,
    /// When a failed or unrunnable wake is tried again: each step of the
    /// ladder in turn, then its last, again and again.
    pub retry: Vec<Span>,
    pub late: Late,
    /// How late still counts as on time.
    pub grace: Span,
    /// The shortest gap between clock times, and before a proposed wake.
    pub min_interval: Span,
    /// The furthest a proposed wake may be.
    pub horizon: Span,
    /// The most events one wake takes.
    pub batch: u32,
    /// The most bytes of events one wake takes.
    pub batch_bytes: u64,
    pub spend: Spend,
    /// The IANA zone a spending day is counted in: absent, the first
    /// time-of-day or cron trigger's, else the machine's.
    pub day: Option<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            on_failure: OnFailure {
                pause_after: Some(3),
            },
            retry: [1, 5, 15, 60].into_iter().map(Span::minutes).collect(),
            late: Late::Run,
            grace: Span::minutes(10),
            min_interval: Span::minutes(5),
            horizon: Span(Duration::days(30)),
            batch: 100,
            batch_bytes: 1024 * 1024,
            spend: Spend::default(),
            day: None,
        }
    }
}

impl Policy {
    /// How long after `failures` failures in a row the wake is tried again.
    #[must_use]
    pub fn retry_after(&self, failures: u32) -> Duration {
        let step = usize::try_from(failures.saturating_sub(1)).unwrap_or(usize::MAX);
        self.retry
            .get(step)
            .or_else(|| self.retry.last())
            .map_or_else(|| Duration::minutes(60), |span| span.0)
    }
}

/// A loop's file as written.
#[derive(Clone, Debug, PartialEq)]
pub struct LoopFile {
    pub calls: Calls,
    pub brief: String,
    pub inputs: Map<String, Value>,
    pub on: Vec<Trigger>,
    pub authority: Option<String>,
    pub policy: Policy,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    calls: String,
    brief: String,
    #[serde(default)]
    inputs: Map<String, Value>,
    #[serde(default)]
    on: Vec<Value>,
    #[serde(default)]
    authority: Option<String>,
    #[serde(default)]
    policy: Policy,
}

impl LoopFile {
    /// Parse a loop file's bytes, refusing it in its own words.
    pub fn parse(bytes: &[u8]) -> Result<Self, LoopError> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| LoopError::Invalid("the loop file isn't UTF-8 text".to_owned()))?;
        let written: Written = runx_parser::parse_yaml_document(text)
            .map_err(|error| LoopError::Invalid(format!("the loop file: {error}")))?;
        if written.brief.trim().is_empty() {
            return Err(LoopError::Invalid("brief: say the job in words".to_owned()));
        }
        let mut on = Vec::with_capacity(written.on.len());
        for trigger in &written.on {
            let trigger = Trigger::parse(trigger)?;
            if on.contains(&trigger) {
                return Err(LoopError::Invalid(
                    "on: a trigger is listed twice".to_owned(),
                ));
            }
            on.push(trigger);
        }
        let file = Self {
            calls: Calls::parse(&written.calls)?,
            brief: written.brief,
            inputs: written.inputs,
            on,
            authority: written.authority,
            policy: written.policy,
        };
        for trigger in &file.on {
            if let Trigger::Every(span) = trigger
                && *span < file.policy.min_interval
            {
                return Err(LoopError::Invalid(format!(
                    "on: every {span} is sooner than the loop's min_interval of {}",
                    file.policy.min_interval
                )));
            }
        }
        if let Some(zone) = &file.policy.day {
            zone.parse::<Tz>().map_err(|_| {
                LoopError::Invalid(format!(
                    "policy.day: {zone} isn't an IANA time zone, such as Australia/Hobart"
                ))
            })?;
        }
        if file.policy.retry.is_empty() {
            return Err(LoopError::Invalid(
                "policy.retry: give at least one length of time".to_owned(),
            ));
        }
        if file.policy.batch == 0 || file.policy.batch_bytes == 0 {
            return Err(LoopError::Invalid(
                "policy.batch and policy.batch_bytes take at least one".to_owned(),
            ));
        }
        Ok(file)
    }

    /// The zone the loop's spending day is counted in.
    #[must_use]
    pub fn day_zone(&self) -> DayZone {
        if let Some(zone) = self
            .policy
            .day
            .as_deref()
            .and_then(|zone| zone.parse().ok())
        {
            return DayZone::Zone(zone);
        }
        self.on
            .iter()
            .find_map(|trigger| match trigger {
                Trigger::At { tz, .. } | Trigger::Cron { tz, .. } => Some(DayZone::Zone(*tz)),
                _ => None,
            })
            .unwrap_or(DayZone::Machine)
    }

    /// Whether `next` would widen what this file lets the loop do: another
    /// method, authority or policy, which waits for a start by someone whose
    /// authority covers it.
    #[must_use]
    pub fn widened_by(&self, next: &Self) -> bool {
        self.calls != next.calls || self.authority != next.authority || self.policy != next.policy
    }
}

/// The zone a loop's day is counted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DayZone {
    Zone(Tz),
    /// The machine's own, as its operating system keeps it.
    Machine,
}

impl DayZone {
    /// The day `at` falls on here, as `YYYY-MM-DD`.
    #[must_use]
    pub fn day_of(self, at: chrono::DateTime<chrono::Utc>) -> String {
        self.date_of(at).to_string()
    }

    fn date_of(self, at: chrono::DateTime<chrono::Utc>) -> chrono::NaiveDate {
        match self {
            Self::Zone(zone) => at.with_timezone(&zone).date_naive(),
            Self::Machine => at.with_timezone(&chrono::Local).date_naive(),
        }
    }

    /// The day `at` falls on here, and the instants it began and ends, so a
    /// method counts what it did today with no zone of its own.
    #[must_use]
    pub fn day_bounds(
        self,
        at: chrono::DateTime<chrono::Utc>,
    ) -> (
        String,
        chrono::DateTime<chrono::Utc>,
        chrono::DateTime<chrono::Utc>,
    ) {
        let date = self.date_of(at);
        let next = date.succ_opt().unwrap_or(date);
        (date.to_string(), self.midnight(date), self.midnight(next))
    }

    /// The first instant of `date` here: its midnight, or, where a clock
    /// change skips midnight, the first minute after it that exists.
    fn midnight(self, date: chrono::NaiveDate) -> chrono::DateTime<chrono::Utc> {
        use chrono::TimeZone;
        let start = date.and_time(chrono::NaiveTime::MIN);
        (0..=180)
            .map(|minutes| start + chrono::Duration::minutes(minutes))
            .find_map(|local| match self {
                Self::Zone(zone) => zone
                    .from_local_datetime(&local)
                    .earliest()
                    .map(|at| at.with_timezone(&chrono::Utc)),
                Self::Machine => chrono::Local
                    .from_local_datetime(&local)
                    .earliest()
                    .map(|at| at.with_timezone(&chrono::Utc)),
            })
            .unwrap_or_else(|| start.and_utc())
    }
}

/// `sha256:` over `bytes`.
#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    let mut hex = String::with_capacity(71);
    hex.push_str("sha256:");
    for byte in hash {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::{LoopFile, LoopName, Span, Trigger};

    #[test]
    fn a_loop_file_reads_as_written_and_is_refused_in_its_own_words()
    -> Result<(), Box<dyn std::error::Error>> {
        let file = LoopFile::parse(
            b"calls: acme/outreach#cycle\nbrief: Win design partners.\ninputs: {company: {name: Acme}}\non: [{every: 1d}, {feed: acme/mail/inbox}]\npolicy: {retry: [1m, 1h]}\n",
        )?;
        assert_eq!(file.calls.to_string(), "acme/outreach#cycle");
        assert_eq!(file.on[0], Trigger::Every(Span::parse("1d")?));
        assert_eq!(
            file.on[1],
            Trigger::Feed {
                connector: "acme/mail".to_owned(),
                feed: "inbox".to_owned()
            }
        );
        assert_eq!(file.policy.batch, 100);
        assert_eq!(file.policy.retry_after(5).num_minutes(), 60);
        let refused = |bytes: &[u8]| LoopFile::parse(bytes).err().map(|error| error.to_string());
        assert!(
            refused(b"calls: acme/outreach\nbrief: x\n")
                .is_some_and(|why| why.contains("names no method"))
        );
        assert!(
            refused(b"calls: a#b\nbrief: x\non: [{every: 1m}]\n")
                .is_some_and(|why| why.contains("min_interval"))
        );
        assert!(
            refused(b"calls: a#b\nbrief: x\non: [{cron: '* * *'}]\n")
                .is_some_and(|why| why.contains("five fields"))
        );
        assert!(
            refused(b"calls: a#b\nbrief: x\non: [{at: '08:00', tz: Mars/Olympus}]\n")
                .is_some_and(|why| why.contains("IANA"))
        );
        let at = LoopFile::parse(b"calls: a#b\nbrief: x\non: [{at: '08:00', days: [tue, mon], tz: Australia/Hobart}, {cron: '0 9 * * 1-5', tz: UTC}]\n")?;
        assert_eq!(
            at.on[0].to_string(),
            "at 08:00 on mon, tue in Australia/Hobart"
        );
        assert!(refused(b"calls: a#b\nbrief: x\nsurprise: 1\n").is_some());
        assert!(LoopName::new("Acme").is_err());
        assert_eq!(Span::parse("90m")?.to_string(), "90m");
        assert_eq!(Span::parse("120m")?.to_string(), "2h");
        Ok(())
    }

    /// A loop's day is its zone's: in Hobart's daylight time, 20:00 UTC is
    /// the next morning there, and that day began at 13:00 UTC.
    #[test]
    fn a_day_is_bounded_in_the_loops_own_zone() -> Result<(), chrono::ParseError> {
        let at = chrono::DateTime::parse_from_rfc3339("2026-11-17T20:00:00Z")?
            .with_timezone(&chrono::Utc);
        let (date, start, end) = super::DayZone::Zone(chrono_tz::Australia::Hobart).day_bounds(at);
        assert_eq!(date, "2026-11-18");
        assert_eq!(start.to_rfc3339(), "2026-11-17T13:00:00+00:00");
        assert_eq!(end.to_rfc3339(), "2026-11-18T13:00:00+00:00");
        Ok(())
    }
}
