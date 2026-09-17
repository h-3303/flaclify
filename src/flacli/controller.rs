//! Runs `flacli --compact ...` and keeps a snapshot of what it reports.
//!
//! Tier 1 of the roadmap: the player reads flacli state, nothing more. Every feature that
//! consults this controller stays hidden until `check` has found `flacli` on the PATH and
//! confirmed that MPD serves the folder flacli files into. The snapshot is refreshed on
//! demand and polled only while a job is running or downloads are in flight.

use gtk::{gio, glib, prelude::*};
use serde::Deserialize;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    fmt,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
};

use super::state::FlacliState;
use crate::client::MpdWrapper;

/// Seconds between polls while something is running.
const POLL_SECONDS: u32 = 15;
/// Milliseconds to wait after an MPD idle event before asking flacli again.
const DEBOUNCE_MS: u64 = 1500;

#[derive(Debug)]
pub enum Error {
    Spawn(std::io::Error),
    /// flacli exited non-zero; carries its `error` field or stderr.
    Exit(String),
    Parse(serde_json::Error),
    Thread,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Spawn(e) => write!(f, "could not run flacli: {e}"),
            Error::Exit(msg) => write!(f, "{msg}"),
            Error::Parse(e) => write!(f, "unexpected flacli output: {e}"),
            Error::Thread => write!(f, "flacli worker thread died"),
        }
    }
}

// The JSON flacli keeps stable for players (GUIDE.md, "Reading the output").

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Job {
    pub status: String,
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub total: Option<u64>,
    #[serde(default)]
    pub done: Option<u64>,
    #[serde(default)]
    pub worker_alive: Option<bool>,
}

impl Job {
    pub fn is_running(&self) -> bool {
        matches!(self.status.as_str(), "running" | "waiting")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MissingTrack {
    #[serde(default)]
    pub track_id: u64,
    /// 1-based position in the imported playlist.
    pub position: u32,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub album: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlaylistStatus {
    pub playlist_id: u32,
    pub name: String,
    /// The name it is stored under in MPD (flacli sanitises `/` and newlines).
    #[serde(default)]
    pub mpd_playlist: String,
    #[serde(default)]
    pub counts: HashMap<String, u32>,
    #[serde(default)]
    pub job: Option<Job>,
    #[serde(default)]
    pub next: String,
    /// Only present after `status <id>`; see `detailed`.
    #[serde(default)]
    pub missing: Vec<MissingTrack>,
    /// True once this entry came from `status <id>` rather than the list.
    #[serde(skip)]
    pub detailed: bool,
}

#[derive(Debug, Deserialize)]
struct StatusList {
    playlists: Vec<PlaylistStatus>,
}

#[derive(Debug, Deserialize)]
struct Setting {
    value: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct Config {
    settings: HashMap<String, Setting>,
}

impl Config {
    fn music_dir(&self) -> String {
        self.settings
            .get("music_dir")
            .and_then(|s| s.value.as_str())
            .unwrap_or_default()
            .to_owned()
    }
}

/// `flacli mpd`: what flacli's own MPD connection reports.
#[derive(Debug, Deserialize)]
struct MpdReport {
    #[serde(default)]
    reachable: bool,
    #[serde(default)]
    music_directory: Option<String>,
    #[serde(default)]
    same_library: bool,
}

#[derive(Debug, Deserialize)]
struct Nicotine {
    #[serde(default)]
    reachable: bool,
}

#[derive(Debug, Deserialize)]
struct Doctor {
    nicotine: Nicotine,
}

// Tier 2: what the write commands answer. The rules are flacli's: naming the music is the yes
// (`get`), a playlist is never queued until the totals have been seen (`queue`, then `--yes`).

/// One item of `flacli get`, as flacli understood it.
#[derive(Debug, Clone, Deserialize)]
pub struct Understood {
    pub kind: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub tracks: u32,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GetResult {
    #[serde(default)]
    pub understood: Vec<Understood>,
    #[serde(default)]
    pub added: u32,
    #[serde(default)]
    pub already_in_library: u32,
    #[serde(default)]
    pub to_fetch: u32,
    #[serde(default)]
    pub note: String,
}

impl GetResult {
    /// One line for a toast: what was understood and what is being fetched.
    pub fn summary(&self) -> String {
        let mut named: Vec<String> = Vec::new();
        for item in &self.understood {
            let what = if item.kind == "album" {
                item.album.clone().unwrap_or_default()
            } else {
                item.title.clone().unwrap_or_default()
            };
            let name = if item.artist.is_empty() {
                what
            } else {
                format!("{} – {what}", item.artist)
            };
            named.push(match &item.error {
                Some(error) => format!("{name}: {error}"),
                None if item.kind == "album" => format!("{name} ({} tracks)", item.tracks),
                None => name,
            });
        }
        let mut line = named.join("; ");
        if self.to_fetch > 0 {
            line.push_str(&format!(
                ". Fetching {} in the background, {} already in the library.",
                self.to_fetch, self.already_in_library
            ));
        } else if self.added > 0 {
            line.push_str(". Everything named is already in the library.");
        } else if line.is_empty() {
            line = self.note.clone();
        }
        line
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Imported {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SyncResult {
    #[serde(default)]
    pub imported: Vec<Imported>,
    #[serde(default)]
    pub to_fetch: u32,
    #[serde(default)]
    pub note: String,
}

/// `flacli queue`: the totals the user must see before `--yes`, then what was queued.
#[derive(Debug, Clone, Deserialize)]
pub struct QueueTotals {
    #[serde(default)]
    pub tracks: u32,
    #[serde(default)]
    pub single_files: u32,
    #[serde(default)]
    pub folders: u32,
    #[serde(default)]
    pub total_mb: f64,
    #[serde(default)]
    pub users: Vec<String>,
    #[serde(default)]
    pub approved_now: Option<u32>,
    #[serde(default)]
    pub queued: Option<serde_json::Value>,
    #[serde(default)]
    pub errors: Option<serde_json::Value>,
}

impl QueueTotals {
    /// "12 tracks: 4 files and 2 folders, 812 MB, from 3 users (a, b, c). 5 approved just now."
    pub fn describe(&self) -> String {
        let users = if self.users.is_empty() {
            String::new()
        } else {
            format!(", from {} user{} ({})", self.users.len(), if self.users.len() == 1 { "" } else { "s" }, self.users.join(", "))
        };
        let mut text = format!(
            "{} track{}: {} single file{} and {} folder{}, {:.0} MB{users}.",
            self.tracks,
            if self.tracks == 1 { "" } else { "s" },
            self.single_files,
            if self.single_files == 1 { "" } else { "s" },
            self.folders,
            if self.folders == 1 { "" } else { "s" },
            self.total_mb,
        );
        if let Some(n) = self.approved_now.filter(|n| *n > 0) {
            text.push_str(&format!(" {n} of them approved just now at {SAFE_CONFIDENCE:.2} or above."));
        }
        text
    }

    pub fn queued_count(&self) -> u64 {
        match &self.queued {
            Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0),
            Some(serde_json::Value::Array(items)) => items.len() as u64,
            Some(serde_json::Value::Object(map)) => map.len() as u64,
            _ => 0,
        }
    }

    pub fn error_count(&self) -> u64 {
        match &self.errors {
            Some(serde_json::Value::Array(items)) => items.len() as u64,
            Some(serde_json::Value::Object(map)) => map.len() as u64,
            Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0),
            _ => 0,
        }
    }
}

/// One Soulseek candidate for a track, with the why flacli computed.
#[derive(Debug, Clone, Deserialize)]
pub struct Candidate {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub quality: serde_json::Value,
    #[serde(default)]
    pub mb: f64,
    #[serde(default)]
    pub free_slot: Option<bool>,
    #[serde(default)]
    pub queue: serde_json::Value,
    #[serde(default)]
    pub why: HashMap<String, f64>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub length: Option<String>,
    #[serde(default)]
    pub track_count: Option<u32>,
}

impl Candidate {
    fn quality_str(&self) -> String {
        match &self.quality {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => String::new(),
            other => other.to_string(),
        }
    }

    /// "flac · 42 MB · someuser, queue 3, free slot · 0.72"
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let quality = self.quality_str();
        if !quality.is_empty() {
            parts.push(quality);
        }
        if self.kind == "folder" {
            parts.push(format!("whole folder, {} tracks", self.track_count.unwrap_or(0)));
        } else if let Some(length) = &self.length {
            parts.push(length.clone());
        }
        parts.push(format!("{:.0} MB", self.mb));
        let mut who = self.user.clone();
        if let Some(queue) = self.queue.as_u64() {
            who.push_str(&format!(", queue {queue}"));
        }
        if self.free_slot == Some(true) {
            who.push_str(", free slot");
        }
        parts.push(who);
        parts.push(format!("{:.2}", self.confidence));
        parts.join(" · ")
    }

    /// Where on the peer's share it is: "folder\\file".
    pub fn location(&self) -> String {
        match (self.folder.as_deref(), self.file.as_deref()) {
            (Some(folder), Some(file)) if !folder.is_empty() => format!("{folder}\\{file}"),
            (_, Some(file)) => file.to_owned(),
            (Some(folder), None) => folder.to_owned(),
            (None, None) => String::new(),
        }
    }

    /// "title 0.9 · artist 1.0 · duration 1.0", in a fixed order.
    pub fn why(&self) -> String {
        let order = ["title", "artist", "album", "duration", "coverage", "track_count"];
        let mut parts: Vec<String> = order
            .iter()
            .filter_map(|k| self.why.get(*k).map(|v| format!("{k} {v:.2}")))
            .collect();
        for (k, v) in &self.why {
            if !order.contains(&k.as_str()) {
                parts.push(format!("{k} {v:.2}"));
            }
        }
        parts.join(" · ")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReviewTrack {
    pub track_id: u64,
    /// 1-based position in the playlist.
    #[serde(default)]
    pub position: u32,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub candidates: Vec<Candidate>,
}

impl ReviewTrack {
    /// "7 · Artist – Title"
    pub fn name(&self) -> String {
        let name = match self.artist.as_deref().filter(|a| !a.is_empty()) {
            Some(artist) => format!("{artist} – {}", self.title),
            None => self.title.clone(),
        };
        if self.position > 0 {
            format!("{} · {name}", self.position)
        } else {
            name
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Review {
    #[serde(default)]
    pub total: u32,
    #[serde(default)]
    pub tracks: Vec<ReviewTrack>,
}

/// The confidence at or above which flacli treats a match as safe.
pub const SAFE_CONFIDENCE: f64 = 0.85;

impl PlaylistStatus {
    /// flacli's own Requests playlist: never stored in MPD, shown on the Requests page.
    pub fn is_requests(&self) -> bool {
        self.name == "Requests"
    }

    pub fn count(&self, status: &str) -> u32 {
        self.counts.get(status).copied().unwrap_or(0)
    }

    pub fn is_running(&self) -> bool {
        self.job.as_ref().is_some_and(Job::is_running)
    }

    pub fn in_flight(&self) -> u32 {
        self.count("queued") + self.count("downloading")
    }

    /// Something is happening: poll while this holds.
    pub fn is_active(&self) -> bool {
        self.is_running() || self.in_flight() > 0
    }

    /// Worth a line in the sidebar: active, or waiting on the user.
    pub fn is_incoming(&self) -> bool {
        self.is_active() || self.count("candidates") > 0 || self.count("approved") > 0
    }

    /// One line, as the sidebar and the playlist header show it.
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(job) = self.job.as_ref().filter(|j| j.is_running()) {
            if job.status == "waiting" {
                // flacli's note says how long: "waiting on the Soulseek search rate limit, ~12 s"
                parts.push(
                    job.note
                        .clone()
                        .unwrap_or_else(|| "waiting on the search rate limit".to_owned()),
                );
            } else {
                parts.push(match (job.phase.as_deref(), job.done, job.total) {
                    (Some("queueing"), _, _) => "queueing".to_owned(),
                    (_, Some(done), Some(total)) => format!("matching {done} of {total}"),
                    (_, None, Some(total)) => format!("matching {total}"),
                    _ => "matching".to_owned(),
                });
            }
            if job.worker_alive == Some(false) {
                parts.push("worker gone".to_owned());
            }
        }
        let in_flight = self.in_flight();
        if in_flight > 0 {
            parts.push(format!("{in_flight} downloading"));
        }
        if self.count("done") > 0 {
            parts.push(format!("{} done", self.count("done")));
        }
        if self.count("candidates") > 0 {
            parts.push(format!("{} to review", self.count("candidates")));
        }
        if self.count("approved") > 0 {
            parts.push(format!("{} approved, not queued", self.count("approved")));
        }
        let not_found = self.count("not_found") + self.count("failed");
        if not_found > 0 {
            parts.push(format!("{not_found} not found"));
        }
        if parts.is_empty() {
            if self.count("pending") > 0 {
                parts.push(format!("{} not matched yet", self.count("pending")));
            } else {
                parts.push("nothing to do".to_owned());
            }
        }
        parts.join(" · ")
    }
}

/// The reason a track is missing, as a ghost row shows it.
pub fn status_label(status: &str) -> &'static str {
    match status {
        "pending" => "not matched yet",
        "searching" => "searching",
        "candidates" => "needs a decision",
        "approved" => "approved, not queued",
        "queued" => "queued",
        "downloading" => "downloading",
        "not_found" => "not found",
        "failed" => "failed",
        "skipped" => "skipped",
        _ => "missing",
    }
}

fn find_on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let candidate = dir.join(name);
            candidate
                .metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}

/// The player's MPD is on this machine: a unix socket, or TCP to a loopback host.
fn connection_is_loopback() -> bool {
    let settings = crate::utils::settings_manager().child("client");
    if settings.boolean("mpd-use-unix-socket") {
        return true;
    }
    let host = settings.string("mpd-host");
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    matches!(host, "localhost" | "localhost.localdomain" | "::1")
        || host.starts_with("127.")
}

fn canonical(path: &str) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| Path::new(path).to_path_buf())
}

/// Equal, or one inside the other: flacli files into a folder MPD can see.
pub fn same_library(mpd_dir: &str, music_dir: &str) -> bool {
    if mpd_dir.is_empty() || music_dir.is_empty() {
        return false;
    }
    let a = canonical(mpd_dir);
    let b = canonical(music_dir);
    a == b || a.starts_with(&b) || b.starts_with(&a)
}

fn run_blocking<T>(args: &[String]) -> Result<T, Error>
where
    T: for<'de> Deserialize<'de>,
{
    let output = Command::new("flacli")
        .arg("--compact")
        .args(args)
        .output()
        .map_err(Error::Spawn)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        let message = serde_json::from_str::<serde_json::Value>(&stdout)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_owned))
            .unwrap_or_else(|| String::from_utf8_lossy(&output.stderr).trim().to_owned());
        return Err(Error::Exit(message));
    }
    serde_json::from_str::<T>(&stdout).map_err(Error::Parse)
}

/// What a command printed, for the Ask flacli console.
pub struct Answer {
    pub stdout: String,
    pub stderr: String,
    pub ok: bool,
}

impl Answer {
    /// JSON pretty-printed where flacli spoke JSON, else the text as it came; stderr appended.
    pub fn pretty(&self) -> String {
        let mut text = match serde_json::from_str::<serde_json::Value>(self.stdout.trim()) {
            Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_else(|_| self.stdout.clone()),
            Err(_) => self.stdout.clone(),
        };
        let err = self.stderr.trim();
        if !err.is_empty() {
            if !text.trim().is_empty() {
                text.push('\n');
            }
            text.push_str(err);
        }
        if !self.ok && !text.contains("error") {
            text.push_str("\n(flacli exited with an error)");
        }
        text
    }
}

/// Spawn `flacli --compact <args>` off the main thread and hand back what it printed, as is.
pub(super) async fn run_raw(args: Vec<String>) -> Result<Answer, Error> {
    gio::spawn_blocking(move || {
        let output = Command::new("flacli")
            .arg("--compact")
            .args(&args)
            .output()
            .map_err(Error::Spawn)?;
        Ok(Answer {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            ok: output.status.success(),
        })
    })
    .await
    .unwrap_or(Err(Error::Thread))
}

/// What the agent is told beyond the sentence, so its answer fits a console pane.
const AGENT_BRIEF: &str = "You are reached from the Ask flacli page of Flaclify, the music player, by a person typing a sentence \
into a small console. Use the flacli tools (or `flacli` on the command line, with --compact) to find out and to act. \
Naming music is the yes; anything else destructive (deleting files, applying a tidy plan, forgetting a playlist) needs a yes \
the person cannot give here: say exactly what you would run and stop. Answer in a few plain lines, no headings, no markdown.";

/// What the agent said, and the session to continue with.
pub struct AgentReply {
    pub text: String,
    pub session_id: Option<String>,
    pub ok: bool,
}

/// Hand a plain sentence to the configured agent (Preferences → Integrations → flacli) and
/// return what it said. The sentence goes last on its command line. With `session` the
/// same conversation continues (Claude Code's `--resume`); the reply carries the id to keep.
pub(super) async fn run_agent(sentence: String, session: Option<String>) -> Result<AgentReply, Error> {
    let command = crate::utils::settings_manager().string("agent-command");
    let mut parts = super::ask_view::split_args(&command);
    if parts.is_empty() {
        return Err(Error::Exit("no agent is set: Preferences → Integrations → flacli → Agent for plain sentences".to_owned()));
    }
    let program = parts.remove(0);
    let is_claude = std::path::Path::new(&program)
        .file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with("claude"));
    gio::spawn_blocking(move || {
        let mut cmd = Command::new(&program);
        cmd.args(&parts);
        if is_claude {
            cmd.arg("--append-system-prompt").arg(AGENT_BRIEF);
            if let Some(id) = session.as_deref() {
                cmd.arg("--resume").arg(id);
            }
        }
        let output = cmd
            .arg(&sentence)
            // A Claude Code session refuses to start inside another one.
            .env_remove("CLAUDECODE")
            .output()
            .map_err(Error::Spawn)?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let ok = output.status.success();
        // Claude Code's JSON envelope: the answer in `result`, the conversation in `session_id`.
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(stdout.trim()) {
            if let Some(result) = value.get("result").and_then(|r| r.as_str()) {
                return Ok(AgentReply {
                    text: result.to_owned(),
                    session_id: value.get("session_id").and_then(|s| s.as_str()).map(str::to_owned),
                    ok: ok && !value.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false),
                });
            }
        }
        let mut text = stdout;
        if !stderr.trim().is_empty() {
            if !text.trim().is_empty() {
                text.push('\n');
            }
            text.push_str(stderr.trim());
        }
        Ok(AgentReply { text, session_id: None, ok })
    })
    .await
    .unwrap_or(Err(Error::Thread))
}

/// Spawn `flacli --compact <args>` off the main thread and parse its JSON.
pub(super) async fn run<T>(args: Vec<String>) -> Result<T, Error>
where
    T: for<'de> Deserialize<'de> + Send + 'static,
{
    gio::spawn_blocking(move || run_blocking::<T>(&args))
        .await
        .unwrap_or(Err(Error::Thread))
}

pub(super) fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

pub struct Flacli {
    state: FlacliState,
    playlists: RefCell<Vec<PlaylistStatus>>,
    /// The flacli playlist the playlist view is showing; its detail is fetched on every tick.
    watched: Cell<Option<u32>>,
    busy: Cell<bool>,
    rerun: Cell<bool>,
    checking: Cell<bool>,
    poll: RefCell<Option<glib::SourceId>>,
    debounce: RefCell<Option<glib::SourceId>>,
}

impl fmt::Debug for Flacli {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Flacli")
            .field("available", &self.state.available())
            .field("playlists", &self.playlists.borrow().len())
            .finish()
    }
}

impl Flacli {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            state: FlacliState::default(),
            playlists: RefCell::new(Vec::new()),
            watched: Cell::new(None),
            busy: Cell::new(false),
            rerun: Cell::new(false),
            checking: Cell::new(false),
            poll: RefCell::new(None),
            debounce: RefCell::new(None),
        })
    }

    pub fn state(&self) -> FlacliState {
        self.state.clone()
    }

    /// flacli's Requests playlist, where `get` puts what is named, if it exists yet.
    pub fn requests_playlist(&self) -> Option<PlaylistStatus> {
        self.playlists.borrow().iter().find(|p| p.is_requests()).cloned()
    }

    /// The last snapshot, every playlist flacli knows.
    pub fn playlists(&self) -> Vec<PlaylistStatus> {
        self.playlists.borrow().clone()
    }

    /// The flacli playlist stored in MPD under this name, if any.
    pub fn playlist_for_mpd_name(&self, name: &str) -> Option<PlaylistStatus> {
        self.playlists
            .borrow()
            .iter()
            .find(|p| p.mpd_playlist == name || (p.mpd_playlist.is_empty() && p.name == name))
            .cloned()
    }

    /// The gate (issue #8): flacli on the PATH, and MPD's music_directory the folder flacli files
    /// into. Runs on every connection so a changed MPD address is re-evaluated.
    pub async fn check(self: Rc<Self>, client: Rc<MpdWrapper>) {
        if self.checking.replace(true) {
            return;
        }
        let state = self.state.clone();
        let on_path = gio::spawn_blocking(|| find_on_path("flacli"))
            .await
            .unwrap_or(false);
        state.set_on_path(on_path);

        let mut same = false;
        let mut reason = String::new();
        if !on_path {
            reason = "flacli is not on the PATH".to_owned();
        } else {
            match run::<Config>(args(&["config"])).await {
                Ok(config) => {
                    let music_dir = config.music_dir();
                    state.set_music_dir(music_dir.clone());
                    match client.get_music_directory().await {
                        Ok(mpd_dir) => {
                            same = same_library(&mpd_dir, &music_dir);
                            if !same {
                                reason = format!("MPD serves {mpd_dir}, flacli files into {music_dir}");
                            }
                        }
                        // MPD answers `config` only over a local socket. Over TCP to this same
                        // machine, flacli's own report on its MPD is the next best answer.
                        Err(_) if connection_is_loopback() => match run::<MpdReport>(args(&["mpd"])).await {
                            Ok(report) if report.reachable => {
                                same = report.same_library
                                    || report
                                        .music_directory
                                        .as_deref()
                                        .is_some_and(|dir| same_library(dir, &music_dir));
                                if !same {
                                    reason = format!(
                                        "MPD serves {}, flacli files into {music_dir}",
                                        report.music_directory.unwrap_or_default()
                                    );
                                }
                            }
                            Ok(_) => {
                                reason = "flacli cannot reach an MPD of its own to compare libraries (flacli mpd)".to_owned();
                            }
                            Err(e) => {
                                reason = format!("flacli mpd failed: {e}");
                            }
                        },
                        Err(_) => {
                            reason = "MPD did not say which folder it serves (a remote connection); flacli needs the library on this machine".to_owned();
                        }
                    }
                }
                Err(e) => {
                    reason = format!("flacli config failed: {e}");
                }
            }
        }
        state.set_same_library(same);
        state.set_reason(reason.clone());
        state.set_available(on_path && same);

        if state.available() {
            eprintln!("[flacli] available: MPD and flacli share {}", state.music_dir());
            self.clone().tick().await;
            let reachable = run::<Doctor>(args(&["doctor"]))
                .await
                .map(|d| d.nicotine.reachable)
                .unwrap_or(false);
            state.set_bridge_reachable(reachable);
        } else {
            eprintln!("[flacli] hidden: {reason}");
            state.set_bridge_reachable(false);
            self.playlists.borrow_mut().clear();
            state.set_active(false);
            state.emit_refreshed();
        }
        self.checking.set(false);
    }

    /// Ask flacli now. Ticks never overlap; a request during one runs once it is done.
    pub fn refresh(self: &Rc<Self>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            this.tick().await;
        });
    }

    /// Ask flacli shortly, coalescing a burst of MPD idle events into one call.
    pub fn refresh_soon(self: &Rc<Self>) {
        if let Some(id) = self.debounce.take() {
            id.remove();
        }
        let this = self.clone();
        let id = glib::timeout_add_local_once(
            std::time::Duration::from_millis(DEBOUNCE_MS),
            move || {
                this.debounce.take();
                this.refresh();
            },
        );
        self.debounce.replace(Some(id));
    }

    /// Keep one playlist's detail (its missing tracks) fresh: the one on screen.
    pub fn watch(self: &Rc<Self>, playlist_id: Option<u32>) {
        self.watched.set(playlist_id);
        if playlist_id.is_some() && self.state.available() {
            self.refresh();
        }
    }

    async fn tick(self: Rc<Self>) {
        if !self.state.available() {
            return;
        }
        if self.busy.replace(true) {
            self.rerun.set(true);
            return;
        }
        loop {
            self.rerun.set(false);
            match run::<StatusList>(args(&["status"])).await {
                Ok(list) => {
                    let watched = self.watched.get();
                    let mut playlists = list.playlists;
                    for playlist in playlists.iter_mut() {
                        if playlist.is_active() || watched == Some(playlist.playlist_id) {
                            let id = playlist.playlist_id.to_string();
                            match run::<PlaylistStatus>(args(&["status", &id])).await {
                                Ok(mut detail) => {
                                    detail.detailed = true;
                                    *playlist = detail;
                                }
                                Err(e) => eprintln!("[flacli] status {id} failed: {e}"),
                            }
                        }
                    }
                    let active = playlists.iter().any(PlaylistStatus::is_active);
                    self.playlists.replace(playlists);
                    self.state.set_active(active);
                    self.state.emit_refreshed();
                }
                Err(e) => eprintln!("[flacli] status failed: {e}"),
            }
            if !self.rerun.get() {
                break;
            }
        }
        self.busy.set(false);
        self.schedule_poll();
    }

    // Tier 2: the write commands. Each one asks flacli again afterwards so the surfaces follow.

    /// The Nicotine+ bridge must answer before a fetch is offered (issue #8). Re-checks with
    /// `flacli doctor` when the last check said no, so a bridge started later is picked up.
    pub async fn ensure_bridge(&self) -> bool {
        if self.state.bridge_reachable() {
            return true;
        }
        let reachable = run::<Doctor>(args(&["doctor"]))
            .await
            .map(|d| d.nicotine.reachable)
            .unwrap_or(false);
        self.state.set_bridge_reachable(reachable);
        reachable
    }

    /// `flacli get <items>`: naming the music is the yes; confident matches are queued at once.
    pub async fn get(self: &Rc<Self>, items: &[String]) -> Result<GetResult, Error> {
        let mut cmd = args(&["get"]);
        cmd.extend(items.iter().cloned());
        let result = run::<GetResult>(cmd).await;
        self.refresh();
        result
    }

    /// `flacli sync <url>`: import, resolve, match in the background. Nothing queued yet.
    pub async fn sync(self: &Rc<Self>, target: &str) -> Result<SyncResult, Error> {
        let result = run::<SyncResult>(args(&["sync", target])).await;
        self.refresh();
        result
    }

    /// `flacli queue <id>`: the totals, and with `min_confidence` every candidate at or above it
    /// approved first. Nothing is queued.
    pub async fn queue_totals(&self, playlist_id: u32, min_confidence: Option<f64>) -> Result<QueueTotals, Error> {
        let id = playlist_id.to_string();
        let mut cmd = args(&["queue", &id]);
        if let Some(min) = min_confidence {
            cmd.push("--min-confidence".to_owned());
            cmd.push(format!("{min}"));
        }
        run::<QueueTotals>(cmd).await
    }

    /// `flacli queue <id> --yes`: only after the totals have been shown.
    pub async fn queue_confirmed(self: &Rc<Self>, playlist_id: u32) -> Result<QueueTotals, Error> {
        let id = playlist_id.to_string();
        let result = run::<QueueTotals>(args(&["queue", &id, "--yes"])).await;
        self.refresh();
        result
    }

    /// `flacli review <id>`: the tracks needing a decision with their best candidates.
    pub async fn review(&self, playlist_id: u32) -> Result<Review, Error> {
        let id = playlist_id.to_string();
        run::<Review>(args(&["review", &id, "--limit", "100"])).await
    }

    /// `flacli approve <id> --tracks a,b --candidate n`.
    pub async fn approve(self: &Rc<Self>, playlist_id: u32, track_ids: &[u64], candidate: usize) -> Result<(), Error> {
        let id = playlist_id.to_string();
        let ids = track_ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        let index = candidate.to_string();
        let result = run::<serde_json::Value>(args(&["approve", &id, "--tracks", &ids, "--candidate", &index])).await;
        self.refresh();
        result.map(|_| ())
    }

    /// `flacli skip <id> --tracks a,b`.
    pub async fn skip(self: &Rc<Self>, playlist_id: u32, track_ids: &[u64]) -> Result<(), Error> {
        let id = playlist_id.to_string();
        let ids = track_ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        let result = run::<serde_json::Value>(args(&["skip", &id, "--tracks", &ids, "--reason", "skipped in Flaclify"])).await;
        self.refresh();
        result.map(|_| ())
    }

    /// `flacli mpd playlist <id>`: store the playlist in MPD under its name, from the tracks
    /// already on disk. The sentence says what happened.
    pub async fn store_in_mpd(&self, playlist_id: u32) -> Result<String, Error> {
        let id = playlist_id.to_string();
        let result = run::<serde_json::Value>(args(&["mpd", "playlist", &id])).await?;
        let local = result.get("local_tracks").and_then(|v| v.as_u64()).unwrap_or(0);
        let mpd = result.get("mpd").cloned().unwrap_or(serde_json::Value::Null);
        if let Some(skipped) = mpd.get("skipped").and_then(|v| v.as_str()) {
            return Err(Error::Exit(format!("not stored: {skipped}")));
        }
        let added = mpd.get("added").and_then(|v| v.as_u64()).unwrap_or(0);
        let not_in_db = mpd.get("not_in_db").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
        let mut line = format!("Stored in MPD with {added} of {local} local track{}", if local == 1 { "" } else { "s" });
        if not_in_db > 0 {
            line.push_str(&format!("; {not_in_db} not in MPD's database yet"));
        }
        Ok(line)
    }

    /// `flacli skip <id> --remaining`: stop the job, cancel queued transfers, skip every track
    /// not yet on disk. The sentence says what happened.
    pub async fn skip_remaining(self: &Rc<Self>, playlist_id: u32) -> Result<String, Error> {
        let id = playlist_id.to_string();
        let result = run::<serde_json::Value>(args(&["skip", &id, "--remaining"])).await;
        self.refresh();
        let result = result?;
        let skipped = result.get("skipped").and_then(|v| v.as_u64()).unwrap_or(0);
        let cancelled = result.get("cancelled_downloads").and_then(|v| v.as_u64()).unwrap_or(0);
        let mut line = format!("Skipped {skipped} track{}", if skipped == 1 { "" } else { "s" });
        if cancelled > 0 {
            line.push_str(&format!(", {cancelled} download{} cancelled in Nicotine+", if cancelled == 1 { "" } else { "s" }));
        }
        if let Some(note) = result.get("cancel_error").and_then(|v| v.as_str()) {
            line.push_str(&format!("; {note}"));
        }
        Ok(line)
    }

    /// `flacli cancel <id>`: stop the running job.
    pub async fn cancel(self: &Rc<Self>, playlist_id: u32) -> Result<(), Error> {
        let id = playlist_id.to_string();
        let result = run::<serde_json::Value>(args(&["cancel", &id])).await;
        self.refresh();
        result.map(|_| ())
    }

    fn schedule_poll(self: &Rc<Self>) {
        if let Some(id) = self.poll.take() {
            id.remove();
        }
        if self.state.active() {
            let this = self.clone();
            let id = glib::timeout_add_local_once(
                std::time::Duration::from_secs(POLL_SECONDS as u64),
                move || {
                    this.poll.take();
                    this.refresh();
                },
            );
            self.poll.replace(Some(id));
        }
    }
}

thread_local! {
    static INSTANCE: RefCell<Option<Rc<Flacli>>> = const { RefCell::new(None) };
}

/// Create the process-wide controller. Call once, before any view asks for it.
pub fn init() -> Rc<Flacli> {
    let flacli = Flacli::new();
    INSTANCE.with(|i| *i.borrow_mut() = Some(flacli.clone()));
    flacli
}

/// The process-wide controller. Panics if `init` has not run.
pub fn flacli() -> Rc<Flacli> {
    INSTANCE.with(|i| {
        i.borrow()
            .clone()
            .expect("flacli::init() must run before flacli()")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(counts: &[(&str, u32)], job: Option<Job>) -> PlaylistStatus {
        PlaylistStatus {
            playlist_id: 1,
            name: "Mix".to_owned(),
            mpd_playlist: "Mix".to_owned(),
            counts: counts.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            job,
            next: String::new(),
            missing: Vec::new(),
            detailed: false,
        }
    }

    #[test]
    fn same_library_accepts_equal_and_nested_paths() {
        assert!(same_library("/srv/music", "/srv/music"));
        assert!(same_library("/srv/music/flac", "/srv/music"));
        assert!(same_library("/srv/music", "/srv/music/incoming"));
        assert!(!same_library("/srv/music2", "/srv/music"));
        assert!(!same_library("", "/srv/music"));
    }

    #[test]
    fn running_job_and_in_flight_downloads_are_active() {
        let running = Job { status: "running".into(), phase: Some("matching".into()), total: Some(40), done: Some(12), ..Job::default() };
        let s = status(&[("pending", 28)], Some(running));
        assert!(s.is_active() && s.is_incoming());
        assert_eq!(s.summary(), "matching 12 of 40");

        let s = status(&[("queued", 2), ("downloading", 1), ("done", 7)], Some(Job { status: "finished".into(), ..Job::default() }));
        assert!(s.is_active());
        assert_eq!(s.summary(), "3 downloading · 7 done");

        let s = status(&[("done", 9), ("candidates", 2), ("not_found", 1)], None);
        assert!(!s.is_active() && s.is_incoming());
        assert_eq!(s.summary(), "9 done · 2 to review · 1 not found");

        let s = status(&[("pending", 5)], None);
        assert!(!s.is_incoming());
        assert_eq!(s.summary(), "5 not matched yet");
    }

    #[test]
    fn waiting_uses_flaclis_note() {
        let waiting = Job { status: "waiting".into(), note: Some("waiting on the Soulseek search rate limit, ~12 s".into()), ..Job::default() };
        assert_eq!(status(&[], Some(waiting)).summary(), "waiting on the Soulseek search rate limit, ~12 s");
    }

    #[test]
    fn write_command_results_read_as_sentences() {
        let get: GetResult = serde_json::from_str(r#"{"playlist_id":5,"playlist":"Requests","created":false,
            "understood":[{"kind":"album","artist":"Radiohead","album":"Kid A","tracks":10,"release":{"id":"x"}},
                          {"kind":"track","artist":"","title":"Pyramid Song","tracks":1}],
            "added":11,"already_in_library":3,"to_fetch":8,"note":"fetching 8 track(s)","job_id":4}"#).unwrap();
        assert_eq!(get.summary(), "Radiohead – Kid A (10 tracks); Pyramid Song. Fetching 8 in the background, 3 already in the library.");

        let totals: QueueTotals = serde_json::from_str(r#"{"playlist_id":5,"tracks":12,"single_files":4,"folders":2,
            "total_mb":811.6,"users":["a","b"],"approved_now":5,"note":"nothing queued"}"#).unwrap();
        assert_eq!(totals.describe(), "12 tracks: 4 single files and 2 folders, 812 MB, from 2 users (a, b). 5 of them approved just now at 0.85 or above.");

        let review: Review = serde_json::from_str(r#"{"playlist_id":5,"status":"candidates","total":1,"offset":0,"tracks":[
            {"track_id":9,"position":3,"artist":"A","title":"T","album":"B","status":"candidates","confidence":0.72,
             "candidates":[{"kind":"file","user":"u","confidence":0.72,"score":0.8,"quality":"flac 16/44.1","mb":31.2,
                            "free_slot":true,"queue":2,"why":{"title":0.9,"artist":1.0,"duration":0.5},"file":"03 T.flac",
                            "folder":"Music\\A\\B","length":"4:12"}]}]}"#).unwrap();
        let track = &review.tracks[0];
        assert_eq!(track.name(), "3 · A – T");
        let best = &track.candidates[0];
        assert_eq!(best.describe(), "flac 16/44.1 · 4:12 · 31 MB · u, queue 2, free slot · 0.72");
        assert_eq!(best.why(), "title 0.90 · artist 1.00 · duration 0.50");
        assert_eq!(best.location(), "Music\\A\\B\\03 T.flac");
    }

    #[test]
    fn status_json_from_flacli_parses() {
        let json = r#"{"playlists":[{"playlist_id":3,"name":"Mix","source":"csv","imported_at":"x","tracks":4,
            "counts":{"done":2,"not_found":1,"candidates":1},"mpd_playlist":"Mix","job":null,"next":"flacli review 3"}]}"#;
        let list: StatusList = serde_json::from_str(json).unwrap();
        assert_eq!(list.playlists[0].count("done"), 2);
        assert!(list.playlists[0].job.is_none());

        let detail = r#"{"playlist_id":3,"name":"Mix","mpd_playlist":"Mix","tracks":4,"job":{"job_id":1,"kind":"match",
            "status":"finished","started_at":"x","updated_at":"y","total":4,"phase":"finished"},"counts":{"done":2},
            "missing":[{"track_id":9,"position":2,"artist":"A","title":"T","album":null,"status":"not_found"}],"next":"done"}"#;
        let one: PlaylistStatus = serde_json::from_str(detail).unwrap();
        assert_eq!(one.missing[0].position, 2);
        assert_eq!(status_label(&one.missing[0].status), "not found");
    }
}
