//! The runbook, as tests.
//!
//! `docs/runbook.md` describes Chief as three people use it — an engineering
//! manager, a working parent and a software engineer — and every scenario in it
//! with an automatic check is a test here, named after its ID (`em_03_…`,
//! `par_01_…`, `eng_02_…`, `any_04_…`). Those are the only IDs; the runbook
//! lists the ones this machine cannot check on its own.
//!
//! **Mock services, not mock code.** Each scenario stands up small HTTP servers
//! that answer the way GitHub, Microsoft Graph and `llama-server` answer, and
//! points the real clients at them. That is the whole of the setup: nothing
//! here replaces a function, so what is tested is the path a request takes in
//! the product — gather, assemble, ask the model once, write a file.
//!
//! The servers answer by **path**, not by position. The older stubs in this
//! repository reply in a fixed order, so every new question a pass asks breaks
//! every test that counts replies. These do not care how many requests arrive
//! or in what order, and they keep everything they were sent so a scenario can
//! assert what the model was told — which is the part of Chief that matters.
//!
//! A server can also go away (`Reply::Offline`), answer with half a message
//! (`Reply::Torn`) or answer slowly, which is how the scenarios for being
//! offline, losing power mid-answer and crossing midnight are written.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, FixedOffset, Local, TimeZone};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::agent::Attention;
use crate::corpus::Corpus;
use crate::db::test_support::migrated_pool;
use crate::intent::{self, Intent, Window};
use crate::{daemon, github, integrations, llama, recipe, sync_state, work_log};

mod bench;

// ---------------------------------------------------------------------------
// The mock services
// ---------------------------------------------------------------------------

/// What a mock service does with one request.
#[derive(Clone)]
enum Reply {
    Json(u16, String),
    /// A server-sent event stream, the way `llama-server` writes one.
    Stream(String),
    /// Nothing at all: the connection closes with no answer, as it does when
    /// the network drops between sending a request and hearing back.
    Offline,
    /// A stream that stops part-way, as it does when the machine loses power
    /// or the engine is killed while it is writing.
    Torn(String),
}

/// One request, with the query string decoded so a test can read it.
#[derive(Clone, Debug)]
struct Seen {
    target: String,
    body: String,
}

struct Mock {
    host: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Mock {
    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().expect("lock").clone()
    }

    fn count(&self) -> usize {
        self.requests().len()
    }

    /// What the model was last asked, as one string.
    fn last_prompt(&self) -> String {
        let body = self
            .requests()
            .last()
            .map(|seen| seen.body.clone())
            .expect("the model should have been asked");
        let body: Value = serde_json::from_str(&body).expect("the request body is JSON");

        body["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|message| message["content"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;

    while at < bytes.len() {
        match bytes[at] {
            b'+' => out.push(b' '),
            b'%' if at + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[at + 1..at + 3]).unwrap_or("00");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
                at += 2;
            }
            byte => out.push(byte),
        }
        at += 1;
    }

    String::from_utf8_lossy(&out).to_string()
}

/// Start a mock service that answers every request with `handler`.
async fn mock<F>(handler: F) -> Mock
where
    F: Fn(&Seen) -> Reply + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let host = format!("http://{}", listener.local_addr().expect("address"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(handler);

    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let log = log.clone();
            let handler = handler.clone();

            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut chunk = [0_u8; 4096];

                loop {
                    let Ok(read) = socket.read(&mut chunk).await else {
                        return;
                    };
                    if read == 0 {
                        return;
                    }
                    request.extend_from_slice(&chunk[..read]);

                    let text = String::from_utf8_lossy(&request).to_string();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let length = head
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);

                        if body.len() >= length {
                            break;
                        }
                    }
                }

                let text = String::from_utf8_lossy(&request).to_string();
                let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
                let target = head
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or_default();

                let seen = Seen {
                    target: decode(target),
                    body: body.to_string(),
                };
                log.lock().expect("lock").push(seen.clone());

                let (status, kind, body, short) = match handler(&seen) {
                    Reply::Json(status, body) => (status, "application/json", body, false),
                    Reply::Stream(body) => (200, "text/event-stream", body, false),
                    Reply::Torn(body) => (200, "text/event-stream", body, true),
                    Reply::Offline => return,
                };

                // A torn reply promises more than it sends and then hangs up.
                let promised = body.len() + if short { 4096 } else { 0 };
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\nContent-Length: {promised}\r\nConnection: close\r\n\r\n{body}"
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            });
        }
    });

    Mock { host, seen }
}

/// A host nothing is listening on: connection refused, which is what being
/// offline looks like from here.
const NOWHERE: &str = "http://127.0.0.1:1";

// ---- GitHub ---------------------------------------------------------------

fn pr(number: i64, repo: &str, title: &str, merged_at: Option<&str>, updated: &str) -> Value {
    json!({
        "number": number,
        "title": title,
        "repository_url": format!("https://api.github.com/repos/{repo}"),
        "state": if merged_at.is_some() { "closed" } else { "open" },
        "draft": false,
        "html_url": format!("https://github.com/{repo}/pull/{number}"),
        "updated_at": updated,
        "pull_request": { "merged_at": merged_at },
    })
}

fn issue(number: i64, repo: &str, title: &str) -> Value {
    json!({
        "number": number,
        "title": title,
        "repository_url": format!("https://api.github.com/repos/{repo}"),
        "html_url": format!("https://github.com/{repo}/issues/{number}"),
        "updated_at": "2026-10-01T09:00:00Z",
    })
}

fn search(items: &[Value]) -> Reply {
    Reply::Json(
        200,
        json!({ "total_count": items.len(), "items": items }).to_string(),
    )
}

/// What GitHub holds for one person. Each field is one of the searches Chief
/// makes; a field left at its default answers "nothing".
#[derive(Clone)]
struct Github {
    mine: Reply,
    merged: Reply,
    reviews: Reply,
    issues: Reply,
}

impl Default for Github {
    fn default() -> Self {
        Self {
            mine: search(&[]),
            merged: search(&[]),
            reviews: search(&[]),
            issues: search(&[]),
        }
    }
}

async fn github_with(held: Github) -> Mock {
    mock(move |seen| {
        let target = &seen.target;

        if !target.contains("/search/issues") {
            return Reply::Json(404, "{}".into());
        }
        if target.contains("is:issue") {
            held.issues.clone()
        } else if target.contains("review-requested:@me") {
            held.reviews.clone()
        } else if target.contains("is:merged") {
            held.merged.clone()
        } else {
            held.mine.clone()
        }
    })
    .await
}

// ---- Microsoft Graph ------------------------------------------------------

/// One meeting. `start` and `end` are local wall-clock stamps, which is what
/// Graph returns.
fn meeting(subject: &str, start: &str, end: &str, with: &[&str]) -> Value {
    json!({
        "subject": subject,
        "start": { "dateTime": format!("{start}.0000000"), "timeZone": "UTC" },
        "end": { "dateTime": format!("{end}.0000000"), "timeZone": "UTC" },
        "organizer": { "emailAddress": { "name": with.first().copied().unwrap_or("Me") } },
        "attendees": with.iter().map(|name| json!({ "emailAddress": { "name": name } })).collect::<Vec<_>>(),
        "isOnlineMeeting": false,
        "isAllDay": false,
    })
}

fn all_day(subject: &str, day: &str) -> Value {
    let mut event = meeting(
        subject,
        &format!("{day}T00:00:00"),
        &format!("{day}T23:59:59"),
        &[],
    );
    event["isAllDay"] = json!(true);
    event
}

fn mail(from: &str, subject: &str, unread: bool) -> Value {
    json!({
        "subject": subject,
        "from": { "emailAddress": { "name": from } },
        "receivedDateTime": "2026-10-02T06:00:00Z",
        "bodyPreview": "…",
        "isRead": !unread,
    })
}

/// Microsoft Graph, holding `events` and `inbox`. Like the real service it
/// returns only the events inside the window it was asked for.
async fn graph_with(events: Vec<Value>, inbox: Vec<Value>) -> Mock {
    mock(move |seen| {
        let target = &seen.target;

        if target.contains("calendarView") {
            let param = |name: &str| {
                target
                    .split(&format!("{name}="))
                    .nth(1)
                    .and_then(|rest| rest.split('&').next())
                    .unwrap_or_default()
                    .to_string()
            };
            let (from, to) = (param("startDateTime"), param("endDateTime"));

            // Like the real service: in start order, and no more than `$top`.
            let top: usize = param("$top").parse().unwrap_or(usize::MAX);
            let mut inside: Vec<&Value> = events
                .iter()
                .filter(|event| {
                    let start = event["start"]["dateTime"].as_str().unwrap_or_default();
                    start >= from.as_str() && start < to.as_str()
                })
                .collect();
            inside.sort_by_key(|event| {
                event["start"]["dateTime"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            });
            inside.truncate(top);

            Reply::Json(200, json!({ "value": inside }).to_string())
        } else if target.contains("/me/messages") {
            Reply::Json(200, json!({ "value": inbox }).to_string())
        } else {
            Reply::Json(
                200,
                json!({ "userPrincipalName": "me@example.com" }).to_string(),
            )
        }
    })
    .await
}

// ---- The model ------------------------------------------------------------

fn sse(content: &str) -> String {
    llama::test_support::events(&[llama::test_support::delta(content)])
}

/// A model that always answers with `text`, however it is asked.
async fn model_saying(text: &'static str) -> Mock {
    mock(move |seen| {
        if seen.body.contains("\"stream\":true") {
            Reply::Stream(sse(text))
        } else {
            Reply::Json(200, llama::test_support::answer(text))
        }
    })
    .await
}

// ---------------------------------------------------------------------------
// The world a scenario runs in
// ---------------------------------------------------------------------------

struct World {
    ctx: recipe::Context,
    root: PathBuf,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

static WORLDS: AtomicUsize = AtomicUsize::new(0);

async fn connect(pool: &sqlx::SqlitePool, service: &str, key: &str) -> i64 {
    integrations::save(
        pool,
        integrations::NewAccount {
            service,
            account_key: key,
            identity: Some(key),
            credential_kind: integrations::OAUTH,
            access_token: "token",
            refresh_token: None,
            expires_at: None,
            scopes: None,
            client_id: None,
            client_secret: None,
        },
    )
    .await
    .expect("should store a credential")
    .id
}

/// Everything a scenario needs: a database, a corpus folder, and clients aimed
/// at whichever services it stood up. A service left as `None` is not
/// connected at all.
async fn world(github: Option<&str>, graph: Option<&str>, engine: &str) -> World {
    let pool = migrated_pool().await;
    let root = std::env::temp_dir().join(format!(
        "chief-scenario-{}-{}",
        std::process::id(),
        WORLDS.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&root);

    let corpus = Corpus::at(root.clone());
    corpus.ensure_shape().await.expect("corpus");

    if github.is_some() {
        connect(&pool, integrations::GITHUB, "me").await;
    }
    if graph.is_some() {
        connect(&pool, integrations::MICROSOFT, "me@example.com").await;
    }

    World {
        ctx: recipe::Context {
            pool,
            github: github::Client::against(github.unwrap_or(NOWHERE)).expect("client"),
            microsoft: crate::microsoft::Client::against(graph.unwrap_or(NOWHERE)).expect("client"),
            calendar: crate::calendar::Client::new().expect("client"),
            linear: crate::linear::Client::new().expect("client"),
            atlassian: crate::atlassian::Client::new().expect("client"),
            atlassian_rest: crate::atlassian::rest::Rest::new().expect("client"),
            engine: llama::Client::with_base_url(engine).expect("client"),
            corpus,
        },
        root,
    }
}

impl World {
    fn daemon(&self) -> daemon::Context {
        daemon::Context {
            pool: self.ctx.pool.clone(),
            github: self.ctx.github.clone(),
            attention: Attention::default(),
        }
    }
}

/// Today's date as the calendar writes it, so a meeting stamped with it falls
/// inside "today" whenever the suite runs.
fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

fn at(day: &str, time: &str) -> String {
    format!("{day}T{time}:00")
}

/// Today's brief, written at six in the morning whatever the machine's clock
/// says. A brief is a window onto what has not happened yet, so a scenario
/// that put a meeting at 07:45 and ran at 08:30 asked for an afternoon brief
/// and failed — which is how the CI runner found this.
async fn dawn_brief(ctx: &recipe::Context) -> Result<recipe::Brief, recipe::Error> {
    recipe::daily_brief_at(ctx, local(&format!("{}T06:00:00", today()))).await
}

fn local(stamp: &str) -> DateTime<Local> {
    let naive = chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%dT%H:%M:%S").expect("stamp");

    Local
        .from_local_datetime(&naive)
        .single()
        .expect("an unambiguous local time")
}

// ---------------------------------------------------------------------------
// Engineering manager: back to back, and needs to be right about the next one
// ---------------------------------------------------------------------------

mod em {
    use super::*;

    pub(super) type Subjects = [(&'static str, &'static str, &'static [&'static str]); 14];

    /// Fourteen meetings, 08:30 to 16:30, on a graph the brief reads.
    pub(super) async fn full_day() -> (Subjects, World, Mock) {
        let day = today();
        let subjects: Subjects = [
            ("08:30", "Team standup", &["Priya", "Tom", "Ana"]),
            ("09:00", "1:1 with Priya", &["Priya"]),
            ("09:30", "1:1 with Tom", &["Tom"]),
            ("10:00", "Sprint planning", &["Team"]),
            ("11:00", "Incident review", &["SRE"]),
            ("11:30", "Hiring sync", &["Recruiter"]),
            ("12:00", "Skip-level with Ana", &["Ana"]),
            ("13:00", "Roadmap review", &["Product"]),
            ("13:30", "Architecture council", &["Staff"]),
            ("14:00", "Vendor call", &["Vendor"]),
            ("14:30", "Budget check-in", &["Finance"]),
            ("15:00", "Design critique", &["Design"]),
            ("15:30", "1:1 with Sam", &["Sam"]),
            ("16:30", "Performance calibration", &["HR"]),
        ];
        let events = subjects
            .iter()
            .map(|(start, subject, with)| {
                meeting(subject, &at(&day, start), &at(&day, start), with)
            })
            .collect();

        let graph = graph_with(events, vec![]).await;
        let engine = model_saying("- ok").await;
        let world = world(None, Some(&graph.host), &engine.host).await;

        (subjects, world, engine)
    }

    /// EM-01. A manager's day is mostly meetings, and the brief is a window
    /// onto it. At 08:00 the model is told about the next three, in the order
    /// they happen, and the brief says in words that eleven more are coming.
    /// It used to be told about all fourteen and wrote fourteen bullets.
    #[tokio::test]
    async fn em_01_a_full_day_of_meetings_reaches_the_brief_three_at_a_time() {
        let (subjects, world, engine) = full_day().await;

        let brief = recipe::daily_brief_at(&world.ctx, local(&format!("{}T08:00:00", today())))
            .await
            .expect("a brief");
        let prompt = engine.last_prompt();

        for (_, subject, _) in &subjects[..3] {
            assert!(prompt.contains(subject), "{subject:?} is next:\n{prompt}");
        }
        for (_, subject, _) in &subjects[3..] {
            assert!(
                !prompt.contains(subject),
                "{subject:?} is beyond the window and would crowd out the reviews:\n{prompt}"
            );
        }

        let first = prompt.find("Team standup").expect("first meeting");
        let third = prompt.find("1:1 with Tom").expect("third meeting");
        assert!(first < third, "meetings must stay in the order they happen");

        assert!(
            brief
                .markdown
                .contains("Showing the next 3 of 14 meetings still to come."),
            "the brief says what it left out, in the code's own words:\n{}",
            brief.markdown
        );
    }

    /// EM-13. As the clock moves the window moves with it: at 14:10 the
    /// morning has gone and the afternoon is what the brief is about. The
    /// daemon asks `refresh_due` and writes the brief again.
    #[tokio::test]
    async fn em_13_the_window_moves_as_the_day_goes() {
        let (subjects, world, engine) = full_day().await;
        let day = today();

        recipe::daily_brief_at(&world.ctx, local(&format!("{day}T08:00:00")))
            .await
            .expect("the morning brief");

        let pool = &world.ctx.pool;
        let morning = local(&format!("{day}T08:00:00"));
        assert!(
            !recipe::refresh_due(pool, &day, &morning)
                .await
                .expect("read"),
            "nothing has moved yet"
        );

        // 09:30 is when the third meeting in the window starts.
        let due = local(&format!("{day}T09:30:00"));
        assert!(
            recipe::refresh_due(pool, &day, &due).await.expect("read"),
            "the window has moved and the brief should be written again"
        );

        let brief = recipe::daily_brief_at(&world.ctx, local(&format!("{day}T14:10:00")))
            .await
            .expect("the afternoon brief");
        let prompt = engine.last_prompt();

        assert!(prompt.contains("Vendor call"), "{prompt}");
        assert!(prompt.contains("Budget check-in"), "{prompt}");
        assert!(prompt.contains("Design critique"), "{prompt}");
        assert!(
            !prompt.contains("Team standup") && !prompt.contains("Sprint planning"),
            "the morning has gone:\n{prompt}"
        );
        assert!(
            brief.markdown.contains("Showing the next 3 of 5"),
            "{}",
            brief.markdown
        );
        let _ = subjects;
    }

    /// EM-14. When the last meeting has finished there is nothing to ask a
    /// model about, so it is not asked.
    #[tokio::test]
    async fn em_14_a_day_that_has_finished_costs_no_model_call() {
        let (_, world, engine) = full_day().await;
        let day = today();

        let brief = recipe::daily_brief_at(&world.ctx, local(&format!("{day}T19:00:00")))
            .await
            .expect("an evening brief");

        assert_eq!(engine.count(), 0, "the model was asked about an empty day");
        assert!(brief.markdown.contains("Nothing else is scheduled"));
        assert!(
            !recipe::refresh_due(&world.ctx.pool, &day, &local(&format!("{day}T23:00:00")))
                .await
                .expect("read"),
            "a finished day has nothing to refresh for"
        );
    }

    /// EM-02. A review somebody asked for and a pull request the manager opened
    /// are different things, and a brief that mixes them tells the manager
    /// they are blocking people when they are not.
    #[tokio::test]
    async fn em_02_reviews_asked_of_me_are_not_my_own_pull_requests() {
        let github = github_with(Github {
            reviews: search(&[
                pr(
                    41,
                    "acme/api",
                    "Retry payment webhooks",
                    None,
                    "2026-10-02T07:00:00Z",
                ),
                pr(
                    42,
                    "acme/api",
                    "Drop the legacy flag",
                    None,
                    "2026-10-02T07:05:00Z",
                ),
            ]),
            mine: search(&[pr(
                7,
                "acme/handbook",
                "Update on-call policy",
                None,
                "2026-10-01T10:00:00Z",
            )]),
            ..Github::default()
        })
        .await;
        let engine = model_saying("- ok").await;
        let world = world(Some(&github.host), None, &engine.host).await;

        let brief = dawn_brief(&world.ctx).await.expect("a brief");
        let prompt = engine.last_prompt();

        let waiting = prompt.find("Waiting on you").expect("waiting heading");
        let own = prompt.find("Your open pull requests").expect("own heading");
        let retry = prompt.find("Retry payment webhooks").expect("review");
        let policy = prompt.find("Update on-call policy").expect("own pr");

        assert!(
            waiting < retry && retry < own,
            "the review is under 'waiting on you'"
        );
        assert!(
            own < policy,
            "the manager's own pull request is under their own heading"
        );
        assert_eq!(brief.sources, ["review requests", "pull requests"]);
    }

    /// EM-03. A 10:00 that moved to 14:00 is one meeting, at 14:00. Chief used
    /// to key a meeting on its start time, so the move left the 10:00 behind
    /// and "what is on my calendar" listed both.
    #[tokio::test]
    async fn em_03_a_moved_meeting_is_listed_once_at_its_new_time() {
        let day = today();
        let before = graph_with(
            vec![meeting(
                "1:1 with Priya",
                &at(&day, "10:00"),
                &at(&day, "10:30"),
                &["Priya"],
            )],
            vec![],
        )
        .await;
        let engine = model_saying("- ok").await;
        let mut world = world(None, Some(&before.host), &engine.host).await;

        daemon::ingest_calendar(&world.ctx).await;

        // Priya asks to move it. The next pass sees the meeting at 14:00.
        let after = graph_with(
            vec![meeting(
                "1:1 with Priya",
                &at(&day, "14:00"),
                &at(&day, "14:30"),
                &["Priya"],
            )],
            vec![],
        )
        .await;
        world.ctx.microsoft = crate::microsoft::Client::against(&after.host).expect("client");
        daemon::ingest_calendar(&world.ctx).await;

        let answer = intent::answer(Intent::Prep, &world.ctx)
            .await
            .expect("an answer");

        assert!(answer.markdown.contains("14:00"), "{}", answer.markdown);
        assert!(
            !answer.markdown.contains("10:00"),
            "the old time is still being offered as a meeting:\n{}",
            answer.markdown
        );
    }

    /// EM-04. A cancelled meeting stops being on the calendar. Showing it as
    /// something to prepare for sends a manager to an empty room.
    #[tokio::test]
    async fn em_04_a_cancelled_meeting_disappears_from_the_calendar() {
        let day = today();
        let both = graph_with(
            vec![
                meeting(
                    "Roadmap review",
                    &at(&day, "11:00"),
                    &at(&day, "12:00"),
                    &["Product"],
                ),
                meeting(
                    "Hiring sync",
                    &at(&day, "15:00"),
                    &at(&day, "15:30"),
                    &["Recruiter"],
                ),
            ],
            vec![],
        )
        .await;
        let engine = model_saying("- ok").await;
        let mut world = world(None, Some(&both.host), &engine.host).await;

        daemon::ingest_calendar(&world.ctx).await;

        let one = graph_with(
            vec![meeting(
                "Hiring sync",
                &at(&day, "15:00"),
                &at(&day, "15:30"),
                &["Recruiter"],
            )],
            vec![],
        )
        .await;
        world.ctx.microsoft = crate::microsoft::Client::against(&one.host).expect("client");
        daemon::ingest_calendar(&world.ctx).await;

        let answer = intent::answer(Intent::Prep, &world.ctx)
            .await
            .expect("an answer");

        assert!(
            answer.markdown.contains("Hiring sync"),
            "{}",
            answer.markdown
        );
        assert!(
            !answer.markdown.contains("Roadmap review"),
            "a cancelled meeting is still on the calendar:\n{}",
            answer.markdown
        );
    }

    /// EM-05. Between two meetings, on a train, with no signal: the calendar
    /// question still answers, from what the last pass saved, and says when.
    #[tokio::test]
    async fn em_05_the_calendar_answers_with_no_network_at_all() {
        let day = today();
        let graph = graph_with(
            vec![meeting(
                "Sprint planning",
                &at(&day, "10:00"),
                &at(&day, "11:00"),
                &["Team"],
            )],
            vec![],
        )
        .await;
        let engine = model_saying("- ok").await;
        let mut world = world(None, Some(&graph.host), &engine.host).await;

        daemon::ingest_calendar(&world.ctx).await;
        let asked_before = graph.count();

        // The network goes away.
        world.ctx.microsoft = crate::microsoft::Client::against(NOWHERE).expect("client");

        let answer = intent::answer(Intent::Prep, &world.ctx)
            .await
            .expect("an answer");

        assert!(
            answer.markdown.contains("Sprint planning"),
            "{}",
            answer.markdown
        );
        assert!(
            answer.provenance.is_some(),
            "an old answer says how old it is"
        );
        assert_eq!(
            graph.count(),
            asked_before,
            "a read of the log must not call the network"
        );
        assert_eq!(engine.count(), 0, "and must not wake the model");
    }

    /// EM-06. The Outlook sign-in expired overnight. The brief still has the
    /// pull requests, and the morning is not lost to one expired token.
    #[tokio::test]
    async fn em_06_an_expired_outlook_sign_in_does_not_cost_the_brief() {
        let github = github_with(Github {
            reviews: search(&[pr(
                41,
                "acme/api",
                "Retry payment webhooks",
                None,
                "2026-10-02T07:00:00Z",
            )]),
            ..Github::default()
        })
        .await;
        let graph = mock(|_| {
            Reply::Json(
                401,
                r#"{"error":{"code":"InvalidAuthenticationToken"}}"#.into(),
            )
        })
        .await;
        let engine = model_saying("- review #41").await;
        let world = world(Some(&github.host), Some(&graph.host), &engine.host).await;

        let brief = dawn_brief(&world.ctx).await.expect("a brief");

        assert!(engine.last_prompt().contains("Retry payment webhooks"));
        assert_eq!(brief.sources, ["review requests"]);
    }

    /// EM-07. Only unread mail is something to act on, and only the unread
    /// reaches the model.
    #[tokio::test]
    async fn em_07_only_unread_mail_is_in_the_brief() {
        let graph = graph_with(
            vec![],
            vec![
                mail("Priya Nair", "Can we move our 1:1?", true),
                mail("Newsletter", "Already read this", false),
            ],
        )
        .await;
        let engine = model_saying("- ok").await;
        let world = world(None, Some(&graph.host), &engine.host).await;

        dawn_brief(&world.ctx).await.expect("a brief");
        let prompt = engine.last_prompt();

        assert!(prompt.contains("Can we move our 1:1?"));
        assert!(!prompt.contains("Already read this"));
    }

    /// EM-08. "What is on my calendar" after a day of back-to-back meetings
    /// lists every one of them — it used to stop at ten.
    #[tokio::test]
    async fn em_08_the_calendar_question_lists_the_whole_busy_day() {
        let day = today();
        let events: Vec<Value> = (0..16)
            .map(|n| {
                meeting(
                    &format!("Meeting {n:02}"),
                    &at(&day, &format!("{:02}:{:02}", 8 + n / 2, (n % 2) * 30)),
                    &at(&day, "23:00"),
                    &["Priya"],
                )
            })
            .collect();
        let graph = graph_with(events, vec![]).await;
        let engine = model_saying("- ok").await;
        let world = world(None, Some(&graph.host), &engine.host).await;

        daemon::ingest_calendar(&world.ctx).await;
        let answer = intent::answer(Intent::Prep, &world.ctx)
            .await
            .expect("an answer");

        for n in 0..16 {
            assert!(
                answer.markdown.contains(&format!("Meeting {n:02}")),
                "Meeting {n:02} is missing:\n{}",
                answer.markdown
            );
        }
    }

    /// EM-09. A calendar that will not answer says nothing about the day. The
    /// meetings saved from the last pass stay; an outage is not a cancellation.
    #[tokio::test]
    async fn em_09_an_unreachable_calendar_does_not_clear_the_saved_day() {
        let day = today();
        let graph = graph_with(
            vec![meeting(
                "Sprint planning",
                &at(&day, "10:00"),
                &at(&day, "11:00"),
                &["Team"],
            )],
            vec![],
        )
        .await;
        let engine = model_saying("- ok").await;
        let mut world = world(None, Some(&graph.host), &engine.host).await;

        daemon::ingest_calendar(&world.ctx).await;

        for down in [
            NOWHERE.to_string(),
            mock(|_| Reply::Offline).await.host,
            mock(|_| Reply::Json(503, "{}".into())).await.host,
        ] {
            world.ctx.microsoft = crate::microsoft::Client::against(&down).expect("client");
            daemon::ingest_calendar(&world.ctx).await;

            let answer = intent::answer(Intent::Prep, &world.ctx)
                .await
                .expect("an answer");
            assert!(
                answer.markdown.contains("Sprint planning"),
                "{down}: {}",
                answer.markdown
            );
        }
    }

    /// EM-10. A meeting the manager typed into the log by hand is theirs, and
    /// a pass that clears cancelled meetings never touches it.
    #[tokio::test]
    async fn em_10_clearing_cancelled_meetings_leaves_hand_written_notes_alone() {
        let day = today();
        let graph = graph_with(vec![], vec![]).await;
        let engine = model_saying("- ok").await;
        let world = world(None, Some(&graph.host), &engine.host).await;

        work_log::insert(
            &world.ctx.pool,
            work_log::NewWorkLogEntry {
                source: "calendar".to_string(),
                content: "Coffee with the new VP, 8:00 — not in anyone's calendar".to_string(),
                summary: None,
                timestamp: Some(format!("{day}T08:00:00Z")),
                account_id: None,
                external_id: None,
            },
        )
        .await
        .expect("a note");

        daemon::ingest_calendar(&world.ctx).await;

        let rows = work_log::fetch(&world.ctx.pool, None).await.expect("rows");
        assert_eq!(
            rows.len(),
            1,
            "the note was swept away with the cancelled meetings"
        );
    }
}

// ---------------------------------------------------------------------------
// Working parent: school logistics, a partner, and a full-time job
// ---------------------------------------------------------------------------

mod par {
    use super::*;

    /// PAR-01. A family calendar and nothing from work. The brief is about
    /// the school run, and does not talk about pull requests nobody has.
    #[tokio::test]
    async fn par_01_a_family_calendar_alone_makes_a_family_brief() {
        let day = today();
        let graph = graph_with(
            vec![
                meeting(
                    "Drop Maya at breakfast club",
                    &at(&day, "07:45"),
                    &at(&day, "08:00"),
                    &[],
                ),
                meeting(
                    "Parents' evening — Year 3",
                    &at(&day, "17:30"),
                    &at(&day, "18:15"),
                    &["Mrs Okafor"],
                ),
                meeting("Dentist — Leo", &at(&day, "16:15"), &at(&day, "16:45"), &[]),
            ],
            vec![mail("Oakfield Primary", "Non-uniform day on Friday", true)],
        )
        .await;
        let engine = model_saying("- ok").await;
        let world = world(None, Some(&graph.host), &engine.host).await;

        let brief = dawn_brief(&world.ctx).await.expect("a brief");
        let prompt = engine.last_prompt();

        assert!(prompt.contains("breakfast club") && prompt.contains("Dentist"));
        assert!(prompt.contains("Non-uniform day"));
        assert!(!prompt.contains("Your open pull requests") && !prompt.contains("Waiting on you"));
        assert_eq!(brief.sources, ["calendar", "inbox"]);

        // And in the order they happen, though the calendar listed the dentist
        // before the parents' evening.
        let dentist = prompt.find("Dentist").expect("dentist");
        let evening = prompt.find("Parents' evening").expect("evening");
        assert!(dentist < evening);
    }

    /// PAR-02. A day-long event — school closed, a bank holiday, a partner
    /// away — is a fact about the day, not a meeting at midnight.
    #[tokio::test]
    async fn par_02_an_all_day_event_is_not_a_meeting_at_midnight() {
        let day = today();
        let graph = graph_with(vec![all_day("INSET day — school closed", &day)], vec![]).await;
        let engine = model_saying("- ok").await;
        let world = world(None, Some(&graph.host), &engine.host).await;

        dawn_brief(&world.ctx).await.expect("a brief");
        let prompt = engine.last_prompt();

        assert!(prompt.contains("INSET day"), "{prompt}");
        assert!(
            !prompt.contains("00:00 INSET"),
            "the model will say the school closes at midnight:\n{prompt}"
        );
    }

    /// PAR-03. Fresh install, nothing connected. Chief says so, in words that
    /// say what to do, rather than inventing a day.
    #[tokio::test]
    async fn par_03_nothing_connected_says_so_instead_of_inventing_a_day() {
        let engine = model_saying("- an invented meeting").await;
        let world = world(None, None, &engine.host).await;

        let refused = dawn_brief(&world.ctx).await.expect_err("no brief");
        assert!(
            refused.to_string().contains("nothing is connected"),
            "{refused}"
        );
        assert_eq!(engine.count(), 0, "no model call for an empty page");

        let prep = intent::answer(Intent::Prep, &world.ctx)
            .await
            .expect("an answer");
        assert!(
            prep.markdown.contains("No calendar is connected"),
            "{}",
            prep.markdown
        );
        assert!(prep.markdown.contains("Settings"));
    }

    /// PAR-04. Names, apostrophes, emoji and accents are how families write
    /// their calendars. None may be mangled, and a very full week of them must
    /// not break the prompt budget — which cuts text, and so can cut it in the
    /// middle of a character.
    #[tokio::test]
    async fn par_04_emoji_and_accents_survive_and_a_flood_of_them_cannot_panic() {
        let day = today();
        let graph = graph_with(
            vec![meeting(
                "🎒 Réunion Maïté & Zoë’s café ☕",
                &at(&day, "08:15"),
                &at(&day, "08:30"),
                &["Zoë"],
            )],
            vec![],
        )
        .await;
        let engine = model_saying("- ok").await;
        let world = world(None, Some(&graph.host), &engine.host).await;

        dawn_brief(&world.ctx).await.expect("a brief");
        assert!(engine
            .last_prompt()
            .contains("🎒 Réunion Maïté & Zoë’s café ☕"));

        // Now so many that the budget has to cut: every line is multi-byte.
        let flood: Vec<Value> = (0..400)
            .map(|n| {
                meeting(
                    &format!("🎒🎒🎒 École №{n} — réunion"),
                    &at(&day, &format!("{:02}:{:02}", 6 + n % 14, n % 60)),
                    &at(&day, "23:00"),
                    &["Zoë"],
                )
            })
            .collect();
        let graph = graph_with(flood, vec![]).await;
        let engine = model_saying("- ok").await;
        let world = super::world(None, Some(&graph.host), &engine.host).await;

        dawn_brief(&world.ctx)
            .await
            .expect("a brief, whatever the flood");
    }

    /// PAR-05. A partner's calendar subscribed beside the user's own: the same
    /// pickup, on two calendars, is two entries and not one overwriting the
    /// other — it is what the account in the dedupe key is for.
    #[tokio::test]
    async fn par_05_two_calendars_holding_the_same_pickup_keep_both() {
        let day = today();
        let event = meeting("School pickup", &at(&day, "15:15"), &at(&day, "15:30"), &[]);
        let graph = graph_with(vec![event], vec![]).await;
        let engine = model_saying("- ok").await;
        let world = world(None, Some(&graph.host), &engine.host).await;

        // A second account on the same machine, reading the same service.
        connect(
            &world.ctx.pool,
            integrations::MICROSOFT,
            "partner@example.com",
        )
        .await;

        let written = daemon::ingest_calendar(&world.ctx).await;

        assert_eq!(written, 2, "one row per account");
    }
}

// ---------------------------------------------------------------------------
// Software engineer: ship it, and say what shipped
// ---------------------------------------------------------------------------

mod eng {
    use super::*;

    /// ENG-01 and ENG-09. The morning stand-up, any day of the week: what merged since yesterday is in the
    /// material the model drafts it from, by name, and nothing is invented.
    #[tokio::test]
    async fn eng_01_standup_material_names_what_merged() {
        let now = Local::now();
        let yesterday = (now - Duration::hours(20)).to_rfc3339();
        let github = github_with(Github {
            merged: search(&[
                pr(
                    88,
                    "acme/api",
                    "Cache the entitlement lookup",
                    Some(&yesterday),
                    &yesterday,
                ),
                pr(
                    87,
                    "acme/web",
                    "Fix focus trap in the modal",
                    Some(&yesterday),
                    &yesterday,
                ),
            ]),
            ..Github::default()
        })
        .await;
        let engine = model_saying("- ok").await;
        let world = world(Some(&github.host), None, &engine.host).await;

        let outcome = daemon::run_once(&world.daemon()).await.expect("a pass");
        assert_eq!(outcome.written, 2);

        let material = intent::material(intent::Ground::Standup, &world.ctx.pool).await;

        assert!(
            material.contains("Cache the entitlement lookup"),
            "{material}"
        );
        assert!(
            material.contains("Fix focus trap in the modal"),
            "{material}"
        );
        assert_eq!(engine.count(), 0, "gathering the facts is not a model call");
    }

    /// ENG-02. Issues assigned on GitHub are the engineer's own queue. They
    /// were fetched and then thrown away, so the brief never mentioned them.
    #[tokio::test]
    async fn eng_02_issues_assigned_on_github_reach_the_brief() {
        let github = github_with(Github {
            issues: search(&[issue(310, "acme/api", "Timeouts on bulk export")]),
            ..Github::default()
        })
        .await;
        let engine = model_saying("- ok").await;
        let world = world(Some(&github.host), None, &engine.host).await;

        let brief = dawn_brief(&world.ctx).await.expect("a brief");

        assert!(
            engine.last_prompt().contains("Timeouts on bulk export"),
            "an assigned issue was fetched and then dropped:\n{}",
            engine.last_prompt()
        );
        assert_eq!(
            brief.sources,
            ["GitHub issues"],
            "and it is not credited to Linear"
        );
    }

    /// ENG-03. GitHub rate-limits one search. The other searches still land:
    /// one bucket is lost, not the brief.
    #[tokio::test]
    async fn eng_03_one_rate_limited_search_costs_one_bucket() {
        let github = github_with(Github {
            reviews: Reply::Json(403, r#"{"message":"API rate limit exceeded"}"#.into()),
            mine: search(&[pr(
                90,
                "acme/api",
                "Paginate the audit log",
                None,
                "2026-10-02T08:00:00Z",
            )]),
            ..Github::default()
        })
        .await;
        let engine = model_saying("- ok").await;
        let world = world(Some(&github.host), None, &engine.host).await;

        let brief = dawn_brief(&world.ctx).await.expect("a brief");

        assert!(engine.last_prompt().contains("Paginate the audit log"));
        assert_eq!(brief.sources, ["pull requests"]);
    }

    /// ENG-04. A pass logs a merge once however many times it runs, and a pull
    /// request that was open and is now merged is revised, not duplicated.
    #[tokio::test]
    async fn eng_04_an_open_pull_request_that_merges_is_one_row_that_changes() {
        let open = github_with(Github {
            mine: search(&[pr(
                95,
                "acme/api",
                "Add the export endpoint",
                None,
                "2026-10-02T08:00:00Z",
            )]),
            ..Github::default()
        })
        .await;
        let engine = model_saying("- ok").await;
        let mut world = world(Some(&open.host), None, &engine.host).await;

        daemon::run_once(&world.daemon()).await.expect("first pass");

        let merged = github_with(Github {
            merged: search(&[pr(
                95,
                "acme/api",
                "Add the export endpoint",
                Some("2026-10-02T09:30:00Z"),
                "2026-10-02T09:30:00Z",
            )]),
            ..Github::default()
        })
        .await;
        world.ctx.github = github::Client::against(&merged.host).expect("client");
        daemon::run_once(&world.daemon())
            .await
            .expect("second pass");
        daemon::run_once(&world.daemon()).await.expect("third pass");

        let rows = work_log::fetch(&world.ctx.pool, None).await.expect("rows");
        let ninety_five: Vec<_> = rows
            .iter()
            .filter(|row| row.title.contains("#95"))
            .collect();

        assert_eq!(ninety_five.len(), 1, "{ninety_five:?}");
        assert_eq!(ninety_five[0].summary.as_deref(), Some("merged"));
    }

    /// ENG-05. "What did I ship today?" and "this week?" are answered from the
    /// log with no model, and a merge that happened just before midnight local
    /// time belongs to the day it happened in.
    #[tokio::test]
    async fn eng_05_what_did_i_ship_is_answered_from_the_log_without_the_model() {
        let now = Local::now();
        let merged_at = (now - Duration::minutes(5)).to_rfc3339();
        let github = github_with(Github {
            merged: search(&[pr(
                101,
                "acme/api",
                "Ship the thing",
                Some(&merged_at),
                &merged_at,
            )]),
            ..Github::default()
        })
        .await;
        let engine = model_saying("- ok").await;
        let world = world(Some(&github.host), None, &engine.host).await;

        daemon::run_once(&world.daemon()).await.expect("a pass");

        for window in [Window::Recent, Window::ThisWeek] {
            let answer = intent::answer(Intent::Log(window), &world.ctx)
                .await
                .expect("answer");
            assert!(
                answer.markdown.contains("Ship the thing"),
                "{window:?}: {}",
                answer.markdown
            );
        }
        assert_eq!(engine.count(), 0);
    }

    /// ENG-06. In the chat, a question that needs a live GitHub read is asked
    /// with no network. The model is handed a plain error to explain, rather
    /// than the conversation collapsing, and the error carries no credential.
    #[tokio::test]
    async fn eng_06_a_live_read_with_no_network_gives_the_model_an_error_to_explain() {
        use crate::llama::{ToolCall, ToolCallFunction};

        let engine = model_saying("- ok").await;
        let world = world(Some(NOWHERE), None, &engine.host).await;
        let context = crate::tools::Context {
            pool: world.ctx.pool.clone(),
            github: world.ctx.github.clone(),
            microsoft: world.ctx.microsoft.clone(),
        };
        let call = ToolCall::new(
            "call_1",
            ToolCallFunction {
                name: "fetch_github_prs".to_string(),
                arguments: "{}".to_string(),
            },
        );

        let result = crate::tools::dispatch(&context, &call).await;

        let error = result["error"]
            .as_str()
            .expect("an error payload, not a crash");
        assert!(error.to_lowercase().contains("reach"), "{error}");
        assert!(
            !error.contains("token"),
            "an error must never carry a credential: {error}"
        );
    }
}

// ---------------------------------------------------------------------------
// Anyone: the edges of a day, the network, the machine, the model
// ---------------------------------------------------------------------------

mod any {
    use super::*;

    /// ANY-01. A brief started at 23:59 is the brief for that day, even if the
    /// model takes until after midnight to write it. It used to be filed under
    /// the next day, which then counted as already briefed.
    #[tokio::test]
    async fn any_01_a_brief_started_before_midnight_is_filed_under_that_day() {
        let day = "2026-12-31";
        let graph = graph_with(
            vec![
                meeting(
                    "New Year's Eve dinner",
                    &at(day, "23:45"),
                    &at(day, "23:59"),
                    &[],
                ),
                meeting(
                    "First day back planning",
                    &at("2027-01-01", "09:00"),
                    &at("2027-01-01", "10:00"),
                    &[],
                ),
            ],
            vec![],
        )
        .await;
        let engine = model_saying("- dinner at 23:45").await;
        let world = world(None, Some(&graph.host), &engine.host).await;

        let started = local("2026-12-31T23:59:50");
        let brief = recipe::daily_brief_at(&world.ctx, started)
            .await
            .expect("a brief");

        assert_eq!(brief.path, "briefs/2026-12-31.md");
        assert!(world.ctx.corpus.read("briefs/2026-12-31.md").await.is_ok());
        assert!(
            world.ctx.corpus.read("briefs/2027-01-01.md").await.is_err(),
            "tomorrow must not start the day already briefed"
        );

        let prompt = engine.last_prompt();
        assert!(prompt.contains("New Year's Eve dinner"));
        assert!(
            !prompt.contains("First day back planning"),
            "tomorrow's meeting leaked into today's brief"
        );
        assert!(
            prompt.contains("23:59 on Thursday 31 December 2026"),
            "the model is told when 'now' is"
        );
    }

    /// ANY-02. Two minutes later the same machine, at 00:01, briefs the new
    /// day with the new day's meetings and none of yesterday's.
    #[tokio::test]
    async fn any_02_the_brief_a_minute_after_midnight_is_the_new_days() {
        let graph = graph_with(
            vec![
                meeting(
                    "Late handover call",
                    &at("2026-12-31", "23:30"),
                    &at("2026-12-31", "23:45"),
                    &[],
                ),
                meeting(
                    "First day back planning",
                    &at("2027-01-01", "09:00"),
                    &at("2027-01-01", "10:00"),
                    &[],
                ),
            ],
            vec![],
        )
        .await;
        let engine = model_saying("- ok").await;
        let world = world(None, Some(&graph.host), &engine.host).await;

        let brief = recipe::daily_brief_at(&world.ctx, local("2027-01-01T00:01:00"))
            .await
            .expect("a brief");

        assert_eq!(brief.path, "briefs/2027-01-01.md");
        let prompt = engine.last_prompt();
        assert!(prompt.contains("First day back planning"));
        assert!(
            !prompt.contains("Late handover call"),
            "yesterday's meeting is not today's"
        );
    }

    /// ANY-03. "This week" turns over on Monday at midnight, not on Sunday
    /// night and not on Monday afternoon; "last week" turns with it.
    #[test]
    fn any_03_the_week_turns_over_exactly_at_midnight_on_monday() {
        let zone = FixedOffset::east_opt(0).expect("zone");
        let stamp = |text: &str| {
            zone.from_local_datetime(
                &chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S").expect("stamp"),
            )
            .single()
            .expect("time")
        };

        // Sunday 4 October 2026, 23:59:59: still the week of Monday 28 September.
        let sunday = stamp("2026-10-04T23:59:59");
        let (from, to) = Window::ThisWeek.bounds(&sunday).expect("bounds");
        assert_eq!(
            (from.as_str(), to.as_str()),
            ("2026-09-28T00:00:00.000Z", "2026-10-05T00:00:00.000Z")
        );

        // One second later it is Monday, and a new week.
        let monday = stamp("2026-10-05T00:00:00");
        let (from, to) = Window::ThisWeek.bounds(&monday).expect("bounds");
        assert_eq!(
            (from.as_str(), to.as_str()),
            ("2026-10-05T00:00:00.000Z", "2026-10-12T00:00:00.000Z")
        );

        let (from, to) = Window::LastWeek.bounds(&monday).expect("bounds");
        assert_eq!(
            (from.as_str(), to.as_str()),
            ("2026-09-28T00:00:00.000Z", "2026-10-05T00:00:00.000Z")
        );
    }

    /// ANY-04. "Today" is the user's today, not Greenwich's. In Auckland
    /// (UTC+13) a meeting at 00:30 is, in UTC, still yesterday; in Los Angeles
    /// (UTC−8) one at 23:30 is already tomorrow. Both are in the right day.
    #[tokio::test]
    async fn any_04_today_is_the_users_today_in_any_time_zone() {
        for (offset_hours, label) in [(13, "Auckland"), (-8, "Los Angeles"), (0, "London")] {
            let zone = FixedOffset::east_opt(offset_hours * 3600).expect("zone");
            let pool = migrated_pool().await;
            let now = zone
                .from_local_datetime(
                    &chrono::NaiveDateTime::parse_from_str(
                        "2026-10-02T12:00:00",
                        "%Y-%m-%dT%H:%M:%S",
                    )
                    .expect("stamp"),
                )
                .single()
                .expect("now");

            // Every meeting on the user's 2 October, and one either side of it.
            for (day, time, subject) in [
                ("2026-10-01", "23:59", "yesterday-late"),
                ("2026-10-02", "00:00", "first-minute"),
                ("2026-10-02", "23:59", "last-minute"),
                ("2026-10-03", "00:00", "tomorrow-early"),
            ] {
                let event = crate::microsoft::Event {
                    subject: subject.to_string(),
                    start: at(day, time),
                    end: at(day, time),
                    organiser: None,
                    attendees: vec![],
                    online: false,
                    all_day: false,
                };
                let record = crate::ingest::from_event(&event, 1, &zone).expect("a row");
                work_log::upsert(&pool, record).await.expect("stored");
            }

            let (from, to) = intent::day_window(&now);
            let rows = crate::retrieval::in_window(&pool, &from, &to, Some("calendar"), 20)
                .await
                .expect("rows");
            let names: Vec<_> = rows.iter().map(|hit| hit.title.as_str()).collect();

            assert!(names.contains(&"first-minute"), "{label}: {names:?}");
            assert!(names.contains(&"last-minute"), "{label}: {names:?}");
            assert!(!names.contains(&"yesterday-late"), "{label}: {names:?}");
            assert!(!names.contains(&"tomorrow-early"), "{label}: {names:?}");
        }
    }

    /// ANY-05. The model is not running. The brief says so and writes nothing,
    /// so the day is not marked briefed and the next pass tries again.
    #[tokio::test]
    async fn any_05_an_engine_that_is_down_writes_no_brief_and_marks_no_day() {
        let github = github_with(Github {
            mine: search(&[pr(7, "acme/api", "Something", None, "2026-10-02T08:00:00Z")]),
            ..Github::default()
        })
        .await;
        let world = world(Some(&github.host), None, NOWHERE).await;

        let failed = dawn_brief(&world.ctx).await;

        assert!(failed.is_err());
        let path = format!("briefs/{}.md", recipe::today_date());
        assert!(world.ctx.corpus.read(&path).await.is_err(), "no file");
        assert_eq!(
            recipe::written_at(&world.ctx.pool, &recipe::today_date())
                .await
                .expect("read"),
            None,
            "and the day is not recorded as done"
        );
    }

    /// ANY-06. A model that answers with nothing is not a brief. An empty file
    /// recorded as today's brief would stand for the whole day.
    #[tokio::test]
    async fn any_06_an_empty_answer_is_not_filed_as_the_brief() {
        let github = github_with(Github {
            mine: search(&[pr(7, "acme/api", "Something", None, "2026-10-02T08:00:00Z")]),
            ..Github::default()
        })
        .await;
        let engine = model_saying("   \n  ").await;
        let world = world(Some(&github.host), None, &engine.host).await;

        let result = dawn_brief(&world.ctx).await;

        assert!(result.is_err(), "a blank brief was accepted: {result:?}");
        assert_eq!(
            recipe::written_at(&world.ctx.pool, &recipe::today_date())
                .await
                .expect("read"),
            None
        );
    }

    /// ANY-07. The power goes while the model is writing. Half an answer plus
    /// an apology is not the day's brief, and the day must stay open so the
    /// next pass writes a whole one.
    #[tokio::test]
    async fn any_07_an_answer_cut_off_by_a_power_cut_is_not_filed_as_the_brief() {
        let github = github_with(Github {
            mine: search(&[pr(7, "acme/api", "Something", None, "2026-10-02T08:00:00Z")]),
            ..Github::default()
        })
        .await;
        // The stream ends without `[DONE]`: the engine never got to finish.
        let engine =
            mock(|_| Reply::Torn(sse("- 10:00 stand").replace("data: [DONE]\n\n", ""))).await;
        let world = world(Some(&github.host), None, &engine.host).await;

        let result = dawn_brief(&world.ctx).await;

        let path = format!("briefs/{}.md", recipe::today_date());
        let written = world.ctx.corpus.read(&path).await;
        assert!(
            result.is_err() && written.is_err(),
            "a half-written brief was kept as the day's: {result:?} / {written:?}"
        );
        assert_eq!(
            recipe::written_at(&world.ctx.pool, &recipe::today_date())
                .await
                .expect("read"),
            None
        );
    }

    /// ANY-08. Signed in, but offline, and nothing saved yet. "Nothing is
    /// connected" is the wrong thing to say: something is, and cannot be
    /// reached.
    #[tokio::test]
    async fn any_08_connected_but_offline_is_not_reported_as_not_connected() {
        let engine = model_saying("- ok").await;
        let world = world(Some(NOWHERE), Some(NOWHERE), &engine.host).await;

        let refused = dawn_brief(&world.ctx).await.expect_err("nothing to say");

        assert!(
            !refused.to_string().contains("nothing is connected"),
            "told the user to connect what is already connected: {refused}"
        );
        assert!(
            refused.to_string().to_lowercase().contains("reach"),
            "say what is wrong — the network — not what to set up: {refused}"
        );
    }

    /// ANY-09. Offline, but there is a work log from earlier: the brief is
    /// still written from it. Being offline never means an empty morning.
    #[tokio::test]
    async fn any_09_offline_with_a_saved_log_still_makes_a_brief() {
        let engine = model_saying("- you merged #12").await;
        let world = world(Some(NOWHERE), None, &engine.host).await;

        work_log::upsert(
            &world.ctx.pool,
            work_log::WorkLogRecord {
                timestamp: "2026-10-01T16:00:00Z".into(),
                source: "github".into(),
                category: crate::ingest::SHIPPED.into(),
                title: "acme/api #12: Add rate limiting".into(),
                content: "Merged pull request #12".into(),
                summary: Some("merged".into()),
                url: None,
                raw_ref: None,
                external_id: "acme/api#12".into(),
                account_id: 1,
            },
        )
        .await
        .expect("stored");

        let brief = dawn_brief(&world.ctx).await.expect("a brief");

        assert!(engine.last_prompt().contains("Add rate limiting"));
        assert_eq!(brief.sources, ["work log"]);
    }

    /// ANY-10. The laptop wakes with no network, the pass fails and says so;
    /// when the network returns the next pass succeeds and the state is
    /// healthy again. An outage is never confused with a revoked sign-in.
    #[tokio::test]
    async fn any_10_a_pass_after_an_outage_recovers_by_itself() {
        let engine = model_saying("- ok").await;
        let mut world = world(Some(NOWHERE), None, &engine.host).await;
        let account = integrations::accounts(&world.ctx.pool, integrations::GITHUB)
            .await
            .expect("accounts")[0]
            .id;

        daemon::run_once(&world.daemon())
            .await
            .expect("a pass reports rather than fails");
        let down = sync_state::for_account(&world.ctx.pool, account)
            .await
            .expect("read")
            .expect("state");
        assert_eq!(
            down.status,
            sync_state::Status::Error,
            "an outage, not a revoked sign-in"
        );

        let now = Local::now().to_rfc3339();
        let back = github_with(Github {
            merged: search(&[pr(12, "acme/api", "Add rate limiting", Some(&now), &now)]),
            ..Github::default()
        })
        .await;
        world.ctx.github = github::Client::against(&back.host).expect("client");

        let outcome = daemon::run_once(&world.daemon()).await.expect("a pass");
        let up = sync_state::for_account(&world.ctx.pool, account)
            .await
            .expect("read")
            .expect("state");

        assert_eq!(outcome.written, 1);
        assert_eq!(up.status, sync_state::Status::Ok);
        assert!(up.last_synced_at.is_some());
        assert!(up.error_message.is_none(), "{up:?}");
    }

    /// ANY-11. The machine loses power part-way through writing a file. Chief
    /// writes through a temporary file and renames it, so what is on disk is
    /// the old file or the whole new one — never a torn one — and no
    /// temporary is left in the folder the user reads.
    #[tokio::test]
    async fn any_11_a_write_replaces_a_file_whole_and_leaves_nothing_behind() {
        let engine = model_saying("- ok").await;
        let world = world(None, None, &engine.host).await;

        world
            .ctx
            .corpus
            .write("briefs/day.md", "the old brief")
            .await
            .expect("write");
        world
            .ctx
            .corpus
            .write("briefs/day.md", "the new brief")
            .await
            .expect("write");

        assert_eq!(
            world.ctx.corpus.read("briefs/day.md").await.expect("read"),
            "the new brief"
        );

        let names: Vec<String> = std::fs::read_dir(world.root.join("briefs"))
            .expect("folder")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            names
                .iter()
                .all(|name| name == "day.md" || name == "README.md"),
            "a temporary file was left behind: {names:?}"
        );

        // And the old file was replaced rather than rewritten in place: on a
        // power cut, an in-place rewrite is the one that can be left torn.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            let path = world.root.join("briefs/day.md");
            let before = std::fs::metadata(&path).expect("file").ino();
            world
                .ctx
                .corpus
                .write("briefs/day.md", "a third brief")
                .await
                .expect("write");
            let after = std::fs::metadata(&path).expect("file").ino();

            assert_ne!(
                before, after,
                "the file was truncated and refilled in place"
            );
        }
    }

    /// ANY-12. The user edits today's brief by hand. Regenerating — which the
    /// daemon does when it wakes — must not take their words away.
    #[tokio::test]
    async fn any_12_a_brief_the_user_edited_survives_the_next_pass() {
        let github = github_with(Github {
            mine: search(&[pr(7, "acme/api", "Something", None, "2026-10-02T08:00:00Z")]),
            ..Github::default()
        })
        .await;
        let engine = model_saying("- the first brief").await;
        let world = world(Some(&github.host), None, &engine.host).await;

        let first = dawn_brief(&world.ctx).await.expect("a brief");

        tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
        world
            .ctx
            .corpus
            .write(&first.path, "- my own plan for today")
            .await
            .expect("edit");

        let again = dawn_brief(&world.ctx).await;

        assert!(again.is_err());
        assert_eq!(
            world.ctx.corpus.read(&first.path).await.expect("read"),
            "- my own plan for today"
        );
    }

    /// ANY-13. One brief makes one model call, however many sources there are
    /// and however full they are. That is the performance promise: a brief
    /// costs a single prompt, and the prompt stays inside the budget.
    #[tokio::test]
    async fn any_13_a_brief_is_one_model_call_inside_the_budget() {
        let day = today();
        let events: Vec<Value> = (0..60)
            .map(|n| {
                meeting(
                    &format!("Meeting number {n}"),
                    &at(&day, &format!("{:02}:{:02}", 7 + n / 6, (n % 6) * 10)),
                    &at(&day, "23:00"),
                    &["Someone Else"],
                )
            })
            .collect();
        let reviews: Vec<Value> = (0..30)
            .map(|n| {
                pr(
                    n,
                    "acme/api",
                    &format!("Change number {n}"),
                    None,
                    "2026-10-02T08:00:00Z",
                )
            })
            .collect();
        let graph = graph_with(
            events,
            (0..30)
                .map(|n| mail("Someone", &format!("Mail {n}"), true))
                .collect(),
        )
        .await;
        let github = github_with(Github {
            reviews: search(&reviews),
            mine: search(&reviews),
            ..Github::default()
        })
        .await;
        let engine = model_saying("- ok").await;
        let world = world(Some(&github.host), Some(&graph.host), &engine.host).await;

        dawn_brief(&world.ctx).await.expect("a brief");

        assert_eq!(engine.count(), 1, "one model call per brief");
        assert!(
            crate::context::estimate_tokens(&engine.last_prompt())
                <= crate::context::DEFAULT_CEILING,
            "the prompt is over the budget"
        );
    }

    /// ANY-14. Questions a user would plausibly type in each persona's words:
    /// the ones that mean exactly a stored read take the free route, and the
    /// ones that only mention a trigger word do not.
    #[test]
    fn any_14_persona_questions_route_only_when_they_mean_it() {
        // Meant, and answered from this machine.
        for question in [
            "What's on my calendar today?",
            "what meetings do I have",
            "Brief me",
            "/prep",
            "What did I ship this week?",
            "What needs my review",
            "who is waiting on me",
        ] {
            assert!(
                intent::route(question).is_some(),
                "{question:?} should route"
            );
        }

        // Mentions a trigger word, means something else: must reach the model.
        for question in [
            "What's on my calendar tomorrow?",
            "Brief me on the Okafor family situation",
            "How do I brief a new hire?",
            "What did I ship to production in March?",
            "Can you prep the agenda for my team offsite",
            "Who is waiting on me for the school form?",
            "",
        ] {
            assert!(
                intent::route(question).is_none(),
                "{question:?} must not be hijacked"
            );
        }
    }

    /// ANY-15. The connection to GitHub is accepted and then dropped with no
    /// answer, as flaky wifi does. The calendar half of the day is unaffected,
    /// the failure is recorded as an outage, and nothing panics.
    #[tokio::test]
    async fn any_15_a_dropped_connection_costs_one_source_not_the_brief() {
        let day = today();
        let github = mock(|_| Reply::Offline).await;
        let graph = graph_with(
            vec![meeting(
                "Sprint planning",
                &at(&day, "10:00"),
                &at(&day, "11:00"),
                &["Team"],
            )],
            vec![],
        )
        .await;
        let engine = model_saying("- ok").await;
        let world = world(Some(&github.host), Some(&graph.host), &engine.host).await;

        let brief = dawn_brief(&world.ctx).await.expect("a brief");
        assert_eq!(brief.sources, ["calendar"]);

        daemon::run_once(&world.daemon())
            .await
            .expect("a pass reports rather than fails");
        let account = integrations::accounts(&world.ctx.pool, integrations::GITHUB)
            .await
            .expect("accounts")[0]
            .id;
        let state = sync_state::for_account(&world.ctx.pool, account)
            .await
            .expect("read")
            .expect("state");
        assert_eq!(state.status, sync_state::Status::Error);
    }
    /// ANY-25. A small model circles one list until it runs out of room, as
    /// Llama 3.2 1B did on an 8 GB Mac. The brief keeps each line once, drops
    /// the apology, and the request asks the model not to repeat itself.
    #[tokio::test]
    async fn any_25_a_looping_model_cannot_fill_the_brief_with_one_list() {
        let github = github_with(Github {
            mine: search(&[pr(7, "acme/api", "Something", None, "2026-10-02T08:00:00Z")]),
            ..Github::default()
        })
        .await;
        let engine = model_saying(
            "- 10:00 Standup\n- Review the billing pull request\n- Review the search pull request\n\
             - Review the billing pull request\n- Review the search pull request\n\
             - Review the billing pull request\n- Review the search pull request",
        )
        .await;
        let world = world(Some(&github.host), None, &engine.host).await;

        let brief = dawn_brief(&world.ctx).await.expect("a brief");

        let written = world
            .ctx
            .corpus
            .read(&format!("briefs/{}.md", recipe::today_date()))
            .await
            .expect("filed");
        assert_eq!(
            written.matches("billing pull request").count(),
            1,
            "the loop was filed as the brief: {written}"
        );
        assert!(written.contains("Standup") && brief.date == recipe::today_date());
        assert!(
            engine
                .requests()
                .iter()
                .any(|seen| seen.body.contains("\"repeat_penalty\"")),
            "the model was not asked to avoid repeating itself"
        );
    }

    /// ANY-24. Qwen3 reasons at length before it answers unless its template is
    /// told not to, which on an 8 GB Intel Mac is minutes of writing nobody
    /// reads. The brief request says so, and asks for the mildest repeat penalty
    /// the benchmark found to cost nothing.
    #[tokio::test]
    async fn any_24_the_brief_request_switches_thinking_off_and_stays_mild() {
        let github = github_with(Github {
            mine: search(&[pr(7, "acme/api", "Something", None, "2026-10-02T08:00:00Z")]),
            ..Github::default()
        })
        .await;
        let engine = model_saying("- 10:00 Standup").await;
        let world = world(Some(&github.host), None, &engine.host).await;

        dawn_brief(&world.ctx).await.expect("a brief");

        let body: serde_json::Value = serde_json::from_str(
            engine
                .requests()
                .last()
                .expect("the model was asked")
                .body
                .as_str(),
        )
        .expect("json");
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(
            body["repeat_penalty"].as_f64().map(|p| (p * 100.0).round()),
            Some(105.0)
        );
    }
}
