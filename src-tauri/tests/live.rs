//! The live check (release gate): one small fixed task, run by the real
//! `DirectEngine` and the real task loop against real OpenRouter models.
//!
//! Ignored, and skipped with "no key" when no key is given. The key comes
//! from `OPENROUTER_API_KEY` or, when that is unset, from the first line of
//! the file named by `OPENROUTER_API_KEY_FILE`. It is held in memory only and
//! replaced by `[key]` in everything this test prints or saves.
//!
//! ```text
//! OPENROUTER_API_KEY_FILE="$HOME/.beluga-live-key" cargo test \
//!   --manifest-path src-tauri/Cargo.toml --test live -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Cards are answered by `answer_for`: only the task's own edit and script
//! are approved; every other command, every secret file and every edit of
//! another file is declined. Both key variables are removed from the
//! process environment as soon as the key is read.
//!
//! Every request the app makes goes through a small local relay that
//! enforces the budget below before anything reaches OpenRouter, records each
//! exchange's bodies (never its headers) and passes the stream through
//! unchanged. The recordings land in a run folder under the cargo target
//! directory; nothing is written into the source tree.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use beluga_desktop_lib::agent::{Answer, Approval};
use beluga_desktop_lib::commands::{chats, turn};
use beluga_desktop_lib::dialogs::Dialogs;
use beluga_desktop_lib::engine::EngineEvent;
use beluga_desktop_lib::openrouter::key_info::{self, KeyStatus};
use beluga_desktop_lib::openrouter::models::{self, ModelCache, ModelInfo};
use beluga_desktop_lib::secrets::{KeyStore, SessionStore};
use beluga_desktop_lib::state::{AppState, EventSink, HttpClients, HttpTimeouts};
use beluga_desktop_lib::store::{Store, StoredMessage};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const OPENROUTER: &str = "https://openrouter.ai";

// ---- the budget, enforced by the relay ----

/// Chat requests one model may make for the fixed task.
const MAX_REQUESTS_PER_MODEL: u32 = 12;
/// Chat requests across every run of this test (kept in a ledger file under
/// the target directory, so re-runs count too).
const MAX_REQUESTS_TOTAL: u32 = 60;
/// Reported cost across every run of this test.
const MAX_COST_USD: f64 = 0.50;
/// Stop when the key's remaining limit (if it has one) falls below this.
const MIN_LIMIT_REMAINING_USD: f64 = 0.50;
/// Least time between two chat requests (free models allow 20 a minute).
const SPACING: Duration = Duration::from_secs(4);
/// Longest one model's task may take before it is stopped.
const MODEL_TIMEOUT: Duration = Duration::from_secs(600);
/// Candidates looked at per group (free, paid) before giving up on finding
/// two that have an endpoint.
const MAX_CANDIDATES: usize = 5;

// ---- the fixed task ----

const TASK: &str =
    "Read notes.txt, change alpha to beta, run the count script, and tell me what it printed.";
const SCRIPT_OUTPUT: &str = "lines: 3";

/// Reads the key, then removes both variables from this process's
/// environment, so nothing started later (the app's commands included) can
/// find the key or the file that holds it. The key stays in memory only.
///
/// Must run before any runtime or thread of the test exists: removing a
/// variable is `unsafe` (edition 2024) because another thread reading the
/// environment at the same moment is undefined behaviour.
fn take_key() -> Option<String> {
    let from_env = std::env::var("OPENROUTER_API_KEY")
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty());
    let file = std::env::var_os("OPENROUTER_API_KEY_FILE");
    // SAFETY: called first thing in the live test, before its runtime and
    // threads exist; the other tests in this file never read the
    // environment, and the documented run uses --test-threads=1.
    unsafe {
        std::env::remove_var("OPENROUTER_API_KEY");
        std::env::remove_var("OPENROUTER_API_KEY_FILE");
    }
    if from_env.is_some() {
        return from_env;
    }
    let text = std::fs::read_to_string(file?).ok()?;
    let k = text.lines().next()?.trim().to_string();
    (!k.is_empty()).then_some(k)
}

/// Replaces the key with `[key]` in anything printed or saved.
#[derive(Clone)]
struct Scrub(Arc<String>);

impl Scrub {
    fn text(&self, s: &str) -> String {
        mask_account(&s.replace(self.0.as_str(), "[key]"))
    }

    fn say(&self, s: impl AsRef<str>) {
        println!("{}", self.text(s.as_ref()));
    }
}

/// Replaces each `prefix` followed by at least `min` characters that `keep`
/// accepts with `prefix` + `with`.
fn mask_runs(s: &str, prefix: &str, min: usize, keep: fn(char) -> bool, with: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find(prefix) {
        let after = &rest[at + prefix.len()..];
        let run = after.find(|c: char| !keep(c)).unwrap_or(after.len());
        out.push_str(&rest[..at + prefix.len()]);
        if after[..run].chars().count() >= min {
            out.push_str(with);
        } else {
            out.push_str(&after[..run]);
        }
        rest = &after[run..];
    }
    out.push_str(rest);
    out
}

/// Removes what identifies the account from OpenRouter's answers: user ids
/// (`user_…`, also inside `creator_user_id`), the key's hash in links, and
/// the key's label (OpenRouter's own masked form of the key).
fn mask_account(s: &str) -> String {
    let s = mask_runs(s, "user_", 16, |c| c.is_ascii_alphanumeric(), "[account]");
    let s = mask_runs(&s, "/keys/", 16, |c| c.is_ascii_hexdigit(), "[key-hash]");
    let s = mask_runs(&s, "\"label\":\"", 1, |c| c != '"' && c != '\\', "[label]");
    mask_runs(
        &s,
        "\\\"label\\\":\\\"",
        1,
        |c| c != '"' && c != '\\',
        "[label]",
    )
}

/// Replaces generation ids, fingerprints and tool call ids in a recorded
/// stream with fixed placeholders (the same original always gets the same
/// placeholder), leaving every other byte as it came.
fn with_placeholders(raw: &str) -> String {
    let mut map: Vec<(String, String)> = Vec::new();
    let mut calls = 0;
    let add = |map: &mut Vec<(String, String)>, from: &str, to: String| {
        if !from.is_empty() && !map.iter().any(|(f, _)| f == from) {
            map.push((from.to_string(), to));
        }
    };
    for line in raw.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(data.trim()) else {
            continue;
        };
        if let Some(id) = v["id"].as_str() {
            add(&mut map, id, "gen-live".to_string());
        }
        if let Some(fp) = v["system_fingerprint"].as_str() {
            add(&mut map, fp, "fp-live".to_string());
        }
        for choice in v["choices"].as_array().into_iter().flatten() {
            for tc in choice["delta"]["tool_calls"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if let Some(id) = tc["id"].as_str()
                    && !map.iter().any(|(f, _)| f == id)
                {
                    calls += 1;
                    add(&mut map, id, format!("call_live_{calls}"));
                }
            }
        }
    }
    map.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
    let mut out = raw.to_string();
    for (from, to) in map {
        out = out.replace(&format!("\"{from}\""), &format!("\"{to}\""));
    }
    out
}

/// The cost a recorded stream reports, summed over its usage chunks.
fn stream_cost(body: &str) -> f64 {
    body.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
        .filter_map(|v| v["usage"]["cost"].as_f64())
        .sum()
}

// ---- the ledger: totals across runs ----

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Ledger {
    requests: u32,
    cost_usd: f64,
}

fn ledger_path() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("live-ledger.json")
}

impl Ledger {
    fn load() -> Self {
        std::fs::read_to_string(ledger_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let _ = std::fs::write(ledger_path(), serde_json::to_string(self).unwrap());
    }
}

// ---- the relay ----

struct Exchange {
    model: String,
    method: String,
    path: String,
    status: u16,
    request: String,
    response: String,
}

struct Relay {
    ledger: Ledger,
    run_requests: u32,
    run_cost: f64,
    /// The model whose task is running, and its counts.
    model: String,
    model_requests: u32,
    model_cost: f64,
    next_allowed: Option<Instant>,
    exchanges: Vec<Exchange>,
    /// A free model answered 429 because the day's free allowance is gone.
    free_daily_gone: bool,
    /// Why the relay refused a request, if it did.
    refused: Option<String>,
}

impl Relay {
    /// Counts a chat request, or says why it may not go out. On admission,
    /// returns how long to wait first.
    fn admit(&mut self, model: &str) -> Result<Duration, String> {
        let why = if model != self.model {
            Some(format!("request for {model} while {} runs", self.model))
        } else if self.model_requests >= MAX_REQUESTS_PER_MODEL {
            Some(format!(
                "{MAX_REQUESTS_PER_MODEL} requests for this model reached"
            ))
        } else if self.ledger.requests >= MAX_REQUESTS_TOTAL {
            Some(format!("{MAX_REQUESTS_TOTAL} requests in total reached"))
        } else if self.ledger.cost_usd > MAX_COST_USD {
            Some(format!("${MAX_COST_USD:.2} reported cost reached"))
        } else {
            None
        };
        if let Some(why) = why {
            self.refused = Some(why.clone());
            return Err(format!("live test budget: {why}"));
        }
        self.model_requests += 1;
        self.run_requests += 1;
        self.ledger.requests += 1;
        self.ledger.save();
        let now = Instant::now();
        let at = self.next_allowed.map_or(now, |t| t.max(now));
        self.next_allowed = Some(at + SPACING);
        Ok(at - now)
    }

    fn add_cost(&mut self, cost: f64) {
        self.model_cost += cost;
        self.run_cost += cost;
        self.ledger.cost_usd += cost;
        self.ledger.save();
    }
}

type Shared = Arc<Mutex<Relay>>;

/// Starts the relay; returns its origin (`http://127.0.0.1:port`).
async fn start_relay(relay: Shared) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let upstream = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(120))
        .build()
        .unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let (relay, upstream) = (relay.clone(), upstream.clone());
            tokio::spawn(async move {
                let _ = serve(sock, relay, upstream).await;
            });
        }
    });
    origin
}

struct Incoming {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

async fn read_request(sock: &mut TcpStream) -> std::io::Result<Incoming> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        let n = sock.read(&mut chunk).await?;
        if n == 0 || buf.len() > 1_000_000 {
            return Err(std::io::Error::other("bad request head"));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let method = first.next().unwrap_or_default().to_string();
    let path = first.next().unwrap_or_default().to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    if length > 50_000_000 {
        return Err(std::io::Error::other("request too large"));
    }
    let mut body = buf[head_end..].to_vec();
    while body.len() < length {
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Ok(Incoming {
        method,
        path,
        headers,
        body,
    })
}

async fn write_head(
    sock: &mut TcpStream,
    status: u16,
    content_type: &str,
    retry_after: Option<&str>,
) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status} {}\r\ncontent-type: {content_type}\r\nconnection: close\r\n",
        reqwest::StatusCode::from_u16(status)
            .ok()
            .and_then(|s| s.canonical_reason())
            .unwrap_or("Status")
    );
    if let Some(r) = retry_after {
        head.push_str(&format!("retry-after: {r}\r\n"));
    }
    head.push_str("\r\n");
    sock.write_all(head.as_bytes()).await
}

/// One request: checked against the budget, passed on with only the headers
/// OpenRouter needs, the response streamed back as it arrives (the body ends
/// when the connection closes) and recorded.
async fn serve(
    mut sock: TcpStream,
    relay: Shared,
    upstream: reqwest::Client,
) -> std::io::Result<()> {
    let req = read_request(&mut sock).await?;
    let is_chat = req.method == "POST" && req.path.ends_with("/chat/completions");
    let request_text = String::from_utf8_lossy(&req.body).to_string();
    let model = serde_json::from_str::<Value>(&request_text)
        .ok()
        .and_then(|v| v["model"].as_str().map(str::to_string))
        .unwrap_or_default();
    let record = |status: u16, response: String| {
        let mut r = relay.lock().unwrap();
        let model = if model.is_empty() {
            r.model.clone()
        } else {
            model.clone()
        };
        r.exchanges.push(Exchange {
            model,
            method: req.method.clone(),
            path: req.path.clone(),
            status,
            request: request_text.clone(),
            response,
        });
    };
    if is_chat {
        let admitted = relay.lock().unwrap().admit(&model);
        match admitted {
            Ok(wait) => tokio::time::sleep(wait).await,
            Err(why) => {
                let body = json!({"error": {"message": why, "code": 400}}).to_string();
                write_head(&mut sock, 400, "application/json", None).await?;
                sock.write_all(body.as_bytes()).await?;
                record(400, body);
                return Ok(());
            }
        }
    }
    let method = reqwest::Method::from_bytes(req.method.as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut out = upstream.request(method, format!("{OPENROUTER}{}", req.path));
    for (k, v) in &req.headers {
        if matches!(
            k.as_str(),
            "authorization" | "content-type" | "x-title" | "http-referer" | "accept"
        ) {
            out = out.header(k, v);
        }
    }
    if !req.body.is_empty() {
        out = out.body(req.body.clone());
    }
    let resp = match out.send().await {
        Ok(r) => r,
        Err(e) => {
            let body =
                json!({"error": {"message": format!("relay: {e}"), "code": 502}}).to_string();
            write_head(&mut sock, 502, "application/json", None).await?;
            sock.write_all(body.as_bytes()).await?;
            record(502, body);
            return Ok(());
        }
    };
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    write_head(&mut sock, status, &content_type, retry_after.as_deref()).await?;
    let mut collected = Vec::new();
    let mut bytes = resp.bytes_stream();
    let mut write_failed = false;
    while let Some(chunk) = bytes.next().await {
        let Ok(chunk) = chunk else { break };
        collected.extend_from_slice(&chunk);
        if !write_failed && sock.write_all(&chunk).await.is_err() {
            // The app hung up (Stop); keep reading so the cost is recorded.
            write_failed = true;
        }
    }
    let _ = sock.shutdown().await;
    let body = String::from_utf8_lossy(&collected).to_string();
    if is_chat {
        let mut r = relay.lock().unwrap();
        r.add_cost(stream_cost(&body));
        if status == 429 && body.contains("per-day") {
            r.free_daily_gone = true;
        }
    }
    record(status, body);
    Ok(())
}

// ---- the app ----

struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _: &str, _: Value) {}
}

struct NoDialogs;
impl Dialogs for NoDialogs {
    fn pick_folder(&self) -> Option<PathBuf> {
        None
    }
    fn save_markdown(&self, _: &str) -> Option<PathBuf> {
        None
    }
    fn pick_zip(&self) -> Option<PathBuf> {
        None
    }
}

/// The project folder of the fixed task.
fn make_project(dir: &Path) {
    std::fs::write(dir.join("notes.txt"), "alpha\n").unwrap();
    std::fs::write(
        dir.join("count.sh"),
        format!("#!/bin/sh\necho \"{SCRIPT_OUTPUT}\"\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("count.ps1"),
        format!("Write-Output \"{SCRIPT_OUTPUT}\"\n"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join("count.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
}

/// What one model's run showed.
#[derive(Debug, Default, serde::Serialize)]
struct Outcome {
    model: String,
    group: String,
    skipped: bool,
    passed: bool,
    turn: String,
    requests: u32,
    cost_usd: f64,
    wall_secs: f64,
    tools: Vec<String>,
    failed_calls: Vec<String>,
    /// Each card shown and the answer it got.
    cards: Vec<String>,
    problems: Vec<String>,
    final_reply: String,
    file_after: String,
}

/// The note sent with every card this test declines.
const ONLY_THE_TASK: &str = "This test only allows the task's own script.";

/// The commands the fixed task needs: its script run directly or through
/// `sh`, `bash` or `pwsh` (with an optional leading `./`), a plain listing,
/// or reading one of the project's three files. Nothing with shell syntax
/// beyond plain words is allowed.
fn task_command(text: &str) -> bool {
    const FILES: [&str; 3] = ["notes.txt", "count.sh", "count.ps1"];
    if text
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || " ./-_".contains(c)))
    {
        return false;
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    let file = |w: &str| FILES.contains(&w.strip_prefix("./").unwrap_or(w));
    let script = |w: &str| matches!(w.strip_prefix("./").unwrap_or(w), "count.sh" | "count.ps1");
    match words.as_slice() {
        [w] => script(w) || matches!(*w, "ls" | "dir"),
        ["sh" | "bash" | "pwsh", w] => script(w),
        ["ls", "-l" | "-a" | "-la" | "-al"] => true,
        ["cat" | "type" | "Get-Content", w] => file(w),
        _ => false,
    }
}

/// How this test answers a card: an edit only of `notes.txt`; a command only
/// when it is one of the task's own (`task_command`), inside the project and
/// not in the background; never a secret file; a keep-going card stops.
/// Everything else is declined with `ONLY_THE_TASK`.
fn answer_for(card: &Approval) -> Answer {
    let decline = || Answer::Decline {
        note: Some(ONLY_THE_TASK.to_string()),
    };
    match card {
        Approval::Edit { path, .. } if path == "notes.txt" => Answer::Once,
        Approval::Command {
            command,
            outside: false,
            background: false,
            ..
        } if task_command(command) => Answer::Once,
        Approval::KeepGoing { .. } => Answer::StopHere,
        _ => decline(),
    }
}

/// One line for the outcome: the card and the answer it got.
fn card_line(card: &Approval, answer: &Answer) -> String {
    let given = match answer {
        Answer::Once => "once",
        Answer::StopHere => "stop here",
        Answer::Decline { .. } => "declined",
        _ => "other",
    };
    let what = match card {
        Approval::Command {
            command,
            outside,
            background,
            ..
        } => format!("command {command:?} (outside: {outside}, background: {background})"),
        Approval::Edit { path, .. } => format!("edit {path}"),
        Approval::Secret { path, .. } => format!("secret {path}"),
        Approval::KeepGoing { requests, .. } => format!("keep going after {requests}"),
    };
    format!("{what} -> {given}")
}

/// Every call has exactly one result, and each reply's results come right
/// after it, before any other row.
fn well_formed(rows: &[StoredMessage]) -> Result<(), String> {
    let mut i = 0;
    while i < rows.len() {
        let r = &rows[i];
        if r.kind == "tool" {
            return Err(format!("result row {i} answers no open call"));
        }
        i += 1;
        let mut open: Vec<&str> = r.tool_calls.iter().map(|c| c.id.as_str()).collect();
        while !open.is_empty() {
            let Some(t) = rows.get(i).filter(|t| t.kind == "tool") else {
                return Err(format!("calls {open:?} have no result before row {i}"));
            };
            let id = t.tool_call_id.as_deref().unwrap_or_default();
            let Some(at) = open.iter().position(|o| *o == id) else {
                return Err(format!("result row {i} answers {id}, not an open call"));
            };
            open.remove(at);
            i += 1;
        }
    }
    Ok(())
}

async fn run_model(
    scrub: &Scrub,
    key: &str,
    origin: &str,
    relay: &Shared,
    all_models: &[ModelInfo],
    model: &ModelInfo,
    group: &str,
) -> Outcome {
    let mut o = Outcome {
        model: model.id.clone(),
        group: group.to_string(),
        ..Outcome::default()
    };
    {
        let mut r = relay.lock().unwrap();
        r.model = model.id.clone();
        r.model_requests = 0;
        r.model_cost = 0.0;
        r.refused = None;
    }
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    make_project(project.path());
    let keys = SessionStore::default();
    keys.set(key).unwrap();
    let http = HttpClients::new(HttpTimeouts::PRODUCTION).unwrap();
    std::fs::create_dir_all(data.path().join("app")).unwrap();
    std::fs::create_dir_all(data.path().join("builtin")).unwrap();
    let state = Arc::new(AppState::new(
        http,
        origin.to_string(),
        Arc::new(keys),
        Store::in_memory().unwrap(),
        data.path().join("app"),
        data.path().join("builtin"),
        Arc::new(NullSink),
        Arc::new(NoDialogs),
    ));
    ModelCache::save(&state.model_cache_file(), all_models).unwrap();
    let chat = state.store.create_chat(&model.id).unwrap().id;
    let root = std::fs::canonicalize(project.path()).unwrap();
    let root = root.to_str().unwrap().to_string();
    state.store.approve_folder(&root).unwrap();
    state.store.set_folder(&chat, Some(&root)).unwrap();

    let events: Arc<Mutex<Vec<EngineEvent>>> = Arc::default();
    let started = Instant::now();
    let send = {
        let (state, chat, model_id, events) = (
            state.clone(),
            chat.clone(),
            model.id.clone(),
            events.clone(),
        );
        tokio::spawn(async move {
            let emit = move |e: EngineEvent| events.lock().unwrap().push(e);
            turn::send_message(&state, &chat, TASK, vec![], &model_id, &emit).await
        })
    };
    let mut answered: Vec<String> = Vec::new();
    let mut stopped = false;
    while !send.is_finished() {
        if let Some(card) = state.approvals.waiting(&chat)
            && !answered.iter().any(|a| a == card.id())
        {
            let answer = answer_for(&card);
            if state
                .approvals
                .answer(&chat, card.id(), answer.clone())
                .is_ok()
            {
                answered.push(card.id().to_string());
                o.cards.push(card_line(&card, &answer));
            }
        }
        let waiting = events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, EngineEvent::Waiting { .. }));
        let free_gone = relay.lock().unwrap().free_daily_gone;
        if !stopped && ((waiting && free_gone) || started.elapsed() > MODEL_TIMEOUT) {
            o.problems.push(if free_gone {
                "stopped: the free daily allowance is used up".to_string()
            } else {
                "stopped: the task ran past the time limit".to_string()
            });
            turn::stop(&state, &chat);
            stopped = true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    o.wall_secs = started.elapsed().as_secs_f64();
    match send.await.unwrap() {
        Ok(result) => o.turn = format!("{result:?}"),
        Err(e) => o.turn = format!("refused: {e:?}"),
    }
    {
        let r = relay.lock().unwrap();
        o.requests = r.model_requests;
        o.cost_usd = r.model_cost;
        if let Some(why) = &r.refused {
            o.problems.push(format!("relay refused a request: {why}"));
        }
    }

    let rows = chats::get_messages(&state, &chat).unwrap();
    for r in &rows {
        for c in &r.tool_calls {
            o.tools.push(c.name.clone());
        }
        if r.kind == "tool"
            && let Some(meta) = &r.tool_meta
            && meta.status != "ok"
        {
            let text: String = r.text.chars().take(300).collect();
            o.failed_calls
                .push(format!("{} [{}]: {}", meta.name, meta.status, text));
        }
    }
    o.file_after = std::fs::read_to_string(project.path().join("notes.txt")).unwrap_or_default();
    o.final_reply = rows
        .iter()
        .rev()
        .find(|r| r.role == "assistant" && r.kind == "message" && r.tool_calls.is_empty())
        .map(|r| r.text.clone())
        .unwrap_or_default();

    // The model's first chat answer. A model this account cannot use is
    // skipped and the next candidate taken: no endpoint (404, "no endpoints
    // found"), an endpoint closed to this app (403) or more credit needed
    // than the account has (402).
    let first = {
        let r = relay.lock().unwrap();
        r.exchanges
            .iter()
            .find(|x| x.model == model.id && x.path.ends_with("/chat/completions"))
            .map(|x| (x.status, x.response.clone()))
    };
    if let Some((status, body)) = &first
        && (matches!(status, 402..=404) || body.to_ascii_lowercase().contains("no endpoints found"))
        && o.tools.is_empty()
    {
        o.skipped = true;
        o.problems.push(format!(
            "could not be used ({status}): {}",
            scrub.text(&body.chars().take(400).collect::<String>())
        ));
        return o;
    }

    if !o.file_after.contains("beta") {
        o.problems
            .push("notes.txt does not contain beta".to_string());
    }
    if !o.final_reply.contains(SCRIPT_OUTPUT) {
        o.problems.push(format!(
            "the final reply does not contain \"{SCRIPT_OUTPUT}\""
        ));
    }
    if o.requests > MAX_REQUESTS_PER_MODEL {
        o.problems.push(format!("{} requests", o.requests));
    }
    if let Err(e) = well_formed(&rows) {
        o.problems.push(format!("stored conversation: {e}"));
    }
    o.passed = o.file_after.contains("beta")
        && o.final_reply.contains(SCRIPT_OUTPUT)
        && o.requests <= MAX_REQUESTS_PER_MODEL
        && well_formed(&rows).is_ok();
    o
}

/// Free models by context, largest first; paid ones by prompt price,
/// cheapest first; ties by id. Tool-capable only.
fn candidates(models: &[ModelInfo]) -> (Vec<ModelInfo>, Vec<ModelInfo>) {
    let tools = models.iter().filter(|m| m.supports_tools);
    let mut free: Vec<ModelInfo> = tools.clone().filter(|m| m.is_free).cloned().collect();
    let mut paid: Vec<ModelInfo> = tools.filter(|m| !m.is_free).cloned().collect();
    free.sort_by(|a, b| {
        b.context_length
            .cmp(&a.context_length)
            .then_with(|| a.id.cmp(&b.id))
    });
    paid.sort_by(|a, b| {
        a.prompt_usd_per_mtok
            .total_cmp(&b.prompt_usd_per_mtok)
            .then_with(|| a.id.cmp(&b.id))
    });
    (free, paid)
}

fn slug(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

async fn key_report(
    scrub: &Scrub,
    http: &reqwest::Client,
    key: &str,
    when: &str,
) -> Option<KeyStatus> {
    match key_info::key_status(http, &format!("{OPENROUTER}/api/v1"), key).await {
        Ok(k) => {
            scrub.say(format!(
                "key {when}: usage ${:.6}, limit remaining {:?}, free tier {}, free daily {:?}",
                k.usage_usd, k.limit_remaining_usd, k.is_free_tier, k.free_daily
            ));
            Some(k)
        }
        Err(e) => {
            scrub.say(format!("key {when}: could not be read: {e:?}"));
            None
        }
    }
}

fn low_limit(k: &Option<KeyStatus>) -> bool {
    k.as_ref()
        .and_then(|k| k.limit_remaining_usd)
        .is_some_and(|l| l < MIN_LIMIT_REMAINING_USD)
}

#[test]
#[ignore = "live: needs the owner's OpenRouter key and the network"]
fn live_fixed_task_on_four_models() {
    // The key is read and its variables cleared before the runtime starts.
    let Some(key) = take_key() else {
        println!("no key: set OPENROUTER_API_KEY or OPENROUTER_API_KEY_FILE to run the live check");
        return;
    };
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap()
        .block_on(live_run(key));
}

async fn live_run(key: String) {
    let scrub = Scrub(Arc::new(key.clone()));
    let api = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    let ledger = Ledger::load();
    scrub.say(format!(
        "ledger before: {} requests, ${:.6} in earlier runs ({})",
        ledger.requests,
        ledger.cost_usd,
        ledger_path().display()
    ));
    if ledger.requests >= MAX_REQUESTS_TOTAL || ledger.cost_usd > MAX_COST_USD {
        scrub.say("the budget for this session is used up; nothing is sent");
        return;
    }

    let before = key_report(&scrub, &api, &key, "before").await;
    if before.is_none() {
        panic!("the key could not be checked; stopping before any chat request");
    }
    if low_limit(&before) {
        scrub.say("the key's remaining limit is below $0.50; nothing is sent");
        return;
    }

    let all = models::list_models(&api, &format!("{OPENROUTER}/api/v1"))
        .await
        .expect("the model list");
    let (free, paid) = candidates(&all);
    scrub.say(format!(
        "{} models listed, {} free and {} paid accept tools",
        all.len(),
        free.len(),
        paid.len()
    ));

    let run_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "live-run-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    ));
    std::fs::create_dir_all(&run_dir).unwrap();
    scrub.say(format!("recordings: {}", run_dir.display()));

    let relay: Shared = Arc::new(Mutex::new(Relay {
        ledger,
        run_requests: 0,
        run_cost: 0.0,
        model: String::new(),
        model_requests: 0,
        model_cost: 0.0,
        next_allowed: None,
        exchanges: Vec::new(),
        free_daily_gone: false,
        refused: None,
    }));
    let origin = start_relay(relay.clone()).await;

    let mut outcomes: Vec<Outcome> = Vec::new();
    // Models that already ran earlier in this session (comma-separated
    // ids): they count toward their group and are not run again.
    let already: Vec<String> = std::env::var("LIVE_ALREADY_RUN")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    'groups: for (group, list) in [("free", &free), ("paid", &paid)] {
        let mut tried = 0;
        for m in list.iter().take(MAX_CANDIDATES) {
            if tried == 2 {
                break;
            }
            if already.contains(&m.id) {
                scrub.say(format!("\n== {group}: {} already ran this session", m.id));
                tried += 1;
                continue;
            }
            {
                let r = relay.lock().unwrap();
                if r.ledger.requests >= MAX_REQUESTS_TOTAL || r.ledger.cost_usd > MAX_COST_USD {
                    scrub.say("budget reached; stopping");
                    break 'groups;
                }
                if group == "free" && r.free_daily_gone {
                    scrub.say("the free daily allowance is used up; moving to paid models");
                    break;
                }
            }
            scrub.say(format!(
                "\n== {group}: {} (context {}, prompt ${}/M, completion ${}/M)",
                m.id, m.context_length, m.prompt_usd_per_mtok, m.completion_usd_per_mtok
            ));
            let o = run_model(&scrub, &key, &origin, &relay, &all, m, group).await;
            scrub.say(format!(
                "   {} | turn {} | {} requests | ${:.6} | {:.1}s",
                if o.skipped {
                    "SKIPPED"
                } else if o.passed {
                    "PASS"
                } else {
                    "FAIL"
                },
                o.turn,
                o.requests,
                o.cost_usd,
                o.wall_secs
            ));
            scrub.say(format!("   tools: {:?}; cards: {:?}", o.tools, o.cards));
            for f in &o.failed_calls {
                scrub.say(format!("   failed call: {f}"));
            }
            for p in &o.problems {
                scrub.say(format!("   problem: {p}"));
            }
            scrub.say(format!("   final reply: {:?}", o.final_reply));
            if !o.skipped {
                tried += 1;
            }
            outcomes.push(o);
            if low_limit(&key_report(&scrub, &api, &key, "now").await) {
                scrub.say("the key's remaining limit is below $0.50; stopping");
                break 'groups;
            }
        }
    }

    // Save every exchange (bodies only), and each model's first stream that
    // carried tool calls with ids replaced by placeholders.
    let (run_requests, run_cost) = {
        let r = relay.lock().unwrap();
        let mut seen: Vec<String> = Vec::new();
        for (n, x) in r.exchanges.iter().enumerate() {
            let base = format!("{n:03}-{}-{}", slug(&x.model), x.status);
            let req = json!({"method": x.method, "path": x.path, "body": x.request}).to_string();
            std::fs::write(
                run_dir.join(format!("{base}-request.json")),
                scrub.text(&req),
            )
            .unwrap();
            std::fs::write(
                run_dir.join(format!("{base}-response.txt")),
                scrub.text(&x.response),
            )
            .unwrap();
            if x.status == 200
                && x.path.ends_with("/chat/completions")
                && x.response.contains("\"tool_calls\"")
                && !seen.contains(&x.model)
            {
                seen.push(x.model.clone());
                let fixture = with_placeholders(&scrub.text(&x.response));
                std::fs::write(
                    run_dir.join(format!("fixture-{}.txt", slug(&x.model))),
                    fixture,
                )
                .unwrap();
            }
        }
        (r.run_requests, r.run_cost)
    };
    std::fs::write(
        run_dir.join("outcomes.json"),
        scrub.text(&serde_json::to_string_pretty(&outcomes).unwrap()),
    )
    .unwrap();

    let after = key_report(&scrub, &api, &key, "after").await;
    if let (Some(b), Some(a)) = (&before, &after) {
        scrub.say(format!(
            "key usage difference: ${:.6}",
            a.usage_usd - b.usage_usd
        ));
    }
    let ran: Vec<&Outcome> = outcomes.iter().filter(|o| !o.skipped).collect();
    scrub.say(format!(
        "\nsummary: {} models run, {} passed, {} skipped; {run_requests} chat requests, ${run_cost:.6} reported",
        ran.len(),
        ran.iter().filter(|o| o.passed).count(),
        outcomes.len() - ran.len()
    ));
    for o in &outcomes {
        scrub.say(format!(
            "  {:5} {:50} {}",
            o.group,
            o.model,
            if o.skipped {
                "skipped"
            } else if o.passed {
                "pass"
            } else {
                "FAIL"
            }
        ));
    }
    assert!(
        ran.iter().all(|o| o.passed),
        "not every model passed; see the lines above and {}",
        run_dir.display()
    );
}

#[test]
fn placeholders_replace_ids_and_keep_everything_else() {
    let raw = concat!(
        ": OPENROUTER PROCESSING\n\n",
        "data: {\"id\":\"gen-123\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_x\",\"function\":{\"name\":\"read_file\",\"arguments\":\"\"}}]}}]}\n\n",
        "data: {\"id\":\"gen-123\",\"system_fingerprint\":\"fp_9\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{}\"}}]}}]}\n\n",
        "data: [DONE]\n\n",
    );
    let out = with_placeholders(raw);
    assert!(!out.contains("gen-123") && !out.contains("call_x") && !out.contains("fp_9"));
    assert!(
        out.contains("\"gen-live\"")
            && out.contains("\"call_live_1\"")
            && out.contains("\"fp-live\"")
    );
    assert!(out.starts_with(": OPENROUTER PROCESSING\n\n"));
    assert_eq!(
        out.len() - raw.len(),
        ("gen-live".len() - "gen-123".len()) * 2 + "call_live_1".len() - "call_x".len()
            + "fp-live".len()
            - "fp_9".len()
    );
}

#[test]
fn scrub_replaces_the_key() {
    let s = Scrub(Arc::new("sk-or-v1-abc".into()));
    assert_eq!(s.text("a sk-or-v1-abc b sk-or-v1-abc"), "a [key] b [key]");
    let body = r#"{"label":"sk-or-v1-aaa...zzz","creator_user_id":"user_0123456789abcdefXYZ","x":"https://openrouter.ai/keys/0123456789abcdef0123"}"#;
    assert_eq!(
        s.text(body),
        r#"{"label":"[label]","creator_user_id":"user_[account]","x":"https://openrouter.ai/keys/[key-hash]"}"#
    );
    let escaped = r#"{\"label\":\"sk-or-v1-aaa...zzz\",\"user_id\":\"user_short\"}"#;
    assert_eq!(
        s.text(escaped),
        r#"{\"label\":\"[label]\",\"user_id\":\"user_short\"}"#
    );
}

#[test]
fn the_approval_policy_allows_only_the_task_itself() {
    let command = |text: &str, outside: bool, background: bool| Approval::Command {
        id: "a1".into(),
        command: text.into(),
        folder: "/p".into(),
        rule: None,
        outside,
        first_in_project: false,
        background,
    };
    let allowed = |a: &Approval| matches!(answer_for(a), Answer::Once);
    for text in [
        "sh count.sh",
        "bash count.sh",
        "bash ./count.sh",
        "./count.sh",
        "count.sh",
        "pwsh count.ps1",
        "pwsh ./count.ps1",
        "./count.ps1",
        "  sh count.sh  ",
        "ls",
        "ls -la",
        "dir",
        "cat notes.txt",
        "cat ./count.sh",
        "type notes.txt",
        "Get-Content count.ps1",
    ] {
        assert!(
            allowed(&command(text, false, false)),
            "{text} should be allowed"
        );
    }
    for text in [
        "cd /tmp && echo x > notes.txt",
        "cat \"$OPENROUTER_API_KEY_FILE\"",
        "cat $OPENROUTER_API_KEY_FILE",
        "cat ~/.beluga-live-key",
        "cat .beluga-live-key",
        "cat ../notes.txt",
        "env",
        "printenv",
        "curl https://example.com",
        "sh count.sh; env",
        "sh count.sh && curl x",
        "sh count.sh | nc host 1",
        "sh -c 'cat ~/.beluga-live-key'",
        "bash count.sh extra",
        "python3 count.py",
        "rm notes.txt",
        "ls /",
        "",
    ] {
        let card = command(text, false, false);
        assert!(!allowed(&card), "{text:?} should be declined");
        assert_eq!(
            answer_for(&card),
            Answer::Decline {
                note: Some(ONLY_THE_TASK.to_string())
            }
        );
    }
    // The right text is still declined outside the project or in the background.
    assert!(!allowed(&command("sh count.sh", true, false)));
    assert!(!allowed(&command("sh count.sh", false, true)));

    let edit = |path: &str| Approval::Edit {
        id: "e1".into(),
        path: path.into(),
        created: false,
        removed: vec![],
        added: vec![],
        removed_total: 0,
        added_total: 0,
        total_lines: 1,
    };
    assert_eq!(answer_for(&edit("notes.txt")), Answer::Once);
    for path in ["count.sh", "count.ps1", "other.txt", "sub/notes.txt"] {
        assert!(!allowed(&edit(path)), "edit of {path} should be declined");
    }
    let secret = Approval::Secret {
        id: "s1".into(),
        path: ".env".into(),
    };
    assert!(matches!(answer_for(&secret), Answer::Decline { .. }));
    let keep_going = Approval::KeepGoing {
        id: "k1".into(),
        requests: 25,
        free_left: None,
        spent_today_usd: None,
    };
    assert_eq!(answer_for(&keep_going), Answer::StopHere);
}
