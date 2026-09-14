//! The background work log.
//!
//! Periodically, for every connected account: ask GitHub what the user has been
//! doing, turn each item into a row through [`ingest`], and write it to
//! `work_logs`. GitHub is an account the user connected, and nothing leaves the
//! machine that they did not point Chief at.
//!
//! **The model is not involved.** A pass used to spend one generation per item
//! writing a one-sentence achievement; `ingest` derives the same fields from
//! what GitHub already said. That is what makes the interval a setting rather
//! than a compromise with `engine::IDLE_TIMEOUT`.
//!
//! Every pass is idempotent: entries carry the pull request's identifier and the
//! account that read it, so a merge already in the log is revised rather than
//! repeated, and an unchanged one is not written at all.

use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Manager, Runtime};

use crate::agent::Attention;
use crate::db;
use crate::github::{self, Involvement, PullRequest, State};
use crate::ingest;
use crate::integrations;
use crate::journal;
use crate::llama;
use crate::propose;
use crate::recipe;
use crate::session::GithubSession;
use crate::settings;
use crate::sync_state;
use crate::work_log;

/// How long to settle after launch before the first pass, so startup is not
/// competing with a model.
const FIRST_PASS_DELAY: Duration = Duration::from_secs(20);

/// Where the pass interval is stored, in whole minutes.
///
/// A setting rather than a constant because the right answer is a person's:
/// somebody who ships all day wants ten minutes and somebody who checks Chief
/// once a morning wants four hours, and neither is wrong. It became affordable
/// with deterministic ingestion — a pass that never wakes the engine costs one
/// GitHub page per account, so a short interval is no longer a decision to hold
/// two gigabytes resident all day.
const CADENCE_KEY: &str = "daemon.pass_interval_minutes";

/// How often to look for new work when nothing says otherwise.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// The narrowest and widest intervals a stored value is allowed to ask for.
///
/// The floor is about the services rather than about this machine: GitHub's
/// rate limit is per hour, and a pass reads one page per connected account, so
/// polling every minute would spend somebody's whole allowance on a question
/// whose answer changes a few times a day. The ceiling is so that a value typed
/// with an extra digit degrades into "daily" rather than into "never".
const MINIMUM_INTERVAL: Duration = Duration::from_secs(5 * 60);
const MAXIMUM_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// How often the loop asks whether a pass is due.
///
/// Short enough that a machine waking from suspend runs within a minute of
/// opening the lid, and the check itself is two clock reads.
const TICK: Duration = Duration::from_secs(60);

/// How long to wait after a pass that stepped aside for the user.
///
/// A pass that yielded read nothing and spent nothing, so coming back shortly
/// costs no request it has already made — which is why this sits below
/// [`MINIMUM_INTERVAL`], whose floor is about GitHub's hourly rate limit and
/// applies to passes that actually read. Long enough that a question in flight
/// has a chance to finish; short enough that one question does not cost the log
/// half an hour, which is what waiting out the full interval used to do.
const YIELD_RETRY: Duration = Duration::from_secs(2 * 60);

/// How many merged pull requests to consider in one pass.
const BATCH: u8 = 25;

/// What a pass did, and whether it got to finish.
///
/// **`yielded` is the part that was missing.** Every place the daemon steps
/// aside for `Attention` used to do it silently and return the same nothing as
/// a pass that had genuinely found nothing — so a pass suppressed by a question
/// in flight was indistinguishable from a caught-up one, waited out the whole
/// interval, and left no trace anywhere that it had happened.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Outcome {
    /// How many entries were written or revised.
    pub written: usize,
    /// Whether any part of the pass stopped early because the user was waiting.
    pub yielded: bool,
}

/// What can go wrong during a pass.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Github(#[from] github::Error),
    #[error(transparent)]
    Storage(#[from] db::Error),
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// The right to be the pass that is running.
///
/// One at a time, whoever asked. The background loop and [`sync_now`] read the
/// same accounts and write the same rows, and two of them at once spends a
/// person's GitHub allowance twice to reach the state one of them would have
/// reached alone.
///
/// **Deliberately not [`Attention`].** Every other foreground path — `ask_agent`,
/// `generate_brief` — takes an `Attention` guard so the daemon steps aside for
/// it. A pass that did that would step aside for *itself*: `run_once` breaks
/// out of its account loop the moment `Attention::is_engaged()`, so the button
/// would return "0 entries" without having read anything, which is precisely
/// the complaint it is being added to answer.
#[derive(Debug, Default, Clone)]
pub struct Passes(Arc<tokio::sync::Mutex<()>>);

/// What one pass did, for the screen that asked for it.
///
/// `written` alone is not enough to report with. A pass that reached an account
/// and found nothing new writes nothing, and so does one whose every account
/// was refused — `run_once` records a per-account failure against the account
/// and carries on, by design. So the states come back too, and the interface
/// tells the two apart from those rather than from the count.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pass {
    /// How many entries this pass wrote or revised.
    pub written: usize,
    /// What every connected account has to say about itself now.
    pub accounts: Vec<sync_state::SyncState>,
}

/// Run one pass now, because somebody asked.
///
/// Until this existed the work log's "Refresh" called `list_work_logs`, which
/// is a `SELECT` — it re-read the rows a pass had already written and asked
/// GitHub for nothing. A user looking at a stale log had no way to ask for a
/// fresh one and no way to find out why it was stale, which is how three days
/// of missing entries reached a release.
///
/// It calls [`run_once`], not a second ingestion path: a button that fetched
/// differently from the daemon would be a second thing to keep correct, and
/// the first divergence would be invisible.
#[tauri::command]
pub async fn sync_now<R: Runtime>(app: AppHandle<R>) -> Result<Pass, Error> {
    let passes = app.state::<Passes>().inner().clone();
    let _running = passes.0.lock().await;

    let context = context(&app).await?;
    let outcome = run_once(&context).await?;

    // The calendars too, because "Refresh" on the work log means all of it. A
    // failure to build the recipe context costs the meetings and not the pass:
    // GitHub has already been read by this point and throwing that away would
    // be the worse answer.
    let meetings = match recipe::context(&app).await {
        Ok(recipe) => ingest_calendar(&recipe).await,
        Err(error) => {
            eprintln!("the calendars could not be read: {error}");
            0
        }
    };

    let accounts = sync_state::all(&context.pool).await?;

    Ok(Pass {
        written: outcome.written + meetings,
        accounts,
    })
}

/// Everything a pass needs. Passed in so the whole thing runs in tests against
/// an in-memory database and stub servers.
/// **No engine.** Ingestion is deterministic (see `ingest`), so a pass has no
/// client to call the model with — which makes "a pass never calls the model"
/// a property of the type rather than something a test has to keep watching.
/// The brief and the proposal pass build their own context through
/// `recipe::context`, and those do have one, because generating prose is what
/// they are for.
///
/// This replaced four tests that stood up an empty engine stub and asserted it
/// received nothing. Once the field went, those could not fail whatever the
/// pass did — a stub nothing can reach records nothing — so they were removed
/// rather than kept for the reassurance. The type is the guard now.
pub struct Context {
    pub pool: sqlx::SqlitePool,
    pub github: github::Client,
    /// Whether the user is waiting on an answer right now.
    pub attention: Attention,
}

/// Start the daemon. Returns immediately; the work happens on the async runtime.
///
/// **The loop outlives whatever one pass does to itself.** This is a single
/// spawned task, and a panic anywhere inside it used to end ingestion, the
/// brief, the journal and the drafts together, for the life of the process,
/// with the message going to a stderr nobody running an installed copy ever
/// sees. That is indistinguishable, from the outside, from Chief being quietly
/// idle — which is what "the work log stops on the 30th and the last brief is
/// the 31st" looks like. Each pass therefore runs in a task of its own and the
/// loop reads how it ended.
pub fn spawn<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_PASS_DELAY).await;

        loop {
            let outcome = isolated("the work log pass", one_pass(app.clone()))
                .await
                .unwrap_or_default();

            // A pass that stepped aside read nothing and spent nothing, so
            // trying again shortly costs no request it has already made — and
            // waiting out the full interval is how one question in flight cost
            // the log half an hour.
            let interval = if outcome.yielded {
                YIELD_RETRY
            } else {
                match db::pool(&app).await {
                    Ok(pool) => cadence(&pool).await,
                    // A pool that cannot be opened is the next pass's problem
                    // to report; waiting the default is right until then.
                    Err(_) => DEFAULT_INTERVAL,
                }
            };

            wait_until_due(interval).await;
        }
    });
}

/// Run `work`, and say whether it finished rather than letting it take the
/// caller down with it.
///
/// `None` means it panicked, which has been reported by the time this returns.
/// A task of its own rather than `catch_unwind`, because a future is not
/// `UnwindSafe` and asserting that it is would be a claim about every await
/// point inside it that nothing here is in a position to make.
async fn isolated<T, F>(what: &str, work: F) -> Option<T>
where
    T: Send + 'static,
    F: std::future::Future<Output = T> + Send + 'static,
{
    match tauri::async_runtime::spawn(work).await {
        Ok(done) => Some(done),
        Err(error) => {
            eprintln!("{what} panicked and was abandoned; the next one will still run: {error}");
            None
        }
    }
}

/// Everything one turn of the daemon does.
async fn one_pass<R: Runtime>(app: AppHandle<R>) -> Outcome {
    // The engine gives its memory back when nothing is using it, so a pass may
    // have to start it. Failing to is not fatal and not worth reporting twice:
    // the pass below will say so in its own terms.
    //
    // The guard is held for the whole pass, not just the start. The brief runs
    // well past the idle timeout on a small model — without this the supervisor
    // stops the engine halfway through, and every pass from then on dies at the
    // same place.
    let working = {
        let engine = app.state::<crate::engine::Engine>();
        let client = app.state::<llama::Client>();
        let _ = engine.ensure_running(client.inner()).await;

        engine.working()
    };

    let mut outcome = Outcome::default();

    {
        // Held only for the ingestion, not for the brief and the draft below:
        // those are model calls and a person clicking Refresh must not wait
        // behind one.
        let passes = app.state::<Passes>().inner().clone();
        let _running = passes.0.lock().await;

        match context(&app).await {
            Ok(context) => match run_once(&context).await {
                Ok(done) => outcome = done,
                // A pass failing is not fatal: GitHub may be unreachable or the
                // engine may still be loading. Try again next time.
                Err(error) => eprintln!("work log pass failed: {error}"),
            },
            Err(error) => eprintln!("work log pass could not start: {error}"),
        }

        match recipe::context(&app).await {
            Ok(recipe) => outcome.written += ingest_calendar(&recipe).await,
            Err(error) => eprintln!("the calendars could not be read: {error}"),
        }
    }

    outcome.yielded |= brief_if_the_day_has_none(&app).await;
    roll_up_old_work(&app).await;
    outcome.yielded |= propose_for_one_thing(&app).await;

    // The engine is free to go idle again from here.
    drop(working);

    outcome
}

/// How long to wait between passes, as the user has it set.
async fn cadence(pool: &sqlx::SqlitePool) -> Duration {
    let stored = settings::get(pool, CADENCE_KEY).await.unwrap_or_default();

    interval_from(stored.as_deref())
}

/// Turn a stored value into an interval.
///
/// Pure, so the clamping is tested without a database. A value that is not a
/// positive whole number of minutes falls back to the default rather than
/// being clamped: `0`, `-5` and `soon` are not somebody asking for the floor,
/// they are somebody having got it wrong, and answering a mistake with the
/// most aggressive polling Chief allows is the worst reading available.
fn interval_from(stored: Option<&str>) -> Duration {
    let Some(minutes) = stored.and_then(|value| value.trim().parse::<u64>().ok()) else {
        return DEFAULT_INTERVAL;
    };

    if minutes == 0 {
        return DEFAULT_INTERVAL;
    }

    Duration::from_secs(minutes * 60).clamp(MINIMUM_INTERVAL, MAXIMUM_INTERVAL)
}

/// Whether a pass is due, given how much time each clock says has passed.
///
/// **Two clocks, because neither is sufficient alone.** `Instant` is monotonic
/// and on Linux does not advance while the machine is suspended, so a laptop
/// closed for four hours wakes believing four *minutes* went by, and the log
/// stays stale until the interval runs out all over again. The wall clock does
/// advance across a suspend — but it also moves when the user corrects it, when
/// a time zone changes, and when NTP steps it backwards, and a clock that
/// jumped backwards would postpone the pass indefinitely.
///
/// So a pass is due when **either** says so. Whichever clock jumped covers the
/// one that did not, and taking the earlier of the two means a wrong clock can
/// make a pass early but can never stop one happening.
///
/// It answers *whether*, never *how many*. A machine suspended across four
/// scheduled passes runs one when it wakes: those four would have read the same
/// GitHub page four times and upserted the same unchanged rows, so three of
/// them are work with nothing at the end of it. This is the same reasoning as
/// `brief_if_the_day_has_none` — ask what is true now, do not replay a
/// schedule that was missed.
const fn is_due(interval: Duration, monotonic: Duration, wall: Duration) -> bool {
    monotonic.as_secs() >= interval.as_secs() || wall.as_secs() >= interval.as_secs()
}

/// Sleep until the next pass is due, checking both clocks as it goes.
async fn wait_until_due(interval: Duration) {
    let monotonic = std::time::Instant::now();
    let started_at = chrono::Utc::now();

    loop {
        tokio::time::sleep(TICK.min(interval)).await;

        // A wall clock that moved backwards yields a negative span, which is
        // no evidence that a pass is due — the monotonic side carries it.
        let wall = (chrono::Utc::now() - started_at)
            .to_std()
            .unwrap_or(Duration::ZERO);

        if is_due(interval, monotonic.elapsed(), wall) {
            return;
        }
    }
}

/// Roll work older than thirty days into a monthly file in the corpus.
///
/// One call line rather than a body: [`journal::run_once`] takes its own
/// `Context`, following this module's, so a whole pass is testable against an
/// in-memory database and a scratch directory.
///
/// **Nothing is deleted.** The roll-up is a second rendering of rows that stay
/// exactly where they are, so a failure here costs a file that will be written
/// again next time and nothing else. That is why it is not fatal and why it
/// runs after the brief: the brief is what somebody is waiting to read.
async fn roll_up_old_work<R: Runtime>(app: &AppHandle<R>) {
    let context = match recipe::context(app).await {
        Ok(context) => context,
        // Said out loud rather than stepped over. This used to be a bare
        // `else { return; }`, which meant a corpus root that could not be
        // resolved stopped the journal, the brief and the drafts together and
        // left no line anywhere saying it had.
        Err(error) => {
            eprintln!("the journal could not be started: {error}");
            return;
        }
    };

    let journal = journal::Context {
        pool: context.pool,
        corpus: context.corpus,
        attention: app.state::<Attention>().inner().clone(),
    };

    if !journal::due(&journal.corpus, chrono::Utc::now()).await {
        return;
    }

    if let Err(error) = journal::run_once(&journal).await {
        eprintln!("the journal could not be written: {error}");
    }
}

/// Write today's brief if today has not had one.
///
/// The whole of the schedule, and deliberately so. An interval would have to
/// account for a laptop that was asleep at the hour it was due — and the
/// accounting is the part that goes wrong, either skipping the day or writing
/// four briefs at once when the lid opens. Asking "is there a brief for today"
/// is naturally correct across sleep, time zones and a machine that was simply
/// switched off: whenever it is next awake, the day gets exactly one.
///
/// Failing is never fatal. Nothing connected, an engine still loading, a model
/// that would not answer — all of them mean no brief this pass and another
/// attempt shortly.
///
/// Returns whether it stepped aside for somebody waiting, so the loop can come
/// back in [`YIELD_RETRY`] rather than treating "the user was mid-question" as
/// "today has been dealt with".
async fn brief_if_the_day_has_none<R: Runtime>(app: &AppHandle<R>) -> bool {
    let pool = match db::pool(app).await {
        Ok(pool) => pool,
        Err(error) => {
            eprintln!("could not open the database to write a brief: {error}");
            return false;
        }
    };

    let date = chrono::Local::now().format("%Y-%m-%d").to_string();

    match recipe::written_at(&pool, &date).await {
        Ok(Some(_)) => return false,
        Ok(None) => {}
        Err(error) => {
            eprintln!("could not tell whether today has a brief: {error}");
            return false;
        }
    }

    // A person waiting on an answer outranks a brief nobody asked for yet. The
    // next pass will find the day still un-briefed and try again — shortly,
    // because this is reported rather than swallowed.
    if app.state::<Attention>().is_engaged() {
        eprintln!("no brief this pass: somebody is waiting on an answer");
        return true;
    }

    let context = match recipe::context(app).await {
        Ok(context) => context,
        Err(error) => {
            eprintln!("no brief this pass: {error}");
            return false;
        }
    };

    match recipe::daily_brief(&context).await {
        Ok(brief) => eprintln!(
            "wrote the brief for {} from {}",
            brief.date,
            brief.sources.join(", ")
        ),
        Err(error) => eprintln!("no brief this pass: {error}"),
    }

    false
}

/// Draft for one thing Chief noticed, if anything is waiting.
///
/// After the brief, not before it: the brief is what the user opens the app to
/// read, and a draft nobody has asked for yet must not be in front of it in the
/// queue for an engine that decodes one request at a time.
///
/// Failing is never fatal. Nothing connected, an engine still loading, a model
/// that would not answer — all of them mean no draft this pass.
async fn propose_for_one_thing<R: Runtime>(app: &AppHandle<R>) -> bool {
    let attention = app.state::<Attention>().inner().clone();

    if attention.is_engaged() {
        eprintln!("no draft this pass: somebody is waiting on an answer");
        return true;
    }

    let context = match recipe::context(app).await {
        Ok(context) => context,
        Err(error) => {
            eprintln!("no draft this pass: {error}");
            return false;
        }
    };

    match propose::run_once(&context, &attention).await {
        Ok(Some(proposal)) => eprintln!("drafted {} at {}", proposal.title, proposal.path),
        Ok(None) => {}
        Err(error) => eprintln!("no draft this pass: {error}"),
    }

    false
}

async fn context<R: Runtime>(app: &AppHandle<R>) -> Result<Context, db::Error> {
    Ok(Context {
        pool: db::pool(app).await?,
        github: app.state::<github::Client>().inner().clone(),
        attention: app.state::<Attention>().inner().clone(),
    })
}

/// Run one pass over every connected account. Returns how many new entries
/// were written.
///
/// Does nothing at all when nothing is connected — there is no work to read and
/// nothing to report. A person with a work and a personal account is two
/// separate readings of GitHub, because the credentials are separate — and
/// separate is what they stay when one of them fails.
pub async fn run_once(context: &Context) -> Result<Outcome, Error> {
    let accounts = integrations::accounts(&context.pool, integrations::GITHUB).await?;
    let mut outcome = Outcome::default();

    for account in accounts {
        // The user's question is worth more than the log being current, and the
        // engine decodes one request at a time. Stopping between accounts as
        // well as between items keeps a second account from starting a read the
        // pass is about to abandon anyway.
        //
        // Said out loud and carried out in the outcome. Stepping aside used to
        // be silent and indistinguishable from having found nothing, which is
        // how one question in flight came to cost the log a whole interval
        // with no record that anything had been skipped.
        if context.attention.is_engaged() {
            eprintln!("work log pass stepped aside: somebody is waiting on an answer");
            outcome.yielded = true;
            break;
        }

        // A failure belongs to the account it happened to, not to the pass. A
        // token the user revoked on a personal account says nothing about
        // whether their work account can be read, and ending the pass on the
        // first refusal would freeze every later account's log until they
        // noticed which one was at fault. Reported against the id, the way a
        // name that could not be read is, and tried again next pass.
        match run_one_account(context, &account).await {
            Ok(entries) => {
                outcome.written += entries;
                record_state(context, &account, sync_state::Status::Ok, None).await;
            }
            Err(error) => {
                // A rejected credential is the user's to fix and says so in
                // the interface; anything else is worth retrying quietly next
                // pass. Conflating the two is how the amber chip becomes a
                // false alarm every time somebody closes their laptop, so the
                // distinction is drawn here, once, from the error itself.
                let status = if is_credential_gone(&error) {
                    sync_state::Status::AuthRequired
                } else {
                    sync_state::Status::Error
                };

                eprintln!("could not read account {}: {error}", account.id);
                record_state(context, &account, status, Some(&error.to_string())).await;
            }
        }
    }

    Ok(outcome)
}

/// Whether this failure means the credential is gone rather than the host is.
///
/// Only a rejected token counts. `session.rs` already renews and retries once,
/// so a `Github` error that survived that has been refused twice — and every
/// other shape (a timeout, a 500, no route to the host) is a reason to try
/// again later rather than to send the user to a browser.
fn is_credential_gone(error: &Error) -> bool {
    match error {
        Error::Github(inner) => {
            <github::Client as crate::oauth::Provider>::is_token_rejected(inner)
        }
        _ => false,
    }
}

/// Write what just happened to an account, and step over a failure to do so.
///
/// Recording freshness is bookkeeping about the pass; it must never be the
/// reason a pass reports failure, because then the interface would lose the
/// state at exactly the moment it became worth showing.
async fn record_state(
    context: &Context,
    account: &integrations::Account,
    status: sync_state::Status,
    message: Option<&str>,
) {
    if let Err(error) = sync_state::record(
        &context.pool,
        account.id,
        integrations::GITHUB,
        status,
        message,
    )
    .await
    {
        eprintln!("could not record sync state for {}: {error}", account.id);
    }
}

/// Read one account's merged work and log whatever is new.
async fn run_one_account(
    context: &Context,
    account: &integrations::Account,
) -> Result<usize, Error> {
    let session = GithubSession::new(&context.pool, &context.github, account.id);

    // An account carried over from before Chief stored identities has none, and
    // the settings screen has nothing to show but the placeholder key. Fill it
    // in on the next read rather than making the user reconnect for a name.
    //
    // Naming is not the work, so a failure here is reported and stepped over.
    // `name_account` asks GitHub who this is without renewing, where the read
    // below does — letting it abort the pass would strand an account whose only
    // problem is a token that has rotated, which is exactly the migrated row
    // this backfill exists for.
    if account.identity.is_none() {
        if let Err(error) = session.name_account().await {
            eprintln!("could not name account {}: {error}", account.id);
        }
    }

    // **Three questions, not one.** This read `author:@me is:merged` and
    // nothing else, so the log held only work that was already finished: an
    // open pull request never appeared until the day it merged, and a review
    // somebody had asked for never appeared at all. Which made "what is
    // waiting on me" unanswerable from local storage — the question REC-41
    // built the `whose` parameter for, still going out to the network on every
    // ask because there was nothing here to answer it from.
    //
    // Merged first: it is the one a failure would be most missed on, and the
    // reads below fail independently.
    let asked = [
        (Involvement::Authored, State::Merged, ingest::Kind::Mine),
        (Involvement::Authored, State::Open, ingest::Kind::Mine),
        (Involvement::Reviewing, State::Open, ingest::Kind::Review),
    ];

    let mut written = 0;

    for (involvement, state, kind) in asked {
        // Stopping between reads as well as between items, so a pass the user
        // interrupted does not spend a request it is about to abandon anyway.
        if context.attention.is_engaged() {
            break;
        }

        let found = session.pull_requests(involvement, state, BATCH).await?;

        for pull_request in found {
            // The engine decodes one request at a time, and the user's own
            // question is worth more than the log being current. The rest keeps
            // until the next pass.
            if context.attention.is_engaged() {
                break;
            }

            if log_one(context, account.id, &pull_request, kind).await? {
                written += 1;
            }
        }
    }

    Ok(written)
}

/// Add one pull request to the log, or bring the row it already has up to date.
///
/// **No model call.** This used to ask the engine for a one-sentence
/// achievement per item, which was the right shape when the log was prose a
/// person read and the wrong one once D9 made it the thing reads are answered
/// from: a feed row wants a title, a state and a link, and the provider already
/// said all three. See `ingest`.
///
/// `upsert` rather than `insert_new`, so a pull request that was open when it
/// was first seen and is merged by the next pass changes rather than being
/// skipped as already known. It reports whether anything actually moved, so a
/// caught-up pass still writes nothing and still counts nothing.
async fn log_one(
    context: &Context,
    account_id: i64,
    pull_request: &PullRequest,
    kind: ingest::Kind,
) -> Result<bool, Error> {
    Ok(work_log::upsert(
        &context.pool,
        ingest::from_pull_request(pull_request, account_id, kind),
    )
    .await?)
}

/// Write today's meetings to the work log, from every calendar there is.
///
/// **The other half of D9, and the half that was never built.**
/// `intent::prep` answers "what is on my calendar" by querying
/// `category = 'calendar'` in `work_logs` — rows that, until now, nothing in
/// production ever wrote. So the query matched nothing, `answer` returned
/// `None`, and the question fell through to the tool loop and Microsoft Graph
/// on every single ask, which is the network round trip D9 exists to remove.
///
/// Takes a [`recipe::Context`] rather than this module's, because it needs the
/// calendar and Outlook clients the brief already assembles — and because
/// `recipe::subscribed_events` is the merge of subscriptions and Graph that
/// keeps a reader from ever having to know which kind a meeting came from.
/// Reusing it is what keeps the two from drifting.
///
/// A calendar that will not load costs that calendar and nothing else, the
/// rule every source in `recipe::gather` already follows.
pub async fn ingest_calendar(context: &recipe::Context) -> usize {
    let zone = chrono::Local::now().timezone();
    let mut written = 0;

    for (account_id, event) in todays_meetings(context).await {
        let Some(record) = ingest::from_event(&event, account_id, &zone) else {
            continue;
        };

        match work_log::upsert(&context.pool, record).await {
            Ok(true) => written += 1,
            Ok(false) => {}
            Err(error) => eprintln!("a meeting could not be logged: {error}"),
        }
    }

    written
}

/// Today's meetings, each with the account it was read from.
///
/// The account matters because the dedupe index is
/// `(source, account_id, external_id)`: two people sharing a machine, or one
/// person with a work and a personal calendar, legitimately hold the same
/// meeting twice and neither copy should overwrite the other.
async fn todays_meetings(context: &recipe::Context) -> Vec<(i64, crate::microsoft::Event)> {
    let mut found = recipe::subscribed_events_by_account(context).await;

    let (from, to) = recipe::today();

    for account in recipe::outlook_accounts(context).await {
        let session =
            crate::session::OutlookSession::new(&context.pool, &context.microsoft, account);

        match session.events(&from, &to, recipe::PER_SOURCE).await {
            Ok(events) => found.extend(events.into_iter().map(|event| (account, event))),
            Err(error) => eprintln!("an Outlook calendar could not be read: {error}"),
        }
    }

    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::migrated_pool;
    use crate::llama::test_support::serve;

    const MERGED_PRS: &str = r#"{
        "total_count": 2,
        "items": [
            {
                "number": 12,
                "title": "Add the tool calling orchestrator",
                "repository_url": "https://api.github.com/repos/scottmallinson/chief.ai",
                "state": "closed",
                "draft": false,
                "html_url": "https://github.com/scottmallinson/chief.ai/pull/12",
                "updated_at": "2026-08-19T14:00:00Z",
                "pull_request": { "merged_at": "2026-08-19T13:58:00Z" }
            },
            {
                "number": 9,
                "title": "Add the local database",
                "repository_url": "https://api.github.com/repos/scottmallinson/chief.ai",
                "state": "closed",
                "draft": false,
                "html_url": "https://github.com/scottmallinson/chief.ai/pull/9",
                "updated_at": "2026-08-18T10:00:00Z",
                "pull_request": { "merged_at": "2026-08-18T09:58:00Z" }
            }
        ]
    }"#;

    const NO_PRS: &str = r#"{"total_count":0,"items":[]}"#;

    /// One pull request somebody else opened and asked the user to review.
    const REVIEW_REQUESTED: &str = r#"{
        "total_count": 1,
        "items": [
            {
                "number": 71,
                "title": "Tighten the calendar parser",
                "repository_url": "https://api.github.com/repos/scottmallinson/chief.ai",
                "state": "open",
                "draft": false,
                "html_url": "https://github.com/scottmallinson/chief.ai/pull/71",
                "updated_at": "2026-09-02T11:00:00Z",
                "pull_request": {}
            }
        ]
    }"#;

    /// The three replies one account's pass now needs: merged, then the user's
    /// open pull requests, then the reviews requested of them.
    ///
    /// A helper rather than three literals per test, because the count is the
    /// thing that changes when a question is added to the pass and every stub
    /// list would otherwise have to be found and counted by hand.
    fn one_account(merged: &'static str) -> Vec<(&'static str, &'static str)> {
        vec![
            ("HTTP/1.1 200 OK", merged),
            ("HTTP/1.1 200 OK", NO_PRS),
            ("HTTP/1.1 200 OK", NO_PRS),
        ]
    }

    const VIEWER: &str = r#"{"login":"octocat","name":"The Octocat"}"#;

    /// Connect one GitHub account and return its id.
    ///
    /// `identity` is what the provider said the account is. Passing `Some`
    /// keeps a pass from asking GitHub who this is, which is a request a test
    /// counting requests would otherwise have to stub.
    async fn connect(pool: &sqlx::SqlitePool, account_key: &str, identity: Option<&str>) -> i64 {
        integrations::save(
            pool,
            integrations::NewAccount {
                service: integrations::GITHUB,
                account_key,
                identity,
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
        .expect("should store a credential")
        .id
    }

    async fn context_for(github_host: &str, connected: bool) -> Context {
        let pool = migrated_pool().await;

        if connected {
            connect(&pool, "octocat", Some("octocat")).await;
        }

        Context {
            pool,
            github: github::Client::against(github_host).expect("should build a github client"),
            attention: Attention::default(),
        }
    }

    #[tokio::test]
    async fn steps_aside_while_the_user_is_waiting_on_an_answer() {
        // Neither host exists, so a pass that read anything at all would fail
        // rather than quietly report nothing: this proves it never started.
        let context = context_for("http://127.0.0.1:1", true).await;
        let _waiting = context.attention.begin();

        let outcome = run_once(&context).await.expect("a pass should not fail");

        assert_eq!(outcome.written, 0, "the user's question comes first");
        assert!(
            outcome.yielded,
            "stepping aside has to be reported, or the loop treats it as caught up"
        );
        assert!(work_log::fetch(&context.pool, None)
            .await
            .expect("read")
            .is_empty());
    }

    #[tokio::test]
    async fn does_nothing_when_github_is_not_connected() {
        let context = context_for("http://127.0.0.1:1", false).await;

        let outcome = run_once(&context).await.expect("a pass should not fail");

        assert_eq!(outcome.written, 0);
        assert!(
            !outcome.yielded,
            "nothing was connected; nobody was in the way"
        );
        assert!(work_log::fetch(&context.pool, None)
            .await
            .expect("read")
            .is_empty());
    }

    #[tokio::test]
    async fn writes_a_row_per_merged_pull_request_without_asking_the_model() {
        let (github_host, github_server) = serve(one_account(MERGED_PRS));

        let context = context_for(&github_host, true).await;
        let written = run_once(&context)
            .await
            .expect("a pass should not fail")
            .written;

        assert_eq!(written, 2);

        let entries = work_log::fetch(&context.pool, None).await.expect("read");
        assert_eq!(entries.len(), 2);

        // Newest first, by the time each was merged.
        // Templated from what GitHub said, not generated: the state is the
        // thing a feed row answers and the thing that changes between passes.
        assert_eq!(entries[0].summary.as_deref(), Some("merged"));
        assert_eq!(entries[0].timestamp, "2026-08-19T13:58:00Z");
        assert_eq!(entries[0].source, "github");
        assert_eq!(
            entries[0].external_id.as_deref(),
            Some("scottmallinson/chief.ai#12")
        );
        assert!(
            entries[0]
                .content
                .contains("Add the tool calling orchestrator"),
            "the raw activity should be kept: {}",
            entries[0].content
        );

        github_server.await.expect("github stub should finish");
    }

    /// The three questions a work log has to be able to answer.
    ///
    /// This used to assert the opposite — that a pass asked for merged work and
    /// nothing else — and it was right about the code and wrong about the
    /// product. A log holding only finished work cannot answer "what is waiting
    /// on me", which is one of the three questions the composer offers, so that
    /// question went to the network on every ask.
    ///
    /// Proved by dropping the last two entries from `asked`:
    ///
    /// ```text
    /// a pass has to read the reviews requested of the user, or "what is
    /// waiting on me" has nothing local to answer from
    /// ```
    #[tokio::test]
    async fn asks_github_for_every_question_the_log_has_to_answer() {
        let (github_host, github_server) = serve(one_account(NO_PRS));
        let context = context_for(&github_host, true).await;

        run_once(&context).await.expect("a pass should not fail");

        // **Bounded.** `serve` blocks on `accept` once per scripted reply, so a
        // pass that stopped making one of these reads would leave this await
        // hanging for ever rather than failing — a guard that fails by hanging
        // is a guard nobody can read the result of.
        let requests = tokio::time::timeout(Duration::from_secs(5), github_server)
            .await
            .expect("a pass has to make all three reads; the stub is still waiting for one of them")
            .expect("github stub should finish");
        let asked: Vec<String> = requests
            .iter()
            .map(|request| crate::llama::test_support::split(request).0.to_string())
            .collect();

        assert!(
            asked.iter().any(|line| line.contains("is%3Amerged")),
            "a pass has to read what the user shipped: {asked:?}"
        );
        assert!(
            asked
                .iter()
                .any(|line| line.contains("author%3A%40me") && line.contains("is%3Aopen")),
            "a pass has to read the user's work in flight: {asked:?}"
        );
        assert!(
            asked
                .iter()
                .any(|line| line.contains("review-requested%3A%40me")),
            "a pass has to read the reviews requested of the user, or \"what is \
             waiting on me\" has nothing local to answer from: {asked:?}"
        );
    }

    /// A review request is filed as a review, not as the user's own work.
    ///
    /// The categories are what let one table answer two questions: `retrieval`
    /// and `intent` filter on it, so a review sitting under `pr` would read as
    /// something the user had shipped.
    #[tokio::test]
    async fn a_review_request_is_filed_as_a_review() {
        let (github_host, github_server) = serve(
            vec![
                ("HTTP/1.1 200 OK", NO_PRS),
                ("HTTP/1.1 200 OK", NO_PRS),
                ("HTTP/1.1 200 OK", REVIEW_REQUESTED),
            ]
            .into_iter()
            .collect(),
        );
        let context = context_for(&github_host, true).await;

        run_once(&context).await.expect("a pass should not fail");

        let hits = crate::retrieval::latest(&context.pool, 10)
            .await
            .expect("should read");

        assert_eq!(hits.len(), 1, "the review request should have been logged");
        assert_eq!(hits[0].category, "review");
        assert_eq!(hits[0].summary.as_deref(), Some("review requested"));

        github_server.await.expect("github stub should finish");
    }

    /// A rejected credential is the user's to fix; anything else is not.
    ///
    /// **The negative is the point.** A revoked token and an unreachable host
    /// both fail a pass, and only one of them should send somebody to a
    /// browser. Recording both as `AuthRequired` would put an amber "sign in
    /// again" chip on screen every time a laptop lid closes, and a signal that
    /// cries wolf is worse than no signal — the user learns to ignore it, and
    /// then misses the real one.
    ///
    /// Proved by making `is_credential_gone` return `true` unconditionally and
    /// watching the second half fail:
    ///
    /// ```text
    /// an unreachable host is not a revoked credential
    ///   left: AuthRequired
    ///  right: Error
    /// ```
    #[tokio::test]
    async fn only_a_rejected_credential_asks_the_user_to_sign_in_again() {
        // One reply, because `serve` hands back what it received only once
        // every reply has been consumed: prime two for one account and the
        // await never returns. `session` does renew and retry, but a credential
        // with nothing to renew from fails without reaching this host again.
        let (github_host, github_server) = serve(vec![(
            "HTTP/1.1 401 Unauthorized",
            r#"{"message":"Bad credentials"}"#,
        )]);
        let context = context_for(&github_host, true).await;
        let account = only_account(&context).await;

        run_once(&context)
            .await
            .expect("a pass reports rather than fails");

        let refused = sync_state::for_account(&context.pool, account)
            .await
            .expect("should read")
            .expect("a pass records what happened");

        assert_eq!(
            refused.status,
            sync_state::Status::AuthRequired,
            "a refused credential is gone, and the user has to go and fix it"
        );
        assert!(
            refused.last_synced_at.is_none(),
            "a failure must never stamp a time no successful read produced"
        );

        github_server.await.expect("github stub should finish");

        // The same pass, against a host that is simply not there.
        let unreachable = context_for("http://127.0.0.1:1", true).await;
        let account = only_account(&unreachable).await;

        run_once(&unreachable)
            .await
            .expect("a pass reports rather than fails");

        let stalled = sync_state::for_account(&unreachable.pool, account)
            .await
            .expect("should read")
            .expect("a pass records what happened");

        assert_eq!(
            stalled.status,
            sync_state::Status::Error,
            "an unreachable host is not a revoked credential"
        );
    }

    /// A pass that worked says so, and stamps the time.
    #[tokio::test]
    async fn a_pass_that_read_an_account_records_it_as_fresh() {
        let (github_host, github_server) = serve(one_account(MERGED_PRS));
        let context = context_for(&github_host, true).await;
        let account = only_account(&context).await;

        run_once(&context).await.expect("a pass should not fail");

        let state = sync_state::for_account(&context.pool, account)
            .await
            .expect("should read")
            .expect("a pass records what happened");

        assert_eq!(state.status, sync_state::Status::Ok);
        assert_eq!(state.source, integrations::GITHUB);
        assert!(
            state.last_synced_at.is_some(),
            "a successful read is what a freshness timestamp means"
        );
        assert!(
            state.error_message.is_none(),
            "nothing went wrong, so there is nothing to explain"
        );

        github_server.await.expect("github stub should finish");
    }

    /// Why [`Pass`] carries the account states and not only a count.
    ///
    /// A pass whose every account was refused still returns `Ok(0)`: a failure
    /// belongs to the account it happened to, is recorded against it, and is
    /// stepped over so one bad credential cannot freeze another account's log.
    /// Which means "0 written" says nothing at all on its own — it is what a
    /// caught-up pass reports and what a completely failed one reports.
    ///
    /// Proved by asserting `Ok` instead:
    ///
    /// ```text
    /// a count alone cannot tell a caught-up pass from a failed one
    ///   left: Error
    ///  right: Ok
    /// ```
    #[tokio::test]
    async fn a_failed_pass_and_a_caught_up_one_both_report_nothing_written() {
        // Nothing is listening on port 1, so the read cannot succeed.
        let context = context_for("http://127.0.0.1:1", true).await;
        let account = only_account(&context).await;

        let written = run_once(&context)
            .await
            .expect("a pass should not fail")
            .written;

        assert_eq!(written, 0, "nothing could be read, so nothing was written");

        let state = sync_state::for_account(&context.pool, account)
            .await
            .expect("should read")
            .expect("a pass records what happened");

        assert_eq!(
            state.status,
            sync_state::Status::Error,
            "a count alone cannot tell a caught-up pass from a failed one"
        );
        assert!(
            state.error_message.is_some(),
            "the screen that asked for this pass has to be able to say why"
        );
    }

    /// Whatever account the fixture connected.
    async fn only_account(context: &Context) -> i64 {
        integrations::accounts(&context.pool, integrations::GITHUB)
            .await
            .expect("should read")
            .into_iter()
            .next()
            .expect("the fixture connects one")
            .id
    }

    #[tokio::test]
    async fn running_again_logs_nothing_new() {
        let (github_host, github_server) = serve(
            one_account(MERGED_PRS)
                .into_iter()
                .chain(one_account(MERGED_PRS))
                .collect(),
        );

        let context = context_for(&github_host, true).await;

        assert_eq!(run_once(&context).await.expect("first pass").written, 2);
        assert_eq!(
            run_once(&context).await.expect("second pass").written,
            0,
            "work already in the log should be skipped"
        );
        assert_eq!(
            work_log::fetch(&context.pool, None)
                .await
                .expect("read")
                .len(),
            2
        );

        github_server.await.expect("github stub should finish");
    }

    #[tokio::test]
    async fn logs_the_same_merge_once_for_each_account_that_made_it() {
        // Both accounts read the same stub, so both see the same two merges.
        // They are still four entries: whose work it was is part of what the
        // log records, and the dedupe key says so too.
        let (github_host, github_server) = serve(
            one_account(MERGED_PRS)
                .into_iter()
                .chain(one_account(MERGED_PRS))
                .collect(),
        );

        let context = context_for(&github_host, true).await;
        let second = connect(&context.pool, "hubot", Some("hubot")).await;

        assert_eq!(
            run_once(&context)
                .await
                .expect("a pass should not fail")
                .written,
            4
        );
        assert!(
            work_log::has_logged(
                &context.pool,
                "github",
                second,
                "scottmallinson/chief.ai#12"
            )
            .await
            .expect("read"),
            "the second account should have logged it too"
        );

        github_server.await.expect("github stub should finish");
    }

    #[tokio::test]
    async fn reads_every_account_even_when_one_is_refused() {
        // A credential is refused per account: a personal token the user
        // revoked months ago says nothing about whether their work account can
        // be read. Letting the first one end the pass would mean the log never
        // moves again, on the account they care about, until they notice the
        // other one and disconnect it.
        let (github_host, github_server) = serve(
            std::iter::once((
                "HTTP/1.1 401 Unauthorized",
                r#"{"message":"Bad credentials"}"#,
            ))
            .chain(one_account(MERGED_PRS))
            .collect(),
        );

        // The refused account is the one read first, by id.
        let context = context_for(&github_host, true).await;
        let healthy = connect(&context.pool, "hubot", Some("hubot")).await;

        let written = run_once(&context)
            .await
            .expect("one account's refusal is not a failed pass")
            .written;

        assert_eq!(written, 2, "the healthy account should have been read");
        assert!(
            work_log::has_logged(
                &context.pool,
                "github",
                healthy,
                "scottmallinson/chief.ai#12"
            )
            .await
            .expect("read"),
            "the healthy account's work should be in the log"
        );

        github_server.await.expect("github stub should finish");
    }

    #[tokio::test]
    async fn names_an_account_that_arrived_without_an_identity() {
        // The row a v1 database was migrated from has no identity, because the
        // old table never stored one. A pass fills it in rather than asking the
        // user to reconnect for a name.
        let (github_host, github_server) = serve(
            std::iter::once(("HTTP/1.1 200 OK", VIEWER))
                .chain(one_account(NO_PRS))
                .collect(),
        );
        let context = context_for(&github_host, false).await;
        connect(&context.pool, "github", None).await;

        run_once(&context).await.expect("a pass should not fail");

        let account = integrations::accounts(&context.pool, integrations::GITHUB)
            .await
            .expect("should read")
            .into_iter()
            .next()
            .expect("the account should be there");

        assert_eq!(account.identity.as_deref(), Some("octocat"));

        github_server.await.expect("github stub should finish");
    }

    #[tokio::test]
    async fn reads_an_account_it_could_not_name() {
        // Asking who an account is does not renew a rotated token, but reading
        // its work does. A refused name must therefore step aside for the read
        // rather than take the account down with it.
        let (github_host, github_server) = serve(
            std::iter::once((
                "HTTP/1.1 401 Unauthorized",
                r#"{"message":"Bad credentials"}"#,
            ))
            .chain(one_account(NO_PRS))
            .collect(),
        );
        let context = context_for(&github_host, false).await;
        connect(&context.pool, "github", None).await;

        run_once(&context)
            .await
            .expect("a name we could not read is not a failed pass");

        let requests = github_server.await.expect("github stub should finish");
        assert_eq!(
            requests.len(),
            4,
            "the refused name, and then the three reads a pass makes"
        );
    }
}

/// Ingesting the calendar, and the question it exists to make answerable.
#[cfg(test)]
mod calendar_tests {
    use super::*;
    use crate::corpus::Corpus;
    use crate::db::test_support::migrated_pool;
    use crate::intent::{self, Intent};
    use crate::llama::test_support::serve;

    /// One meeting today, as Graph returns it: a local wall-clock stamp with no
    /// offset, which is the whole reason `ingest::as_utc` exists.
    fn graph_calendar(start: &str) -> String {
        format!(
            r#"{{"value":[{{"subject":"Standup","start":{{"dateTime":"{start}","timeZone":"GMT Standard Time"}},"end":{{"dateTime":"{start}","timeZone":"GMT Standard Time"}},"organizer":{{"emailAddress":{{"name":"Dana Vance"}}}},"attendees":[{{"emailAddress":{{"name":"Dana Vance"}}}}],"isOnlineMeeting":true}}]}}"#
        )
    }

    /// A scratch corpus that removes itself.
    struct Scratch(std::path::PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A recipe context with Outlook pointed at `graph_host` and everything
    /// else pointed nowhere, so any other source failing is a failure to
    /// connect rather than a stub answering by accident.
    async fn context_for(graph_host: &str, name: &str) -> (recipe::Context, Scratch) {
        let pool = migrated_pool().await;

        crate::integrations::save(
            &pool,
            crate::integrations::NewAccount {
                service: crate::integrations::MICROSOFT,
                account_key: "dana@example.com",
                identity: Some("dana@example.com"),
                credential_kind: crate::integrations::OAUTH,
                access_token: "graph_token",
                refresh_token: None,
                expires_at: None,
                scopes: None,
                client_id: None,
                client_secret: None,
            },
        )
        .await
        .expect("should store a credential");

        let root = std::env::temp_dir().join(format!("chief-daemon-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let corpus = Corpus::at(root.clone());
        corpus.ensure_shape().await.expect("should create");

        let (engine_host, _engine) = serve(Vec::<(&str, &str)>::new());

        let context = recipe::Context {
            pool,
            github: github::Client::against("127.0.0.1:1").expect("client"),
            microsoft: crate::microsoft::Client::against(graph_host).expect("client"),
            calendar: crate::calendar::Client::new().expect("client"),
            linear: crate::linear::Client::new().expect("client"),
            atlassian: crate::atlassian::Client::new().expect("client"),
            atlassian_rest: crate::atlassian::rest::Rest::new().expect("client"),
            engine: llama::Client::with_base_url(&engine_host).expect("client"),
            corpus,
        };

        (context, Scratch(root))
    }

    /// The guard REC-57 exists for, end to end.
    ///
    /// `intent::prep` has always queried `category = 'calendar'`, and until now
    /// **nothing in production ever wrote a row with that category** — so the
    /// query matched nothing, `answer` returned `None`, and "what is on my
    /// calendar" fell through to the tool loop and Graph on every ask, which is
    /// the network round trip D9 exists to remove. The existing test in
    /// `intent` did not catch it because it writes the row itself; this one
    /// makes a pass write it.
    ///
    /// Proved by removing the `ingest_calendar` call below:
    ///
    /// ```text
    /// /prep has to answer from rows a pass wrote, not only from rows a test
    /// wrote by hand
    /// ```
    #[tokio::test]
    async fn a_pass_writes_the_rows_prep_has_always_queried_for() {
        // Midday local, so the row lands inside today's window at any offset
        // rather than on an edge of it.
        let today = chrono::Local::now().format("%Y-%m-%dT12:00:00").to_string();
        let (graph_host, graph) = serve(vec![("HTTP/1.1 200 OK", &graph_calendar(&today))]);

        let (context, _scratch) = context_for(&graph_host, "prep").await;

        let written = ingest_calendar(&context).await;
        assert_eq!(written, 1, "the meeting should have been logged");

        let answered = intent::answer(Intent::Prep, &context).await;

        assert!(
            answered.is_some(),
            "/prep has to answer from rows a pass wrote, not only from rows a \
             test wrote by hand"
        );
        assert!(
            answered
                .as_ref()
                .is_some_and(|answered| answered.markdown.contains("Standup")),
            "the answer should be the meeting the pass ingested: {answered:?}"
        );

        graph.await.expect("graph stub should finish");
    }

    /// Running twice writes one row, not two.
    ///
    /// `Event` carries no identifier of its own, so the dedupe key is built
    /// from the start and the subject. If that were not stable, every pass
    /// would add today's meetings again and the agenda would grow all day.
    #[tokio::test]
    async fn the_same_meeting_is_logged_once_however_many_passes_run() {
        let today = chrono::Local::now().format("%Y-%m-%dT12:00:00").to_string();
        let calendar = graph_calendar(&today);
        let (graph_host, graph) = serve(vec![
            ("HTTP/1.1 200 OK", calendar.as_str()),
            ("HTTP/1.1 200 OK", calendar.as_str()),
        ]);

        let (context, _scratch) = context_for(&graph_host, "twice").await;

        assert_eq!(ingest_calendar(&context).await, 1);
        assert_eq!(
            ingest_calendar(&context).await,
            0,
            "a meeting already in the log is not new work"
        );

        let hits = crate::retrieval::latest(&context.pool, 10)
            .await
            .expect("should read");

        assert_eq!(hits.len(), 1, "one meeting, one row");

        graph.await.expect("graph stub should finish");
    }
}

#[cfg(test)]
mod cadence_tests {
    use std::time::Duration;

    use super::{
        cadence, interval_from, is_due, isolated, CADENCE_KEY, DEFAULT_INTERVAL, MAXIMUM_INTERVAL,
        MINIMUM_INTERVAL, TICK, YIELD_RETRY,
    };
    use crate::db::test_support::migrated_pool;
    use crate::settings;

    /// The guard on REC-59's third silent path.
    ///
    /// The daemon is one spawned task, and a panic anywhere in the loop body
    /// used to end ingestion, the brief, the journal and the drafts together —
    /// for the life of the process, with the message going to a stderr nobody
    /// running an installed copy ever sees. From the outside that is
    /// indistinguishable from Chief being quietly idle, which is what "the work
    /// log stops on the 30th and the last brief is the 31st" looks like.
    ///
    /// Proved by awaiting the future directly instead of isolating it:
    ///
    /// ```text
    /// panicked at src/daemon.rs: a pass that goes wrong
    /// ```
    ///
    /// — the panic propagates, and in production it takes the loop with it.
    /// With `isolated` in the way it is reported and the caller carries on,
    /// which is what the second half of this test is standing in for.
    #[tokio::test]
    async fn a_pass_that_panics_is_reported_and_the_next_one_still_runs() {
        let first = isolated("a pass that goes wrong", async {
            panic!("a pass that goes wrong");
        })
        .await;

        assert_eq!(
            first, None,
            "a panicking pass has to come back as a pass that did not finish"
        );

        // And the loop is still here to run the next one, which is the whole
        // point: reaching this line at all is the assertion.
        let second = isolated("a pass that does not", async { 7 }).await;

        assert_eq!(second, Some(7));
    }

    #[tokio::test]
    async fn a_pass_stepping_aside_comes_back_sooner_than_the_cadence() {
        // The floor on `interval_from` is about GitHub's rate limit and applies
        // to passes that actually read. A pass that yielded read nothing, so
        // coming back inside that floor spends nothing it has not already
        // spent — and waiting the full interval is what cost the log half an
        // hour for one question in flight.
        assert!(
            YIELD_RETRY < MINIMUM_INTERVAL,
            "a pass that read nothing must not wait as long as one that did"
        );
        assert!(
            YIELD_RETRY >= TICK,
            "the loop cannot notice anything sooner than it ticks"
        );
    }

    #[tokio::test]
    async fn an_unset_cadence_is_half_an_hour() {
        let pool = migrated_pool().await;

        assert_eq!(cadence(&pool).await, DEFAULT_INTERVAL);
        assert_eq!(DEFAULT_INTERVAL, Duration::from_secs(30 * 60));
    }

    #[tokio::test]
    async fn a_stored_cadence_is_read_in_minutes() {
        let pool = migrated_pool().await;

        settings::set(&pool, CADENCE_KEY, "10")
            .await
            .expect("write");

        assert_eq!(cadence(&pool).await, Duration::from_secs(10 * 60));
    }

    /// Proved by deleting the `minutes == 0` arm, which leaves `0` clamping
    /// to the floor instead:
    ///
    /// ```text
    /// Some("0") should have fallen back
    ///   left: 300s
    ///  right: 1800s
    /// ```
    #[test]
    fn a_value_that_is_not_a_number_of_minutes_falls_back() {
        // Not clamped to the floor: `0` and `soon` are somebody having got it
        // wrong, and answering a mistake with the most aggressive polling
        // Chief allows would spend their GitHub rate limit on it.
        for stored in [
            None,
            Some(""),
            Some("soon"),
            Some("0"),
            Some("-5"),
            Some("1.5"),
        ] {
            assert_eq!(
                interval_from(stored),
                DEFAULT_INTERVAL,
                "{stored:?} should have fallen back"
            );
        }
    }

    #[test]
    fn a_number_outside_the_range_is_clamped_rather_than_refused() {
        assert_eq!(interval_from(Some("1")), MINIMUM_INTERVAL);
        assert_eq!(interval_from(Some(" 90 ")), Duration::from_secs(90 * 60));
        assert_eq!(interval_from(Some("100000")), MAXIMUM_INTERVAL);
    }

    /// The laptop-lid case, which is the whole reason two clocks are read.
    ///
    /// On Linux `Instant` does not advance across a suspend, so the monotonic
    /// side of a four-hour sleep reads as a few seconds. Without the wall
    /// clock the machine would wake and wait out the rest of the interval on
    /// a log that is four hours stale.
    ///
    /// Proved by dropping the wall-clock half of `is_due`, which no other
    /// test in this module notices:
    ///
    /// ```text
    /// a machine that slept through four passes should run one on waking
    /// ```
    #[test]
    fn a_suspend_the_monotonic_clock_slept_through_still_makes_a_pass_due() {
        let interval = Duration::from_secs(30 * 60);

        assert!(
            is_due(
                interval,
                Duration::from_secs(3),
                Duration::from_secs(4 * 3600)
            ),
            "a machine that slept through four passes should run one on waking"
        );
    }

    /// The other half, and the reason it is `or` rather than `and`.
    ///
    /// A wall clock stepped backwards by NTP, or corrected by hand, reports
    /// less time than has actually passed — and on the reading that requires
    /// both clocks it would postpone the pass for as long as the correction.
    ///
    /// Proved by dropping the monotonic half, likewise alone:
    ///
    /// ```text
    /// a clock correction must not be able to postpone a pass
    /// ```
    #[test]
    fn a_wall_clock_that_went_backwards_cannot_postpone_a_pass() {
        let interval = Duration::from_secs(30 * 60);

        assert!(
            is_due(interval, Duration::from_secs(30 * 60), Duration::ZERO),
            "a clock correction must not be able to postpone a pass"
        );
    }

    #[test]
    fn nothing_is_due_before_either_clock_reaches_the_interval() {
        let interval = Duration::from_secs(30 * 60);

        assert!(!is_due(
            interval,
            Duration::from_secs(29 * 60),
            Duration::from_secs(29 * 60 + 59)
        ));
    }
}
