//! The files: caps from budget.toml, the call log, and the session folders. Read at the edge; nothing here decides.

use crate::machine::{Caps, Spent, Tally, Usd};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

fn home() -> PathBuf { PathBuf::from(std::env::var_os("HOME").unwrap_or_default()) }

pub fn aion_dir() -> PathBuf {
    std::env::var_os("AION_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share/aion"))
}
/// The caps file: AION_BUDGET, else ~/.config/aion/budget.toml.
fn budget_path() -> PathBuf {
    std::env::var_os("AION_BUDGET").map(PathBuf::from).unwrap_or_else(|| home().join(".config/aion/budget.toml"))
}
pub fn session_dir(name: &str) -> PathBuf { aion_dir().join("sessions").join(name) }
pub fn log_path() -> PathBuf { aion_dir().join("calls.jsonl") }

// ---- caps ----

/// Where the caps came from, so `aion status` can say when they are the defaults.
pub enum CapSource { File(PathBuf), Default }

#[derive(Deserialize)]
struct RawFile { aion: Option<RawAion> }
#[derive(Deserialize)]
struct RawAion { per_call_usd: f64, per_day_usd: f64, per_month_usd: f64 }

/// $0.10 a call, $10 a day, $10 a month, unless a caps file says otherwise.
const DEFAULT: Caps = Caps { per_call: Usd(100_000), per_day: Usd(10_000_000), per_month: Usd(10_000_000) };

pub fn caps() -> Result<(Caps, CapSource), String> {
    let path = budget_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((DEFAULT, CapSource::Default)),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let raw: RawFile = toml::from_str(&text).map_err(|e| format!("{}: {}", path.display(), e.message()))?;
    match raw.aion {
        None => Ok((DEFAULT, CapSource::Default)),
        Some(a) => Ok((Caps { per_call: Usd::from_dollars(a.per_call_usd), per_day: Usd::from_dollars(a.per_day_usd), per_month: Usd::from_dollars(a.per_month_usd) }, CapSource::File(path))),
    }
}

// ---- the call log ----

/// How a call ended: one variant per final state.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome { Finished, Capped, Failed, RefusedDayCap, RefusedMonthCap, RefusedTooBig }

#[derive(Serialize, Deserialize)]
pub struct Line {
    pub at: String,
    pub day: String,
    pub session: Option<String>,
    pub claude_session: Option<String>,
    pub cwd: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    pub seconds: f64,
    pub outcome: Outcome,
}

pub fn now() -> (String, String) {
    let run = |fmt: &str| Command::new("date").arg(fmt).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    (run("+%Y-%m-%dT%H:%M:%S%z"), run("+%Y-%m-%d"))
}

pub fn lines() -> Vec<Line> {
    std::fs::read_to_string(log_path()).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

pub fn spent(day: &str) -> Spent {
    let month = &day[..7.min(day.len())];
    let sum = |keep: &dyn Fn(&Line) -> bool| Usd(lines().iter().filter(|l| keep(l)).map(|l| Usd::from_dollars(l.cost_usd).0).sum());
    Spent { today: sum(&|l| l.day == day), month: sum(&|l| l.day.starts_with(month)) }
}

pub fn append(line: &Line) -> Result<(), String> {
    use std::io::Write;
    std::fs::create_dir_all(aion_dir()).map_err(|e| e.to_string())?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(log_path()).map_err(|e| format!("{}: {e}", log_path().display()))?;
    writeln!(f, "{}", serde_json::to_string(line).unwrap()).map_err(|e| e.to_string())
}

pub fn line_for(session: Option<&str>, tally: Tally, seconds: f64, outcome: Outcome) -> Line {
    let (at, day) = now();
    Line {
        at, day, session: session.map(String::from), claude_session: std::env::var("CLAUDE_CODE_SESSION_ID").ok(),
        cwd: std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
        tokens_in: tally.tokens_in, tokens_out: tally.tokens_out, cost_usd: tally.cost.0 as f64 / 1e6, seconds, outcome,
    }
}

// ---- the key ----

/// OPENROUTER_API_KEY, else ENGAGE_OPENROUTER_KEY, from the environment. Never printed.
pub fn key() -> Result<String, String> {
    for name in ["OPENROUTER_API_KEY", "ENGAGE_OPENROUTER_KEY"] {
        if let Ok(k) = std::env::var(name) { if !k.trim().is_empty() { return Ok(k.trim().to_string()); } }
    }
    Err("no OpenRouter key: set OPENROUTER_API_KEY (make one at https://openrouter.ai/keys)".into())
}
