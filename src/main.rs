//! aion: one capped call to Aion 3.5 (OpenRouter, through pi) for any agent. Aion has no tools: the caller attaches what it already read.
//! This file is the adapter: it turns argv into a `Command`, runs the machine in machine.rs, prints the state. It decides nothing.

mod machine;
mod pi;
mod store;

use machine::{Refusal, State, Tally, step, Event};
use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "aion: ask Aion 3.5 a question, with spend caps. One call per run. Aion has no tools: you pass it what you already read.

  aion ask [--attach FILE]... [--session NAME [--persona FILE]] PROMPT...   answer on stdout; cost and caps on stderr
  aion ask ... -                                                     read the prompt from stdin
  aion transcript NAME                                               the whole exchange of a session
  aion status                                                        spend today and this month against the caps

One-off: no --session. Back and forth: the first call with a new --session NAME starts it (--persona FILE is read as who
Aion is, and is kept for later turns); each later call with the same NAME is the next turn. Each --attach FILE is put into
the message (repeat it for more files); attach again on a later turn to show Aion a file again. Aion cannot open files itself,
so attach every source it should speak from; it is told to say so when something is missing.
Caps default to $0.10 a call, $10 a day and $10 a month; the [aion] table of ~/.config/aion/budget.toml (or the file AION_BUDGET names) sets per_call_usd, per_day_usd and per_month_usd. A call is stopped at the end of the turn that reaches its cap, so it can overshoot by one turn. Exit codes:
0 answered, 1 failed, 2 bad usage, 3 refused at a cap, 4 answered but stopped at the per-call cap. Calls are logged to
~/.local/share/aion/calls.jsonl (AION_DIR overrides) with the calling Claude session. The key comes from the OPENROUTER_API_KEY environment variable.";

enum Mode { OneOff, Start { name: String, persona: Option<PathBuf> }, Continue { name: String } }

enum Command { Ask { prompt: String, mode: Mode, attach: Vec<PathBuf> }, Transcript(String), Status, Help }

fn valid_name(n: &str) -> bool { !n.is_empty() && n.len() <= 60 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') }

fn parse(args: Vec<String>) -> Result<Command, String> {
    let mut it = args.into_iter();
    match it.next().as_deref() {
        None | Some("help") | Some("--help") | Some("-h") => Ok(Command::Help),
        Some("status") => Ok(Command::Status),
        Some("transcript") => it.next().filter(|n| valid_name(n)).map(Command::Transcript).ok_or("transcript needs a session name (letters, digits, - and _)".into()),
        Some("ask") => {
            let (mut attach, mut session, mut persona, mut words) = (Vec::<PathBuf>::new(), None::<String>, None::<PathBuf>, Vec::<String>::new());
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--attach" => {
                        let f = PathBuf::from(it.next().ok_or("--attach needs a file")?);
                        let f = f.canonicalize().map_err(|e| format!("--attach {}: {e}", f.display()))?;
                        if !f.is_file() { return Err(format!("--attach {}: not a file", f.display())); }
                        attach.push(f);
                    }
                    "--dir" => return Err("--dir is gone: Aion has no tools and reads nothing itself; attach what it needs with --attach FILE".into()),
                    "--session" => session = Some(it.next().ok_or("--session needs a name")?),
                    "--persona" => persona = Some(it.next().ok_or("--persona needs a file")?.into()),
                    "--" => { words.extend(it.by_ref()); break; }
                    f if f.starts_with("--") && !f.contains(char::is_whitespace) => return Err(format!("unknown option {f}; to ask a question that starts with --, put -- before it")),
                    _ => words.push(a),
                }
            }
            let prompt = if words == ["-"] || (words.is_empty() && !std::io::stdin().is_terminal()) {
                let mut s = String::new();
                std::io::stdin().read_to_string(&mut s).map_err(|e| e.to_string())?;
                s
            } else { words.join(" ") };
            if prompt.trim().is_empty() { return Err("no prompt: give it as words, or pipe it in with -".into()); }
            if prompt.len() > 100_000 { return Err("prompt is over 100 KB: put it in a file and pass it with --attach".into()); }
            let mode = match session {
                None if persona.is_some() => return Err("--persona needs --session (a one-off has no later turns to keep it for)".into()),
                None => Mode::OneOff,
                Some(n) if !valid_name(&n) => return Err(format!("session name {n:?} must be letters, digits, - and _")),
                Some(n) if store::session_dir(&n).join("system.md").exists() => {
                    if persona.is_some() { return Err(format!("session {n} already started: its persona was set on the first turn")); }
                    Mode::Continue { name: n }
                }
                Some(name) => Mode::Start { name, persona },
            };
            Ok(Command::Ask { prompt, mode, attach })
        }
        Some(other) => Err(format!("unknown command {other:?}; try `aion help`")),
    }
}

fn fail(msg: &str, code: u8) -> ExitCode { eprintln!("aion: {msg}"); ExitCode::from(code) }

fn ask(prompt: String, mode: Mode, attach: Vec<PathBuf>) -> ExitCode {
    let (caps, _) = match store::caps() { Ok(c) => c, Err(e) => return fail(&e, 1) };
    let key = match store::key() { Ok(k) => k, Err(e) => return fail(&e, 1) };
    let (_, day) = store::now();
    let started = std::time::Instant::now();
    let name = match &mode { Mode::OneOff => None, Mode::Start { name, .. } | Mode::Continue { name } => Some(name.clone()) };

    // The system prompt: the base, plus the persona kept from the first turn.
    let sdir = name.as_deref().map(store::session_dir);
    let system = match &mode {
        Mode::Start { persona, .. } => {
            let extra = match persona { Some(p) => match std::fs::read_to_string(p) { Ok(t) => format!("\n\n{t}"), Err(e) => return fail(&format!("{}: {e}", p.display()), 2) }, None => String::new() };
            format!("{}{extra}", pi::BASE_SYSTEM)
        }
        Mode::Continue { name } => match std::fs::read_to_string(store::session_dir(name).join("system.md")) { Ok(t) => t, Err(e) => return fail(&format!("session {name}: {e}"), 1) },
        Mode::OneOff => pi::BASE_SYSTEM.to_string(),
    };

    let bytes = (prompt.len() + system.len()) as u64 + attach.iter().map(|f| std::fs::metadata(f).map(|m| m.len()).unwrap_or(0)).sum::<u64>();
    let (state, _) = step(State::Checking, Event::Spent(store::spent(&day), bytes), caps);
    let state = match state {
        State::Running { .. } => {
            if let (Mode::Start { .. }, Some(d)) = (&mode, &sdir) {
                if let Err(e) = std::fs::create_dir_all(d).and_then(|_| std::fs::write(d.join("system.md"), &system)) { return fail(&format!("{}: {e}", d.display()), 1); }
            }
            let session = match (&mode, &sdir) { (Mode::Start { .. }, Some(d)) => pi::Session::Start(d), (Mode::Continue { .. }, Some(d)) => pi::Session::Continue(d), _ => pi::Session::OneOff };
            pi::run(&pi::Run { prompt: &prompt, attach: &attach, system: &system, session, dir: &std::env::current_dir().unwrap_or_default(), key: &key }, state, caps)
        }
        other => other,
    };
    let seconds = started.elapsed().as_secs_f64();
    let log = |tally: Tally, outcome| { if let Err(e) = store::append(&store::line_for(name.as_deref(), tally, seconds, outcome)) { eprintln!("aion: could not write the call log: {e}"); } };
    match state {
        State::Refused(r) => {
            let (msg, outcome) = match r {
                Refusal::DayCap { spent, cap } => (format!("refused: today's Aion spend is {spent}, at the day cap {cap}. Nothing was sent. Raise per_day_usd or per_month_usd in the [aion] table of ~/.config/aion/budget.toml."), store::Outcome::RefusedDayCap),
                Refusal::MonthCap { spent, cap } => (format!("refused: this month's Aion spend is {spent}, at the month cap {cap}. Nothing was sent. Raise per_day_usd or per_month_usd in the [aion] table of ~/.config/aion/budget.toml."), store::Outcome::RefusedMonthCap),
                Refusal::TooBig { estimate, cap } => (format!("refused: the prompt and attachments would cost about {estimate} to read, over the per-call cap {cap}. Nothing was sent. Attach less, or raise per_call_usd in the [aion] table of ~/.config/aion/budget.toml."), store::Outcome::RefusedTooBig),
            };
            log(Tally::default(), outcome);
            fail(&msg, 3)
        }
        State::Finished { tally, text } => { log(tally, store::Outcome::Finished); println!("{}", text.trim_end()); eprintln!("[aion: {} in {seconds:.0}s, {} tokens in, {} out]", tally.cost, tally.tokens_in, tally.tokens_out); ExitCode::SUCCESS }
        State::Capped { tally, text } => {
            log(tally, store::Outcome::Capped);
            let said = if text.trim().is_empty() { "Aion had said nothing yet (it was still reading files)." } else { println!("{}", text.trim_end()); "The answer above is what Aion had said by then." };
            eprintln!("[aion: stopped at the per-call cap {}: spent {}. {said}]", caps.per_call, tally.cost);
            ExitCode::from(4)
        }
        State::Failed { tally, why } => { log(tally, store::Outcome::Failed); fail(&format!("failed ({}): {}", tally.cost, why.trim()), 1) }
        State::Checking | State::Running { .. } => fail("internal: the call ended without a final state", 1),
    }
}

fn status() -> ExitCode {
    let (caps, source) = match store::caps() { Ok(c) => c, Err(e) => return fail(&e, 1) };
    let (_, day) = store::now();
    let spent = store::spent(&day);
    let calls = store::lines().iter().filter(|l| l.day == day).count();
    println!("Aion spend, {day}: today {} of {} ({calls} calls); this month {} of {}; per call {}.", spent.today, caps.per_day, spent.month, caps.per_month, caps.per_call);
    match source {
        store::CapSource::File(p) => println!("Caps from {}.", p.display()),
        store::CapSource::Default => println!("Caps are the defaults (no [aion] table in ~/.config/aion/budget.toml)."),
    }
    ExitCode::SUCCESS
}

fn transcript(name: &str) -> ExitCode {
    let dir = store::session_dir(name);
    let mut files: Vec<_> = std::fs::read_dir(&dir).map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect()).unwrap_or_default();
    files.sort();
    if files.is_empty() { return fail(&format!("no session {name} under {}", dir.display()), 1); }
    for f in files {
        for line in std::fs::read_to_string(&f).unwrap_or_default().lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            let m = if v["message"].is_object() { &v["message"] } else { &v };
            let role = m["role"].as_str().unwrap_or("");
            if role != "user" && role != "assistant" { continue; }
            let text = match &m["content"] {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Array(bs) => bs.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
                _ => String::new(),
            };
            if !text.trim().is_empty() { println!("{}:\n{}\n", if role == "user" { "You" } else { "Aion" }, text.trim()); }
        }
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    match parse(std::env::args().skip(1).collect()) {
        Ok(Command::Help) => { println!("{USAGE}"); ExitCode::SUCCESS }
        Ok(Command::Status) => status(),
        Ok(Command::Transcript(n)) => transcript(&n),
        Ok(Command::Ask { prompt, mode, attach }) => ask(prompt, mode, attach),
        Err(e) => fail(&e, 2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(a: &[&str]) -> Vec<String> { a.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn a_mistyped_flag_is_an_error_not_part_of_the_prompt() {
        match parse(args(&["ask", "--attch", "notes.md", "q"])) {
            Err(e) => assert!(e.contains("unknown option --attch"), "{e}"),
            Ok(_) => panic!("--attch was accepted as part of the prompt"),
        }
    }

    #[test]
    fn a_prompt_may_start_with_a_dash_or_follow_double_dash() {
        let prompt = |a: &[&str]| match parse(args(a)) { Ok(Command::Ask { prompt, .. }) => prompt, _ => panic!("not an ask") };
        assert_eq!(prompt(&["ask", "-5 degrees is cold, why?"]), "-5 degrees is cold, why?");
        assert_eq!(prompt(&["ask", "--", "--attch is a flag name"]), "--attch is a flag name");
    }
}
