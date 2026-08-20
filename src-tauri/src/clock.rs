//! What "now" means, written out for the model.
//!
//! A language model has no clock. Asked what shipped "last week" it will
//! happily answer against whenever its training data ended, which is how Chief
//! ends up confidently describing the wrong seven days. So every question is
//! prefixed with the current local date and time, plus the ranges a small model
//! cannot reliably work out for itself.
//!
//! The clock read here is the user's own — `Local`, not UTC — because "today"
//! means their today.

use chrono::{DateTime, Datelike, Days, Local, NaiveDate, TimeZone};

/// The day boundaries a working week is talked about in.
///
/// Weeks run Monday to Sunday: that is what "this week" means to someone
/// looking at their own work, and it matches ISO-8601.
const DAYS_IN_WEEK: u64 = 7;

/// How the present is described to the model, using the machine's clock.
pub fn present() -> String {
    describe(&Local::now())
}

/// Describe `now` as a block of facts the model can quote from.
///
/// Generic over the time zone so it can be exercised at a fixed offset rather
/// than against whatever clock the test machine happens to keep.
fn describe<Tz: TimeZone>(now: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let today = now.date_naive();
    let monday = start_of_week(today);
    let last_monday = monday - Days::new(DAYS_IN_WEEK);
    let next_monday = monday + Days::new(DAYS_IN_WEEK);

    format!(
        "The current date and time is {stamp}.

Resolve every relative date the user mentions against this list, in the user's \
local time. Dates are ISO-8601 (YYYY-MM-DD). Never work a date out from memory, \
and never answer with a date that is not derived from these:
- today: {today} ({weekday})
- yesterday: {yesterday}
- tomorrow: {tomorrow}
- this week (Monday to Sunday): {monday} to {sunday}
- last week: {last_monday} to {last_sunday}
- next week: {next_monday} to {next_sunday}
- the last 7 days: {week_ago} to {today}
- this month: {month_start} to {month_end}

Anything dated later than {today} has not happened yet.",
        stamp = now.format("%H:%M on %A %-d %B %Y (UTC%:z)"),
        weekday = today.format("%A"),
        yesterday = today - Days::new(1),
        tomorrow = today + Days::new(1),
        sunday = monday + Days::new(DAYS_IN_WEEK - 1),
        last_sunday = monday - Days::new(1),
        next_sunday = next_monday + Days::new(DAYS_IN_WEEK - 1),
        week_ago = today - Days::new(DAYS_IN_WEEK - 1),
        month_start = today.with_day(1).unwrap_or(today),
        month_end = end_of_month(today),
    )
}

/// The Monday of `day`'s week.
fn start_of_week(day: NaiveDate) -> NaiveDate {
    day - Days::new(u64::from(day.weekday().num_days_from_monday()))
}

/// The last day of `day`'s month, whatever length it is.
fn end_of_month(day: NaiveDate) -> NaiveDate {
    let first = day.with_day(1).unwrap_or(day);

    // Step to the first of next month and back one day, so February and leap
    // years take care of themselves.
    first
        .checked_add_months(chrono::Months::new(1))
        .and_then(|next| next.checked_sub_days(Days::new(1)))
        .unwrap_or(day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    /// A Thursday, two hours ahead of UTC.
    fn thursday() -> DateTime<FixedOffset> {
        at(2026, 8, 20, 14, 32)
    }

    fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(2 * 3600)
            .expect("a two hour offset is valid")
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("the timestamp should be unambiguous")
    }

    #[test]
    fn states_the_time_in_the_users_own_offset() {
        let described = describe(&thursday());

        assert!(
            described.contains("14:32 on Thursday 20 August 2026 (UTC+02:00)"),
            "got {described}"
        );
    }

    #[test]
    fn names_today_and_the_days_either_side() {
        let described = describe(&thursday());

        assert!(
            described.contains("- today: 2026-08-20 (Thursday)"),
            "{described}"
        );
        assert!(described.contains("- yesterday: 2026-08-19"), "{described}");
        assert!(described.contains("- tomorrow: 2026-08-21"), "{described}");
    }

    #[test]
    fn runs_weeks_from_monday_to_sunday() {
        let described = describe(&thursday());

        assert!(
            described.contains("- this week (Monday to Sunday): 2026-08-17 to 2026-08-23"),
            "{described}"
        );
        assert!(
            described.contains("- last week: 2026-08-10 to 2026-08-16"),
            "{described}"
        );
        assert!(
            described.contains("- next week: 2026-08-24 to 2026-08-30"),
            "{described}"
        );
    }

    #[test]
    fn a_monday_is_the_start_of_its_own_week() {
        // The awkward case: on a Monday, "this week" has barely begun and
        // "last week" is the seven days before today, not before Sunday.
        let described = describe(&at(2026, 8, 17, 9, 0));

        assert!(
            described.contains("- this week (Monday to Sunday): 2026-08-17 to 2026-08-23"),
            "{described}"
        );
        assert!(
            described.contains("- last week: 2026-08-10 to 2026-08-16"),
            "{described}"
        );
    }

    #[test]
    fn a_sunday_still_belongs_to_the_week_that_started_on_monday() {
        let described = describe(&at(2026, 8, 23, 21, 0));

        assert!(
            described.contains("- this week (Monday to Sunday): 2026-08-17 to 2026-08-23"),
            "{described}"
        );
    }

    #[test]
    fn the_last_seven_days_include_today() {
        let described = describe(&thursday());

        assert!(
            described.contains("- the last 7 days: 2026-08-14 to 2026-08-20"),
            "{described}"
        );
    }

    #[test]
    fn a_week_can_span_two_months() {
        // Wednesday 2 September 2026: last week ran across the month boundary.
        let described = describe(&at(2026, 9, 2, 10, 0));

        assert!(
            described.contains("- this week (Monday to Sunday): 2026-08-31 to 2026-09-06"),
            "{described}"
        );
        assert!(
            described.contains("- last week: 2026-08-24 to 2026-08-30"),
            "{described}"
        );
    }

    #[test]
    fn the_month_ends_where_the_month_ends() {
        assert_eq!(end_of_month(day(2026, 2, 10)), day(2026, 2, 28));
        assert_eq!(
            end_of_month(day(2028, 2, 10)),
            day(2028, 2, 29),
            "2028 is a leap year"
        );
        assert_eq!(end_of_month(day(2026, 4, 30)), day(2026, 4, 30));
        assert_eq!(end_of_month(day(2026, 12, 1)), day(2026, 12, 31));
    }

    #[test]
    fn describes_the_month_around_today() {
        let described = describe(&thursday());

        assert!(
            described.contains("- this month: 2026-08-01 to 2026-08-31"),
            "{described}"
        );
    }

    #[test]
    fn tells_the_model_not_to_invent_dates() {
        let described = describe(&thursday());

        assert!(
            described.contains("Never work a date out from memory"),
            "{described}"
        );
        assert!(
            described.contains("Anything dated later than 2026-08-20 has not happened yet."),
            "{described}"
        );
    }

    #[test]
    fn reads_the_machines_own_clock() {
        // Only that it produces something shaped right — the value is whatever
        // day it happens to be.
        let described = present();

        assert!(
            described.starts_with("The current date and time is "),
            "{described}"
        );
        assert!(described.contains("- today: "), "{described}");
    }

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("a valid date")
    }
}
