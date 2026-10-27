//! Who is who across the user's tools, kept in a file they can read and edit.
//!
//! A calendar says "Dana Vance" and GitHub says `dvance`, and nothing in
//! either says they are one person. Guessing names the wrong person, which is
//! worse than naming nobody, so the link is written down where the user can
//! see it: `people.md` in the corpus, one line per person.
//!
//! ```text
//! ## Confirmed
//! - Dana Vance = dvance
//!
//! ## Suggested
//! - Priya Rao = prao
//! ```
//!
//! **Chief suggests; the user decides.** A line under *Suggested* is Chief's
//! proof of one exact match and is used for nothing until the user moves it up
//! to *Confirmed*. Chief only ever appends under *Suggested*: everything else
//! in the file, prose included, is the user's and is never rewritten.
//!
//! The proof is deliberately narrow. A person's public GitHub profile name has
//! to equal their calendar name exactly (ignoring case and spacing), with one
//! match on each side. A nickname, two people sharing a name, or no public name
//! produces nothing, and the user can add the line themselves.
//!
//! No model is involved anywhere here: the file is read and written as text and
//! the suggestions are string equality.

use serde::{Deserialize, Serialize};

use crate::{corpus, facts, recipe, session::GithubSession, settings};

/// Where the file lives, relative to the corpus.
pub const FILE: &str = "people.md";

/// What a new file starts with: the rules, in the file the user will read.
const STARTER: &str = "# People\n\n\
Who is who across your tools. Chief suggests, you decide.\n\n\
One line per person: `- Name as it appears on your calendar = GitHub handle`.\n\
Chief only uses the lines under Confirmed. Move a line up from Suggested to confirm it, \
or delete it to say no. You can add your own lines under Confirmed at any time.\n\n\
## Confirmed\n\n\
## Suggested\n";

/// Which GitHub handles' profile names have been looked up, so a daily recount
/// does not ask GitHub about the same person again.
const NAMES_KEY: &str = "people.github_names";

/// Every pair Chief has ever suggested, so a line the user deleted stays gone.
const SUGGESTED_KEY: &str = "people.suggested";

/// How many handles are looked up at most in one pass.
const LOOKUPS: usize = 30;

/// One line of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// The name as the calendar writes it.
    pub name: String,
    /// The GitHub login, without an `@`.
    pub handle: String,
}

/// What the file says.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct People {
    /// Lines the user has confirmed. The only ones anything may rely on.
    pub confirmed: Vec<Person>,
    pub suggested: Vec<Person>,
    /// List lines that could not be read, so Settings can say so rather than
    /// the line silently doing nothing.
    pub unread: Vec<String>,
}

/// Names compare without regard to case or the spacing between words.
fn key(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// A GitHub login: letters, digits and hyphens, and not empty.
fn is_handle(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 39
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn line_person(line: &str) -> Option<Person> {
    let body = line.trim().strip_prefix("- ")?;
    let (name, handle) = body.split_once(" = ")?;
    let handle = handle.trim().trim_start_matches('@');
    // "dvance (GitHub)" is how somebody might naturally write it.
    let handle = handle
        .strip_suffix("(GitHub)")
        .or_else(|| handle.strip_suffix("(github)"))
        .unwrap_or(handle)
        .trim();
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");

    (!name.is_empty() && is_handle(handle)).then(|| Person {
        name,
        handle: handle.to_string(),
    })
}

/// Read the file. Total: any text parses, and nothing is dropped silently —
/// a list line that is not a person is reported under `unread`.
#[must_use]
pub fn parse(text: &str) -> People {
    #[derive(PartialEq)]
    enum Section {
        Other,
        Confirmed,
        Suggested,
    }

    let mut people = People::default();
    let mut section = Section::Other;

    for line in text.lines() {
        let trimmed = line.trim();

        if let Some(heading) = trimmed.strip_prefix("## ") {
            section = match key(heading).as_str() {
                "confirmed" => Section::Confirmed,
                "suggested" => Section::Suggested,
                _ => Section::Other,
            };
            continue;
        }

        if section == Section::Other || !trimmed.starts_with("- ") {
            continue;
        }

        match (line_person(trimmed), &section) {
            (Some(person), Section::Confirmed) => people.confirmed.push(person),
            (Some(person), Section::Suggested) => people.suggested.push(person),
            _ => people.unread.push(trimmed.to_string()),
        }
    }

    people
}

/// The same pair, whichever way round it was written or cased.
fn same(left: &Person, right: &Person) -> bool {
    key(&left.name) == key(&right.name) && left.handle.eq_ignore_ascii_case(&right.handle)
}

/// Add suggestions under *Suggested* and touch nothing else.
///
/// A pair already anywhere in the file, in either section, is not added again.
/// Returns the text unchanged when there is nothing new, so a pass that finds
/// nothing does not rewrite the file.
#[must_use]
pub fn add_suggestions(text: &str, new: &[Person]) -> String {
    let existing = parse(text);
    let fresh: Vec<&Person> = new
        .iter()
        .filter(|person| {
            !existing
                .confirmed
                .iter()
                .chain(&existing.suggested)
                .any(|known| same(known, person))
        })
        .collect();

    if fresh.is_empty() {
        return text.to_string();
    }

    let mut lines: Vec<String> = text.lines().map(ToString::to_string).collect();

    let heading = lines.iter().position(|line| {
        line.trim()
            .strip_prefix("## ")
            .is_some_and(|name| key(name) == "suggested")
    });

    let at = match heading {
        Some(heading) => {
            // The end of the section: before the next heading, and after the
            // last line that is not blank.
            let end = lines[heading + 1..]
                .iter()
                .position(|line| line.trim().starts_with("## "))
                .map_or(lines.len(), |offset| heading + 1 + offset);

            (heading + 1..end)
                .rev()
                .find(|&index| !lines[index].trim().is_empty())
                .map_or(heading + 1, |index| index + 1)
        }
        None => {
            if lines.last().is_some_and(|line| !line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push("## Suggested".to_string());
            lines.len()
        }
    };

    for (offset, person) in fresh.iter().enumerate() {
        lines.insert(
            at + offset,
            format!("- {} = {}", person.name, person.handle),
        );
    }

    let mut written = lines.join("\n");
    written.push('\n');
    written
}

/// Which pairs a calendar and a set of GitHub profiles prove.
///
/// A pair needs the profile name to equal exactly one calendar name, and that
/// calendar name to be claimed by exactly one profile. Anything less is a
/// guess, and a guess here puts a stranger's name next to somebody's pull
/// request.
#[must_use]
pub fn matches(calendar: &[String], profiles: &[(String, String)]) -> Vec<Person> {
    let mut found = Vec::new();

    for (handle, profile_name) in profiles {
        let wanted = key(profile_name);
        if wanted.is_empty() {
            continue;
        }

        let mut names: Vec<&String> = Vec::new();
        for name in calendar {
            if key(name) == wanted && !names.iter().any(|seen| key(seen) == wanted) {
                names.push(name);
            }
        }

        let claimed = profiles
            .iter()
            .filter(|(other, other_name)| {
                !other.eq_ignore_ascii_case(handle) && key(other_name) == wanted
            })
            .count();

        if let ([name], 0) = (names.as_slice(), claimed) {
            found.push(Person {
                name: (*name).clone(),
                handle: handle.clone(),
            });
        }
    }

    found
}

/// What can go wrong.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Corpus(#[from] corpus::Error),
    #[error(transparent)]
    Storage(#[from] sqlx::Error),
    #[error(transparent)]
    Facts(#[from] crate::db::Error),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Names(std::collections::BTreeMap<String, String>);

async fn load<T: for<'de> Deserialize<'de> + Default>(context: &recipe::Context, key: &str) -> T {
    settings::get(&context.pool, key)
        .await
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Look for people the calendar and GitHub prove to be one, and add them to
/// `people.md` as suggestions. Returns how many were added.
///
/// Reads the facts Chief has already counted — who reviews the user's pull
/// requests, who they meet — and asks GitHub only for the public profile name
/// of a handle it has not asked about before, at most thirty at a time. No
/// model call. A GitHub that will not answer ends the pass quietly; the next
/// recount tries again.
pub async fn suggest(context: &recipe::Context) -> Result<usize, Error> {
    let rows = facts::list(&context.pool).await?;

    let handles: Vec<String> = rows
        .iter()
        .filter(|row| row.what == facts::REVIEWS)
        .map(|row| row.about.clone())
        .filter(|handle| is_handle(handle))
        .fold(Vec::new(), |mut seen, handle| {
            if !seen.contains(&handle) {
                seen.push(handle);
            }
            seen
        });

    let calendar: Vec<String> = rows
        .iter()
        .filter(|row| row.what == facts::MEETS)
        .map(|row| row.value.clone())
        .filter(|name| !name.is_empty())
        .collect();

    if handles.is_empty() || calendar.is_empty() {
        return Ok(0);
    }

    let mut cached: Names = load(context, NAMES_KEY).await;

    if let Some(account) = recipe::github_accounts(context).await.first().copied() {
        let session = GithubSession::new(&context.pool, &context.github, account);

        let missing: Vec<&String> = handles
            .iter()
            .filter(|handle| !cached.0.contains_key(*handle))
            .take(LOOKUPS)
            .collect();

        for handle in missing {
            match session.profile_name(handle).await {
                // An empty name is a remembered "has none", not a gap to refill.
                Ok(name) => {
                    cached.0.insert(handle.clone(), name.unwrap_or_default());
                }
                Err(error) => {
                    eprintln!("people: could not read a GitHub profile: {error}");
                    break;
                }
            }
        }

        if let Ok(text) = serde_json::to_string(&cached) {
            settings::set(&context.pool, NAMES_KEY, &text).await?;
        }
    }

    let profiles: Vec<(String, String)> = handles
        .iter()
        .filter_map(|handle| {
            cached
                .0
                .get(handle)
                .filter(|name| !name.is_empty())
                .map(|name| (handle.clone(), name.clone()))
        })
        .collect();

    let before: Vec<String> = load(context, SUGGESTED_KEY).await;
    let pair = |person: &Person| format!("{}|{}", key(&person.name), person.handle.to_lowercase());

    let proven: Vec<Person> = matches(&calendar, &profiles)
        .into_iter()
        .filter(|person| !before.contains(&pair(person)))
        .collect();

    if proven.is_empty() {
        return Ok(0);
    }

    let text = match context.corpus.read(FILE).await {
        Ok(text) => text,
        Err(corpus::Error::NoSuchFile(_)) => STARTER.to_string(),
        Err(error) => return Err(error.into()),
    };

    let updated = add_suggestions(&text, &proven);
    let added = parse(&updated).suggested.len() - parse(&text).suggested.len();

    if updated != text {
        context.corpus.write(FILE, &updated).await?;
    }

    let mut remembered = before;
    remembered.extend(proven.iter().map(pair));
    if let Ok(text) = serde_json::to_string(&remembered) {
        settings::set(&context.pool, SUGGESTED_KEY, &text).await?;
    }

    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::Corpus;
    use crate::db::test_support::migrated_pool;
    use crate::facts::NewFact;
    use crate::llama::test_support::serve;
    use crate::{integrations, llama, microsoft};

    fn person(name: &str, handle: &str) -> Person {
        Person {
            name: name.to_string(),
            handle: handle.to_string(),
        }
    }

    #[test]
    fn reads_both_sections_and_ignores_the_users_own_prose() {
        let text = "# People\n\nDana is my skip-level; ask her about budgets - not a list.\n\n\
## Confirmed\n- Dana  Vance = @dvance (GitHub)\n\nNotes I keep here.\n\n\
## Suggested\n- Priya Rao = prao\n\n## Elsewhere\n- Not = a person\n";

        let people = parse(text);

        assert_eq!(people.confirmed, [person("Dana Vance", "dvance")]);
        assert_eq!(people.suggested, [person("Priya Rao", "prao")]);
        assert!(people.unread.is_empty(), "{:?}", people.unread);
    }

    #[test]
    fn reports_a_list_line_it_cannot_read_instead_of_dropping_it() {
        let people = parse("## Confirmed\n- Dana Vance\n- Sam = not a handle!\n- Ok Person = ok\n");

        assert_eq!(people.confirmed, [person("Ok Person", "ok")]);
        assert_eq!(people.unread, ["- Dana Vance", "- Sam = not a handle!"]);
    }

    #[test]
    fn adds_under_suggested_and_leaves_everything_else_alone() {
        let text = "# People\n\nMy note.\n\n## Confirmed\n- Dana Vance = dvance\n\n\
## Suggested\n- Priya Rao = prao\n\n## Later\nUser text.\n";

        let updated = add_suggestions(text, &[person("Sam Lee", "slee")]);

        assert_eq!(
            updated,
            "# People\n\nMy note.\n\n## Confirmed\n- Dana Vance = dvance\n\n\
## Suggested\n- Priya Rao = prao\n- Sam Lee = slee\n\n## Later\nUser text.\n"
        );
    }

    #[test]
    fn adds_a_suggested_section_when_there_is_none() {
        let updated = add_suggestions("# People\nMy own notes.", &[person("Sam Lee", "slee")]);

        assert_eq!(
            updated,
            "# People\nMy own notes.\n\n## Suggested\n- Sam Lee = slee\n"
        );
    }

    #[test]
    fn never_suggests_a_pair_the_file_already_holds_and_does_not_rewrite_for_nothing() {
        let text = "## Confirmed\n- dana vance = DVance\n";

        assert_eq!(
            add_suggestions(text, &[person("Dana Vance", "dvance")]),
            text,
            "a confirmed line is not suggested again, whatever its case"
        );
    }

    #[test]
    fn proves_a_pair_only_from_one_exact_match_on_each_side() {
        let calendar = vec!["Dana Vance".to_string(), "Sam Lee".to_string()];
        let profiles = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(handle, name)| ((*handle).to_string(), (*name).to_string()))
                .collect()
        };

        assert_eq!(
            matches(&calendar, &profiles(&[("dvance", "  dana   VANCE ")])),
            [person("Dana Vance", "dvance")],
            "case and spacing do not matter"
        );
        assert!(
            matches(&calendar, &profiles(&[("dvance", "Dana")])).is_empty(),
            "a first name is not a match"
        );
        assert!(
            matches(
                &calendar,
                &profiles(&[("dvance", "Dana Vance"), ("dana2", "Dana Vance")])
            )
            .is_empty(),
            "two handles claiming one name is a guess"
        );
        assert_eq!(
            matches(
                &["Dana Vance".to_string(), "dana vance".to_string()],
                &profiles(&[("dvance", "Dana Vance")])
            )
            .len(),
            1,
            "one person written two ways is still one"
        );
        assert!(matches(&calendar, &profiles(&[("nobody", "")])).is_empty());
    }

    const PROFILE: &str = r#"{"login":"dvance","name":"Dana Vance"}"#;

    async fn seeded(github_host: &str, name: &str) -> (recipe::Context, std::path::PathBuf) {
        let pool = migrated_pool().await;
        integrations::save(
            &pool,
            integrations::NewAccount {
                service: integrations::GITHUB,
                account_key: "octocat",
                identity: Some("octocat"),
                credential_kind: integrations::OAUTH,
                access_token: "gho_token",
                refresh_token: None,
                expires_at: None,
                scopes: None,
                client_id: None,
                client_secret: None,
            },
        )
        .await
        .expect("account");

        let root = std::env::temp_dir().join(format!("chief-people-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let corpus = Corpus::at(root.clone());
        corpus.ensure_shape().await.expect("shape");

        let row = |about: &str, what: &str, value: &str| NewFact {
            about: about.to_string(),
            what: what.to_string(),
            scope: String::new(),
            value: value.to_string(),
            count: 3,
            total: 4,
            source: "test".to_string(),
            seen_on: "2026-10-27".to_string(),
        };
        facts::recount(
            &pool,
            &[facts::REVIEWS],
            vec![row("dvance", facts::REVIEWS, "")],
        )
        .await
        .expect("seed reviewers");
        facts::recount(
            &pool,
            &[facts::MEETS],
            vec![row("dana vance", facts::MEETS, "Dana Vance")],
        )
        .await
        .expect("seed meetings");

        let context = recipe::Context {
            pool,
            github: crate::github::Client::against(github_host).expect("client"),
            microsoft: microsoft::Client::against("127.0.0.1:1").expect("client"),
            calendar: crate::calendar::Client::new().expect("client"),
            linear: crate::linear::Client::new().expect("client"),
            atlassian: crate::atlassian::Client::new().expect("client"),
            atlassian_rest: crate::atlassian::rest::Rest::new().expect("client"),
            engine: llama::Client::with_base_url("http://127.0.0.1:1").expect("client"),
            corpus,
            short_brief_lines: 0,
        };

        (context, root)
    }

    #[tokio::test]
    async fn suggests_once_and_a_deleted_line_stays_deleted() {
        let (host, github) = serve(vec![("HTTP/1.1 200 OK", PROFILE)]);
        let (context, root) = seeded(&host, "suggest").await;

        assert_eq!(suggest(&context).await.expect("pass"), 1);

        let file = context.corpus.read(FILE).await.expect("file written");
        assert!(
            file.contains("## Suggested\n- Dana Vance = dvance\n"),
            "{file}"
        );
        assert!(
            parse(&file).confirmed.is_empty(),
            "a suggestion is never confirmed for the user"
        );

        // The user deletes the line. The next pass must not bring it back, and
        // must not ask GitHub about the same person again (the stub has one
        // reply and is finished, so a second request would fail the lookup).
        context
            .corpus
            .write(FILE, STARTER)
            .await
            .expect("the user edits");
        assert_eq!(suggest(&context).await.expect("second pass"), 0);
        let after = context.corpus.read(FILE).await.expect("file");
        assert_eq!(after, STARTER, "nothing came back");

        github.await.expect("one lookup, once");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_name_that_does_not_match_writes_no_file() {
        let (host, github) = serve(vec![(
            "HTTP/1.1 200 OK",
            r#"{"login":"dvance","name":"D. Vance"}"#,
        )]);
        let (context, root) = seeded(&host, "nomatch").await;

        assert_eq!(suggest(&context).await.expect("pass"), 0);
        assert!(
            matches!(
                context.corpus.read(FILE).await,
                Err(corpus::Error::NoSuchFile(_))
            ),
            "no proof, no file"
        );

        github.await.expect("lookup");
        let _ = std::fs::remove_dir_all(root);
    }
}
