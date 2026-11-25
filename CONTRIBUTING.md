# Contributing to modeld

## Where to start

1. Read [the as-built architecture](docs/architecture.md) for how the code is
   organised today, and [the roadmap](docs/ROADMAP.md) for the authoritative
   task list. Old planning drafts live in `docs/history/` and are **not**
   specifications.
2. Pick the first `todo` task in the roadmap §9 status board. Do not work from
   an old phase plan in `docs/history/`.
3. Check `docs/ROADMAP.md` §12.3 before starting a change — the issue may
   already be known and assigned to a later task.

## Development environment

- Rust toolchain: MSRV is `1.89` (`rust-version` in the root `Cargo.toml`).
- Components: `rustfmt`, `clippy`, and `llvm-tools-preview` for coverage.
- Node.js is **not** required to run the WebUI; it is only needed for
  `crates/modeld-webui/ui/tests/*.test.js`.
- Python ≥ 3.9 for the optional hook; `python -m pytest tests/ -v` from
  `python/`.
- On some Windows setups with MSVC 14.5x, the bundled ring build script fails
  with `cl.exe: fatal error C1034`. Use `CC=clang-cl CXX=clang-cl` locally. CI
  uses a pinned toolchain and is unaffected.
- If the sandbox refuses `%TEMP%`, set `TMPDIR`, `TMP`, and `TEMP` to a
  project-local directory before running the full suite.

## Branching and commits

- Branch from the current task base: `git switch -c task/<ID>-<slug>`, e.g.
  `task/B9-docs-refactor`.
- One task = one independently revertable commit (or a squashed PR).
- Commit messages follow Conventional Commits and carry the task ID:
  ```text
  fix(core): replace delete-then-link with atomic rename [A1]

  What changed, why, and how it was verified.

  Refs: docs/ROADMAP.md#A1
  ```
- Allowed types: `feat`, `fix`, `perf`, `refactor`, `test`, `docs`, `build`,
  `chore`, `security`. Scopes: `core`, `cli`, `proxy`, `webui`, `client`, `py`,
  `ci`, `docs`.

## Required checks

Run these before opening a PR:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p modeld-webui --test api_e2e
cargo test -p modeld-core --test interruption_matrix
cargo test -p modeld-cli --test environment
cd crates/modeld-webui/ui && npm test
cd python && python -m pytest tests/ -v
```

Coverage and performance gates (also enforced in CI):

```bash
python3 scripts/coverage.py          # core line coverage must stay >= 75%
cargo bench -p modeld-core
python3 scripts/bench_gate.py        # fail on >20% regression; baselines are per platform
```

## Code conventions

- No `unwrap()` / `expect()` / `panic!()` in library crates for new code.
- No silent error swallowing. `let _ = ...` is allowed only for cleanup or
  best-effort telemetry and must carry a comment explaining why.
- Never replace a file by deleting it first: use a same-directory temporary
  name plus an atomic rename (`StagedLink` in `modeld-core/src/links.rs`).
- Library crates must not print to stdout. Logs go to stderr; `--json` output
  must stay parseable.
- Every layout-mutating command must take the store lock through
  `modeld-cli/src/util.rs::lock_store` and expose `--wait`.
- New HTTP routes need an HTTP-level e2e test. `api_e2e.rs` parses
  `src/api/mod.rs` at compile time to prevent silent route additions.
- Add an ADR under `docs/adr/` when a decision changes storage, transaction,
  authentication, statistics, or HF compatibility semantics.

## Documentation policy

- `docs/ROADMAP.md` is the only authoritative plan. Do not create new phase
  plans at the repository root.
- Root Markdown is limited to `README.md`, `CHANGELOG.md`, `LICENSE`,
  `CONTRIBUTING.md`, and `SECURITY.md`; historical drafts belong in
  `docs/history/`.
- Documentation must describe as-built behaviour. If a capability is planned
  but absent, say so explicitly and add it to roadmap §12.3 rather than
  writing it in the present tense.
- Update both `CHANGELOG.md` and the four required roadmap locations
  (acceptance items, §9 status board, §10 change log, §11 progress) when a task
  completes.

## Pull request checklist

- [ ] Scope matches one roadmap task; no unrelated refactors.
- [ ] All checks above pass locally.
- [ ] New behaviour has tests that fail before the fix.
- [ ] No new library `unwrap` / `expect` / `println!`.
- [ ] Docs and ADRs match the implemented behaviour.
- [ ] Roadmap acceptance boxes, status board, change log, and progress updated.
- [ ] `CHANGELOG.md` updated for breaking or security-relevant changes.
