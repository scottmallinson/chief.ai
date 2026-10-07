//! The model benchmark's runner: the runbook's scenarios, with a real model.
//!
//! `pnpm runbook` mocks the model, so every model passes it identically. This
//! file keeps everything else the runbook uses — the mock GitHub and Graph, the
//! real clients, the real prompt assembly, the real tool loop — and puts one
//! real `llama-server` where the mock model was. What is measured is therefore
//! Chief's own path, not a model on its own.
//!
//! It is an ignored test, driven by `bench/run.mjs`, which owns the models, the
//! scoring and the report. One invocation runs **one model**:
//!
//! * the server is started with `engine::arguments`, Chief's own flags;
//! * a recording proxy sits between Chief and the server. It captures the exact
//!   request Chief built, times the first word and the last, keeps the
//!   server's own `timings`, and sets a `seed` (the repeat number) so the same
//!   five seeds are used for every model;
//! * the server's memory is sampled while it works.
//!
//! Each fixture writes one JSON line to `CHIEF_BENCH_OUT`, carrying its own
//! answer key, so the scorer needs nothing but that file.
//!
//! Nothing here is compiled into the app, and nothing leaves this machine: the
//! only network traffic is loopback.

use super::*;

use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::AtomicU64;
use std::time::Instant;

use futures_util::StreamExt;
use tokio::process::{Child, Command};

use crate::probe::Tier;
use crate::{agent, engine, tools};

// ---------------------------------------------------------------------------
// The recording proxy
// ---------------------------------------------------------------------------

/// One request Chief made to the engine, and what came back.
#[derive(Clone, Default)]
struct Exchange {
    request: Value,
    status: u16,
    response: String,
    first_ms: Option<u64>,
    total_ms: u64,
    timings: Option<Value>,
}

impl Exchange {
    fn to_json(&self) -> Value {
        json!({
            "request": self.request,
            "status": self.status,
            "response": self.response,
            "first_ms": self.first_ms,
            "total_ms": self.total_ms,
            "timings": self.timings,
        })
    }
}

struct Proxy {
    host: String,
    log: Arc<Mutex<Vec<Exchange>>>,
    seed: Arc<AtomicU64>,
}

impl Proxy {
    fn take(&self) -> Vec<Exchange> {
        std::mem::take(&mut *self.log.lock().expect("lock"))
    }
}

/// Read one HTTP request off a socket: method, target and body.
async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<(String, String, String)> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 8192];

    loop {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
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
                let mut first = head.lines().next()?.split_whitespace();
                let method = first.next()?.to_string();
                let target = first.next()?.to_string();
                return Some((method, target, body.to_string()));
            }
        }
    }
}

/// The server's own timing report is the last stream event that carries one.
fn timings_in(response: &str) -> Option<Value> {
    response
        .lines()
        .filter(|line| line.contains("\"timings\""))
        .filter_map(|line| serde_json::from_str::<Value>(line.strip_prefix("data: ")?).ok())
        .filter_map(|event| event.get("timings").cloned())
        .next_back()
}

/// Whether this piece of the stream carries the first word (or the first tool
/// call), which is the moment a person sees anything happen.
fn is_first_word(piece: &str) -> bool {
    piece.contains("\"tool_calls\"")
        || piece
            .split("\"content\":\"")
            .skip(1)
            .any(|rest| !rest.starts_with('"'))
}

async fn proxy(upstream: String) -> Proxy {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let host = format!("http://{}", listener.local_addr().expect("address"));
    let log = Arc::new(Mutex::new(Vec::new()));
    let seed = Arc::new(AtomicU64::new(1));
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("a client");

    let (recorded, seeds) = (log.clone(), seed.clone());
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let (client, upstream) = (client.clone(), upstream.clone());
            let (recorded, seeds) = (recorded.clone(), seeds.clone());

            tokio::spawn(async move {
                let Some((method, target, body)) = read_request(&mut socket).await else {
                    return;
                };

                // The same seeds for every model, so repeats differ from one
                // another and a model's repeat 3 is another model's repeat 3.
                let mut request: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                if let Some(object) = request.as_object_mut() {
                    object
                        .entry("seed")
                        .or_insert(json!(seeds.load(Ordering::SeqCst)));
                }

                let started = Instant::now();
                let url = format!("{upstream}{target}");
                let sent = if method == "GET" {
                    client.get(&url).send().await
                } else {
                    client
                        .post(&url)
                        .header("content-type", "application/json")
                        .body(request.to_string())
                        .send()
                        .await
                };

                let Ok(answer) = sent else {
                    return;
                };
                let status = answer.status().as_u16();
                let kind = answer
                    .headers()
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("application/json")
                    .to_string();

                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
                );
                if socket.write_all(head.as_bytes()).await.is_err() {
                    return;
                }

                let mut exchange = Exchange {
                    request,
                    status,
                    ..Exchange::default()
                };
                let mut stream = answer.bytes_stream();

                while let Some(Ok(piece)) = stream.next().await {
                    let text = String::from_utf8_lossy(&piece).to_string();
                    if exchange.first_ms.is_none() && is_first_word(&text) {
                        exchange.first_ms = Some(started.elapsed().as_millis() as u64);
                    }
                    exchange.response.push_str(&text);

                    let framed = format!("{:x}\r\n", piece.len());
                    if socket.write_all(framed.as_bytes()).await.is_err()
                        || socket.write_all(&piece).await.is_err()
                        || socket.write_all(b"\r\n").await.is_err()
                    {
                        break;
                    }
                }
                let _ = socket.write_all(b"0\r\n\r\n").await;
                let _ = socket.flush().await;

                exchange.total_ms = started.elapsed().as_millis() as u64;
                exchange.timings = timings_in(&exchange.response);
                if method != "GET" {
                    recorded.lock().expect("lock").push(exchange);
                }
            });
        }
    });

    Proxy { host, log, seed }
}

// ---------------------------------------------------------------------------
// The engine, started the way Chief starts it
// ---------------------------------------------------------------------------

struct Server {
    child: Child,
    url: String,
    peak_mb: Arc<AtomicU64>,
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("a free port")
        .local_addr()
        .expect("address")
        .port()
}

/// Resident memory of a process, in mebibytes. `ps` is on macOS and Linux.
fn resident_mb(pid: u32) -> Option<u64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let kilobytes: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;

    Some(kilobytes / 1024)
}

/// Swap in use across the machine, in mebibytes, where the platform says.
fn swap_used_mb() -> Option<u64> {
    if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
        let field = |name: &str| -> Option<u64> {
            text.lines()
                .find(|line| line.starts_with(name))?
                .split_whitespace()
                .nth(1)?
                .parse()
                .ok()
        };
        return Some((field("SwapTotal:")? - field("SwapFree:")?) / 1024);
    }

    let out = std::process::Command::new("sysctl")
        .args(["-n", "vm.swapusage"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let used = text.split("used = ").nth(1)?.split('M').next()?;

    used.trim().parse::<f64>().ok().map(|value| value as u64)
}

impl Server {
    async fn start(
        server: &Path,
        weights: &Path,
        tier: Tier,
        log: &Path,
    ) -> (Self, u64, Vec<String>) {
        let port = free_port();
        let arguments = engine::arguments(weights, port, tier);
        let shown: Vec<String> = arguments
            .iter()
            .map(|argument| argument.to_string_lossy().to_string())
            .collect();

        // Our own join rather than `engine::library_path`: that one hands the
        // inherited value to `join_paths` as a single entry, which refuses it
        // when it already holds several, as cargo's own does.
        let variable = engine::library_path_variable();
        let mut entries: Vec<PathBuf> = server
            .parent()
            .map(|dir| dir.join("lib"))
            .into_iter()
            .collect();
        entries.extend(
            std::env::var_os(variable)
                .iter()
                .flat_map(std::env::split_paths),
        );
        let search = std::env::join_paths(entries).expect("a library path");

        // llama.cpp finds its CPU backends beside the executable it was started
        // from, so where the libraries are kept separately from the server, as
        // they are in the source tree, a copy of the server goes in with them.
        let staged = server
            .parent()
            .map(|dir| dir.join("lib"))
            .filter(|dir| dir.is_dir())
            .map(|dir| {
                dir.join(format!(
                    "bench-{}",
                    server.file_name().unwrap_or_default().to_string_lossy()
                ))
            });
        let server = match staged {
            Some(copy) => {
                std::fs::copy(server, &copy).expect("a copy of the server");
                copy
            }
            None => server.to_path_buf(),
        };

        let started = Instant::now();
        let child = Command::new(&server)
            .args(&arguments)
            .env(variable, search)
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(log).expect("a log file"))
            .kill_on_drop(true)
            .spawn()
            .expect("llama-server should start");

        let url = format!("http://127.0.0.1:{port}");
        let probe = llama::Client::with_base_url(&url).expect("client");
        let mut child = child;
        loop {
            if probe.health().await == llama::Health::Ready {
                break;
            }
            if let Ok(Some(status)) = child.try_wait() {
                let said = std::fs::read_to_string(log).unwrap_or_default();
                panic!("the engine stopped ({status}) before it was ready: {said}");
            }
            assert!(
                started.elapsed().as_secs() < 600,
                "the engine did not become ready in ten minutes; see {}",
                log.display()
            );
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        let load_ms = started.elapsed().as_millis() as u64;

        let peak_mb = Arc::new(AtomicU64::new(0));
        if let Some(pid) = child.id() {
            let peak = peak_mb.clone();
            tokio::spawn(async move {
                while let Some(now) = tokio::task::spawn_blocking(move || resident_mb(pid))
                    .await
                    .ok()
                    .flatten()
                {
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                }
            });
        }

        (
            Self {
                child,
                url,
                peak_mb,
            },
            load_ms,
            shown,
        )
    }

    fn resident_now(&self) -> Option<u64> {
        self.child.id().and_then(resident_mb)
    }
}

// ---------------------------------------------------------------------------
// The fixtures
// ---------------------------------------------------------------------------

/// What one fixture produced, and what a good answer to it looks like.
struct Played {
    answer: Result<String, String>,
    key: Value,
    /// True when Chief answered without the model, so there is nothing to rank.
    routed: bool,
}

fn clock(stamp: &str) -> DateTime<Local> {
    local(&format!("{}T{stamp}:00", today()))
}

fn tomorrow() -> String {
    (Local::now() + Duration::days(1))
        .format("%Y-%m-%d")
        .to_string()
}

fn markdown_of(brief: Result<recipe::Brief, recipe::Error>) -> Result<String, String> {
    brief
        .map(|brief| brief.markdown)
        .map_err(|error| error.to_string())
}

fn names(subjects: &[(&str, &str, &[&str])]) -> Vec<String> {
    subjects
        .iter()
        .map(|(_, subject, _)| (*subject).to_string())
        .collect()
}

/// Every fixture, in the order they run. The IDs are the runbook's where one
/// exists; the rest are named for the situation.
const FIXTURES: &[(&str, &str, &str)] = &[
    // (id, group, where it comes from)
    (
        "em-01",
        "brief-busy",
        "EM-01: fourteen meetings, brief at 08:00",
    ),
    (
        "em-12",
        "brief-busy",
        "EM-12: fourteen meetings and three reviews",
    ),
    ("em-13", "brief-busy", "EM-13: the same day, brief at 14:10"),
    (
        "par-01",
        "brief-quiet",
        "PAR-01: a school day, no work tools",
    ),
    ("par-02", "brief-quiet", "PAR-02: an all-day event"),
    (
        "quiet-mail",
        "brief-quiet",
        "mail only, no meetings: nothing to invent",
    ),
    (
        "em-02",
        "brief-mixed",
        "EM-02: reviews asked of me and my own pull request",
    ),
    (
        "loop-prs",
        "brief-mixed",
        "the repeated pull request case that looped",
    ),
    (
        "eng-standup",
        "standup",
        "ENG-01/09: stand-up after two merges",
    ),
    (
        "eng-standup-empty",
        "standup",
        "stand-up with an empty work log",
    ),
    (
        "tool-open-prs",
        "tools",
        "open pull requests: fetch_github_prs, mine",
    ),
    (
        "tool-reviews",
        "tools",
        "reviews waiting: fetch_github_prs, reviewing",
    ),
    (
        "tool-tomorrow",
        "tools",
        "tomorrow's meetings: fetch_calendar, tomorrow",
    ),
    ("chat-no-tool", "chat", "a question no tool fits"),
];

async fn play(id: &str, engine_url: &str, tier: Tier) -> Played {
    let day = today();

    match id {
        "em-01" | "em-12" | "em-13" => {
            let (subjects, mut world, _mock) = em::full_day().await;
            let all = names(&subjects);
            let mut key = json!({});

            if id == "em-12" {
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
                        pr(
                            43,
                            "acme/web",
                            "Fix focus trap in the modal",
                            None,
                            "2026-10-02T07:10:00Z",
                        ),
                    ]),
                    ..Github::default()
                })
                .await;
                world.ctx.github = github::Client::against(&github.host).expect("client");
                connect(&world.ctx.pool, integrations::GITHUB, "me").await;
            }
            world.ctx.engine = llama::Client::with_base_url(engine_url).expect("client");

            let at = if id == "em-13" { "14:10" } else { "08:00" };
            let brief = recipe::daily_brief_at(&world.ctx, clock(at)).await;

            if id == "em-13" {
                // 14:10: the morning has gone; the window is what is next.
                let next = ["Vendor call", "Budget check-in", "Design critique"];
                let forbid: Vec<&String> = all
                    .iter()
                    .filter(|name| !next.contains(&name.as_str()))
                    .collect();
                key = json!({
                    "must": next, "order": next, "forbid": forbid,
                    "note": "Showing the next 3 of 5 meetings still to come.",
                    "lead_time_bound": true, "max_bullets": 5,
                });
            } else {
                let next = &all[..3];
                let mut must: Vec<String> = next.to_vec();
                if id == "em-12" {
                    must.push("Retry payment webhooks".into());
                    must.push("Drop the legacy flag".into());
                    must.push("Fix focus trap in the modal".into());
                }
                key = json!({
                    "must": must, "order": next, "forbid": &all[3..],
                    "note": "Showing the next 3 of 14 meetings still to come.",
                    "lead_time_bound": true, "max_bullets": 5,
                });
            }

            Played {
                answer: markdown_of(brief),
                key,
                routed: false,
            }
        }

        "par-01" => {
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
            let mut world = world(None, Some(&graph.host), NOWHERE).await;
            world.ctx.engine = llama::Client::with_base_url(engine_url).expect("client");

            let brief = dawn_brief(&world.ctx).await;
            Played {
                answer: markdown_of(brief),
                key: json!({
                    "must": ["breakfast club", "Dentist", "Parents' evening", "Non-uniform day"],
                    "order": ["breakfast club", "Dentist", "Parents' evening"],
                    "forbid": ["pull request", "review", "GitHub", "sprint", "stand-up", "standup"],
                    "lead_time_bound": true, "max_bullets": 5,
                }),
                routed: false,
            }
        }

        "par-02" => {
            let graph = graph_with(vec![all_day("INSET day — school closed", &day)], vec![]).await;
            let mut world = world(None, Some(&graph.host), NOWHERE).await;
            world.ctx.engine = llama::Client::with_base_url(engine_url).expect("client");

            let brief = dawn_brief(&world.ctx).await;
            Played {
                answer: markdown_of(brief),
                key: json!({
                    "must": ["INSET"], "forbid": ["00:00", "midnight", "pull request"],
                    "forbid_times": true, "max_bullets": 5,
                }),
                routed: false,
            }
        }

        "quiet-mail" => {
            let graph = graph_with(
                vec![],
                vec![
                    mail("Oakfield Primary", "Non-uniform day on Friday", true),
                    mail("Bright Energy", "Your October statement is ready", true),
                ],
            )
            .await;
            let mut world = world(None, Some(&graph.host), NOWHERE).await;
            world.ctx.engine = llama::Client::with_base_url(engine_url).expect("client");

            let brief = dawn_brief(&world.ctx).await;
            Played {
                answer: markdown_of(brief),
                key: json!({
                    "must": ["Non-uniform day", "October statement"],
                    "forbid": ["meeting", "call", "appointment", "pull request"],
                    "forbid_times": true, "max_bullets": 5,
                }),
                routed: false,
            }
        }

        "em-02" => {
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
            let mut world = world(Some(&github.host), None, NOWHERE).await;
            world.ctx.engine = llama::Client::with_base_url(engine_url).expect("client");

            let brief = dawn_brief(&world.ctx).await;
            Played {
                answer: markdown_of(brief),
                key: json!({
                    "must": ["Retry payment webhooks", "Drop the legacy flag", "Update on-call policy"],
                    "forbid": ["meeting"], "forbid_times": true, "max_bullets": 5,
                }),
                routed: false,
            }
        }

        "loop-prs" => {
            // Eight of the user's own pull requests with the same title: the
            // shape that made a 4B model write one line eight times over.
            let repeated: Vec<Value> = (1..=8)
                .map(|n| {
                    pr(
                        100 + n,
                        &format!("acme/service-{n}"),
                        "Update dependencies",
                        None,
                        "2026-10-02T07:00:00Z",
                    )
                })
                .collect();
            let github = github_with(Github {
                mine: search(&repeated),
                ..Github::default()
            })
            .await;
            let mut world = world(Some(&github.host), None, NOWHERE).await;
            world.ctx.engine = llama::Client::with_base_url(engine_url).expect("client");

            let brief = dawn_brief(&world.ctx).await;
            Played {
                answer: markdown_of(brief),
                key: json!({
                    "must": ["Update dependencies"], "forbid_times": true, "max_bullets": 5,
                    "no_loop": true,
                }),
                routed: false,
            }
        }

        "eng-standup" | "eng-standup-empty" => {
            let now = Local::now();
            let yesterday = (now - Duration::hours(20)).to_rfc3339();
            let merged = if id == "eng-standup" {
                search(&[
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
                ])
            } else {
                search(&[])
            };
            let github = github_with(Github {
                merged,
                ..Github::default()
            })
            .await;
            // The work log is filled by the daemon's real pass, with a plain
            // mock where the model would be: the benchmark is of the stand-up,
            // not of the summary of each merge.
            let filler = model_saying("- Shipped the change.").await;
            let world = world(Some(&github.host), None, &filler.host).await;
            daemon::run_once(&world.daemon()).await.expect("a pass");

            let context = tools::Context {
                pool: world.ctx.pool.clone(),
                github: world.ctx.github.clone(),
                microsoft: world.ctx.microsoft.clone(),
            };
            let client = llama::Client::with_base_url(engine_url).expect("client");
            let answer = agent::answer_for_bench(&client, &context, tier, "Draft my standup")
                .await
                .map_err(|error| error.to_string());

            let key = if id == "eng-standup" {
                json!({
                    "must": ["Cache the entitlement lookup", "Fix focus trap in the modal"],
                    "standup": true, "no_tool": true, "forbid_times": true, "max_words": 120,
                })
            } else {
                // Nothing logged: any pull request number or title is invented.
                json!({
                    "must": ["nothing"], "standup": true, "no_tool": true,
                    "forbid_times": true, "forbid_numbers": true, "max_words": 80,
                })
            };
            Played {
                answer,
                key,
                routed: false,
            }
        }

        "tool-open-prs" | "tool-reviews" => {
            let github = github_with(Github {
                mine: search(&[
                    pr(
                        90,
                        "acme/api",
                        "Paginate the audit log",
                        None,
                        "2026-10-02T08:00:00Z",
                    ),
                    pr(
                        91,
                        "acme/web",
                        "Add dark mode toggle",
                        None,
                        "2026-10-02T08:30:00Z",
                    ),
                ]),
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
                ..Github::default()
            })
            .await;
            let world = world(Some(&github.host), None, NOWHERE).await;
            let context = tools::Context {
                pool: world.ctx.pool.clone(),
                github: world.ctx.github.clone(),
                microsoft: world.ctx.microsoft.clone(),
            };
            let client = llama::Client::with_base_url(engine_url).expect("client");

            let (question, key) = if id == "tool-open-prs" {
                (
                    "Which pull requests of mine are still open?",
                    json!({
                        "tool": { "name": "fetch_github_prs", "args": { "state": ["open", "all"], "whose": ["mine"] } },
                        "must": ["Paginate the audit log", "Add dark mode toggle"],
                        "forbid": ["Retry payment webhooks"],
                    }),
                )
            } else {
                (
                    "Which pull requests are waiting for my review?",
                    json!({
                        "tool": { "name": "fetch_github_prs", "args": { "state": ["open", "all"], "whose": ["reviewing"] } },
                        "must": ["Retry payment webhooks", "Drop the legacy flag"],
                        "forbid": ["Paginate the audit log"],
                    }),
                )
            };

            let routed = intent::route(question).is_some();
            let answer = if routed {
                Err("routed".to_string())
            } else {
                agent::answer_for_bench(&client, &context, tier, question)
                    .await
                    .map_err(|error| error.to_string())
            };
            Played {
                answer,
                key,
                routed,
            }
        }

        "tool-tomorrow" => {
            let next = tomorrow();
            let graph = graph_with(
                vec![
                    meeting(
                        "Quarterly planning",
                        &at(&next, "10:00"),
                        &at(&next, "11:00"),
                        &["Product"],
                    ),
                    meeting(
                        "1:1 with Priya",
                        &at(&next, "15:00"),
                        &at(&next, "15:30"),
                        &["Priya"],
                    ),
                ],
                vec![],
            )
            .await;
            let world = world(None, Some(&graph.host), NOWHERE).await;
            let context = tools::Context {
                pool: world.ctx.pool.clone(),
                github: world.ctx.github.clone(),
                microsoft: world.ctx.microsoft.clone(),
            };
            let client = llama::Client::with_base_url(engine_url).expect("client");
            let question = "What meetings do I have tomorrow?";

            let routed = intent::route(question).is_some();
            let answer = if routed {
                Err("routed".to_string())
            } else {
                agent::answer_for_bench(&client, &context, tier, question)
                    .await
                    .map_err(|error| error.to_string())
            };
            Played {
                answer,
                key: json!({
                    "tool": { "name": "fetch_calendar", "args": { "range": ["tomorrow"] } },
                    "must": ["Quarterly planning", "1:1 with Priya"],
                    "order": ["Quarterly planning", "1:1 with Priya"],
                }),
                routed,
            }
        }

        "chat-no-tool" => {
            let world = world(None, None, NOWHERE).await;
            let context = tools::Context {
                pool: world.ctx.pool.clone(),
                github: world.ctx.github.clone(),
                microsoft: world.ctx.microsoft.clone(),
            };
            let client = llama::Client::with_base_url(engine_url).expect("client");
            let answer = agent::answer_for_bench(
                &client,
                &context,
                tier,
                "Write me a short thank-you note for a colleague who covered my on-call shift.",
            )
            .await
            .map_err(|error| error.to_string());
            Played {
                answer,
                key: json!({ "no_tool": true, "must": ["thank"], "forbid_numbers": true, "max_words": 120 }),
                routed: false,
            }
        }

        other => panic!("no fixture called {other}"),
    }
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

fn setting(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Run every fixture, `CHIEF_BENCH_REPEATS` times each, against the model in
/// `CHIEF_BENCH_WEIGHTS`, writing one JSON line per run to `CHIEF_BENCH_OUT`.
///
/// Ignored: it needs a model and takes minutes to hours. `bench/run.mjs` sets
/// the variables and runs it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a model on disk; run it with `pnpm bench`"]
async fn run_the_benchmark() {
    use std::io::Write;

    let weights = PathBuf::from(setting("CHIEF_BENCH_WEIGHTS").expect("CHIEF_BENCH_WEIGHTS"));
    let server = PathBuf::from(setting("CHIEF_BENCH_SERVER").expect("CHIEF_BENCH_SERVER"));
    let out = PathBuf::from(setting("CHIEF_BENCH_OUT").expect("CHIEF_BENCH_OUT"));
    let repeats: u64 = setting("CHIEF_BENCH_REPEATS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let only = setting("CHIEF_BENCH_ONLY");
    let tier = match setting("CHIEF_BENCH_TIER").as_deref() {
        Some("standard") => Tier::Standard,
        _ => Tier::Light,
    };

    let mut file = std::fs::File::create(&out).expect("the output file");
    let mut line = |value: Value| {
        writeln!(file, "{value}").expect("write");
        file.flush().expect("flush");
    };

    let swap_before = swap_used_mb();
    let (engine, load_ms, flags) =
        Server::start(&server, &weights, tier, &out.with_extension("engine.log")).await;
    let proxy = proxy(engine.url.clone()).await;

    let mut system = sysinfo::System::new();
    system.refresh_memory();
    line(json!({
        "type": "env",
        "tier": tier.as_str(),
        "flags": flags,
        "load_ms": load_ms,
        "resident_after_load_mb": engine.resident_now(),
        "swap_used_mb_before": swap_before,
        "machine_memory_mb": system.total_memory() / (1024 * 1024),
        "machine_cores": std::thread::available_parallelism().map(usize::from).unwrap_or(0),
        "repeats": repeats,
    }));

    let mut first = true;
    for (id, group, source) in FIXTURES {
        if only.as_deref().is_some_and(|only| !id.contains(only)) {
            continue;
        }

        for repeat in 0..repeats {
            proxy.seed.store(repeat + 1, Ordering::SeqCst);
            proxy.take();
            engine
                .peak_mb
                .store(engine.resident_now().unwrap_or(0), Ordering::SeqCst);
            let swap_start = swap_used_mb();

            let started = Instant::now();
            let played = play(id, &proxy.host, tier).await;
            let wall_ms = started.elapsed().as_millis() as u64;
            let exchanges = proxy.take();

            let (answer, error) = match played.answer {
                Ok(text) => (text, Value::Null),
                Err(error) => (String::new(), json!(error)),
            };
            eprintln!(
                "{id} #{repeat}: {wall_ms} ms, {} chars{}",
                answer.chars().count(),
                if error.is_null() {
                    String::new()
                } else {
                    format!(", error {error}")
                }
            );

            line(json!({
                "type": "run",
                "fixture": id,
                "group": group,
                "source": source,
                "repeat": repeat,
                "seed": repeat + 1,
                "phase": if first { "cold" } else { "warm" },
                "routed": played.routed,
                "answer": answer,
                "error": error,
                "key": played.key,
                "wall_ms": wall_ms,
                "peak_resident_mb": engine.peak_mb.load(Ordering::SeqCst),
                "swap_used_mb_start": swap_start,
                "swap_used_mb_end": swap_used_mb(),
                "exchanges": exchanges.iter().map(Exchange::to_json).collect::<Vec<_>>(),
            }));
            first = false;
        }
    }
}
