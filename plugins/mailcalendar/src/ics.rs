//! Minimal iCalendar (RFC 5545) reader: the upcoming timed events of a feed,
//! with recurrences (RRULE/RDATE/EXDATE) and moved or cancelled instances.

use std::collections::HashSet;

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use rrule::{RRule, RRuleSet, Tz, Unvalidated};

/// Hosts whose links are opened when an event is pressed.
const MEETING_HOSTS: [&str; 6] = [
    "meet.google.com",
    "teams.microsoft.com",
    "teams.live.com",
    "zoom.us",
    "webex.com",
    "whereby.com",
];
/// Guards against feeds with huge unbounded recurrences.
const MAX_INSTANCES: u16 = 500;

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub summary: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub link: Option<String>,
}

struct Prop<'a> {
    name: String,
    params: Vec<(String, String)>,
    value: &'a str,
}

impl Prop<'_> {
    fn param(&self, key: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Unfolds continuation lines (leading space or tab).
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        match (line.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(rest), Some(last)) => last.push_str(rest),
            _ => lines.push(line.to_owned()),
        }
    }
    lines
}

/// `NAME;KEY=VALUE;KEY="VALUE":value` – colons inside quotes belong to the parameter.
fn parse_line(line: &str) -> Option<Prop<'_>> {
    let mut quoted = false;
    let colon = line.char_indices().find_map(|(i, c)| {
        if c == '"' {
            quoted = !quoted;
        }
        (c == ':' && !quoted).then_some(i)
    })?;
    let mut head = line[..colon].split(';');
    let name = head.next()?.to_ascii_uppercase();
    let params = head
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_ascii_uppercase(), v.trim_matches('"').to_owned()))
        .collect();
    Some(Prop {
        name,
        params,
        value: &line[colon + 1..],
    })
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Time zone of a `TZID`; unknown ids (e.g. Windows names from Outlook) fall
/// back to local time, which is right for most single-zone users.
fn zone(tzid: Option<&str>) -> Tz {
    tzid.and_then(|id| id.parse::<chrono_tz::Tz>().ok())
        .map_or(Tz::LOCAL, Tz::Tz)
}

/// A date or date-time value; date-only values are all-day (local midnight).
fn parse_time(value: &str, tzid: Option<&str>) -> Option<(DateTime<Tz>, bool)> {
    let value = value.trim();
    if let Some(utc) = value.strip_suffix('Z') {
        let naive = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        return Some((Tz::UTC.from_utc_datetime(&naive), false));
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S") {
        return Some((zone(tzid).from_local_datetime(&naive).earliest()?, false));
    }
    let date = NaiveDate::parse_from_str(value, "%Y%m%d").ok()?;
    let midnight = Tz::LOCAL
        .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
        .earliest()?;
    Some((midnight, true))
}

/// `P1D`, `PT1H30M`, `-PT15M`, `P2W` …
fn parse_duration(value: &str) -> Option<Duration> {
    let (sign, rest) = match value.trim().strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, value.trim().trim_start_matches('+')),
    };
    let rest = rest.strip_prefix('P')?;
    let mut total = Duration::zero();
    let mut number = String::new();
    for c in rest.chars() {
        match c {
            '0'..='9' => number.push(c),
            'T' => {}
            unit => {
                let n: i64 = std::mem::take(&mut number).parse().ok()?;
                total += match unit {
                    'W' => Duration::weeks(n),
                    'D' => Duration::days(n),
                    'H' => Duration::hours(n),
                    'M' => Duration::minutes(n),
                    'S' => Duration::seconds(n),
                    _ => return None,
                };
            }
        }
    }
    Some(total * sign)
}

/// First meeting link in the given texts. Invitations from others end up in
/// the feed, so the host is taken from a real URL parser (a `\` or `user@`
/// trick must not smuggle in another site) and the parsed URL is returned.
fn meeting_link(texts: &[String]) -> Option<String> {
    texts
        .iter()
        .flat_map(|t| t.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | ',')))
        .filter(|word| word.starts_with("https://") && !word.contains('\\'))
        .filter_map(|word| reqwest::Url::parse(word).ok())
        .find(|url| {
            let host = url.host_str().unwrap_or_default().trim_end_matches('.');
            url.scheme() == "https"
                && url.username().is_empty()
                && url.password().is_none()
                && url.port().is_none()
                && MEETING_HOSTS
                    .iter()
                    .any(|h| host == *h || host.ends_with(&format!(".{h}")))
        })
        .map(String::from)
}

/// UNTIL must be UTC for zoned starts (RFC 5545 §3.3.10); feeds do not
/// always follow that, so floating and date-only values are converted.
fn normalize_rrule(rule: &str, tzid: Option<&str>) -> String {
    rule.split(';')
        .map(|part| match part.split_once('=') {
            Some((key, value)) if key.eq_ignore_ascii_case("UNTIL") && !value.ends_with('Z') => {
                let until = parse_time(value, tzid).map(|(t, all_day)| {
                    let t = t.with_timezone(&Utc);
                    if all_day {
                        t + Duration::days(1) - Duration::seconds(1)
                    } else {
                        t
                    }
                });
                match until {
                    Some(t) => format!("UNTIL={}", t.format("%Y%m%dT%H%M%SZ")),
                    None => part.to_owned(),
                }
            }
            _ => part.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(";")
}

#[derive(Default)]
struct RawEvent {
    uid: String,
    summary: String,
    start: Option<(DateTime<Tz>, bool)>,
    start_tzid: Option<String>,
    end: Option<DateTime<Tz>>,
    duration: Option<Duration>,
    rrule: Option<String>,
    rdates: Vec<DateTime<Tz>>,
    exdates: Vec<DateTime<Tz>>,
    recurrence_id: Option<DateTime<Utc>>,
    cancelled: bool,
    texts: Vec<String>,
}

fn times(prop: &Prop) -> Vec<DateTime<Tz>> {
    prop.value
        .split(',')
        .filter_map(|v| parse_time(v, prop.param("TZID")).map(|(t, _)| t))
        .collect()
}

fn raw_events(lines: &[String]) -> Vec<RawEvent> {
    let mut events = Vec::new();
    let mut current: Option<RawEvent> = None;
    // Nested components (VALARM) must not overwrite event properties.
    let mut depth = 0;
    for line in lines {
        let Some(prop) = parse_line(line) else {
            continue;
        };
        match (prop.name.as_str(), prop.value.trim()) {
            ("BEGIN", "VEVENT") => {
                current = Some(RawEvent::default());
                depth = 0;
                continue;
            }
            ("END", "VEVENT") => {
                events.extend(current.take());
                continue;
            }
            ("BEGIN", _) => depth += 1,
            ("END", _) => depth -= 1,
            _ => {}
        }
        let Some(event) = current.as_mut().filter(|_| depth == 0) else {
            continue;
        };
        let tzid = prop.param("TZID");
        match prop.name.as_str() {
            "UID" => event.uid = prop.value.to_owned(),
            "SUMMARY" => event.summary = unescape(prop.value),
            "DTSTART" => {
                event.start = parse_time(prop.value, tzid);
                event.start_tzid = tzid.map(str::to_owned);
            }
            "DTEND" => event.end = parse_time(prop.value, tzid).map(|(t, _)| t),
            "DURATION" => event.duration = parse_duration(prop.value),
            "RRULE" => event.rrule = Some(prop.value.to_owned()),
            "RDATE" => event.rdates.extend(times(&prop)),
            "EXDATE" => event.exdates.extend(times(&prop)),
            "RECURRENCE-ID" => {
                event.recurrence_id = parse_time(prop.value, tzid).map(|(t, _)| t.to_utc());
            }
            "STATUS" => event.cancelled = prop.value.eq_ignore_ascii_case("CANCELLED"),
            "URL" | "LOCATION" | "DESCRIPTION" | "X-GOOGLE-CONFERENCE" => {
                event.texts.push(unescape(prop.value))
            }
            _ => {}
        }
    }
    events
}

/// Instance starts of a raw event that begin before `until`.
fn starts(event: &RawEvent, start: DateTime<Tz>, until: DateTime<Utc>) -> Vec<DateTime<Utc>> {
    let Some(rule) = &event.rrule else {
        let mut list = vec![start.to_utc()];
        list.extend(event.rdates.iter().map(DateTime::to_utc));
        return list;
    };
    let set = normalize_rrule(rule, event.start_tzid.as_deref())
        .parse::<RRule<Unvalidated>>()
        .and_then(|r| r.build(start));
    let set: RRuleSet = match set {
        Ok(set) => set,
        // An unreadable rule still yields the first instance.
        Err(_) => return vec![start.to_utc()],
    };
    set.set_rdates(event.rdates.clone())
        .set_exdates(event.exdates.clone())
        .before(until.with_timezone(&Tz::UTC))
        .all(MAX_INSTANCES)
        .dates
        .into_iter()
        .map(|t| t.to_utc())
        .collect()
}

/// Timed events of `text` that have not ended at `now` and start before
/// `now + horizon`, sorted by start. All-day events are left out: they are
/// rarely what a "next appointment" key should show.
pub fn upcoming(text: &str, now: DateTime<Utc>, horizon: Duration) -> Vec<Event> {
    let lines = unfold(text);
    let raw = raw_events(&lines);
    let until = now + horizon;
    let moved: HashSet<(&str, DateTime<Utc>)> = raw
        .iter()
        .filter_map(|e| Some((e.uid.as_str(), e.recurrence_id?)))
        .collect();
    let mut events = Vec::new();
    for event in &raw {
        let Some((start, false)) = event.start else {
            continue;
        };
        if event.cancelled {
            continue;
        }
        let length = event
            .end
            .map(|end| end.to_utc() - start.to_utc())
            .or(event.duration)
            .unwrap_or_else(Duration::zero);
        let link = meeting_link(&event.texts);
        for start in starts(event, start, until) {
            let overridden =
                event.recurrence_id.is_none() && moved.contains(&(event.uid.as_str(), start));
            let end = start + length;
            let over = if length.is_zero() {
                start < now
            } else {
                end <= now
            };
            if overridden || over || start >= until {
                continue;
            }
            events.push(Event {
                summary: event.summary.clone(),
                start,
                end,
                link: link.clone(),
            });
        }
    }
    events.sort_by_key(|e| e.start);
    events
}

/// Key title for the next event: `jetzt`, `in 12 min`, `14:30`, `Mo 09:00`.
pub fn when(event: &Event, now: DateTime<Utc>) -> String {
    if event.start <= now {
        return "jetzt".into();
    }
    let minutes = ((event.start - now).num_seconds() + 59) / 60;
    if minutes < 60 {
        return format!("in {minutes} min");
    }
    let start = event.start.with_timezone(&Local);
    if start.date_naive() == now.with_timezone(&Local).date_naive() {
        return start.format("%H:%M").to_string();
    }
    const DAYS: [&str; 7] = ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"];
    let day = DAYS[start.format("%u").to_string().parse::<usize>().unwrap_or(1) - 1];
    format!("{day} {}", start.format("%H:%M"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().to_utc()
    }

    const FEED: &str = "BEGIN:VCALENDAR\r
BEGIN:VEVENT\r
UID:weekly\r
SUMMARY:Jour fixe\\, Team\r
DTSTART;TZID=Europe/Berlin:20260105T100000\r
DTEND;TZID=Europe/Berlin:20260105T103000\r
RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261231T235959Z\r
EXDATE;TZID=Europe/Berlin:20261012T100000\r
DESCRIPTION:Beitreten: https://meet.google.com/abc-defg-hij\\nDanke\r
BEGIN:VALARM\r
DESCRIPTION:Erinnerung\r
END:VALARM\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:weekly\r
RECURRENCE-ID;TZID=Europe/Berlin:20261019T100000\r
SUMMARY:Jour fixe (verschoben)\r
DTSTART;TZID=Europe/Berlin:20261019T140000\r
DTEND;TZID=Europe/Berlin:20261019T143000\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:once\r
SUMMARY:Zahnarzt\r
DTSTART:20261013T070000Z\r
DURATION:PT1H\r
LOCATION:Praxis\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:cancelled\r
SUMMARY:Abgesagt\r
STATUS:CANCELLED\r
DTSTART:20261013T080000Z\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:allday\r
SUMMARY:Urlaub\r
DTSTART;VALUE=DATE:20261013\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:folded\r
SUMMARY:Sehr lange\r
  Besprechung\r
DTSTART:20261020T090000Z\r
END:VEVENT\r
END:VCALENDAR\r
";

    #[test]
    fn expands_recurrences_and_exceptions() {
        let now = utc("2026-10-11T12:00:00Z");
        let events = upcoming(FEED, now, Duration::days(10));
        let list: Vec<(&str, DateTime<Utc>)> = events
            .iter()
            .map(|e| (e.summary.as_str(), e.start))
            .collect();
        assert_eq!(
            list,
            [
                // 12 Oct excluded by EXDATE, 19 Oct moved to 14:00.
                ("Zahnarzt", utc("2026-10-13T07:00:00Z")),
                ("Jour fixe (verschoben)", utc("2026-10-19T12:00:00Z")),
                ("Sehr lange Besprechung", utc("2026-10-20T09:00:00Z")),
            ]
        );
        let later = upcoming(FEED, utc("2026-10-25T00:00:00Z"), Duration::days(2));
        assert_eq!(later[0].summary, "Jour fixe, Team");
        assert_eq!(later[0].start, utc("2026-10-26T09:00:00Z"), "CET after DST");
        assert_eq!(later[0].end - later[0].start, Duration::minutes(30));
        assert_eq!(
            later[0].link.as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
        assert_eq!(events[0].end, utc("2026-10-13T08:00:00Z"), "DURATION");
    }

    #[test]
    fn running_events_stay_until_they_end() {
        let events = upcoming(FEED, utc("2026-10-13T07:30:00Z"), Duration::days(1));
        assert_eq!(events[0].summary, "Zahnarzt");
        assert_eq!(when(&events[0], utc("2026-10-13T07:30:00Z")), "jetzt");
        assert_eq!(when(&events[0], utc("2026-10-13T06:48:30Z")), "in 12 min");
    }

    #[test]
    fn helpers() {
        assert_eq!(parse_duration("-PT15M"), Some(Duration::minutes(-15)));
        assert_eq!(
            parse_duration("P1DT2H"),
            Some(Duration::hours(26)),
            "days and hours"
        );
        assert_eq!(
            normalize_rrule("FREQ=DAILY;UNTIL=20260101T000000", Some("UTC")),
            "FREQ=DAILY;UNTIL=20260101T000000Z"
        );
        assert_eq!(
            meeting_link(&["Zoom: https://us02web.zoom.us/j/123?pwd=x, sonst Telefon".into()])
                .as_deref(),
            Some("https://us02web.zoom.us/j/123?pwd=x")
        );
        for evil in [
            "https://example.com/zoom.us",
            "https://evil.com\\.zoom.us/j/1",
            "https://zoom.us@evil.com/j/1",
            "https://user@zoom.us/j/1",
            "http://zoom.us/j/1",
        ] {
            assert!(meeting_link(&[evil.into()]).is_none(), "{evil}");
        }
        let line = parse_line("ATTENDEE;CN=\"Doe: J\":mailto:j@x").unwrap();
        assert_eq!(line.param("CN"), Some("Doe: J"));
        assert_eq!(line.value, "mailto:j@x");
    }
}
