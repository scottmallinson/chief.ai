//! Does an answer stay inside what the model was shown?
//!
//! A small model asked to write up a day it was given will, sooner or later,
//! add a meeting at 3pm, a pull request number or a colleague that nobody gave
//! it. Each of those looks like every other line of the answer, and the reader
//! has no way to tell. This module compares the answer with the material it was
//! written from and names what it cannot find there: times, numbers and
//! capitalised words in the middle of a sentence (names, nearly always).
//!
//! **It compares values, not wording.** There is no list of phrases to be
//! suspicious of, and nothing here tries to understand the question. A time in
//! the answer is either somewhere in the material or it is not.
//!
//! It is a heuristic and says so. A capitalised word at the start of a sentence
//! cannot be told from a common word without a dictionary, so those are not
//! checked, and an invented claim that names no time, number or person ("you
//! have a busy day") passes. What it catches, it catches with certainty: an
//! empty result here means only that nothing checkable was invented.
//!
//! The rules are the benchmark's (`inventedThings` in `bench/score.mjs`) on
//! purpose, so what the product refuses and what the gate counts agree. That
//! also means the benchmark cannot, on its own, show this check working: a
//! check and a scorer built from the same rules agree by construction. The
//! forbidden-term checks and a judge are the independent measures.

use std::collections::HashSet;

/// Capitalised words that are not names however they sit in a sentence.
const COMMON_CAPITALISED: &[&str] = &[
    "i",
    "i'm",
    "i've",
    "i'll",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "chief",
    "github",
    "outlook",
    "ok",
    "utc",
];

/// Things in `answer` that are not in `shown`: `time 3pm`, `number #42`,
/// `name Priya`, each once, in the order they first appear.
#[cfg(test)]
pub fn unsupported(answer: &str, shown: &str) -> Vec<String> {
    unsupported_in(answer, shown, shown)
}

/// [`unsupported`], with clock times looked for in `times_in` rather than in
/// everything shown.
///
/// A brief is stamped with the time it was written, and that time is in the
/// prompt, so a model that writes "Meeting at 06:00" over an all-day event has
/// used a time it was given. Measured on Llama 3.2 3B. It is the time of
/// nothing the user has, so a time is looked for in the material alone.
pub fn unsupported_in(answer: &str, shown: &str, times_in: &str) -> Vec<String> {
    let names_in = without_placeholders(answer);
    let seen = normal(shown);
    let times_seen = normal(times_in);
    let known: HashSet<String> = tokens(&seen).into_iter().collect();
    let mut found: Vec<String> = Vec::new();
    let mut note = |what: String| {
        if !found.contains(&what) {
            found.push(what);
        }
    };

    // **Times and numbers are read from the whole answer, brackets and all.**
    // A placeholder is not a name, but `[10:00 AM]` is still a time.
    for time in times(answer) {
        let full = normal(&time);
        let bare = strip_meridiem(&full);
        if !times_seen.contains(&bare) && !times_seen.contains(&full) {
            note(format!("time {time}"));
        }
    }

    for number in numbers(answer) {
        if !seen.contains(&number) {
            note(format!("number {number}"));
        }
    }

    for line in names_in.lines() {
        let line = line_body(line);
        let mut after_stop = false;

        // The first word of a line is skipped, and so is the word after a full
        // stop: a capital there means nothing.
        for word in line.split_whitespace().skip(1) {
            let clean = word
                .trim_start_matches(|c: char| !c.is_ascii_alphabetic())
                .trim_end_matches(|c: char| !c.is_ascii_alphabetic());

            if !after_stop && looks_like_a_name(clean) {
                let key = normal(clean);
                // "Parents'" and "Primary's" are the words the material has, possessed.
                let stem = key
                    .strip_suffix("'s")
                    .or_else(|| key.strip_suffix('\''))
                    .unwrap_or(&key);
                if !known.contains(&key)
                    && !known.contains(stem)
                    && !COMMON_CAPITALISED.contains(&key.as_str())
                {
                    note(format!("name {clean}"));
                }
            }

            after_stop = word.ends_with(['.', '!', '?', ':']);
        }
    }

    found
}

/// A line without the marker that opens it: a bullet or a list number, and
/// then a clock time, as in `- 14:00: Attend the call`. The word that follows
/// is the first word of the line, whatever its case.
fn line_body(line: &str) -> &str {
    let rest = line.trim_start();
    let rest = match rest.strip_prefix(['-', '*', '\u{2022}']) {
        Some(after) => after,
        None => {
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            match rest[digits..].strip_prefix(['.', ')']) {
                Some(after) if digits > 0 => after,
                _ => rest,
            }
        }
    };
    let rest = rest.trim_start();

    let chars: Vec<char> = rest.chars().take(12).collect();
    let digits = run(&chars, 0, |c| c.is_ascii_digit());
    let mut end = digits;
    if digits == 0 || digits > 2 {
        return rest;
    }
    let clock = chars.get(end) == Some(&':') && run(&chars, end + 1, |c| c.is_ascii_digit()) == 2;
    if clock {
        end += 3;
    }
    let meridiem = meridiem_end(&chars, end);
    if let Some(after) = meridiem {
        end = after;
    } else if !clock {
        return rest;
    }

    let skipped: usize = chars[..end].iter().map(|c| c.len_utf8()).sum();
    rest[skipped..].trim_start_matches(|c: char| {
        c.is_whitespace() || matches!(c, ':' | '-' | '\u{2013}' | '\u{2014}' | '.' | ')')
    })
}

/// `Abc…`: a capital followed by at least two lower-case letters.
fn looks_like_a_name(word: &str) -> bool {
    let mut chars = word.chars();

    chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
}

/// Case, curly quotes and dash variants removed, so "1:1" and "Parents’ evening" match.
fn normal(text: &str) -> String {
    let folded: String = text
        .to_lowercase()
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            '\u{2013}' | '\u{2014}' => '-',
            other => other,
        })
        .collect();

    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `[Name]` is a placeholder the model was asked to leave, not a name.
fn without_placeholders(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;

    for c in text.chars() {
        match c {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }

    out
}

/// Words and numbers: runs of letters and digits, keeping `priya's` whole.
fn tokens(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;

    while at < chars.len() {
        if !chars[at].is_ascii_alphanumeric() {
            at += 1;
            continue;
        }

        let start = at;
        while at < chars.len() {
            let c = chars[at];
            let joins = c == '\''
                && at > start
                && chars.get(at + 1).is_some_and(|n| n.is_ascii_lowercase());
            if c.is_ascii_alphanumeric() || joins {
                at += 1;
            } else {
                break;
            }
        }

        out.push(chars[start..at].iter().collect());
    }

    out
}

/// `10:30`, `3pm`, `3 pm`, `10:30 am`: a clock time as a person writes one.
fn times(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;

    while at < chars.len() {
        let starts_word = chars[at].is_ascii_digit() && (at == 0 || !is_word(chars[at - 1]));
        if !starts_word {
            at += 1;
            continue;
        }

        let digits = run(&chars, at, |c| c.is_ascii_digit());
        let mut end = at + digits;
        let mut matched = false;

        if digits <= 2 {
            let minutes = chars.get(end) == Some(&':')
                && run(&chars, end + 1, |c| c.is_ascii_digit()) == 2
                && !chars.get(end + 3).is_some_and(|c| is_word(*c));
            if minutes {
                end += 3;
                matched = true;
            }

            // An optional single space, then am or pm on a word boundary.
            if let Some(after) = meridiem_end(&chars, end) {
                end = after;
                matched = true;
            }
        }

        if matched {
            out.push(chars[at..end].iter().collect());
            at = end;
        } else {
            at += digits.max(1);
        }
    }

    out
}

/// `#42` of any length, and any other standalone run of two or more digits.
fn numbers(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;

    while at < chars.len() {
        if !chars[at].is_ascii_digit() {
            at += 1;
            continue;
        }

        let digits = run(&chars, at, |c| c.is_ascii_digit());
        let end = at + digits;
        let before = at.checked_sub(1).map(|i| chars[i]);
        let hashed = before == Some('#');
        let bounded = before.map_or(true, |c| !is_word(c) || c == '#')
            && !chars.get(end).is_some_and(|c| is_word(*c));

        if bounded && (hashed || digits >= 2) && !part_of_a_time(&chars, at, end) {
            out.push(chars[at..end].iter().collect());
        }

        at = end;
    }

    out
}

/// Whether the digits at `from..to` are the hour or the minutes of a time
/// that [`times`] reports, so the same figure is not named twice.
fn part_of_a_time(chars: &[char], from: usize, to: usize) -> bool {
    let after_colon = from >= 2 && chars[from - 1] == ':' && chars[from - 2].is_ascii_digit();
    let before_colon =
        chars.get(to) == Some(&':') && chars.get(to + 1).is_some_and(char::is_ascii_digit);

    after_colon || before_colon || meridiem_end(chars, to).is_some()
}

/// Where `am` or `pm` (after at most one space) starting at `from` ends, if one
/// does, and is a word of its own.
fn meridiem_end(chars: &[char], from: usize) -> Option<usize> {
    let at = if chars.get(from) == Some(&' ') {
        from + 1
    } else {
        from
    };
    let first = chars.get(at)?;
    let second = chars.get(at + 1)?;

    let is_meridiem = (first.eq_ignore_ascii_case(&'a') || first.eq_ignore_ascii_case(&'p'))
        && second.eq_ignore_ascii_case(&'m')
        && !chars.get(at + 2).is_some_and(|c| is_word(*c));

    is_meridiem.then_some(at + 2)
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn run(chars: &[char], from: usize, keep: impl Fn(char) -> bool) -> usize {
    chars.iter().skip(from).take_while(|c| keep(**c)).count()
}

/// `10:30 am` becomes `10:30`, `3pm` becomes `3`.
fn strip_meridiem(time: &str) -> String {
    let trimmed = time.trim_end();

    for suffix in ["am", "pm"] {
        if let Some(rest) = trimmed.strip_suffix(suffix) {
            return rest.trim_end().to_string();
        }
    }

    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_in_the_material_is_not_an_invention() {
        assert!(unsupported("Standup at 09:30.", "09:30 Standup").is_empty());
        assert!(unsupported("Standup at 9:30am.", "9:30 Standup").is_empty());
    }

    #[test]
    fn a_time_nobody_gave_is_named() {
        assert_eq!(
            unsupported("- Dentist at 3pm", "16:15 Dentist"),
            vec!["time 3pm"]
        );
        assert_eq!(
            unsupported("Call at 14:00.", "09:00 Standup"),
            vec!["time 14:00"]
        );
    }

    #[test]
    fn a_time_with_a_space_before_the_meridiem_is_read_as_one() {
        assert_eq!(
            unsupported("Lunch at 12 pm", "09:00 Standup"),
            vec!["time 12 pm"]
        );
    }

    #[test]
    fn a_number_is_checked_and_a_single_digit_is_not() {
        assert!(unsupported("You have 3 reviews", "nothing").is_empty());
        assert_eq!(unsupported("Fix #482", "#12 other"), vec!["number 482"]);
        assert_eq!(
            unsupported("There are 14 of them", "a b"),
            vec!["number 14"]
        );
        assert!(unsupported("There are 14 of them", "14 meetings").is_empty());
    }

    #[test]
    fn a_pull_request_number_of_one_digit_is_checked_too() {
        assert_eq!(unsupported("Review #7", "#12 other"), vec!["number 7"]);
        assert!(unsupported("Review #7", "#7 other").is_empty());
    }

    #[test]
    fn a_name_in_the_middle_of_a_sentence_is_checked() {
        assert_eq!(
            unsupported("- Reply to Priya today", "Mail from Sam"),
            vec!["name Priya"]
        );
        assert!(unsupported("- Reply to Priya today", "Mail from Priya").is_empty());
        assert!(unsupported("- Reply to Priya's note", "Priya's note").is_empty());
    }

    #[test]
    fn a_possessive_of_a_word_the_material_has_is_not_a_name() {
        assert!(unsupported("- 17:30 Parents' evening", "17:30 Parents' evening").is_empty());
        assert!(unsupported("- Review Oakfield Primary's day", "Oakfield Primary: day").is_empty());
        assert_eq!(
            unsupported("- Reply to Marcus's note", "Mail from Priya"),
            vec!["name Marcus's"]
        );
    }

    #[test]
    fn the_word_after_a_leading_time_is_the_first_word_of_the_line() {
        let shown = "14:00 Vendor call\n14:30 Budget check-in";
        assert!(unsupported(
            "- 14:00: Attend vendor call\n• 14:30 Meet for the check-in",
            shown
        )
        .is_empty());
        assert!(unsupported("3pm - Prepare the call\n1. Review it", "3pm call").is_empty());
        assert_eq!(
            unsupported("- 14:00: Attend vendor call with Marcus", shown),
            vec!["name Marcus"]
        );
    }

    #[test]
    fn the_first_word_of_a_line_and_of_a_sentence_are_not_checked() {
        assert!(unsupported("- Standup first. Meeting after", "nothing").is_empty());
    }

    #[test]
    fn days_months_and_the_names_chief_uses_are_not_names() {
        assert!(unsupported("- Due on Friday in October, per GitHub", "nothing").is_empty());
    }

    #[test]
    fn a_time_in_brackets_is_still_a_time() {
        assert_eq!(
            unsupported("- [10:00 AM] - time not mentioned", "all day INSET"),
            vec!["time 10:00 AM"]
        );
    }

    #[test]
    fn the_time_the_prompt_was_stamped_with_is_not_the_time_of_anything() {
        let clock = "The current date and time is Sunday 18 October 2026, 06:00.";
        let material = "## Today's meetings\n- all day INSET day";
        let prompt = format!("{clock}\n\n{material}");

        assert!(unsupported("- Meeting at 06:00", &prompt).is_empty());
        assert_eq!(
            unsupported_in("- Meeting at 06:00", &prompt, material),
            vec!["time 06:00"]
        );
        assert!(unsupported_in("- INSET day, 18 October 2026", &prompt, material).is_empty());
    }

    #[test]
    fn a_placeholder_is_not_a_name() {
        assert!(unsupported("- Thanks to [Colleague Name] for it", "nothing").is_empty());
    }

    #[test]
    fn curly_quotes_and_dashes_match_straight_ones() {
        assert!(
            unsupported("- Bring Parents' evening notes", "Parents\u{2019} evening").is_empty()
        );
    }

    #[test]
    fn each_finding_is_reported_once_in_order() {
        assert_eq!(
            unsupported("- At 3pm see Priya, then 3pm again with Priya", "x"),
            vec!["time 3pm", "name Priya"]
        );
    }

    #[test]
    fn digits_inside_a_word_are_not_numbers() {
        assert!(unsupported("- See abc123 and 7zip", "x").is_empty());
    }

    /// Not a test: reads the benchmark records named by `CHECK_REPLAY` (colon
    /// separated paths) and prints what this check says about every answer, one
    /// JSON object per line, for comparison with the benchmark's own scorer.
    ///
    /// `CHECK_REPLAY=run.jsonl cargo test --lib check::tests::replay -- --ignored --nocapture`
    #[test]
    #[ignore = "reads benchmark records from CHECK_REPLAY"]
    fn replay_a_benchmark_file() {
        let paths = std::env::var("CHECK_REPLAY").expect("CHECK_REPLAY");

        for path in paths.split(':') {
            let text = std::fs::read_to_string(path).expect("record");
            for line in text.lines() {
                let run: serde_json::Value = serde_json::from_str(line).expect("json");
                if run["type"] != "run" {
                    continue;
                }
                let shown: String = run["exchanges"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|e| e["request"]["messages"].as_array().into_iter().flatten())
                    .filter_map(|m| m["content"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let answer = run["answer"].as_str().unwrap_or_default();
                println!(
                    "REPLAY {}",
                    serde_json::json!({
                        "file": path,
                        "fixture": run["fixture"],
                        "repeat": run["repeat"],
                        "findings": unsupported(answer, &shown),
                    })
                );
            }
        }
    }
}
