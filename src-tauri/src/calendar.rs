//! Calendars the user subscribed to, rather than signed in to.
//!
//! Every calendar worth reading publishes an iCalendar URL — Google's "secret
//! address in iCal format", Outlook's published link, iCloud, Fastmail. The
//! user pastes one and Chief can see their day: no OAuth application, no
//! registration, no administrator consent, no cost, and it works for providers
//! Chief will never integrate.
//!
//! Parsing lives in [`crate::ical`], which is pure. This module is the part
//! that touches the network, and it is deliberately thin.
//!
//! ## The URL is a credential
//!
//! A subscription address grants read access to somebody's whole calendar to
//! anyone holding it. It is a bearer token that happens to look like a link,
//! and it is treated as one: stored in `integrations` beside the OAuth tokens,
//! **never logged, never put in an error message, never shown in full**. See
//! [`Error`] — every variant is careful about this, which is why none of them
//! carry the URL.
//!
//! ## The first outbound host the user chooses
//!
//! Everywhere else Chief talks to a constant: `api.github.com`,
//! `login.microsoftonline.com`, one pinned URL on `huggingface.co`. Here the
//! host is typed by a person. That is still inside the rule — a service the
//! user explicitly connected — and it takes the same care `weights.rs` takes:
//! HTTPS only, every redirect required to stay HTTPS, and no credential
//! belonging to anything else attached to the request.

use std::time::Duration;

use crate::ical::{self, Occurrence};
use crate::microsoft::Event;

/// How long to wait for a calendar that is not answering.
const TIMEOUT: Duration = Duration::from_secs(20);

/// A calendar file large enough to be a mistake rather than a calendar.
///
/// A busy year of meetings is a few hundred kilobytes. Ten megabytes is either
/// somebody's decade of history or a URL that does not point at a calendar, and
/// neither belongs in memory on a laptop.
const MAX_BYTES: usize = 10 * 1024 * 1024;

/// What can go wrong reading a subscription.
///
/// **No variant carries the URL.** These strings reach the screen and the log,
/// and a subscription address in either is the whole calendar handed to whoever
/// reads it.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("a calendar subscription has to be an https:// address")]
    NotHttps,
    #[error("that calendar could not be reached")]
    Unreachable,
    #[error("that calendar answered, but not with a calendar")]
    NotACalendar,
    #[error("that calendar is too large to read")]
    TooLarge,
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// Is this something Chief will fetch at all?
///
/// `webcal://` is the scheme a calendar link is usually offered under and is
/// plain HTTP underneath, so it is rewritten rather than refused — pasting the
/// link the provider gives you should work.
pub fn normalise(url: &str) -> Result<String, Error> {
    let trimmed = url.trim();

    let candidate = match trimmed.split_once("://") {
        Some(("webcal" | "webcals", rest)) => format!("https://{rest}"),
        _ => trimmed.to_string(),
    };

    if !candidate.starts_with("https://") {
        return Err(Error::NotHttps);
    }

    Ok(candidate)
}

/// Only ever talk HTTPS, however many hops the host redirects through.
///
/// The same policy `weights.rs` uses, for the same reason: a redirect down to
/// plain HTTP would put a credential-bearing URL on the network in the clear.
fn https_only() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.url().scheme() != "https" {
            return attempt.error(std::io::Error::other("a redirect tried to leave HTTPS"));
        }

        if attempt.previous().len() >= 10 {
            return attempt.error(std::io::Error::other("too many redirects"));
        }

        attempt.follow()
    })
}

/// Reads calendars the user has subscribed to.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    /// Whether the HTTPS requirement is waived. `#[cfg(test)]` sets it and
    /// nothing else can, so a release build cannot be aimed at plain HTTP —
    /// the same shape as `github::Client::against`.
    #[cfg(test)]
    insecure: bool,
}

impl Client {
    pub fn new() -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .redirect(https_only())
            .timeout(TIMEOUT)
            // No proxy, for the same reason `llama.rs` disables them: a
            // misconfiguration must not put the user's calendar somewhere else.
            .no_proxy()
            .build()
            .map_err(|_| Error::Unreachable)?;

        Ok(Self {
            http,
            #[cfg(test)]
            insecure: false,
        })
    }

    /// A client that will talk to a stub on loopback, for tests only.
    #[cfg(test)]
    pub fn insecure() -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .no_proxy()
            .build()
            .map_err(|_| Error::Unreachable)?;

        Ok(Self {
            http,
            insecure: true,
        })
    }

    /// The address to fetch, having checked it is one we will fetch at all.
    fn address(&self, url: &str) -> Result<String, Error> {
        #[cfg(test)]
        if self.insecure {
            return Ok(url.trim().to_string());
        }

        normalise(url)
    }

    /// Fetch the file and hand back its text.
    ///
    /// The error deliberately says nothing about *why* beyond the four cases
    /// above: a transport error from `reqwest` renders the URL it was given.
    pub async fn read(&self, url: &str) -> Result<String, Error> {
        let url = self.address(url)?;

        let response = self
            .http
            .get(&url)
            .header("Accept", "text/calendar, text/plain")
            .send()
            .await
            .map_err(|_| Error::Unreachable)?;

        if !response.status().is_success() {
            return Err(Error::Unreachable);
        }

        if response
            .content_length()
            .is_some_and(|length| length > MAX_BYTES as u64)
        {
            return Err(Error::TooLarge);
        }

        let body = response.text().await.map_err(|_| Error::Unreachable)?;

        if body.len() > MAX_BYTES {
            return Err(Error::TooLarge);
        }

        if !body.contains("BEGIN:VCALENDAR") {
            return Err(Error::NotACalendar);
        }

        Ok(body)
    }

    /// The events in this subscription between two local moments.
    pub async fn events(
        &self,
        url: &str,
        from: chrono::NaiveDateTime,
        to: chrono::NaiveDateTime,
        offset_seconds: i32,
    ) -> Result<Vec<Event>, Error> {
        let text = self.read(url).await?;
        let parsed = ical::parse(&text);

        Ok(ical::occurrences(&parsed, from, to, offset_seconds)
            .into_iter()
            .map(as_event)
            .collect())
    }
}

/// Shape an occurrence into the event the rest of Chief already reads.
///
/// The same type Outlook produces, so a brief merges the two without knowing
/// which calendar an entry came from — which is the whole point of not adding
/// a second calendar concept.
fn as_event(occurrence: Occurrence) -> Event {
    Event {
        subject: occurrence.summary,
        start: occurrence.start.format("%Y-%m-%dT%H:%M:%S").to_string(),
        end: occurrence.end.format("%Y-%m-%dT%H:%M:%S").to_string(),
        organiser: occurrence.organiser,
        attendees: occurrence.attendees,
        // A subscription file does not say, and guessing from a location that
        // happens to contain a URL would be a guess.
        online: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_an_https_subscription() {
        assert_eq!(
            normalise("https://calendar.example.com/a/basic.ics").expect("should accept"),
            "https://calendar.example.com/a/basic.ics"
        );
    }

    #[test]
    fn rewrites_the_scheme_providers_actually_hand_out() {
        // Google, Outlook and iCloud all offer `webcal://` links. Refusing one
        // would mean telling people to edit a URL before pasting it.
        assert_eq!(
            normalise("webcal://calendar.example.com/a/basic.ics").expect("should accept"),
            "https://calendar.example.com/a/basic.ics"
        );
        assert_eq!(
            normalise("webcals://calendar.example.com/a/basic.ics").expect("should accept"),
            "https://calendar.example.com/a/basic.ics"
        );
    }

    #[test]
    fn refuses_plain_http() {
        assert!(matches!(
            normalise("http://calendar.example.com/a/basic.ics"),
            Err(Error::NotHttps)
        ));
    }

    #[test]
    fn refuses_anything_that_is_not_a_url_it_will_fetch() {
        for candidate in [
            "file:///etc/passwd",
            "ftp://example.com/basic.ics",
            "javascript:alert(1)",
            "calendar.example.com/basic.ics",
            "",
        ] {
            assert!(
                normalise(candidate).is_err(),
                "{candidate} should be refused"
            );
        }
    }

    #[test]
    fn takes_the_whitespace_off_a_pasted_url() {
        assert_eq!(
            normalise("  https://calendar.example.com/a/basic.ics\n").expect("should accept"),
            "https://calendar.example.com/a/basic.ics"
        );
    }

    /// The failure that hands somebody's calendar to whoever reads the console.
    #[test]
    fn no_error_ever_repeats_the_url_back() {
        let secret = "https://calendar.example.com/private-1234567890abcdef/basic.ics";

        for error in [
            Error::NotHttps,
            Error::Unreachable,
            Error::NotACalendar,
            Error::TooLarge,
        ] {
            let said = error.to_string();

            assert!(
                !said.contains("calendar.example.com") && !said.contains("1234567890abcdef"),
                "an error must not carry the subscription address: {said}"
            );
            assert!(!secret.is_empty());
        }
    }

    #[test]
    fn turns_an_occurrence_into_the_event_everything_else_reads() {
        let occurrence = Occurrence {
            summary: "Standup".to_string(),
            start: chrono::NaiveDate::from_ymd_opt(2026, 8, 29)
                .expect("real")
                .and_hms_opt(9, 30, 0)
                .expect("real"),
            end: chrono::NaiveDate::from_ymd_opt(2026, 8, 29)
                .expect("real")
                .and_hms_opt(10, 0, 0)
                .expect("real"),
            location: None,
            organiser: Some("Ana Silva".to_string()),
            attendees: vec!["Sam Patel".to_string()],
            all_day: false,
        };

        let event = as_event(occurrence);

        assert_eq!(event.subject, "Standup");
        assert_eq!(event.start, "2026-08-29T09:30:00");
        assert_eq!(event.organiser.as_deref(), Some("Ana Silva"));
    }
}

#[cfg(test)]
mod fetching {
    //! The whole chain — HTTP, parse, expand, merge — against a stub calendar.

    use super::*;
    use crate::llama::test_support::serve;

    /// A weekly Monday standup, defined once in 2026.
    ///
    /// Built from a list rather than one long literal: a stray leading space
    /// in a fixture is a *folded line* to RFC 5545, so a malformed fixture
    /// silently glues `SUMMARY` onto the line above and the parser is blamed
    /// for it. That happened while writing this test.
    fn calendar_file() -> String {
        [
            "BEGIN:VCALENDAR",
            "VERSION:2.0",
            "BEGIN:VEVENT",
            "UID:standup@example.com",
            "SUMMARY:Standup",
            "DTSTART:20260824T093000",
            "DTEND:20260824T100000",
            "RRULE:FREQ=WEEKLY;BYDAY=MO",
            "ORGANIZER;CN=Ana Silva:mailto:ana@example.com",
            "END:VEVENT",
            "END:VCALENDAR",
        ]
        .join("\r\n")
            + "\r\n"
    }

    fn monday(day: u32) -> (chrono::NaiveDateTime, chrono::NaiveDateTime) {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 10, day).expect("a real date");

        (
            date.and_time(chrono::NaiveTime::MIN),
            date.and_hms_opt(23, 59, 0).expect("a real time"),
        )
    }

    #[tokio::test]
    async fn reads_a_calendar_and_expands_it_onto_today() {
        let (host, _server) = serve(vec![("HTTP/1.1 200 OK", calendar_file())]);
        let client = Client::insecure().expect("client");

        // 5 October 2026 is a Monday, six weeks after the series began.
        let (from, to) = monday(5);
        let events = client
            .events(&host, from, to, 0)
            .await
            .expect("should read");

        assert_eq!(events.len(), 1, "the standup recurs onto this Monday");
        assert_eq!(events[0].subject, "Standup");
        assert_eq!(events[0].start, "2026-10-05T09:30:00");
        assert_eq!(events[0].organiser.as_deref(), Some("Ana Silva"));
    }

    #[tokio::test]
    async fn says_nothing_happened_on_a_day_nothing_happened() {
        let (host, _server) = serve(vec![("HTTP/1.1 200 OK", calendar_file())]);
        let client = Client::insecure().expect("client");

        let (from, to) = monday(6); // a Tuesday
        assert!(client
            .events(&host, from, to, 0)
            .await
            .expect("read")
            .is_empty());
    }

    #[tokio::test]
    async fn refuses_something_that_is_not_a_calendar() {
        let (host, _server) = serve(vec![(
            "HTTP/1.1 200 OK",
            "<html>Sign in to continue</html>",
        )]);
        let client = Client::insecure().expect("client");

        let error = client.read(&host).await.expect_err("that is a login page");

        assert!(matches!(error, Error::NotACalendar), "{error:?}");
    }

    #[tokio::test]
    async fn treats_a_refusal_as_unreachable_without_saying_where() {
        let (host, _server) = serve(vec![("HTTP/1.1 404 Not Found", "nope")]);
        let client = Client::insecure().expect("client");

        let error = client.read(&host).await.expect_err("404");

        assert!(matches!(error, Error::Unreachable), "{error:?}");
        assert!(
            !error.to_string().contains("127.0.0.1"),
            "not even the host, which in production is the credential"
        );
    }

    /// The guard that keeps a release build from ever fetching plain HTTP.
    #[test]
    fn a_normal_client_still_refuses_http_however_it_is_reached() {
        let client = Client::new().expect("client");

        assert!(matches!(
            client.address("http://calendar.example.com/basic.ics"),
            Err(Error::NotHttps)
        ));
    }
}
