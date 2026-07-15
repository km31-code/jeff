// apex f3b: the companion rendezvous relay.
//
// this is a ciphertext-only forwarder. a party (the daemon = "host", the companion
// = "guest") dials in and sends one header line:
//
//     JEFFRDV1 <role> <token>\n
//
// the relay matches a host with a guest by the opaque <token> and then pipes raw
// bytes between them, both ways, forever -- it never parses, stores, or inspects
// what follows. the token is a random routing id, not an identity. everything the
// two parties exchange is Noise ciphertext, so a compromised relay learns nothing
// about the user or the world model. it holds no keys.

import net from "node:net";

const MAGIC = "JEFFRDV1";
const MAX_HEADER_BYTES = 256;

export function createRendezvousServer() {
  // token -> the first party waiting for its complement.
  const waiting = new Map();

  const server = net.createServer((socket) => {
    socket.on("error", () => socket.destroy());
    let header = Buffer.alloc(0);

    const onData = (chunk) => {
      header = Buffer.concat([header, chunk]);
      const nl = header.indexOf(0x0a);
      if (nl === -1) {
        // never buffer an unbounded "header" from a misbehaving peer.
        if (header.length > MAX_HEADER_BYTES) socket.destroy();
        return;
      }
      socket.removeListener("data", onData);
      // bytes after the newline are the peer's first ciphertext -- forward them.
      const rest = header.subarray(nl + 1);
      const line = header.subarray(0, nl).toString("utf8").trim();
      admit(socket, line, rest);
    };
    socket.on("data", onData);
  });

  function admit(socket, line, rest) {
    const [magic, role, token] = line.split(" ");
    if (magic !== MAGIC || (role !== "host" && role !== "guest") || !token) {
      socket.destroy();
      return;
    }
    const party = { socket, role, rest };
    const held = waiting.get(token);
    if (held && held.role !== role) {
      waiting.delete(token);
      bridge(held, party);
    } else {
      // replace any stale same-role waiter; keep only the newest.
      if (held) held.socket.destroy();
      waiting.set(token, party);
      socket.on("close", () => {
        if (waiting.get(token) === party) waiting.delete(token);
      });
    }
  }

  function bridge(a, b) {
    // deliver each side's already-received first bytes to the other, then pipe.
    if (a.rest.length) b.socket.write(a.rest);
    if (b.rest.length) a.socket.write(b.rest);
    a.socket.pipe(b.socket);
    b.socket.pipe(a.socket);
    const teardown = () => {
      a.socket.destroy();
      b.socket.destroy();
    };
    a.socket.on("close", teardown);
    b.socket.on("close", teardown);
    a.socket.on("error", teardown);
    b.socket.on("error", teardown);
  }

  return server;
}

if (process.argv[1] === new URL(import.meta.url).pathname) {
  const port = Number(process.env.RENDEZVOUS_PORT ?? 8788);
  createRendezvousServer().listen(port, "0.0.0.0", () => {
    process.stdout.write(`jeff companion rendezvous relay listening on ${port}\n`);
  });
}
