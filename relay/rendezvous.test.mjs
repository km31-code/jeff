// apex f3b: the rendezvous relay matches parties by token and forwards bytes
// opaquely. it must (1) cross-connect a host and guest sharing a token, (2) carry
// arbitrary bytes verbatim in both directions, and (3) never bridge mismatched
// tokens.

import assert from "node:assert/strict";
import net from "node:net";
import test from "node:test";
import { createRendezvousServer } from "./rendezvous.mjs";

function listen(server) {
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => resolve(server.address().port));
  });
}

function dial(port, role, token) {
  const socket = net.connect(port, "127.0.0.1");
  socket.setNoDelay(true);
  socket.write(`JEFFRDV1 ${role} ${token}\n`);
  return socket;
}

function nextChunk(socket) {
  return new Promise((resolve) => socket.once("data", (d) => resolve(d)));
}

test("bridges a host and guest by token and forwards bytes verbatim both ways", async () => {
  const server = createRendezvousServer();
  const port = await listen(server);

  const host = dial(port, "host", "token-abc");
  const guest = dial(port, "guest", "token-abc");

  // guest -> host
  const hostRecv = nextChunk(host);
  guest.write(Buffer.from([0x00, 0x01, 0x02, 0xff, 0xfe]));
  assert.deepEqual([...(await hostRecv)], [0x00, 0x01, 0x02, 0xff, 0xfe]);

  // host -> guest
  const guestRecv = nextChunk(guest);
  host.write(Buffer.from("opaque-ciphertext"));
  assert.equal((await guestRecv).toString(), "opaque-ciphertext");

  host.destroy();
  guest.destroy();
  await new Promise((r) => server.close(r));
});

test("does not bridge parties with different tokens", async () => {
  const server = createRendezvousServer();
  const port = await listen(server);

  const host = dial(port, "host", "token-one");
  const guest = dial(port, "guest", "token-two");

  let bridged = false;
  guest.once("data", () => {
    bridged = true;
  });
  host.write(Buffer.from("should-not-arrive"));

  await new Promise((r) => setTimeout(r, 100));
  assert.equal(bridged, false, "mismatched tokens must never be bridged");

  host.destroy();
  guest.destroy();
  await new Promise((r) => server.close(r));
});
