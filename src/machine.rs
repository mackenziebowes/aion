//! One call to Aion as a state machine. `step` takes a state and an event and gives the next state and the effects;
//! it does no I/O. The adapters in main.rs and pi.rs turn the world into events and carry out the effects.

use std::fmt;

/// Money in millionths of a dollar: integers, so caps compare exactly.
#[derive(Clone, Copy, PartialEq, PartialOrd, Debug, Default)]
pub struct Usd(pub u64);

impl Usd {
    pub fn from_dollars(d: f64) -> Usd { Usd((d * 1e6).round().max(0.0) as u64) }
    pub fn plus(self, o: Usd) -> Usd { Usd(self.0 + o.0) }
}
impl fmt::Display for Usd {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { write!(f, "${:.4}", self.0 as f64 / 1e6) }
}

/// The caller's to change (budget.toml `[aion]`).
#[derive(Clone, Copy, Debug)]
pub struct Caps { pub per_call: Usd, pub per_day: Usd, pub per_month: Usd }

#[derive(Clone, Copy, Debug, Default)]
pub struct Spent { pub today: Usd, pub month: Usd }

#[derive(Clone, Debug, PartialEq)]
pub enum Refusal { DayCap { spent: Usd, cap: Usd }, MonthCap { spent: Usd, cap: Usd }, TooBig { estimate: Usd, cap: Usd } }

/// Input price in USD per million tokens (seen 13:13: 550 tokens cost $0.00165), at about 4 bytes a token. An estimate, to refuse a hopeless call before sending it.
pub fn estimate_input(bytes: u64) -> Usd { Usd(bytes / 4 * 3) }

#[derive(Clone, Copy, Debug, Default)]
pub struct Tally { pub cost: Usd, pub tokens_in: u64, pub tokens_out: u64 }

#[derive(Clone, Debug)]
pub enum State {
    Checking,
    Refused(Refusal),
    Running { tally: Tally, text: String, stop: String },
    Finished { tally: Tally, text: String },
    /// Killed at a turn end because the call reached its cap; `text` is what Aion had said by then.
    Capped { tally: Tally, text: String },
    Failed { tally: Tally, why: String },
}

pub enum Event {
    /// Spend so far, and the bytes about to be sent (prompt, persona and attachments).
    Spent(Spent, u64),
    /// One finished turn of Aion's (a turn that calls a tool is a turn too).
    TurnEnded { tally: Tally, text: String, stop: String },
    Exited { ok: bool, stderr: String },
}

#[derive(PartialEq, Debug)]
pub enum Effect { Spawn, Kill }

pub fn step(state: State, event: Event, caps: Caps) -> (State, Vec<Effect>) {
    use Effect::*;
    match (state, event) {
        (State::Checking, Event::Spent(s, bytes)) => {
            if s.today >= caps.per_day { return (State::Refused(Refusal::DayCap { spent: s.today, cap: caps.per_day }), vec![]); }
            if s.month >= caps.per_month { return (State::Refused(Refusal::MonthCap { spent: s.month, cap: caps.per_month }), vec![]); }
            let estimate = estimate_input(bytes);
            if estimate >= caps.per_call { return (State::Refused(Refusal::TooBig { estimate, cap: caps.per_call }), vec![]); }
            (State::Running { tally: Tally::default(), text: String::new(), stop: String::new() }, vec![Spawn])
        }
        (State::Running { tally, text, .. }, Event::TurnEnded { tally: t, text: said, stop }) => {
            let tally = Tally { cost: tally.cost.plus(t.cost), tokens_in: tally.tokens_in + t.tokens_in, tokens_out: tally.tokens_out + t.tokens_out };
            // The last turn's text is the answer; earlier turns were Aion reading files.
            let text = if said.trim().is_empty() { text } else { said };
            if tally.cost >= caps.per_call { (State::Capped { tally, text }, vec![Kill]) } else { (State::Running { tally, text, stop }, vec![]) }
        }
        (State::Running { tally, text, stop }, Event::Exited { ok, stderr }) => {
            if ok && !text.trim().is_empty() { (State::Finished { tally, text }, vec![]) }
            else {
                let why = if !stderr.trim().is_empty() { stderr }
                    else if ok { format!("Aion returned no text (stop reason: {}); run it again", if stop.is_empty() { "none" } else { &stop }) }
                    else { "pi exited without an answer".into() };
                (State::Failed { tally, why }, vec![])
            }
        }
        // A capped call ends when pi dies; a late event after a final state changes nothing.
        (s, _) => (s, vec![]),
    }
}
