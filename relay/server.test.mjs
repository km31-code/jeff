import assert from "node:assert/strict";
import test from "node:test";
import { issueToken, safeCompletionBody, verifyToken } from "./server.mjs";

const secret = "0123456789abcdef0123456789abcdef";

test("issues scoped expiring tokens and rejects tampering", () => {
  const issued = issueToken("jeff-installation-0001", secret, 1_000);
  assert.equal(issued.scope, "inference:chat");
  const claims = verifyToken(issued.token, secret, 1_001);
  assert.equal(claims.sub, "jeff-installation-0001");
  assert.equal(claims.scope, "inference:chat");
  assert.equal(claims.max_requests, 100);
  assert.throws(() => verifyToken(`${issued.token}x`, secret, 1_001), /signature/);
  assert.throws(() => verifyToken(issued.token, secret, claims.exp), /expired/);
});

test("pins the upstream model and clamps generation limits", () => {
  const safe = safeCompletionBody({
    model: "attacker-selected-model",
    max_tokens: 99_999,
    stream: "yes",
    messages: [{ role: "user", content: "hi" }]
  }, "server-allowlisted-model");
  assert.equal(safe.model, "server-allowlisted-model");
  assert.equal(safe.max_tokens, 4096);
  assert.equal(safe.stream, true);
  assert.throws(() => safeCompletionBody({ messages: [] }, "model"), /messages/);
});
