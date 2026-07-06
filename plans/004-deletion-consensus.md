# Plan 004: Let watch mode complete when a record has been deleted everywhere

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat c298021..HEAD -- src/app.rs src/main.rs`
> Plans 001/002/005 touch these files (fmt, sanitization, arg parsing). The
> excerpts below are from `summary()` and the watch-stop condition — if
> THOSE specific regions changed, treat it as a STOP condition.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW
- **Depends on**: plans/001-ci-gate.md (soft — gates only)
- **Category**: bug
- **Planned at**: commit `c298021`, 2026-07-05

## Why this matters

Watch mode's promise is "re-polls until the record has propagated
everywhere" (README). That works for record *creation and changes* — but for
**deletion** it never completes. When all resolvers return NXDOMAIN,
`summary.agree` stays 0 (agreement is only computed over rows that returned
records), so the stop condition `agree == responding` is unsatisfiable: the
gauge sits at red `0/34 (0%)` and the app re-polls every 30 s forever — even
though "gone everywhere" IS full propagation. Consensus-on-absence must be
representable.

## Current state

- `src/app.rs` — `Summary` struct and `App::summary()` compute answer groups
  via union-find over rows that returned records (`ok_rows`); rows with
  `NoRecords` only increment `summary.no_records` and count as `responding`.
- `src/main.rs:85-96` — the round-completion handler with the watch-stop
  condition.

The stop condition, `src/main.rs:88-95` (comment included):

```rust
                    // Round complete: stop watching once every responding
                    // resolver agrees (refused/unreachable ones carry no
                    // propagation signal), otherwise schedule the next poll.
                    let summary = app.summary();
                    if summary.responding > 0 && summary.agree == summary.responding {
                        app.auto_refresh = false;
                        app.next_poll = None;
                    } else if app.rows... (unchanged)
```

The relevant end of `App::summary()`, `src/app.rs:237-256`:

```rust
        if let Some(root) = majority_root {
            summary.agree = counts[&root];
            let mut union: Vec<String> = Vec::new();
            for &i in &ok_rows {
                if find(&mut parent, i) == root {
                    summary.majority_rows[i] = true;
                    ...
                }
            }
            union.sort();
            union.dedup();
            summary.majority_values = union;
        }
        summary
```

Semantics that must be preserved:
- `NoRecords` (NXDOMAIN / NOERROR-empty) counts toward `responding` — it is
  a real propagation signal. There is an existing test asserting a mixed
  records+NXDOMAIN round does NOT count as full propagation
  (`nxdomain_counts_as_responding_and_blocks_full_propagation`). That test
  must keep passing — mixed state means a deletion (or creation) is still
  propagating.
- `Error` rows (refused/timeout) carry no signal and must not block
  completion (test `unreachable_resolvers_do_not_block_full_propagation`).

## Commands you will need

| Purpose   | Command                                      | Expected on success        |
|-----------|----------------------------------------------|----------------------------|
| Tests     | `cargo test`                                 | all pass                   |
| Lint      | `cargo clippy --all-targets -- -D warnings`  | exit 0                     |
| Format    | `cargo fmt --check`                          | exit 0                     |

## Scope

**In scope** (the only files you should modify):
- `src/app.rs` (`summary()` + new tests)

**Out of scope** (do NOT touch, even though they look related):
- `src/main.rs` — the stop condition `agree == responding` becomes
  satisfiable by this change; it does not itself need to change.
- `src/ui.rs` — row rendering for `NoRecords` stays red `∅ NONE` (per-row
  truth: "this resolver sees nothing"), and the map dot stays red. Only the
  aggregate (gauge/completion) changes meaning. This is a deliberate design
  choice — do not make NONE rows green.
- `run_once` output formatting in `src/main.rs`.

## Git workflow

- Branch: `advisor/004-deletion-consensus`
- Commit style: short imperative summary (match `git log`).
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Treat unanimous absence as consensus in `summary()`

In `src/app.rs`, at the end of `summary()` — after the existing
`if let Some(root) = majority_root { ... }` block, immediately before
`summary` is returned — add:

```rust
        // A deletion that has propagated everywhere is full agreement:
        // every responding resolver says "nothing there". Absence only
        // counts as consensus when it is unanimous — any row still holding
        // records means propagation is in progress, and the record-holding
        // group stays the majority.
        if summary.ok == 0 && summary.no_records > 0 {
            summary.agree = summary.no_records;
            summary.groups = 1;
        }
        summary
```

Notes:
- `majority_rows` stays all-false and `majority_values` stays empty in this
  case — nothing downstream renders "the majority answer" for absence
  (ui.rs only prints majority_values when non-empty, which remains correct).
- `groups = 1` makes the footer read `1 answer group(s)` instead of `0` —
  the one shared view is "absent".

**Verify**: `cargo test` → all existing tests pass, in particular
`nxdomain_counts_as_responding_and_blocks_full_propagation` (mixed state
still blocks) and `unreachable_resolvers_do_not_block_full_propagation`.

### Step 2: Add regression tests

In the `#[cfg(test)] mod tests` module of `src/app.rs`, add:

```rust
    #[test]
    fn unanimous_nxdomain_counts_as_full_propagation() {
        // A record deleted everywhere: all responding resolvers say
        // "nothing there" — that IS 100% propagation, and watch mode's
        // stop condition (agree == responding) must be satisfiable.
        let mut app = App::new("example.com".into());
        app.rows = (0..RESOLVERS.len())
            .map(|_| RowState::Done {
                result: QueryResult::NoRecords("NXDOMAIN".into()),
                elapsed: Duration::from_millis(20),
            })
            .collect();
        let s = app.summary();
        assert_eq!(s.ok, 0);
        assert_eq!(s.responding, RESOLVERS.len());
        assert_eq!(s.agree, s.responding);
        assert_eq!(s.groups, 1);
        assert!(s.majority_values.is_empty());
    }

    #[test]
    fn unanimous_absence_with_errors_still_completes() {
        // Unreachable resolvers carry no signal for deletions either.
        let mut app = App::new("example.com".into());
        app.rows = (0..RESOLVERS.len() - 1)
            .map(|_| RowState::Done {
                result: QueryResult::NoRecords("NXDOMAIN".into()),
                elapsed: Duration::from_millis(20),
            })
            .collect();
        app.rows.push(RowState::Done {
            result: QueryResult::Error("refused".into()),
            elapsed: Duration::from_secs(3),
        });
        let s = app.summary();
        assert_eq!(s.responding, RESOLVERS.len() - 1);
        assert_eq!(s.agree, s.responding);
    }
```

**Verify**: `cargo test` → all pass (2 more than before this plan).

### Step 3: Full gate

**Verify**:
- `cargo clippy --all-targets -- -D warnings` → exit 0
- `cargo fmt --check` → exit 0

## Test plan

- `unanimous_nxdomain_counts_as_full_propagation` — the fixed behavior.
- `unanimous_absence_with_errors_still_completes` — errors don't block
  deletion consensus (mirrors the existing error test for records).
- Existing `nxdomain_counts_as_responding_and_blocks_full_propagation` is
  the guard that mixed state still blocks — it must remain untouched and
  passing.
- Pattern: model after the existing tests in the same module (they build
  `app.rows` directly and assert on `summary()`).

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo test` exits 0; the 2 new tests exist and pass; the existing
      mixed-state NXDOMAIN test is unmodified (verify with
      `git diff --stat -- src/app.rs` touching only summary() + tests)
- [ ] `cargo clippy --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --check` exits 0
- [ ] `git status` shows no modified files outside the in-scope list
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- `summary()` no longer ends with the `if let Some(root) = majority_root`
  block shown in "Current state" (the aggregation was refactored).
- The existing mixed-state test fails after Step 1 — the guard
  `summary.ok == 0` should make the new branch unreachable in mixed rounds;
  a failure means the change is in the wrong place.
- You find yourself wanting to modify `src/ui.rs` or `src/main.rs` to make
  the behavior feel complete — that's out of scope; report the impulse
  instead (see Maintenance for the deliberate UI decision).

## Maintenance notes

- Deliberate UX decision: with a fully-propagated deletion the gauge shows
  green `34/34 (100%) · complete` while every row shows red `∅ NONE`. Red
  rows are per-resolver truth; the green gauge is the aggregate verdict. If
  users find this confusing, a follow-up could recolor NONE rows when
  absence is the consensus — UI-only change, isolated to `draw_table` /
  `draw_map` in `src/ui.rs`.
- If a future "expected value" feature is added (user declares what the
  record SHOULD be), this consensus logic is where "expected: absent" would
  plug in.
- Reviewers: check the new branch runs only when `ok == 0` — partial
  absence must keep blocking completion.
