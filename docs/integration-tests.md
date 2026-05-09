# Integration Tests

End-to-end tests that spawn the plugin binary and drive it over JSON-RPC stdio
against a real Firestore emulator. Catches the I/O-bound paths that pure-logic
unit tests can't reach (handler dispatch, gRPC round-trips, response shape
contracts with Tabularis).

## Prerequisites

- **bun** — orchestrates the emulator and runs the seed script.
  Install: `curl -fsSL https://bun.sh/install | bash`
- **Java 21+** — required by `firebase-tools` ≥ v15. Check: `java --version`. Install on Arch/CachyOS: `sudo pacman -S jdk21-openjdk`. SDKMAN works cross-platform: `sdk install java 21.0.5-tem`.
- **cargo** — for the test binary itself.

No GCP credentials are needed; the emulator runs locally as a `demo-project`.

## Quick start

```bash
# One terminal: start the emulator (foreground, Ctrl-C to stop).
just emulator

# Another terminal: seed + run the integration suite.
just test-integration
```

The recipe assumes the emulator is reachable on `127.0.0.1:8080` (the default
`firebase.json`). Override via `FIRESTORE_EMULATOR_HOST=...` if needed.

## CI / one-shot

```bash
just ci-integration
```

Spawns the emulator, waits for it to become ready, seeds, runs the tests, and
kills the emulator. Self-contained — what GitHub Actions will eventually run.

## What's covered

| Area | Test |
|---|---|
| `test_connection`, `get_databases`, `get_tables` | `end_to_end_against_emulator` |
| Phase 2 query layer — WHERE, IN, OR, ARRAY_CONTAINS, pagination, total_count, ER FK | `phase2_query_layer_against_emulator` |
| Phase 3 CRUD — insert, update, single-field rename, delete | `phase3_crud_against_emulator` |
| Rename collision detection | `rename_collision_returns_structured_error` |
| Auto-generated doc id when `id` is omitted | `auto_generated_doc_id_when_id_omitted` |
| `explain_query` shape matches Tabularis' `ExplainPlan` contract | `explain_plan_shape_matches_tabularis_contract` |
| Schema-overrides required-field validation blocks insert | `schema_overrides_required_field_blocks_insert` |

All tests are gated `#[ignore]` so `cargo test` (without `--ignored`) skips them
silently. The `just test-integration` recipe passes `--ignored` to enable.

## Files

```
firebase.json              # Emulator config (port 8080, no UI)
.firebaserc                # Project alias = demo-project
package.json               # firebase-tools devDep + bun scripts
tests/firestore_emulator.rs  # The Rust test binary
tests/fixtures/seed.ts     # Bun-native fixture seeder
```

## Resetting state

The emulator persists data within a run. Between tests the suite uses unique
doc-ids (`crud-test-doc`, `override-test`, etc.) and cleans up after itself,
but if a test panics mid-run, leftovers may interfere on rerun.

```bash
just emulator-reset    # wipes Firestore data without restarting the emulator
just emulator-seed     # repopulates the fixtures
```

## Why bun + firebase-tools (and not Docker)?

- **Bun** runs the seed script natively in TypeScript without a build step
  and orchestrates `firebase-tools` via npm scripts in ~30 lines of config.
- **firebase-tools** is the official Google emulator orchestrator; it handles
  JAR downloads, port management, persistence, and shutdown cleanly.
- A Docker option exists (`google/cloud-sdk:emulators`) but it adds another
  prerequisite (Docker daemon, image pull) for marginal benefit. We can add
  it later as a CI alternative.
