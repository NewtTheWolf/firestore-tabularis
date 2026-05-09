set shell := ["bash", "-cu"]
set working-directory := '..'

# Firestore emulator orchestration via bun + firebase-tools.
# Java 21+ is required (firebase-tools v15+ refuses older JVMs).

# Install bun deps (firebase-tools).
deps:
    @command -v bun >/dev/null || { echo "Install bun first: curl -fsSL https://bun.sh/install | bash"; exit 1; }
    bun install --silent

# Start the emulator in the foreground (Ctrl-C to stop). Uses the port in firebase.json.
start: deps
    JAVA_HOME=${JAVA_HOME:-/usr/lib/jvm/java-21-openjdk} \
    PATH=${JAVA_HOME:-/usr/lib/jvm/java-21-openjdk}/bin:$PATH \
    bun run emulator

# Reset the emulator's data without restarting.
reset:
    FIRESTORE_EMULATOR_HOST=${FIRESTORE_EMULATOR_HOST:-127.0.0.1:8080} bun run emulator:reset
    @echo

# Seed the integration-test fixtures into a running emulator.
seed:
    FIRESTORE_EMULATOR_HOST=${FIRESTORE_EMULATOR_HOST:-127.0.0.1:8080} bun run emulator:seed

# Self-contained: random port, fresh firebase.json, seed, run integration tests, clean up.
test: deps
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
