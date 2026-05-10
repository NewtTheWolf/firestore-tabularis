# Contributing

Thanks for taking a look! This driver is small enough that any reasonable
patch is welcome — bug reports, fixes, new query features, UI extensions,
docs, all of it.

## Getting set up

1. Install Rust stable (`rustup` will pick up `rust-toolchain.toml` and
   pin you to the right version + components).
2. Install [`just`](https://github.com/casey/just) for the dev recipes.
3. For the integration suite: install [`bun`](https://bun.sh) and
   **Java 21+** (`firebase-tools` ≥ v15 needs it). See
   [`docs/integration-tests.md`](docs/integration-tests.md).

```bash
just build              # debug build
just test               # unit tests
just emulator test      # integration suite against a local Firestore emulator
just lint               # clippy with -D warnings
just fmt                # rustfmt
```

`just plugin install` builds and copies the plugin into your local
Tabularis plugin folder so you can iterate against a real Tabularis
install.

## Where to start

- [`docs/ROADMAP.md`](docs/ROADMAP.md) — phase status.
- [`tasks/todo.md`](tasks/todo.md) — active followups, parked ideas, and
  upstream Tabularis issues to file.
- [`docs/superpowers/brainstorms/`](docs/superpowers/brainstorms/) — wide
  catalogue of "things this plugin could become". Pick one that excites
  you.

Good first contributions:

- Extra query features (`COUNT(field IS NOT NULL)`, `LIMIT` cap warning,
  composite-index suggestion polish).
- "Open in Firebase Console" right-click action (once the `ui/` build
  pipeline is bootstrapped).
- Test coverage in `handlers/metadata.rs` (currently relies on the
  integration suite — unit tests via a `FirestoreOps` trait + mockall
  would help).
- Fixing typos / clarifying docs.

## Code style

- `cargo fmt` before committing — there's no review-time leniency, the
  CI lint job (when added) will reject unformatted code.
- `cargo clippy --all-targets -- -D warnings` clean. We don't suppress
  lints unless there's a real reason — comment the `#[allow(…)]` when you
  do.
- Match the existing module shape: pure-logic modules stay free of
  Firestore I/O so they can be unit-tested without mocks; handler modules
  do the I/O but stay thin.
- Comments only when the *why* is non-obvious. The code should read as
  self-explaining.
- No new dependencies without a real reason. The dep tree is intentionally
  small (no `anyhow` / `thiserror`, hand-rolled error type) — adding a
  crate is a tradeoff we should discuss in the PR.

## Commit / PR style

- One concern per commit. `git log --oneline` should read as a clean
  changelog.
- Conventional-commits-ish prefixes are used in history (`feat:`, `fix:`,
  `docs:`, `test:`, `refactor:`, `build:`, `ci:`) but not strictly
  enforced — clarity beats convention.
- Reference the relevant `tasks/todo.md` entry or roadmap phase in the PR
  body.
- For non-trivial changes, please add or update a test. The pure-logic
  modules (`query_parser`, `firestore_filter`, `coercion`,
  `schema_infer`, `schema_overrides`, `cache`) all have unit tests —
  follow the patterns there.

## Filing issues

Before opening one, check `tasks/todo.md` — it may already be parked.

Useful info:

- What you ran (Tabularis version, plugin version, OS).
- The Firestore project setup (production, emulator, IAM role).
- The actual JSON-RPC request/response if you can capture it (run the
  plugin binary directly with `cargo run` and pipe a request in).
- For query bugs: the SQL Tabularis sent + what came back.

## Reporting security issues

If you find something with security implications (credential handling,
RPC injection, etc.), please email `tech@luninora.de` instead of opening
a public issue.

## License

By contributing you agree your changes ship under the project's
[Apache-2.0](LICENSE) license.
