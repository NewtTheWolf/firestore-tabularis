#!/usr/bin/env bash
# Seed the local Firestore emulator with Phase 2 test fixtures.
#
# Requires: gcloud emulators firestore start --host-port=localhost:8080
# Usage:    FIRESTORE_EMULATOR_HOST=localhost:8080 bash tests/fixtures/seed.sh

set -euo pipefail

HOST="${FIRESTORE_EMULATOR_HOST:-localhost:8080}"
PROJECT="${FIRESTORE_TEST_PROJECT:-demo-project}"
BASE="http://$HOST/v1/projects/$PROJECT/databases/(default)/documents"

# Helper: PATCH a document (creates if missing).
write_doc() {
    local collection="$1"
    local doc_id="$2"
    local body="$3"
    curl -fsS -X PATCH \
        "$BASE/$collection/$doc_id" \
        -H "Content-Type: application/json" \
        -d "$body" > /dev/null
}

# users
write_doc users alice '{
  "fields": {
    "email":  { "stringValue": "alice@x.de" },
    "active": { "booleanValue": true },
    "region": { "stringValue": "eu" },
    "tags":   { "arrayValue": { "values": [{"stringValue":"vip"}] } },
    "address": { "mapValue": { "fields": {
      "city":    { "stringValue": "Berlin" },
      "country": { "stringValue": "DE" }
    }}}
  }
}'

write_doc users bob '{
  "fields": {
    "email":  { "stringValue": "bob@x.de" },
    "active": { "booleanValue": false },
    "region": { "stringValue": "us" },
    "tags":   { "arrayValue": { "values": [{"stringValue":"early"}] } }
  }
}'

# posts (with reference to users)
write_doc posts post1 "$(cat <<EOF
{
  "fields": {
    "title":  { "stringValue": "Hello" },
    "views":  { "integerValue": "150" },
    "status": { "stringValue": "published" },
    "tags":   { "arrayValue": { "values": [{"stringValue":"launch"}, {"stringValue":"news"}] } },
    "priority": { "stringValue": "high" },
    "author": { "referenceValue": "projects/$PROJECT/databases/(default)/documents/users/alice" }
  }
}
EOF
)"

write_doc posts post2 "$(cat <<EOF
{
  "fields": {
    "title":  { "stringValue": "Followup" },
    "views":  { "integerValue": "50" },
    "status": { "stringValue": "draft" },
    "tags":   { "arrayValue": { "values": [{"stringValue":"draft"}] } },
    "priority": { "stringValue": "low" },
    "author": { "referenceValue": "projects/$PROJECT/databases/(default)/documents/users/bob" }
  }
}
EOF
)"

echo "seeded users (2 docs) and posts (2 docs)"
