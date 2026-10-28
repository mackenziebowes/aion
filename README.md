# aion

One capped call to Aion 3.5 (Aion Labs' model, through OpenRouter) from the command line, for any agent or script. Aion has no tools: you attach the files it should speak from, and it answers on stdout. Cost and caps go to stderr.

    aion ask --attach notes.md "Which of these three claims is weakest?"
    aion ask --session reader --persona persona.md "Here is my page. What do you feel first?"   # starts a back-and-forth
    aion ask --session reader "And the second screen?"                                              # the next turn
    aion transcript reader
    aion status

## Install

You need Rust, an [OpenRouter key](https://openrouter.ai/keys) and the [pi agent](https://github.com/earendil-works/pi), which aion runs to talk to the model (tested with pi 1.4.0: `mise use -g pi`).

    cargo install --git https://github.com/mackenziebowes/aion
    export OPENROUTER_API_KEY=...

## What it costs, and the caps

Aion 3.5 on OpenRouter is billed per token. The calls in our log mostly cost about a cent: roughly 2,000 tokens in and 500 to 2,000 out. Caps stop a runaway: US$0.10 a call, $10 a day and $10 a month by default. To change them, write `~/.config/aion/budget.toml` (or point `AION_BUDGET` at a file):

    [aion]
    per_call_usd = 0.25
    per_day_usd = 5
    per_month_usd = 20

A call that would clearly cost more than the per-call cap just to read is refused before it is sent. The day and month caps are checked before every call.

**The per-call cap is a stop after the turn, not a ceiling.** Cost is known only when a turn ends, so a call that reaches the cap is stopped at the end of the turn that reached it. Aion's long answers can cost well over the cap in one turn: in our own log, the ten calls the $0.10 cap stopped had cost between $0.105 and $0.216, up to 2.2 times the cap. Set `per_call_usd` to half of what you can afford to lose on one call.

Every call is logged to `~/.local/share/aion/calls.jsonl` (`AION_DIR` moves it). Exit codes: 0 answered, 1 failed, 2 bad usage, 3 refused at a cap, 4 answered but stopped at the per-call cap. Run `aion help` for everything else.

MIT licensed.
