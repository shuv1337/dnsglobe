# Plan 003: Upgrade hickory-resolver to 0.26 (fixes RUSTSEC-2026-0119) and ratatui to 0.30

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat c298021..HEAD -- src/dns.rs src/ui.rs Cargo.toml`
> Plans 001 (rustfmt) and 002 (`short_error` change) touch `src/dns.rs` —
> that drift is expected; the replacement file below already incorporates
> both. Any drift beyond those two plans' changes is a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED
- **Depends on**: plans/001-ci-gate.md (gates), plans/002-non-ascii-panic.md
  (its `short_error` fix is baked into the replacement `dns.rs` below —
  execute 002 first or reconcile)
- **Category**: security / migration
- **Planned at**: commit `c298021`, 2026-07-05

## Why this matters

`cargo audit` flags **RUSTSEC-2026-0119** — CPU exhaustion via O(n²) name
compression in `hickory-proto 0.24.4`, fixed only in ≥ 0.26.1. Practical
exploitability in dnsglobe is low, but every downstream `cargo audit` /
`cargo deny` gate now fails for anyone depending on or packaging this crate.
The upgrade path is hickory-resolver 0.24 → 0.26, which is a full API rework
of the one file that touches DNS (`src/dns.rs`). Riding along: ratatui
0.29 → 0.30 + crossterm 0.28 → 0.29 (drops the unmaintained `paste` dep),
and a fix for the audit finding F-06 (the old outer 4 s timeout could
pre-empt hickory's UDP→TCP truncation retry; the new outer backstop is 7 s).

**This entire migration was compile- and runtime-verified by the advisor in a
scratch copy of the repo**: `cargo build` clean, all 5 tests pass, live
`--once example.com A` query returns correct answers from real resolvers, and
`cargo audit` reports 0 vulnerabilities afterward. The `dns.rs` below is that
verified file — not a sketch.

## Current state

- `Cargo.toml:15-21` — deps today:
  `crossterm = { version = "0.28", features = ["event-stream"] }`,
  `hickory-resolver = "0.24"`, `ratatui = "0.29"`.
- `src/dns.rs` — the only file using hickory. Today it uses
  `TokioAsyncResolver::tokio(config, opts)`, `ResolverConfig::new()` +
  `add_name_server(NameServerConfig::new(addr, Protocol::Udp/Tcp))`, and
  matches on `ResolveErrorKind::NoRecordsFound { response_code, .. }`.
  **All of those are gone in 0.26.**
- `src/ui.rs:2` — `use ratatui::style::{Color, Modifier, Style, Stylize};`
  — under ratatui 0.30 `Stylize` becomes an unused import (style methods
  are inherent now); everything else in ui.rs compiles unchanged.
- `src/main.rs` and `src/app.rs` — no changes needed (verified).

Key 0.24 → 0.26 API mapping (all verified against the published 0.26.1
source):

| 0.24 | 0.26 |
|------|------|
| `TokioAsyncResolver::tokio(config, opts)` | `Resolver::builder_with_config(config, TokioRuntimeProvider::default())` … `.build()?` |
| `ResolverConfig::new()` | `ResolverConfig::default()` |
| `NameServerConfig::new(sockaddr, Protocol::Udp)` ×2 | `NameServerConfig::udp_and_tcp(ip_addr)` (one entry, UDP+TCP connections, `trust_negative_responses: true`) |
| `opts.use_hosts_file = false` | `opts.use_hosts_file = ResolveHosts::Never` |
| `ResolverOpts { .. }` struct literal | `#[non_exhaustive]` — mutate via `builder.options_mut()` |
| `err.kind()` → `ResolveErrorKind::NoRecordsFound { response_code, .. }` | error is `NetError`; match `NetError::Dns(DnsError::NoRecordsFound(no_records))`, code at `no_records.response_code` |
| `lookup.record_iter()` | `lookup.answers()` (returns `&[Record]`) |
| `record.data()` → `Option<&RData>`, `record.ttl()` | public fields: `record.data`, `record.ttl` |

Behavior notes the executor should NOT "fix":
- `NameServerConfig::udp_and_tcp` sets `trust_negative_responses: true`
  (0.24 code had it false). With a single upstream server this only skips
  pointless retries of the same server on negative answers — acceptable and
  intended.
- The outer timeout changes from `QUERY_TIMEOUT + 1s` (4 s) to
  `QUERY_TIMEOUT * 2 + 1s` (7 s) **deliberately** — that is finding F-06's
  fix (headroom for the TCP retry after a truncated UDP answer). Worst-case
  per-resolver wall time rises from 4 s to 7 s; rows arrive independently,
  so the UI is unaffected.

## Commands you will need

| Purpose   | Command                                      | Expected on success          |
|-----------|----------------------------------------------|------------------------------|
| Build     | `cargo build`                                | exit 0, no warnings          |
| Tests     | `cargo test`                                 | all pass                     |
| Lint      | `cargo clippy --all-targets -- -D warnings`  | exit 0                       |
| Format    | `cargo fmt --check`                          | exit 0                       |
| Audit     | `cargo audit`                                | 0 vulnerabilities            |
| Smoke     | `timeout 20 cargo run -- --once example.com A` | table with `OK` rows (needs network) |

## Scope

**In scope** (the only files you should modify):
- `Cargo.toml` (three version bumps)
- `Cargo.lock` (regenerated by cargo)
- `src/dns.rs` (replace with the verified file below)
- `src/ui.rs` (remove `Stylize` from one import line — nothing else)

**Out of scope** (do NOT touch, even though they look related):
- `src/main.rs`, `src/app.rs`, `src/resolvers.rs` — verified to compile
  unchanged against the new deps.
- Enabling hickory TLS/HTTPS features (DoH/DoT) — direction finding D-03,
  separate decision.
- `dist-workspace.toml` / release workflow.

## Git workflow

- Branch: `advisor/003-hickory-026`
- Commit style: short imperative summary (match `git log`).
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Bump dependencies in `Cargo.toml`

Change exactly these three lines in `[dependencies]`:

```toml
crossterm = { version = "0.29", features = ["event-stream"] }
hickory-resolver = "0.26"
ratatui = "0.30"
```

(ratatui 0.30 uses crossterm 0.29; keeping our direct crossterm at 0.28
would produce two crossterm versions and type mismatches in the event loop.)

**Verify**: `cargo fetch` → exit 0.

### Step 2: Replace `src/dns.rs`

Replace the **entire file** with the following verified content. If plan 002
has landed, its `short_error` change is already reflected here (the
`message.chars().take(48).collect()` branch). If plan 002 has NOT landed,
this file is still correct — but flag it in your report so plan 002's Step 3
is marked done-by-003.

```rust
use std::net::IpAddr;
use std::time::{Duration, Instant};

use hickory_resolver::config::{NameServerConfig, ResolveHosts, ResolverConfig};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::net::{DnsError, NetError};
use hickory_resolver::proto::op::ResponseCode;
use hickory_resolver::proto::rr::RecordType;
use hickory_resolver::Resolver;

const QUERY_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub enum QueryResult {
    /// Record values (rdata strings) and the minimum TTL seen.
    Records { values: Vec<String>, min_ttl: u32 },
    /// The server answered that the record does not exist (NXDOMAIN or
    /// NOERROR with an empty answer section). This is a real propagation
    /// signal — the server's view is "nothing there" — so it counts toward
    /// the responding total.
    NoRecords(String),
    /// No usable answer: timeout, network error, or the server refused to
    /// serve us (REFUSED/SERVFAIL). Says nothing about propagation, so these
    /// are excluded from the percentage.
    Error(String),
}

#[derive(Debug)]
pub struct QueryOutcome {
    pub resolver_index: usize,
    pub generation: u64,
    pub result: QueryResult,
    pub elapsed: Duration,
}

/// Query a single upstream resolver directly (no cache, single attempt) so
/// each server's own view of the record is what we measure.
pub async fn query(server: IpAddr, domain: String, rtype: RecordType) -> (QueryResult, Duration) {
    let mut config = ResolverConfig::default();
    // udp_and_tcp: UDP primary, TCP retry when a UDP answer comes back
    // truncated (large TXT sets, long MX lists, …).
    config.add_name_server(NameServerConfig::udp_and_tcp(server));

    let mut builder = Resolver::builder_with_config(config, TokioRuntimeProvider::default());
    // ResolverOpts is #[non_exhaustive]; mutate fields through the builder.
    let opts = builder.options_mut();
    opts.timeout = QUERY_TIMEOUT;
    opts.attempts = 1;
    opts.cache_size = 0;
    opts.use_hosts_file = ResolveHosts::Never;
    opts.edns0 = true; // allow >512-byte UDP answers
    let resolver = match builder.build() {
        Ok(resolver) => resolver,
        Err(err) => {
            return (
                QueryResult::Error(short_error(&err.to_string())),
                Duration::ZERO,
            )
        }
    };

    let start = Instant::now();
    // Outer backstop only: generous enough that hickory's own per-attempt
    // timeout and UDP→TCP truncation retry always finish first.
    let lookup = tokio::time::timeout(
        QUERY_TIMEOUT * 2 + Duration::from_secs(1),
        resolver.lookup(domain.as_str(), rtype),
    )
    .await;
    let elapsed = start.elapsed();

    let result = match lookup {
        Err(_) => QueryResult::Error("timeout".into()),
        Ok(Err(err)) => match &err {
            NetError::Dns(DnsError::NoRecordsFound(no_records)) => {
                match no_records.response_code {
                    // "Won't serve you" / "couldn't resolve" — not a statement
                    // about whether the record exists.
                    ResponseCode::Refused => QueryResult::Error("refused".into()),
                    ResponseCode::ServFail => QueryResult::Error("SERVFAIL".into()),
                    code => QueryResult::NoRecords(code.to_string()),
                }
            }
            NetError::Dns(DnsError::ResponseCode(ResponseCode::Refused)) => {
                QueryResult::Error("refused".into())
            }
            NetError::Dns(DnsError::ResponseCode(ResponseCode::ServFail)) => {
                QueryResult::Error("SERVFAIL".into())
            }
            other => QueryResult::Error(short_error(&other.to_string())),
        },
        Ok(Ok(lookup)) => {
            let mut values: Vec<String> = Vec::new();
            let mut min_ttl = u32::MAX;
            for record in lookup.answers() {
                let data = &record.data;
                min_ttl = min_ttl.min(record.ttl);
                // A lookup can carry other types too (e.g. the CNAME hops on
                // the way to an A record); label those so answers stay
                // comparable across resolvers.
                if record.record_type() == rtype {
                    values.push(data.to_string());
                } else {
                    values.push(format!("{} {}", record.record_type(), data));
                }
            }
            values.sort();
            values.dedup();
            if values.is_empty() {
                QueryResult::NoRecords("empty answer".into())
            } else {
                QueryResult::Records { values, min_ttl }
            }
        }
    };

    (result, elapsed)
}

fn short_error(message: &str) -> String {
    let msg = message.to_ascii_lowercase();
    if msg.contains("timed out") || msg.contains("timeout") {
        "timeout".into()
    } else if msg.contains("refused") {
        "refused".into()
    } else {
        // Char-boundary-safe: byte-indexed truncate can panic mid-codepoint.
        message.chars().take(48).collect()
    }
}
```

**Verify**: `cargo build` → fails ONLY with `unused import: Stylize` in
`src/ui.rs` — or succeeds with that as a warning. Any other error is a STOP
condition.

### Step 3: Fix the ui.rs import

Change `src/ui.rs:2` from:

```rust
use ratatui::style::{Color, Modifier, Style, Stylize};
```

to:

```rust
use ratatui::style::{Color, Modifier, Style};
```

**Verify**: `cargo build` → exit 0, zero warnings.

### Step 4: Full gate + audit

**Verify**:
- `cargo test` → all pass (5, or 7 if plan 002 landed first)
- `cargo clippy --all-targets -- -D warnings` → exit 0
- `cargo fmt --check` → exit 0
- `cargo audit` → **0 vulnerabilities** (RUSTSEC-2026-0119 gone; the
  unmaintained-`paste` warning also disappears with ratatui 0.30)
- `grep 'name = "hickory-proto"' -A1 Cargo.lock` → `version = "0.26.x"`

### Step 5: Live smoke test (needs network)

```
timeout 20 cargo run -- --once example.com A
```

Expected: a resolver table where the majority of rows show `OK` with
identical answer values (the advisor's verification run showed Google,
Cloudflare, Quad9, OpenDNS all `OK` in ~16 ms). Some `ERR` rows are normal
(networks that block your region). If **every** row is `ERR`, check general
network connectivity before concluding the migration broke queries.

## Test plan

No new unit tests — `src/dns.rs` has no test seam (it does live network
I/O), and the existing `src/app.rs` suite covers everything downstream of
`QueryResult`. The verification story is the compile gate + the live smoke
test in Step 5. (A mock-server test harness for `dns.rs` would be nice but
is out of scope; noted in Maintenance.)

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `Cargo.toml` has hickory-resolver 0.26, ratatui 0.30, crossterm 0.29
- [ ] `cargo build` exits 0 with zero warnings
- [ ] `cargo test` exits 0
- [ ] `cargo clippy --all-targets -- -D warnings` exits 0
- [ ] `cargo audit` reports 0 vulnerabilities
- [ ] `timeout 20 cargo run -- --once example.com A` prints rows with `OK`
- [ ] `git status` shows no modified files outside the in-scope list
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- `cargo build` after Step 2 fails with anything other than the `Stylize`
  unused-import issue — the published 0.26.x API has moved past what this
  plan verified (it was verified against hickory-resolver **0.26.1**;
  pin `hickory-resolver = "=0.26.1"` and retry once before stopping).
- ratatui 0.30 breaks anything in `src/ui.rs` beyond the import (the
  advisor's scratch build compiled ui.rs clean except for that import; more
  breakage means a newer 0.30.x changed API).
- The Step 5 smoke test shows all rows `ERR` **and** `ping -c1 8.8.8.8`
  works — that suggests the query path itself regressed.
- Plan 002 landed with a different `short_error` implementation than the one
  baked in here — reconcile with the operator instead of picking one.

## Maintenance notes

- `lru` (unsound-advisory *warning*, RUSTSEC-2026-0002) may remain as a
  transitive dep of ratatui — it's a warning, not a vulnerability; do not
  chase it in this plan.
- hickory 0.26 puts DoH/DoT/DoQ behind feature flags (`ConnectionConfig::tls/
  https/quic`) — this unlocks direction finding D-03 (encrypted transports,
  possibly restoring African resolver coverage) if the maintainer wants it.
- Future: a `dns.rs` test seam (in-process mock DNS server, e.g. binding a
  UDP socket on localhost and answering one query) would let the
  REFUSED/SERVFAIL/NXDOMAIN classification be unit-tested. Deferred.
- Reviewers: scrutinize the error-classification match — the REFUSED/
  SERVFAIL→`Error` vs NXDOMAIN/NOERROR→`NoRecords` split is what the whole
  propagation-percentage semantic rests on.
