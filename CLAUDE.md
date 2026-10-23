# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

pcron: a cron where each command's delay until its next run is drawn from a probability distribution.
Bin-only Rust crate (edition 2024, MSRV 1.85). User-facing docs: `README.md` and `src/help.txt`.

## Commands

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test   # what CI runs
cargo test <name>                     # one unit test (all unit tests live in src/)
cargo test --test server <name>       # one integration test (tests/server.rs, tests/cli.rs)
rustup run 1.85 cargo test            # MSRV check (rust-version in Cargo.toml)
cargo run -- man | man -l -           # render the man page
cargo publish --dry-run               # before a release
```

Run the server in isolation: `PCRON_TAB=/tmp/t/tab XDG_STATE_HOME=/tmp/t/state cargo run -- server`.

## Architecture

Tab pipeline: `file/lexer.rs` → `file/parser.rs` → `file.rs` → `TabCmd` (`tab.rs`).
- The lexer is nom plus hand-written scanners. Numbers are scanned by hand because nom's `recognize` broke at EOF.
- The parser is hand-written recursive descent: `Cmd := NAME? Expr WINDOW? SCRIPT`. A distribution is an `Expr::Call`.
- `file.rs` folds the constant parts of the delay into a `Delay` tree, parses the `@` window (`window.rs`) and reads
  the `PCRON_*` lines into `Settings`.
- A tab is the file `X` plus `X.d/*.tab`. Each file is parsed on its own (no variable inheritance), and names are
  unique across files.

Scheduling: `schedule.rs` (`next_run`, `open_at`) is shared by the server and `list --simulate`. Delays are clamped by
`DelayBounds`. A windowed delay counts only open time, computed in the zone's naive local time (`window::Zone`).

Server (`server.rs`):
- One tokio task per command, grouped in `Loops` by `CmdKey` (`state.rs`). On SIGHUP, commands whose key is unchanged
  keep their loop and timer. Anything that should restart a command when it changes must be part of `CmdKey`.
- Loops wait on wall-clock time in steps of at most 60s, so suspends and clock changes don't throw them off.
- Each script runs in its own process group. `watch_child` handles `PCRON_TIMEOUT`, the shutdown `Phase` (a `watch`
  channel), output forwarding and `PCRON_ON_FAILURE` hooks. Hooks never fire during shutdown or for a failed hook.
- Runs of the same command may overlap on purpose, like cron.
- Next runs are persisted by `state::Timers` (debounced JSON in `$PCRON_STATE` or
  `$XDG_STATE_HOME/pcron/timers-<fnv1a(tab path)>.json`) and resumed on start.

Other modules: `list.rs` (statistics from 10k seeded samples, `--json`, `--simulate`), `edit.rs` (visudo-like: random
`.NAME.tmp` copy next to the file, validates the whole tab, atomic rename) and `man.rs` (clap_mangen output plus
`help.txt`).

## Conventions

- `PCRON_*` lines in a tab are pcron settings and are not passed to scripts. To add one, add it to `file::SETTINGS`
  and to `settings()` in `file.rs`.
- `src/help.txt` is both `--help`'s `after_long_help` and the man page. `man.rs` converts it to roff: `HEADING:`
  lines become sections, `- ` starts an item, and lines indented by 6 or more spaces are code.
- Log lines are `TAG(args) (label)`. The integration tests match on them, so keep the tags stable.
- Integration tests (`tests/common`) run the binary in a sandbox (`HOME`, `XDG_STATE_HOME`, `PCRON_TAB`, `TZ=UTC`).
  Wait for a log line rather than sleeping. A script that traps signals must print a line once it is ready, and the
  test waits for that line.
- Releases: `package.version` stays at the last published version and is bumped in the release commit. Tags are
  `vX.Y.Z`.
- Commit messages are in Spanish, with conventional prefixes (`feat:`, `fix:`, `chore:`, `docs:`).
