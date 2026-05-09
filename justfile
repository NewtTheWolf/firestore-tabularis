set shell := ["bash", "-cu"]
set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

# ---------------------------------------------------------------------------
# Cross-platform recipes (only shell-agnostic tooling — cargo, npm).
# ---------------------------------------------------------------------------

# Build the plugin binary in debug mode (plus UI if present).
build: build-ui
    cargo build

# Build for release (what the GitHub Actions workflow ships).
release: build-ui
    cargo build --release

# Run unit tests.
test:
    cargo test

# Coverage report (region-level via cargo-llvm-cov).
cov:
    cargo llvm-cov --summary-only

# ---------------------------------------------------------------------------
# Firestore emulator orchestration (bun + firebase-tools).
# Java (JRE 11+) is required by the underlying emulator.
# ---------------------------------------------------------------------------

# Install bun deps (firebase-tools).
[unix]
emulator-deps:
    @command -v bun >/dev/null || { echo "Install bun first: curl -fsSL https://bun.sh/install | bash"; exit 1; }
    bun install --silent

# Self-contained: random free port, fresh firebase.json, seed, run integration
# tests, clean up. Picks Java 21 explicitly (firebase-tools v15+ requirement;
# default-java may be older). Used both for local dev and CI.
[unix]
test-integration: emulator-deps
    @bash -c 'set -euo pipefail; \
        export JAVA_HOME=${JAVA_HOME:-/usr/lib/jvm/java-21-openjdk}; \
        export PATH=$JAVA_HOME/bin:$PATH; \
        PORT=$(bun run tests/fixtures/free-port.ts); \
        echo "[it] picked port $PORT"; \
        cp firebase.json firebase.json.bak; \
        printf "{\"emulators\":{\"firestore\":{\"host\":\"127.0.0.1\",\"port\":%s},\"ui\":{\"enabled\":false},\"singleProjectMode\":true}}\n" "$PORT" > firebase.json; \
        cleanup() { kill $EMU_PID 2>/dev/null || true; mv firebase.json.bak firebase.json; }; \
        trap cleanup EXIT; \
        bun run emulator > /tmp/firestore-it.log 2>&1 & \
        EMU_PID=$!; \
        echo "[it] emulator pid=$EMU_PID, waiting for :$PORT..."; \
        for i in $(seq 1 60); do \
            if curl -fsS http://127.0.0.1:$PORT >/dev/null 2>&1; then echo "[it] ready after ${i}s"; break; fi; \
            if ! kill -0 $EMU_PID 2>/dev/null; then echo "[it] emulator died early — log:"; tail -30 /tmp/firestore-it.log; exit 1; fi; \
            sleep 1; \
        done; \
        FIRESTORE_EMULATOR_HOST=127.0.0.1:$PORT bun run emulator:seed; \
        FIRESTORE_EMULATOR_HOST=127.0.0.1:$PORT \
        FIRESTORE_TEST_PROJECT=demo-project \
        cargo test --test firestore_emulator -- --ignored --test-threads=1'

# Launch the local REPL that simulates Tabularis JSON-RPC calls over stdio.
repl:
    cargo run --bin test_plugin

# Run clippy on the workspace.
lint:
    cargo clippy --all-targets -- -D warnings

# Format the codebase.
fmt:
    cargo fmt --all

# ---------------------------------------------------------------------------
# Platform-specific recipes (file operations + plugin-dir conventions).
# ---------------------------------------------------------------------------

# Build the UI extension if present (no-op otherwise).
[unix]
build-ui:
    @if [ -f ui/package.json ]; then \
        echo "Building UI extension..."; \
        (cd ui && npm install --no-audit --no-fund && npm run build); \
    fi

[windows]
build-ui:
    if (Test-Path ui/package.json) {
        Write-Host "Building UI extension..."
        Push-Location ui
        try {
            npm install --no-audit --no-fund
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
            npm run build
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        } finally {
            Pop-Location
        }
    }

# Build + copy binary, manifest and (if present) UI bundle into Tabularis's plugin folder.
[linux]
dev-install: build
    mkdir -p ~/.local/share/tabularis/plugins/firestore
    cp target/debug/firestore-plugin ~/.local/share/tabularis/plugins/firestore/
    cp manifest.json ~/.local/share/tabularis/plugins/firestore/
    @if [ -f ui/dist/index.js ]; then \
        mkdir -p ~/.local/share/tabularis/plugins/firestore/ui/dist; \
        cp ui/dist/index.js ~/.local/share/tabularis/plugins/firestore/ui/dist/; \
    fi
    @echo "Installed to ~/.local/share/tabularis/plugins/firestore"
    @echo "Restart Tabularis (or toggle the plugin in Settings) to pick up changes."

[macos]
dev-install: build
    mkdir -p "$HOME/Library/Application Support/com.debba.tabularis/plugins/firestore"
    cp target/debug/firestore-plugin "$HOME/Library/Application Support/com.debba.tabularis/plugins/firestore/"
    cp manifest.json "$HOME/Library/Application Support/com.debba.tabularis/plugins/firestore/"
    @if [ -f ui/dist/index.js ]; then \
        mkdir -p "$HOME/Library/Application Support/com.debba.tabularis/plugins/firestore/ui/dist"; \
        cp ui/dist/index.js "$HOME/Library/Application Support/com.debba.tabularis/plugins/firestore/ui/dist/"; \
    fi
    @echo "Installed to ~/Library/Application Support/com.debba.tabularis/plugins/firestore"
    @echo "Restart Tabularis (or toggle the plugin in Settings) to pick up changes."

[windows]
dev-install: build
    $dest = Join-Path $env:APPDATA "com.debba.tabularis\plugins\firestore"
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    Copy-Item "target\debug\firestore-plugin.exe" $dest
    Copy-Item "manifest.json" $dest
    if (Test-Path "ui\dist\index.js") {
        New-Item -ItemType Directory -Force -Path (Join-Path $dest "ui\dist") | Out-Null
        Copy-Item "ui\dist\index.js" (Join-Path $dest "ui\dist")
    }
    Write-Host "Installed to $dest"
    Write-Host "Restart Tabularis (or toggle the plugin in Settings) to pick up changes."

# Remove the installed plugin.
[linux]
uninstall:
    rm -rf ~/.local/share/tabularis/plugins/firestore

[macos]
uninstall:
    rm -rf "$HOME/Library/Application Support/com.debba.tabularis/plugins/firestore"

[windows]
uninstall:
    $dest = Join-Path $env:APPDATA "com.debba.tabularis\plugins\firestore"
    if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
