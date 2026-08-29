//! Reading an iCalendar file.
//!
//! Every calendar worth reading publishes one of these at a URL: Google's
//! "secret address in iCal format", Outlook's published link, iCloud, Fastmail.
//! No OAuth, no registration, no admin consent — the user pastes a link and
//! Chief can see their day. It is the calendar an organisation that refuses
//! third-party applications can still offer.
//!
//! This module is **pure**: text in, events out. Fetching lives next door in
//! [`crate::calendar`], so the whole of the parsing — which is where the
//! mistakes are — is testable without a server.
//!
//! ## What is supported, and what is not
//!
//! Stated exactly, because a calendar parser that quietly drops events is worse
//! than none: a person with four meetings sees an empty Tuesday and believes it.
//!
//! Supported: line unfolding, escaped text, `VEVENT` with `SUMMARY`, `DTSTART`,
//! `DTEND`, `LOCATION`, `ORGANIZER`, `ATTENDEE`; times as UTC, as a floating
//! local time, as a `DATE` for an all-day event, and with a `TZID` whose offset
//! the file declares in its own `VTIMEZONE`; `STATUS:CANCELLED` skipped;
//! recurrence by `RRULE` for `DAILY`, `WEEKLY`, `MONTHLY` and `YEARLY` with
//! `INTERVAL`, `COUNT`, `UNTIL` and `BYDAY`, and `EXDATE` exclusions.
//!
//! **Not supported**, and each would silently misplace an event rather than
//! fail: `BYSETPOS`, `BYMONTHDAY` and `BYMONTH` as recurrence selectors, and
//! `RECURRENCE-ID` overrides — a single occurrence moved to another time still
//! shows at its original one. A `TZID` with no `VTIMEZONE` in the file is read
//! as local time, which is right far more often than it is wrong and is the
//! only guess available without shipping a timezone database.

use std::collections::HashMap;

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Weekday};

/// One event, as it appears in the file before recurrence is expanded.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VEvent {
    pub summary: String,
    pub start: Moment,
    pub end: Option<Moment>,
    pub location: Option<String>,
    pub organiser: Option<String>,
    pub attendees: Vec<String>,
    pub cancelled: bool,
    /// The `RRULE`, unparsed, or `None` for a one-off.
    pub rule: Option<String>,
    /// Dates and times this recurrence skips.
    pub exclusions: Vec<NaiveDateTime>,
    /// The `UID`, so an override can be tied to the series it belongs to.
    pub uid: String,
}

/// When something happens, before it is placed on a clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    /// A wall-clock time with no zone, or one already resolved to local.
    Local(NaiveDateTime),
    /// An instant, known exactly.
    Utc(NaiveDateTime),
    /// A whole day, with no time at all.
    Day(NaiveDate),
}

impl Default for Moment {
    fn default() -> Self {
        Self::Day(NaiveDate::default())
    }
}

impl Moment {
    /// The local wall-clock time this falls at.
    fn naive_local(self, offset_seconds: i32) -> NaiveDateTime {
        match self {
            Self::Local(at) => at,
            Self::Utc(at) => at + Duration::seconds(i64::from(offset_seconds)),
            Self::Day(day) => day.and_time(NaiveTime::MIN),
        }
    }

    fn is_all_day(self) -> bool {
        matches!(self, Self::Day(_))
    }
}

/// One line of an iCalendar file, split into what it is and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    name: String,
    /// The parameters after the name, e.g. `TZID=Europe/Madrid`.
    params: HashMap<String, String>,
    value: String,
}

/// Undo RFC 5545's line folding.
///
/// A long line is wrapped by inserting CRLF and a single space or tab, and the
/// continuation must be glued back on before anything else looks at it — read
/// naively, every long `SUMMARY` is silently truncated at 75 octets.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);

        match line.strip_prefix([' ', '\t']) {
            Some(continuation) => {
                if let Some(last) = lines.last_mut() {
                    last.push_str(continuation);
                } else {
                    lines.push(continuation.to_string());
                }
            }
            None => lines.push(line.to_string()),
        }
    }

    lines
}

/// Split one unfolded line into its name, parameters and value.
fn read_line(line: &str) -> Option<Line> {
    let (head, value) = line.split_once(':')?;
    let mut parts = head.split(';');
    let name = parts.next()?.to_ascii_uppercase();

    let params = parts
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| {
            (
                key.to_ascii_uppercase(),
                value.trim_matches('"').to_string(),
            )
        })
        .collect();

    Some(Line {
        name,
        params,
        value: value.to_string(),
    })
}

/// Undo the escaping RFC 5545 applies to text values.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();

    while let Some(character) = chars.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }

        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(escaped) => out.push(escaped),
            None => out.push('\\'),
        }
    }

    out
}

/// `20260829T093000Z`, `20260829T093000`, or `20260829`.
fn read_moment(line: &Line, zones: &HashMap<String, i32>) -> Option<Moment> {
    let value = line.value.trim();

    if line.params.get("VALUE").is_some_and(|kind| kind == "DATE") || value.len() == 8 {
        return NaiveDate::parse_from_str(value, "%Y%m%d")
            .ok()
            .map(Moment::Day);
    }

    if let Some(stamp) = value.strip_suffix('Z') {
        return NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%S")
            .ok()
            .map(Moment::Utc);
    }

    let at = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()?;

    // A TZID the file itself declared. Without one this is a floating time,
    // which RFC 5545 says is local wherever it is read — which is here.
    match line.params.get("TZID").and_then(|id| zones.get(id)) {
        Some(offset) => Some(Moment::Utc(at - Duration::seconds(i64::from(*offset)))),
        None => Some(Moment::Local(at)),
    }
}

/// The offset each `VTIMEZONE` in this file declares, so a `TZID` can be placed
/// without shipping a timezone database.
///
/// The most recently declared `TZOFFSETTO` wins, which for a file listing
/// STANDARD and DAYLIGHT is whichever came last rather than whichever applies
/// today. Imperfect by an hour across a DST boundary, and far better than
/// treating a zoned time as local — which is wrong by the whole offset.
fn read_zones(lines: &[String]) -> HashMap<String, i32> {
    let mut zones = HashMap::new();
    let mut current: Option<String> = None;

    for line in lines {
        let Some(parsed) = read_line(line) else {
            continue;
        };

        match (parsed.name.as_str(), parsed.value.as_str()) {
            ("BEGIN", "VTIMEZONE") => current = Some(String::new()),
            ("END", "VTIMEZONE") => current = None,
            ("TZID", id) if current.is_some() => current = Some(id.to_string()),
            ("TZOFFSETTO", offset) => {
                if let (Some(id), Some(seconds)) = (current.as_ref(), read_offset(offset)) {
                    if !id.is_empty() {
                        zones.insert(id.clone(), seconds);
                    }
                }
            }
            _ => {}
        }
    }

    zones
}

/// `+0200`, `-0530`, `+020000`.
fn read_offset(value: &str) -> Option<i32> {
    let (sign, digits) = match value.chars().next()? {
        '+' => (1, &value[1..]),
        '-' => (-1, &value[1..]),
        _ => (1, value),
    };

    if digits.len() < 4 {
        return None;
    }

    let hours: i32 = digits.get(0..2)?.parse().ok()?;
    let minutes: i32 = digits.get(2..4)?.parse().ok()?;
    let seconds: i32 = digits.get(4..6).and_then(|s| s.parse().ok()).unwrap_or(0);

    Some(sign * (hours * 3600 + minutes * 60 + seconds))
}

/// Every `VEVENT` in the file, unexpanded.
#[must_use]
pub fn parse(text: &str) -> Vec<VEvent> {
    let lines = unfold(text);
    let zones = read_zones(&lines);

    let mut events = Vec::new();
    let mut current: Option<VEvent> = None;

    for line in &lines {
        let Some(parsed) = read_line(line) else {
            continue;
        };

        match parsed.name.as_str() {
            "BEGIN" if parsed.value == "VEVENT" => current = Some(VEvent::default()),
            "END" if parsed.value == "VEVENT" => {
                if let Some(event) = current.take() {
                    events.push(event);
                }
            }
            _ => {
                if let Some(event) = current.as_mut() {
                    fill(event, &parsed, &zones);
                }
            }
        }
    }

    events
}

/// Put one line into the event being built.
fn fill(event: &mut VEvent, line: &Line, zones: &HashMap<String, i32>) {
    match line.name.as_str() {
        "UID" => event.uid = line.value.clone(),
        "SUMMARY" => event.summary = unescape(&line.value),
        "LOCATION" => event.location = Some(unescape(&line.value)).filter(|it| !it.is_empty()),
        "DTSTART" => {
            if let Some(moment) = read_moment(line, zones) {
                event.start = moment;
            }
        }
        "DTEND" => event.end = read_moment(line, zones),
        "STATUS" => event.cancelled = line.value.eq_ignore_ascii_case("CANCELLED"),
        "RRULE" => event.rule = Some(line.value.clone()),
        "ORGANIZER" => event.organiser = Some(person(line)),
        "ATTENDEE" => event.attendees.push(person(line)),
        "EXDATE" => {
            for part in line.value.split(',') {
                let one = Line {
                    name: "EXDATE".to_string(),
                    params: line.params.clone(),
                    value: part.to_string(),
                };

                if let Some(moment) = read_moment(&one, zones) {
                    event.exclusions.push(moment.naive_local(0));
                }
            }
        }
        _ => {}
    }
}

/// A name where the file gives one, and never a bare mailbox address.
///
/// The same rule `microsoft.rs` follows: this text goes to the model, and an
/// address is more identifying than any answer needs.
fn person(line: &Line) -> String {
    line.params
        .get("CN")
        .map(|name| unescape(name))
        .unwrap_or_else(|| {
            line.value.rsplit_once('@').map_or_else(
                || line.value.clone(),
                |(local, _)| local.trim_start_matches("mailto:").to_string(),
            )
        })
}

/// One dated instance of an event, placed on the local clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub summary: String,
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub location: Option<String>,
    pub organiser: Option<String>,
    pub attendees: Vec<String>,
    pub all_day: bool,
}

/// How long an event with no `DTEND` is assumed to last.
const ASSUMED_LENGTH: i64 = 30;

/// How many candidates a single rule may generate before we give up on it.
///
/// A malformed rule — `INTERVAL=0`, a `FREQ` that steps nowhere — would
/// otherwise spin for ever inside a background pass nobody is watching. The
/// ceiling is far above any real calendar: a daily meeting for eleven years.
const MAX_STEPS: usize = 4_000;

/// Every occurrence falling inside `[from, to]`, in local wall-clock terms.
///
/// `offset_seconds` places a UTC time on the reader's own clock. Passed in
/// rather than read from `Local`, so the expansion is testable at a fixed
/// offset instead of against whatever zone the test machine keeps — the same
/// reason `clock::describe` is generic over its time zone.
#[must_use]
pub fn occurrences(
    events: &[VEvent],
    from: NaiveDateTime,
    to: NaiveDateTime,
    offset_seconds: i32,
) -> Vec<Occurrence> {
    let mut found = Vec::new();

    for event in events {
        if event.cancelled {
            continue;
        }

        let start = event.start.naive_local(offset_seconds);
        let length = event
            .end
            .map(|end| end.naive_local(offset_seconds) - start)
            .filter(|length| *length > Duration::zero())
            .unwrap_or_else(|| Duration::minutes(ASSUMED_LENGTH));

        for at in starts(event, start, to, offset_seconds) {
            if at < from || at > to {
                continue;
            }

            if event.exclusions.contains(&at) {
                continue;
            }

            found.push(Occurrence {
                summary: event.summary.clone(),
                start: at,
                end: at + length,
                location: event.location.clone(),
                organiser: event.organiser.clone(),
                attendees: event.attendees.clone(),
                all_day: event.start.is_all_day(),
            });
        }
    }

    found.sort_by_key(|one| one.start);
    found
}

/// When this event begins, once for a one-off and repeatedly for a series.
fn starts(
    event: &VEvent,
    first: NaiveDateTime,
    until_at_least: NaiveDateTime,
    offset_seconds: i32,
) -> Vec<NaiveDateTime> {
    let Some(rule) = event
        .rule
        .as_deref()
        .and_then(|text| Rule::read(text, offset_seconds))
    else {
        // No rule, or one this module does not understand. A rule it cannot
        // read is treated as the one-off the event was defined as: showing a
        // meeting on a day it does not happen is worse than missing a series.
        return vec![first];
    };

    rule.expand(first, until_at_least)
}

/// The subset of `RRULE` this module reads. See the module note for the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Debug, Clone)]
struct Rule {
    frequency: Frequency,
    interval: u32,
    count: Option<usize>,
    until: Option<NaiveDateTime>,
    /// Which weekdays a `WEEKLY` rule lands on. Empty means the day `DTSTART`
    /// fell on, which is what RFC 5545 says when `BYDAY` is absent.
    days: Vec<Weekday>,
}

impl Rule {
    /// `None` when the rule uses something this module does not support, so the
    /// caller can fall back rather than expand it wrongly.
    fn read(text: &str, offset_seconds: i32) -> Option<Self> {
        let mut frequency = None;
        let mut interval = 1;
        let mut count = None;
        let mut until = None;
        let mut days = Vec::new();

        for part in text.split(';') {
            let (key, value) = part.split_once('=')?;

            match key.to_ascii_uppercase().as_str() {
                "FREQ" => {
                    frequency = match value.to_ascii_uppercase().as_str() {
                        "DAILY" => Some(Frequency::Daily),
                        "WEEKLY" => Some(Frequency::Weekly),
                        "MONTHLY" => Some(Frequency::Monthly),
                        "YEARLY" => Some(Frequency::Yearly),
                        // SECONDLY, MINUTELY, HOURLY: real, and no meeting uses
                        // them. Refuse rather than approximate.
                        _ => return None,
                    };
                }
                "INTERVAL" => interval = value.parse().ok()?,
                "COUNT" => count = Some(value.parse().ok()?),
                "UNTIL" => {
                    let line = Line {
                        name: "UNTIL".to_string(),
                        params: HashMap::new(),
                        value: value.to_string(),
                    };

                    until = Some(read_moment(&line, &HashMap::new())?.naive_local(offset_seconds));
                }
                "BYDAY" => {
                    for day in value.split(',') {
                        // A numbered day — `2MO`, the second Monday — is a
                        // selector this module does not implement.
                        days.push(weekday(day)?);
                    }
                }
                // Anything else changes which dates are selected, so guessing
                // would put a meeting on a day it does not happen.
                _ => return None,
            }
        }

        // An interval of zero steps nowhere and would never terminate.
        if interval == 0 {
            return None;
        }

        Some(Self {
            frequency: frequency?,
            interval,
            count,
            until,
            days,
        })
    }

    /// Every start from `first` up to at least `horizon`, bounded.
    fn expand(&self, first: NaiveDateTime, horizon: NaiveDateTime) -> Vec<NaiveDateTime> {
        let mut found = Vec::new();
        let mut at = first;

        for _ in 0..MAX_STEPS {
            if at > horizon {
                break;
            }

            if self.until.is_some_and(|until| at > until) {
                break;
            }

            if self.frequency == Frequency::Weekly && !self.days.is_empty() {
                // One period is a week; each named day within it is a start.
                for offset in 0..7 {
                    let candidate = at + Duration::days(offset);

                    if candidate < first || candidate > horizon {
                        continue;
                    }

                    if self.until.is_some_and(|until| candidate > until) {
                        continue;
                    }

                    if self.days.contains(&candidate.weekday()) {
                        found.push(candidate);
                    }
                }
            } else {
                found.push(at);
            }

            if self.count.is_some_and(|count| found.len() >= count) {
                break;
            }

            at = match self.step(at) {
                Some(next) if next > at => next,
                // Stepped nowhere, so it never will.
                _ => break,
            };
        }

        found.sort_unstable();

        if let Some(count) = self.count {
            found.truncate(count);
        }

        found
    }

    /// The next period after `at`.
    fn step(&self, at: NaiveDateTime) -> Option<NaiveDateTime> {
        let interval = i64::from(self.interval);

        match self.frequency {
            Frequency::Daily => Some(at + Duration::days(interval)),
            Frequency::Weekly => Some(at + Duration::weeks(interval)),
            Frequency::Monthly => add_months(at, self.interval),
            Frequency::Yearly => add_months(at, self.interval.checked_mul(12)?),
        }
    }
}

/// Add whole months, keeping the day of the month where the target has one.
///
/// The 31st in a 30-day month is clamped rather than rolled into the next: a
/// monthly meeting on the 31st is a meeting at the end of the month, not one on
/// the 1st of the following one.
fn add_months(at: NaiveDateTime, months: u32) -> Option<NaiveDateTime> {
    let total = at.year() * 12 + (at.month() as i32 - 1) + months as i32;
    let year = total.div_euclid(12);
    let month = u32::try_from(total.rem_euclid(12) + 1).ok()?;

    let last = days_in_month(year, month)?;
    let day = at.day().min(last);

    NaiveDate::from_ymd_opt(year, month, day).map(|date| date.and_time(at.time()))
}

fn days_in_month(year: i32, month: u32) -> Option<u32> {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };

    let first_of_next = NaiveDate::from_ymd_opt(next_year, next_month, 1)?;

    Some(first_of_next.pred_opt()?.day())
}

fn weekday(code: &str) -> Option<Weekday> {
    match code.to_ascii_uppercase().as_str() {
        "MO" => Some(Weekday::Mon),
        "TU" => Some(Weekday::Tue),
        "WE" => Some(Weekday::Wed),
        "TH" => Some(Weekday::Thu),
        "FR" => Some(Weekday::Fri),
        "SA" => Some(Weekday::Sat),
        "SU" => Some(Weekday::Sun),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file with the shapes a real export actually contains.
    const CALENDAR: &str = "\
BEGIN:VCALENDAR\r
VERSION:2.0\r
BEGIN:VEVENT\r
UID:one@example.com\r
SUMMARY:Standup\r
DTSTART:20260829T073000Z\r
DTEND:20260829T074500Z\r
ORGANIZER;CN=Ana Silva:mailto:ana@example.com\r
ATTENDEE;CN=Sam Patel:mailto:sam@example.com\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:two@example.com\r
SUMMARY:A meeting with a subject long enough that the exporter had to fold i\r
 t across two lines\r
DTSTART:20260829T090000Z\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:three@example.com\r
SUMMARY:Cancelled thing\r
DTSTART:20260829T100000Z\r
STATUS:CANCELLED\r
END:VEVENT\r
END:VCALENDAR\r
";

    #[test]
    fn reads_the_events_in_a_file() {
        let events = parse(CALENDAR);

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].summary, "Standup");
    }

    /// The trap that silently truncates every long subject.
    #[test]
    fn glues_a_folded_line_back_together() {
        let events = parse(CALENDAR);

        assert_eq!(
            events[1].summary,
            "A meeting with a subject long enough that the exporter had to fold it across two lines"
        );
    }

    #[test]
    fn reads_a_name_rather_than_a_mailbox() {
        let events = parse(CALENDAR);

        assert_eq!(events[0].organiser.as_deref(), Some("Ana Silva"));
        assert_eq!(events[0].attendees, vec!["Sam Patel"]);
    }

    #[test]
    fn falls_back_to_the_local_part_when_there_is_no_name() {
        let line = read_line("ATTENDEE:mailto:sam@example.com").expect("should parse");

        assert_eq!(person(&line), "sam");
    }

    #[test]
    fn notices_a_cancelled_event() {
        let events = parse(CALENDAR);

        assert!(!events[0].cancelled);
        assert!(events[2].cancelled);
    }

    #[test]
    fn unescapes_the_text_values() {
        assert_eq!(unescape(r"Retro\, then lunch"), "Retro, then lunch");
        assert_eq!(unescape(r"One\nTwo"), "One\nTwo");
        assert_eq!(unescape(r"A\;B"), "A;B");
    }

    #[test]
    fn reads_an_all_day_event_as_a_day_rather_than_a_midnight() {
        let events = parse("BEGIN:VEVENT\nSUMMARY:Off\nDTSTART;VALUE=DATE:20260829\nEND:VEVENT");

        assert_eq!(events[0].start, Moment::Day(from_ymd(2026, 8, 29)));
        assert!(events[0].start.is_all_day());
    }

    #[test]
    fn reads_a_floating_time_as_local() {
        let events = parse("BEGIN:VEVENT\nSUMMARY:Lunch\nDTSTART:20260829T123000\nEND:VEVENT");

        assert!(matches!(events[0].start, Moment::Local(_)));
    }

    #[test]
    fn places_a_zoned_time_with_the_offset_the_file_declares() {
        let text = "\
BEGIN:VCALENDAR
BEGIN:VTIMEZONE
TZID:Europe/Madrid
BEGIN:DAYLIGHT
TZOFFSETFROM:+0100
TZOFFSETTO:+0200
END:DAYLIGHT
END:VTIMEZONE
BEGIN:VEVENT
SUMMARY:Standup
DTSTART;TZID=Europe/Madrid:20260829T093000
END:VEVENT
END:VCALENDAR";

        // 09:30 in +0200 is 07:30 UTC.
        assert_eq!(
            parse(text)[0].start,
            Moment::Utc(from_ymd(2026, 8, 29).and_hms_opt(7, 30, 0).expect("valid"))
        );
    }

    #[test]
    fn reads_an_offset_in_every_shape_a_file_writes_it() {
        assert_eq!(read_offset("+0200"), Some(7_200));
        assert_eq!(read_offset("-0530"), Some(-19_800));
        assert_eq!(read_offset("+000000"), Some(0));
        assert_eq!(read_offset("nonsense"), None);
    }

    #[test]
    fn keeps_the_recurrence_rule_for_later() {
        let events = parse(
            "BEGIN:VEVENT\nSUMMARY:Standup\nDTSTART:20260824T073000Z\nRRULE:FREQ=WEEKLY;BYDAY=MO\nEND:VEVENT",
        );

        assert_eq!(events[0].rule.as_deref(), Some("FREQ=WEEKLY;BYDAY=MO"));
    }

    #[test]
    fn survives_a_line_it_does_not_understand() {
        let events = parse(
            "BEGIN:VEVENT\nX-MICROSOFT-SOMETHING:whatever\nSUMMARY:Standup\nDTSTART:20260829T073000Z\nEND:VEVENT",
        );

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary, "Standup");
    }

    fn from_ymd(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("a real date")
    }

    #[test]
    fn has_nothing_to_say_about_an_empty_file() {
        assert!(parse("").is_empty());
        assert!(parse("BEGIN:VCALENDAR\nEND:VCALENDAR").is_empty());
    }
}

#[cfg(test)]
mod recurrence {
    use super::*;

    /// Monday 24 August 2026, 09:30 local, half an hour long.
    fn standup(rule: Option<&str>) -> VEvent {
        VEvent {
            summary: "Standup".to_string(),
            start: Moment::Local(at(2026, 8, 24, 9, 30)),
            end: Some(Moment::Local(at(2026, 8, 24, 10, 0))),
            rule: rule.map(ToString::to_string),
            ..VEvent::default()
        }
    }

    fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(year, month, day)
            .expect("a real date")
            .and_hms_opt(hour, minute, 0)
            .expect("a real time")
    }

    /// A window covering one whole local day.
    fn day(year: i32, month: u32, day: u32) -> (NaiveDateTime, NaiveDateTime) {
        (at(year, month, day, 0, 0), at(year, month, day, 23, 59))
    }

    fn summaries(found: &[Occurrence]) -> Vec<&str> {
        found.iter().map(|one| one.summary.as_str()).collect()
    }

    #[test]
    fn a_one_off_shows_on_its_own_day_and_no_other() {
        let events = [standup(None)];

        let (from, to) = day(2026, 8, 24);
        assert_eq!(summaries(&occurrences(&events, from, to, 0)), ["Standup"]);

        let (from, to) = day(2026, 8, 25);
        assert!(occurrences(&events, from, to, 0).is_empty());
    }

    /// The case the whole feature turns on: a work calendar is mostly these.
    #[test]
    fn a_weekly_standup_shows_up_every_week() {
        let events = [standup(Some("FREQ=WEEKLY;BYDAY=MO"))];

        // Six weeks later, still a Monday.
        let (from, to) = day(2026, 10, 5);
        assert_eq!(
            summaries(&occurrences(&events, from, to, 0)),
            ["Standup"],
            "a recurring meeting defined once must appear on later Mondays"
        );

        let (from, to) = day(2026, 10, 6);
        assert!(
            occurrences(&events, from, to, 0).is_empty(),
            "and not on the Tuesday"
        );
    }

    #[test]
    fn keeps_the_time_of_day_across_a_recurrence() {
        let (from, to) = day(2026, 10, 5);
        let found = occurrences(&[standup(Some("FREQ=WEEKLY;BYDAY=MO"))], from, to, 0);

        assert_eq!(found[0].start, at(2026, 10, 5, 9, 30));
        assert_eq!(found[0].end, at(2026, 10, 5, 10, 0));
    }

    #[test]
    fn honours_an_interval() {
        let events = [standup(Some("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO"))];

        let (from, to) = day(2026, 9, 7); // two weeks on
        assert_eq!(summaries(&occurrences(&events, from, to, 0)).len(), 1);

        let (from, to) = day(2026, 8, 31); // one week on
        assert!(
            occurrences(&events, from, to, 0).is_empty(),
            "a fortnightly meeting is not a weekly one"
        );
    }

    #[test]
    fn stops_at_until() {
        let events = [standup(Some("FREQ=WEEKLY;BYDAY=MO;UNTIL=20260901T000000Z"))];

        let (from, to) = day(2026, 8, 31);
        assert_eq!(summaries(&occurrences(&events, from, to, 0)).len(), 1);

        let (from, to) = day(2026, 9, 7);
        assert!(occurrences(&events, from, to, 0).is_empty(), "past UNTIL");
    }

    #[test]
    fn stops_after_count() {
        let events = [standup(Some("FREQ=WEEKLY;BYDAY=MO;COUNT=2"))];

        let (from, to) = day(2026, 8, 31); // the second
        assert_eq!(summaries(&occurrences(&events, from, to, 0)).len(), 1);

        let (from, to) = day(2026, 9, 7); // the third
        assert!(occurrences(&events, from, to, 0).is_empty(), "past COUNT");
    }

    #[test]
    fn skips_a_cancelled_occurrence() {
        let mut event = standup(Some("FREQ=WEEKLY;BYDAY=MO"));
        event.exclusions.push(at(2026, 10, 5, 9, 30));

        let (from, to) = day(2026, 10, 5);
        assert!(
            occurrences(&[event], from, to, 0).is_empty(),
            "EXDATE is how a single occurrence is called off"
        );
    }

    #[test]
    fn never_shows_a_cancelled_series() {
        let mut event = standup(Some("FREQ=WEEKLY;BYDAY=MO"));
        event.cancelled = true;

        let (from, to) = day(2026, 10, 5);
        assert!(occurrences(&[event], from, to, 0).is_empty());
    }

    #[test]
    fn expands_a_daily_rule() {
        let events = [standup(Some("FREQ=DAILY"))];

        for date in [25, 26, 27] {
            let (from, to) = day(2026, 8, date);
            assert_eq!(
                summaries(&occurrences(&events, from, to, 0)).len(),
                1,
                "daily means the {date}th too"
            );
        }
    }

    #[test]
    fn expands_monthly_and_yearly_rules() {
        let (from, to) = day(2026, 11, 24);
        assert_eq!(
            summaries(&occurrences(&[standup(Some("FREQ=MONTHLY"))], from, to, 0)).len(),
            1
        );

        let (from, to) = day(2027, 8, 24);
        assert_eq!(
            summaries(&occurrences(&[standup(Some("FREQ=YEARLY"))], from, to, 0)).len(),
            1
        );
    }

    #[test]
    fn reads_several_days_in_one_byday() {
        let events = [standup(Some("FREQ=WEEKLY;BYDAY=MO,WE,FR"))];

        for (date, expected) in [(26, 1), (27, 0), (28, 1)] {
            let (from, to) = day(2026, 8, date);
            assert_eq!(
                occurrences(&events, from, to, 0).len(),
                expected,
                "the {date}th"
            );
        }
    }

    #[test]
    fn places_an_all_day_event_on_its_day() {
        let event = VEvent {
            summary: "Off".to_string(),
            start: Moment::Day(NaiveDate::from_ymd_opt(2026, 8, 24).expect("real")),
            ..VEvent::default()
        };

        let (from, to) = day(2026, 8, 24);
        let found = occurrences(&[event], from, to, 0);

        assert_eq!(summaries(&found), ["Off"]);
        assert!(
            found[0].all_day,
            "and says it is one, so it is not shown at midnight"
        );
    }

    /// A malformed or unbounded rule must not spin.
    #[test]
    fn refuses_to_run_away_on_a_rule_it_cannot_finish() {
        let events = [standup(Some("FREQ=DAILY;INTERVAL=0"))];

        // An interval of zero would step nowhere for ever. Answer, do not hang.
        let (from, to) = day(2030, 1, 1);
        let _ = occurrences(&events, from, to, 0);
    }

    #[test]
    fn ignores_a_rule_it_cannot_read() {
        let events = [standup(Some("FREQ=HOURLY;BYSETPOS=-1"))];

        // Unsupported, so it behaves as the one-off it was defined as rather
        // than guessing — which is the failure that puts a meeting on a day
        // it does not happen.
        let (from, to) = day(2026, 8, 24);
        assert_eq!(summaries(&occurrences(&events, from, to, 0)).len(), 1);

        let (from, to) = day(2026, 8, 25);
        assert!(occurrences(&events, from, to, 0).is_empty());
    }
}
