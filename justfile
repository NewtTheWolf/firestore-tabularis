set shell := ["bash", "-cu"]
set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

mod plugin "just/plugin.just"
mod emulator "just/emulator.just"
mod ui "just/ui.just"

# Show available recipes (run `just <module> --list` to see a module's recipes).
default:
    @just --list

# Build the plugin binary in debug mode (plus UI if present).
build:
    @just ui build
    cargo build

# Build for release (what the GitHub Actions workflow ships).
release:
    @just ui build
    cargo build --release

# Run unit tests.
test:
    cargo test

# Coverage report (region-level via cargo-llvm-cov).
cov:
    cargo llvm-cov --summary-only

# Launch the local REPL that simulates Tabularis JSON-RPC calls over stdio.
repl:
    cargo run --bin test_plugin

# Run clippy on the workspace.
lint:
    cargo clippy --all-targets -- -D warnings

# Format the codebase.
fmt:
    cargo fmt --all
