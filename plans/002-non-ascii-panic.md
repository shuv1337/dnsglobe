# Plan 002: Stop non-ASCII CLI domains from panicking the TUI

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat c298021..HEAD -- src/app.rs src/main.rs src/dns.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding. Note:
> plan 001 reformats `src/dns.rs`/`src/main.rs` (line wrapping only) — that
> drift is expected and fine. Anything beyond formatting is a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: plans/001-ci-gate.md (soft — only for the CI gate; the code
  changes are independent)
- **Category**: bug
- **Planned at**: commit `c298021`, 2026-07-05

## Why this matters

The TUI's input model assumes the domain string is pure ASCII, so
byte-index == char-index cursor math is safe. Keystrokes are filtered to
guarantee that — but **the CLI argument bypasses the filter**. Running
`dnsglobe müller.de` and pressing ← then any edit key panics with
`byte index is not a char boundary` (confirmed by a standalone repro during
the audit). The app dies on a completely reasonable invocation. The fix
enforces the ASCII invariant at the one place it can be violated, and
removes a second latent panic of the same class in error truncation.

## Current state

- `src/app.rs` — TUI state. `App::new` seeds the cursor from the raw string;
  all cursor/edit ops do byte-indexed `String` operations.
- `src/main.rs` — passes the raw CLI arg into `App::new` (`main.rs:43`:
  `let initial_domain = args.next().unwrap_or_default();`). The keystroke
  filter that normally guarantees ASCII is at `main.rs:152-156`.
- `src/ui.rs:73` — `app.domain.split_at(app.cursor.min(app.domain.len()))` —
  panics when `cursor` lands mid-codepoint. (No change needed here once the
  invariant holds; do not modify ui.rs.)
- `src/dns.rs:99-110` — `short_error` does `m.truncate(48)`, which panics if
  byte 48 is not a char boundary (latent — same bug class).

`src/app.rs:53-57` documents the invariant:

```rust
pub struct App {
    pub domain: String,
    /// Cursor position in `domain`. The input only accepts ASCII
    /// (alphanumerics, `.`, `-`, `_`), so byte index == char index.
    pub cursor: usize,
```

`src/app.rs:74-88`, the constructor that trusts its input today:

```rust
    pub fn new(domain: String) -> Self {
        Self {
            cursor: domain.len(),
            domain,
            ...
```

`src/dns.rs:99-110`, the latent truncation panic:

```rust
fn short_error(message: &str) -> String {
    let msg = message.to_ascii_lowercase();
    if msg.contains("timed out") || msg.contains("timeout") {
        "timeout".into()
    } else if msg.contains("refused") {
        "refused".into()
    } else {
        let mut m = message.to_string();
        m.truncate(48);
        m
    }
}
```

Repo conventions: no external deps for small things (arg parsing is
hand-rolled); tests live in a `#[cfg(test)] mod tests` at the bottom of
`src/app.rs` — match that module's style (small focused `#[test]` fns with
a one-line comment stating the invariant under test).

## Commands you will need

| Purpose   | Command                                      | Expected on success        |
|-----------|----------------------------------------------|----------------------------|
| Tests     | `cargo test`                                 | all pass (5 existing + new)|
| Lint      | `cargo clippy --all-targets -- -D warnings`  | exit 0                     |
| Format    | `cargo fmt --check`                          | exit 0                     |

## Scope

**In scope** (the only files you should modify):
- `src/app.rs` (sanitize in `App::new` + new tests)
- `src/main.rs` (reject non-ASCII in the `--once` path)
- `src/dns.rs` (char-safe truncation in `short_error`)

**Out of scope** (do NOT touch, even though they look related):
- `src/ui.rs` — once the invariant is enforced at construction and input,
  the render code is safe as-is; defensive changes there would hide future
  violations instead of surfacing them in tests.
- IDN/punycode conversion (accepting `münchen.de` by converting to
  `xn--mnchen-3ya.de`) — a feature, not a fix; deferred (see Maintenance).
- The keystroke filter at `src/main.rs:152-156` — already correct.

## Git workflow

- Branch: `advisor/002-non-ascii-panic`
- Commit style: short imperative summary (match `git log`).
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Sanitize the domain in `App::new`

In `src/app.rs`, change `App::new` to filter the incoming string through the
same character set the keystroke filter allows, lowercasing as it goes —
so the ASCII invariant is enforced at the type's boundary:

```rust
    pub fn new(domain: String) -> Self {
        // Enforce the ASCII invariant documented on `cursor`: the CLI arg
        // is the only path that can carry arbitrary bytes, so filter it
        // through the same charset the keystroke filter accepts.
        let domain: String = domain
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            .map(|c| c.to_ascii_lowercase())
            .collect();
        Self {
            cursor: domain.len(),
            domain,
            ...  // rest unchanged
```

**Verify**: `cargo test` → existing 5 tests still pass.

### Step 2: Make the `--once` path honest about non-ASCII input

Silently stripping characters is fine interactively (the user sees the
filtered domain in the input field) but wrong for scripts: `--once münchen.de`
would silently query `mnchen.de`. In `src/main.rs`, in the `--once` branch
(currently `main.rs:30-41`), validate the domain **before** calling
`run_once`:

```rust
        let domain = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("usage: dnsglobe --once <domain> [type]"))?;
        if !domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        {
            anyhow::bail!(
                "domain contains characters outside [a-z0-9.-_]; \
                 for internationalized names use punycode (xn--…)"
            );
        }
```

**Verify**: `cargo run -- --once münchen.de` → exits non-zero, prints the
punycode message, does not query.
**Verify**: `cargo run -- --once example.com` → prints the resolver table
(requires network; if offline, expect a table full of `ERR` rows — that
still proves the arg was accepted).

### Step 3: Char-safe truncation in `short_error`

In `src/dns.rs`, replace the final `else` branch of `short_error`:

```rust
    } else {
        // Char-boundary-safe: byte-indexed truncate can panic mid-codepoint.
        message.chars().take(48).collect()
    }
```

**Verify**: `cargo build` → exit 0.

### Step 4: Add regression tests

In the existing `#[cfg(test)] mod tests` in `src/app.rs`, add:

```rust
    #[test]
    fn cli_domain_is_sanitized_to_ascii() {
        // The CLI arg bypasses the keystroke filter; App::new must enforce
        // the ASCII invariant or cursor math panics (byte != char index).
        let app = App::new("MüLLer.De".into());
        assert_eq!(app.domain, "mller.de");
        assert!(app.domain.is_ascii());
        assert_eq!(app.cursor, app.domain.len());
    }

    #[test]
    fn cursor_ops_never_panic_after_unicode_arg() {
        // Regression: `dnsglobe müller.de` then ← + edit used to panic.
        let mut app = App::new("müller.de".into());
        for _ in 0..20 {
            app.move_cursor_left();
        }
        app.insert_char('x');
        app.backspace();
        app.delete();
        app.move_cursor_right();
        assert!(app.cursor <= app.domain.len());
    }
```

**Verify**: `cargo test` → all pass (5 existing + 2 new = 7).

## Test plan

- `cli_domain_is_sanitized_to_ascii` — the invariant at the boundary.
- `cursor_ops_never_panic_after_unicode_arg` — the exact reported crash
  sequence as a regression test.
- Pattern: model after the existing tests in `src/app.rs` (e.g.
  `round_robin_subsets_form_one_group`) — plain constructors, direct
  assertions, one-line comment stating the invariant.
- Verification: `cargo test` → 7 passing.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo test` exits 0 with 7 passing tests (2 new)
- [ ] `cargo clippy --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --check` exits 0 (only if plan 001 already landed;
      otherwise skip this line)
- [ ] `grep -n "truncate(48)" src/dns.rs` returns no matches
- [ ] `cargo run -- --once münchen.de` exits non-zero with the punycode hint
- [ ] `git status` shows no modified files outside the in-scope list
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- `App::new` in `src/app.rs` no longer matches the "Current state" excerpt
  in structure (someone refactored construction).
- Plan 005 already landed and restructured `main.rs` argument parsing into a
  dedicated function — in that case the Step 2 validation belongs inside
  that parser instead; stop and report so the plans can be reconciled.
- Any existing test fails after Step 1 — the filter should be a no-op for
  ASCII input; a failure means it isn't.

## Maintenance notes

- Deferred feature: real IDN support via punycode conversion (the `idna`
  crate) would let `dnsglobe münchen.de` query `xn--mnchen-3ya.de` instead
  of rejecting/filtering. If added later, do the conversion **before**
  `App::new` and keep the ASCII invariant intact.
- Reviewers: confirm the sanitize filter in Step 1 and the keystroke filter
  at `src/main.rs:152-156` accept the identical character set — if they ever
  diverge, the invariant silently weakens.
- If a future change ever allows non-ASCII in `domain`, every byte-indexed
  operation in `src/app.rs` (insert/remove/split_at usage in ui.rs) must be
  rewritten to char-boundary-aware ops first.
