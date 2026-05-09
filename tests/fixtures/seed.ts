// Seed the local Firestore emulator with integration-test fixtures.
//
// Required env (set by `just it-seed` or any wrapper):
//   FIRESTORE_EMULATOR_HOST  — e.g. localhost:8080
//   FIRESTORE_TEST_PROJECT   — defaults to demo-project
//
// Run:  bun run tests/fixtures/seed.ts

const host = process.env.FIRESTORE_EMULATOR_HOST ?? "localhost:8080";
const project = process.env.FIRESTORE_TEST_PROJECT ?? "demo-project";
const base = `http://${host}/v1/projects/${project}/databases/(default)/documents`;

type ProtoValue =
  | { stringValue: string }
  | { booleanValue: boolean }
  | { integerValue: string }
  | { doubleValue: number }
  | { timestampValue: string }
  | { arrayValue: { values: ProtoValue[] } }
  | { mapValue: { fields: Record<string, ProtoValue> } }
  | { referenceValue: string }
  | { nullValue: null };

async function writeDoc(
  collection: string,
  docId: string,
  fields: Record<string, ProtoValue>,
): Promise<void> {
  const url = `${base}/${collection}/${docId}`;
  const res = await fetch(url, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ fields }),
  });
  if (!res.ok) {
    throw new Error(
      `seed ${collection}/${docId} failed: ${res.status} ${await res.text()}`,
    );
  }
}

const userRef = (id: string): ProtoValue => ({
  referenceValue: `projects/${project}/databases/(default)/documents/users/${id}`,
});

await writeDoc("users", "alice", {
  email: { stringValue: "alice@x.de" },
  active: { booleanValue: true },
  region: { stringValue: "eu" },
  tags: { arrayValue: { values: [{ stringValue: "vip" }] } },
  address: {
    mapValue: {
      fields: {
        city: { stringValue: "Berlin" },
        country: { stringValue: "DE" },
      },
    },
  },
});

await writeDoc("users", "bob", {
  email: { stringValue: "bob@x.de" },
  active: { booleanValue: false },
  region: { stringValue: "us" },
  tags: { arrayValue: { values: [{ stringValue: "early" }] } },
});

await writeDoc("posts", "post1", {
  title: { stringValue: "Hello" },
  views: { integerValue: "150" },
  status: { stringValue: "published" },
  tags: {
    arrayValue: {
      values: [{ stringValue: "launch" }, { stringValue: "news" }],
    },
  },
  priority: { stringValue: "high" },
  author: userRef("alice"),
});

await writeDoc("posts", "post2", {
  title: { stringValue: "Followup" },
  views: { integerValue: "50" },
  status: { stringValue: "draft" },
  tags: { arrayValue: { values: [{ stringValue: "draft" }] } },
  priority: { stringValue: "low" },
  author: userRef("bob"),
});

console.log(
  `seeded users (2 docs) + posts (2 docs) into ${project} on ${host}`,
);
