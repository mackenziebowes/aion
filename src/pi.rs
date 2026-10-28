//! Runs pi on Aion 3.5 and turns its JSON stream into machine events. The only place that spawns a process.

use crate::machine::{Effect, Event, State, Tally, Usd, step, Caps};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const BASE_SYSTEM: &str = "You are Aion, working with a small business and its team of AI agents. You have no tools and cannot open files: everything you are asked about is in the message or in the files attached to it. If someone mentions a post, page or file that is not there, say you do not have it instead of describing it. Never invent a source, a quote, a number or a page.";

pub enum Session<'a> { OneOff, Start(&'a Path), Continue(&'a Path) }

pub struct Run<'a> { pub prompt: &'a str, pub attach: &'a [PathBuf], pub system: &'a str, pub session: Session<'a>, pub dir: &'a Path, pub key: &'a str }

/// A turn_end line: the text blocks Aion wrote, and the tokens and dollars of that turn.
fn turn(v: &serde_json::Value) -> Option<(Tally, String, String)> {
    let m = &v["message"];
    let u = &m["usage"];
    let text = m["content"].as_array()?.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n");
    let tally = Tally {
        cost: Usd::from_dollars(u["cost"]["total"].as_f64().unwrap_or(0.0)),
        tokens_in: u["input"].as_u64().unwrap_or(0) + u["cacheRead"].as_u64().unwrap_or(0),
        tokens_out: u["output"].as_u64().unwrap_or(0),
    };
    Some((tally, text, m["stopReason"].as_str().unwrap_or("").to_string()))
}

/// Starts pi, feeds `state` from the stream, and returns the final state. Kills pi when the machine says Kill.
/// The attachments, then `--`, then the prompt: after `--` pi reads a prompt that starts with a dash as the prompt, not a flag.
fn prompt_args(r: &Run) -> Vec<std::ffi::OsString> {
    let mut v: Vec<std::ffi::OsString> = r.attach.iter().map(|f| format!("@{}", f.display()).into()).collect();
    v.push("--".into());
    v.push(r.prompt.into());
    v
}

pub fn run(r: &Run, mut state: State, caps: Caps) -> State {
    let mut cmd = Command::new("pi");
    cmd.args(["--provider", "openrouter", "--model", "aion-labs/aion-3.5", "--no-tools", "--no-mcp", "--no-context-files", "--no-skills", "--mode", "json", "-p"])
        .args(["--system-prompt", r.system]);
    match &r.session {
        Session::OneOff => { cmd.arg("--no-session"); }
        Session::Start(d) => { cmd.arg("--session-dir").arg(d); }
        Session::Continue(d) => { cmd.arg("--session-dir").arg(d).arg("-c"); }
    }
    cmd.args(prompt_args(r)).current_dir(r.dir).env("OPENROUTER_API_KEY", r.key).stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return State::Failed { tally: Tally::default(), why: if e.kind() == std::io::ErrorKind::NotFound { "pi is not installed (aion runs Aion through the pi agent, https://github.com/earendil-works/pi: `mise use -g pi`, or `bun add -g @earendil-works/pi-coding-agent`)".to_string() } else { format!("could not start pi: {e}") } },
    };
    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let err_reader = std::thread::spawn(move || { let mut s = String::new(); let _ = stderr.read_to_string(&mut s); s });
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if v["type"] != "turn_end" { continue; }
        let Some((tally, text, stop)) = turn(&v) else { continue };
        let (next, effects) = step(state, Event::TurnEnded { tally, text, stop }, caps);
        state = next;
        if effects.contains(&Effect::Kill) { let _ = child.kill(); break; }
    }
    let status = child.wait();
    let stderr = err_reader.join().unwrap_or_default();
    let ok = status.map(|s| s.success()).unwrap_or(false);
    step(state, Event::Exited { ok, stderr }, caps).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_that_starts_with_a_dash_comes_after_double_dash() {
        let attach = [PathBuf::from("/tmp/notes.md")];
        let r = Run { prompt: "-5 degrees is cold, why?", attach: &attach, system: "", session: Session::OneOff, dir: Path::new("."), key: "" };
        let args: Vec<_> = prompt_args(&r).iter().map(|a| a.to_string_lossy().to_string()).collect();
        assert_eq!(args, ["@/tmp/notes.md", "--", "-5 degrees is cold, why?"]);
    }
}
