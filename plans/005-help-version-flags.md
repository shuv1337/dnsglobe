# Plan 005: Add --help/--version and stop querying unknown flags as domains

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat c298021..HEAD -- src/main.rs`
> Plans 001 (fmt + clippy fix) and 002 (--once validation) also touch
> `src/main.rs`. Expected drift: formatting, the collapsed match guard, and
> a non-ASCII validation block inside the `--once` branch. If `main()`'s
> argument handling has been restructured beyond that, STOP.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW
- **Depends on**: plans/002-non-ascii-panic.md (both edit `main()`'s arg
  handling; run 002 first to avoid conflicts)
- **Category**: dx
- **Planned at**: commit `c298021`, 2026-07-05

## Why this matters

dnsglobe is distributed via crates.io, Homebrew, and prebuilt binaries —
users WILL run `dnsglobe --help` and `dnsglobe --version`. Today both launch
the TUI and query the literal string `--help` as a domain (the hand-rolled
parser treats any non-`--once` argument as a domain). There is no way at
all to check the installed version (`strip = true` removes even the
metadata a debugger could find). Fix: a tiny hand-rolled parser — the repo
deliberately has no CLI-framework dependency, and two flags don't justify
adding clap.

## Current state

`src/main.rs:24-48` — all argument handling lives at the top of `main()`:

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1).peekable();

    // `--once <domain> [type]` runs a single check and prints plain text —
    // handy for scripts and for testing without a TTY.
    if args.peek().map(String::as_str) == Some("--once") {
        args.next();
        let domain = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("usage: dnsglobe --once <domain> [type]"))?;
        let rtype = match args.next() {
            Some(t) => RecordType::from_str(&t.to_uppercase())
                .map_err(|_| anyhow::anyhow!("unknown record type: {t}"))?,
            None => RecordType::A,
        };
        return run_once(domain, rtype).await;
    }

    let initial_domain = args.next().unwrap_or_default();
    let terminal = ratatui::init();
    let result = run_tui(terminal, initial_domain).await;
    ratatui::restore();
    result
}
```

(If plan 002 landed, the `--once` branch additionally contains a non-ASCII
domain check — keep it.)

Repo conventions: hand-rolled args (no clap); version available at compile
time via `env!("CARGO_PKG_VERSION")`; errors via `anyhow`; record types
listed in `src/app.rs` `RECORD_TYPES` (A, AAAA, CNAME, MX, NS, TXT, SOA).

## Commands you will need

| Purpose   | Command                                      | Expected on success        |
|-----------|----------------------------------------------|----------------------------|
| Tests     | `cargo test`                                 | all pass                   |
| Lint      | `cargo clippy --all-targets -- -D warnings`  | exit 0                     |
| Format    | `cargo fmt --check`                          | exit 0                     |
| Help      | `cargo run -- --help`                        | usage text, exit 0         |
| Version   | `cargo run -- --version`                     | `dnsglobe 0.1.1`, exit 0   |
| Reject    | `cargo run -- --bogus`                       | error mentioning `--bogus`, exit ≠ 0 |

## Scope

**In scope** (the only files you should modify):
- `src/main.rs`

**Out of scope** (do NOT touch, even though they look related):
- Adding `clap` or any new dependency — deliberate repo convention.
- `README.md` — the usage section is already accurate; `--help` text should
  MATCH it, not replace it.
- Long-form option parsing beyond these flags (no `--type=A` etc.).

## Git workflow

- Branch: `advisor/005-help-version`
- Commit style: short imperative summary (match `git log`).
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Add help/version/unknown-flag handling at the top of `main()`

Insert immediately after `let mut args = ...` and BEFORE the `--once` check:

```rust
    match args.peek().map(String::as_str) {
        Some("-h") | Some("--help") => {
            print!("{HELP}");
            return Ok(());
        }
        Some("-V") | Some("--version") => {
            println!("dnsglobe {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some(flag) if flag != "--once" && flag.starts_with('-') => {
            anyhow::bail!("unknown flag: {flag} (try --help)");
        }
        _ => {}
    }
```

And add the help text as a module-level constant (near the existing
`POLL_INTERVAL` const):

```rust
const HELP: &str = "\
dnsglobe — global DNS propagation checker TUI

USAGE:
  dnsglobe                      start empty, type a domain
  dnsglobe <domain>             query immediately and watch
  dnsglobe --once <domain> [type]
                                no TUI: print results and exit (for scripts)

OPTIONS:
  -h, --help                    print this help
  -V, --version                 print version

Record types: A, AAAA, CNAME, MX, NS, TXT, SOA (default A).
Watch mode re-polls every 30s until propagation reaches 100%.
Keys are listed in the TUI footer and in the README.
";
```

**Verify**: `cargo run -- --help` → prints the text above, exit 0.
**Verify**: `cargo run -- --version` → `dnsglobe 0.1.1` (or current
version), exit 0.
**Verify**: `cargo run -- --bogus` → `Error: unknown flag: --bogus (try --help)`,
exit ≠ 0. Check with `echo $?` → non-zero.
**Verify**: `cargo run -- --once example.com` still works (the `--once`
branch must remain reachable — the unknown-flag arm explicitly excludes it).

### Step 2: Full gate

**Verify**:
- `cargo test` → all pass (no count change; behavior is exercised via the
  run commands above, which are part of Done criteria)
- `cargo clippy --all-targets -- -D warnings` → exit 0
- `cargo fmt --check` → exit 0

## Test plan

No new unit tests: the logic is four match arms in `main()`, and the three
`cargo run` verifications above are stronger (they check the real binary
end-to-end, including exit codes). If a future refactor extracts a
`parse_args` function, port these checks into unit tests then.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo run -- --help` prints usage and exits 0
- [ ] `cargo run -- --version` prints `dnsglobe <version>` and exits 0
- [ ] `cargo run -- --bogus; echo $?` prints an unknown-flag error and a
      non-zero code
- [ ] `cargo run -- --once example.com` still prints the resolver table
- [ ] `cargo test` / clippy `-D warnings` / `fmt --check` all exit 0
- [ ] `git status` shows only `src/main.rs` modified
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- `main()` no longer matches the "Current state" shape (argument handling
  was moved or restructured beyond plans 001/002's documented changes).
- You feel the need to add a dependency (clap, pico-args, lexopt) — that's
  an explicit non-goal here; report instead.
- The `--once` path stops working after Step 1 — the guard ordering is
  wrong; fix the match arm (it must not swallow `--once`) or stop.

## Maintenance notes

- If more flags ever accumulate (e.g. `--json` from direction finding D-01,
  or `--resolvers <file>` from D-02), THAT is the moment to extract a
  proper `parse_args() -> Cli` enum with unit tests — or reconsider a
  zero-dep parser like `lexopt`. Two flags don't justify it; four would.
- The HELP text duplicates facts from README (poll interval, record types).
  If either changes, update both — grep for `re-polls every` to find them.
- Reviewers: verify `-h`/`-V` short forms work and that a bare `-` argument
  is rejected as an unknown flag rather than queried as a domain.
