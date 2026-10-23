//! Writing a long list in pieces, so nothing is cut off and nothing is left out.
//!
//! A small model asked to write out thirty lines in one go runs into the
//! length limit, or silently drops the ones it found dull. Both are the same
//! mistake for the reader: an overview that looks complete and is not. So a
//! list is written a section at a time, a section a few lines at a time, one
//! short request each. The headings are written here in code, so the structure
//! cannot drift, and any line the model did not mention is put back as it was
//! found.
//!
//! The standup and the daily brief both use it. They differ in who is
//! speaking and in whether the wording is checked against the material, which
//! the caller says.

use crate::llama::{self, ChatRequest};

/// The most lines asked about in one request. A request this size ends well
/// inside the answer ceiling on the smallest model Chief runs, which a whole
/// day in one request did not.
pub const BATCH: usize = 8;

/// One headed list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub heading: String,
    pub lines: Vec<String>,
    /// Whether a model is asked to word it. A section of a line or two is its
    /// own list: a model could only reword it, and a rewording is where
    /// invention comes from.
    pub worded: bool,
}

/// What was written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Written {
    pub text: String,
    /// What the checks refused, so the line behind it could be written from
    /// the material instead. Empty when nothing was refused.
    pub refused: Vec<String>,
    /// How many requests were made.
    pub requests: usize,
}

/// A line this long, written this many times, is a loop and not a list.
const LOOP_MIN_CHARS: usize = 12;
const LOOP_REPEATS: usize = 3;

/// The line as compared for repeats: no list marker, no case, no padding.
fn loop_key(line: &str) -> String {
    line.trim()
        .trim_start_matches(['-', '*', '•', ' '])
        .to_lowercase()
}

/// Whether the model circled: one substantial line written three or more times.
fn is_looping(text: &str) -> bool {
    let mut counts = std::collections::HashMap::new();
    text.lines().map(loop_key).any(|key| {
        if key.chars().count() < LOOP_MIN_CHARS {
            return false;
        }
        let seen = counts.entry(key).or_insert(0usize);
        *seen += 1;
        *seen >= LOOP_REPEATS
    })
}

/// An answer with its loop taken out.
///
/// A small model that cannot stop listing the same items runs to the length
/// limit and leaves one good copy of each, then the same lines again, then an
/// apology. Rejecting that would retry on every pass and loop again on a slow
/// machine, so instead each line is kept once, in order, and the length-limit
/// marker is dropped: the model did not run out of things to say, it ran out of
/// new ones. An answer that never loops is returned exactly as written.
fn break_loop(text: &str) -> String {
    if !is_looping(text) {
        return text.to_string();
    }
    let body = text.strip_suffix(llama::ANSWER_CUT).unwrap_or(text);
    let mut seen = std::collections::HashSet::new();
    let kept: Vec<&str> = body
        .lines()
        .filter(|line| {
            let key = loop_key(line);
            key.chars().count() < LOOP_MIN_CHARS || seen.insert(key)
        })
        .collect();
    kept.join("\n").trim().to_string()
}

/// What identifies a line when looking for it in what was written about it.
///
/// A number the line carries — a pull request, a ticket key, a time — is the
/// thing a model copies faithfully or drops entirely, so that is what is looked
/// for. A line with none is looked for by its longest word. `None` is a line
/// with nothing distinctive to look for, which is not held to it.
fn marker(line: &str) -> Option<String> {
    if let Some((_, after)) = line.split_once('#') {
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();

        if !digits.is_empty() {
            return Some(format!("#{digits}"));
        }
    }

    let words: Vec<&str> = line
        .split(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | ':')))
        .filter(|word| !word.is_empty())
        .collect();

    // A time (09:30) or a key (REC-42, ENG-7).
    if let Some(word) = words.iter().find(|word| {
        word.chars().any(|c| c.is_ascii_digit())
            && (word.contains(':') || (word.contains('-') && word.chars().any(char::is_alphabetic)))
    }) {
        return Some(word.to_lowercase());
    }

    words
        .iter()
        .filter(|word| word.chars().filter(|c| c.is_alphabetic()).count() >= 5)
        .max_by_key(|word| word.chars().count())
        .map(|word| word.to_lowercase())
}

/// A line mentioned in `written`, or one with nothing to look for.
fn mentioned(line: &str, written: &str) -> bool {
    marker(line).map_or(true, |marker| written.to_lowercase().contains(&marker))
}

/// The lines `written` does not mention.
#[must_use]
pub fn left_out<'a>(lines: &'a [String], written: &str) -> Vec<&'a String> {
    lines
        .iter()
        .filter(|line| !mentioned(line, written))
        .collect()
}

/// The lines as bullets, exactly as they were found.
fn bullets(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| format!("- {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Write every section.
///
/// * `heading(title)` is how a section's title is written into the text:
///   `Done:` in a chat, `## Waiting on you` in a file the brief view reads.
/// * `prompt(title, lines)` is the instruction for one request.
/// * `request(prompt)` wraps it with whatever the caller sends every time.
/// * `unsupported(written, lines)` names what the answer says that the lines
///   do not. A non-empty answer means the lines are written as found instead.
///   A caller that streams must pass a check that never refuses, since what
///   was shown cannot be taken back.
/// * `on_delta` hears the text as it is settled.
///
/// # Errors
/// When the engine cannot be reached or stops answering.
pub async fn write(
    client: &llama::Client,
    sections: &[Section],
    heading: impl Fn(&str) -> String,
    prompt: impl Fn(&str, &str) -> String,
    request: impl Fn(String) -> ChatRequest,
    unsupported: impl Fn(&str, &[String]) -> Vec<String>,
    mut on_delta: impl FnMut(&str),
) -> Result<Written, llama::Error> {
    let mut written = Written::default();

    for (index, section) in sections.iter().enumerate() {
        let title = format!(
            "{}{}\n",
            if index == 0 { "" } else { "\n\n" },
            heading(&section.heading)
        );
        on_delta(&title);
        written.text.push_str(&title);

        for (batch_index, batch) in section.lines.chunks(BATCH).enumerate() {
            if batch_index > 0 {
                on_delta("\n");
                written.text.push('\n');
            }

            let text = if section.worded {
                written.requests += 1;
                let reply = client
                    .chat_stream(
                        &request(prompt(&section.heading, &batch.join("\n"))),
                        |token| on_delta(token),
                    )
                    .await?;

                let reply = break_loop(reply.content.trim_end());
                let unfinished = reply.ends_with(llama::ANSWER_CUT)
                    || reply.ends_with(llama::ANSWER_INTERRUPTED);
                let invented = if unfinished || reply.is_empty() {
                    vec!["no complete answer".to_string()]
                } else {
                    unsupported(&reply, batch)
                };

                if invented.is_empty() {
                    let mut text = reply;

                    for line in left_out(batch, &text) {
                        let restored = format!("\n- {line}");
                        on_delta(&restored);
                        text.push_str(&restored);
                    }

                    text
                } else {
                    written.refused.extend(invented);
                    let text = bullets(batch);
                    on_delta(&text);
                    text
                }
            } else {
                let text = bullets(batch);
                on_delta(&text);
                text
            };

            written.text.push_str(&text);
        }
    }

    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llama::test_support::{delta, events, serve};
    use crate::llama::Message;

    fn said(text: &str) -> String {
        events(&[delta(text)])
    }

    fn ask(prompt: String) -> ChatRequest {
        ChatRequest::new("model", vec![Message::user(prompt)])
    }

    fn section(heading: &str, lines: &[&str], worded: bool) -> Section {
        Section {
            heading: heading.to_string(),
            lines: lines.iter().map(ToString::to_string).collect(),
            worded,
        }
    }

    fn never(_: &str, _: &[String]) -> Vec<String> {
        Vec::new()
    }

    #[test]
    fn a_line_is_looked_for_by_its_number_its_time_its_key_or_its_longest_word() {
        assert_eq!(
            marker("Fix it (acme/api#88, draft)").as_deref(),
            Some("#88")
        );
        assert_eq!(marker("09:30 Standup with Ana").as_deref(), Some("09:30"));
        assert_eq!(
            marker("REC-42 Read Linear [Todo]").as_deref(),
            Some("rec-42")
        );
        assert_eq!(marker("Priya: Re: Budget").as_deref(), Some("budget"));
        assert_eq!(
            marker("ok"),
            None,
            "a line with nothing to look for is not held to it"
        );
        assert_eq!(marker("Trailing hash #"), Some("trailing".to_string()));
    }

    #[test]
    fn a_line_the_model_left_out_is_found() {
        let lines: Vec<String> = [
            "Fix it (acme/api#88)",
            "Add it (acme/web#7)",
            "09:30 Standup",
            "ok",
        ]
        .map(String::from)
        .to_vec();

        let missing = left_out(
            &lines,
            "- Fixed the thing in acme/api #88\n- Standup is at 09:30",
        );

        assert_eq!(missing, vec![&lines[1]]);
    }

    #[test]
    fn a_brief_that_does_not_loop_is_returned_exactly_as_written() {
        let text = "- Review the billing pull request\n- Review the billing pull request\n- ok";
        assert_eq!(break_loop(text), text);
    }

    #[test]
    fn a_loop_is_cut_to_one_copy_of_each_line_and_loses_its_apology() {
        let looped = format!(
            "- Fix the login bug today\n- Ship the release notes\n- Fix the login bug today\n\
             - Ship the release notes\n- Fix the login bug today{}",
            llama::ANSWER_CUT
        );
        assert_eq!(
            break_loop(&looped),
            "- Fix the login bug today\n- Ship the release notes"
        );
    }

    #[test]
    fn short_lines_and_headings_are_never_treated_as_a_loop() {
        let text = "## Today\n- ok\n- ok\n- ok\n- ok";
        assert_eq!(break_loop(text), text);
    }

    /// One request each, carrying only its own section's lines. Headings are
    /// the caller's, a line the model dropped is put back, and a section that
    /// is not worded asks nobody.
    ///
    /// Proved by sending every section's lines in each request:
    ///
    /// ```text
    /// the second request carried a line from the first
    /// ```
    #[tokio::test]
    async fn each_section_is_its_own_request_and_a_dropped_line_comes_back() {
        let (host, server) = serve(vec![
            ("HTTP/1.1 200 OK", said("- reworded one")),
            (
                "HTTP/1.1 200 OK",
                said("- reworded two\n- reworded three #3"),
            ),
        ]);
        let client = llama::Client::with_base_url(&host).expect("client");
        let sections = [
            section("First", &["alpha #1", "beta #2"], true),
            section("Second", &["gamma #3", "delta #4"], true),
            section("Third", &["epsilon only"], false),
        ];

        let mut streamed = String::new();
        let written = write(
            &client,
            &sections,
            |heading| format!("## {heading}"),
            |heading, lines| format!("{heading}\n{lines}"),
            ask,
            never,
            |text| streamed.push_str(text),
        )
        .await
        .expect("written");

        let requests = server.await.expect("stub");
        assert_eq!(
            requests.len(),
            2,
            "the section that is not worded asks nobody"
        );
        assert!(
            requests[0].contains("alpha #1") && !requests[0].contains("gamma #3"),
            "the first request carried a line from the second"
        );
        assert!(
            requests[1].contains("gamma #3") && !requests[1].contains("alpha #1"),
            "the second request carried a line from the first"
        );

        assert_eq!(
            written.text,
            "## First\n- reworded one\n- alpha #1\n- beta #2\n\n\
             ## Second\n- reworded two\n- reworded three #3\n- delta #4\n\n\
             ## Third\n- epsilon only"
        );
        assert_eq!(written.requests, 2);
        assert_eq!(streamed, written.text, "what streamed is what was returned");
    }

    /// More lines than a request carries are asked in turn under one heading.
    ///
    /// Proved by setting `BATCH` to 100: one request carries all eleven.
    #[tokio::test]
    async fn a_section_longer_than_a_batch_is_asked_in_pieces_under_one_heading() {
        let lines: Vec<String> = (1..=11).map(|n| format!("item #{n}")).collect();
        let all: Vec<&str> = lines.iter().map(String::as_str).collect();
        let (host, server) = serve(vec![
            ("HTTP/1.1 200 OK", said("- a")),
            ("HTTP/1.1 200 OK", said("- b")),
        ]);
        let client = llama::Client::with_base_url(&host).expect("client");

        let written = write(
            &client,
            &[section("Many", &all, true)],
            |heading| format!("## {heading}"),
            |_, lines| lines.to_string(),
            ask,
            never,
            |_| {},
        )
        .await
        .expect("written");

        let requests = tokio::time::timeout(std::time::Duration::from_secs(5), server)
            .await
            .expect("the second batch was never asked")
            .expect("stub");
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("item #8") && !requests[0].contains("item #9"));
        assert!(requests[1].contains("item #9") && !requests[1].contains("item #8"));
        assert_eq!(written.text.matches("## ").count(), 1, "{}", written.text);
        for n in 1..=11 {
            assert!(
                written.text.contains(&format!("item #{n}")),
                "{}",
                written.text
            );
        }
    }

    /// An answer that names what the lines do not is not used: the lines are.
    #[tokio::test]
    async fn an_answer_the_check_refuses_is_replaced_by_the_lines_as_found() {
        let (host, _server) = serve(vec![("HTTP/1.1 200 OK", said("- a meeting with John"))]);
        let client = llama::Client::with_base_url(&host).expect("client");

        let written = write(
            &client,
            &[section("Day", &["standup #1", "retro #2"], true)],
            |heading| format!("## {heading}"),
            |_, lines| lines.to_string(),
            ask,
            |answer, _| {
                if answer.contains("John") {
                    vec!["name John".to_string()]
                } else {
                    Vec::new()
                }
            },
            |_| {},
        )
        .await
        .expect("written");

        assert_eq!(written.text, "## Day\n- standup #1\n- retro #2");
        assert_eq!(written.refused, ["name John"]);
    }

    /// An answer that stopped at the length limit may end halfway through a
    /// claim, so it is not used.
    #[tokio::test]
    async fn an_answer_cut_off_at_the_limit_is_replaced_by_the_lines() {
        let cut = events(&[
            delta("- half a lin"),
            serde_json::json!({
                "choices": [{ "index": 0, "delta": {}, "finish_reason": "length" }],
            }),
        ]);
        let (host, _server) = serve(vec![("HTTP/1.1 200 OK", cut)]);
        let client = llama::Client::with_base_url(&host).expect("client");

        let written = write(
            &client,
            &[section("Day", &["standup #1", "retro #2"], true)],
            |heading| format!("## {heading}"),
            |_, lines| lines.to_string(),
            ask,
            never,
            |_| {},
        )
        .await
        .expect("written");

        assert_eq!(written.text, "## Day\n- standup #1\n- retro #2");
        assert!(!written.text.contains("Cut short"));
    }
}
