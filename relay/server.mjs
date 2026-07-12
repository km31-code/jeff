import { createHmac, timingSafeEqual } from "node:crypto";
import { createServer } from "node:http";
import { appendFile } from "node:fs/promises";

const MAX_BODY_BYTES = 256 * 1024;
const TOKEN_TTL_SECONDS = 24 * 60 * 60;
const TOKEN_REQUEST_LIMIT = 100;
const ENROLLMENTS_PER_DAY = 3;

function encoded(value) {
  return Buffer.from(value).toString("base64url");
}

export function signToken(payload, secret) {
  const body = encoded(JSON.stringify(payload));
  const signature = createHmac("sha256", secret).update(body).digest("base64url");
  return `${body}.${signature}`;
}

export function verifyToken(token, secret, nowSeconds) {
  const [body, signature, extra] = String(token).split(".");
  if (!body || !signature || extra) throw new Error("malformed token");
  const expected = createHmac("sha256", secret).update(body).digest();
  const actual = Buffer.from(signature, "base64url");
  if (actual.length !== expected.length || !timingSafeEqual(actual, expected)) {
    throw new Error("invalid token signature");
  }
  const payload = JSON.parse(Buffer.from(body, "base64url").toString("utf8"));
  if (payload.exp <= nowSeconds) throw new Error("expired token");
  if (payload.scope !== "inference:chat") throw new Error("invalid token scope");
  if (!/^[a-zA-Z0-9_-]{16,96}$/.test(payload.sub ?? "")) {
    throw new Error("invalid token subject");
  }
  return payload;
}

export function issueToken(installationId, secret, nowSeconds) {
  const payload = {
    sub: installationId,
    scope: "inference:chat",
    iat: nowSeconds,
    exp: nowSeconds + TOKEN_TTL_SECONDS,
    max_requests: TOKEN_REQUEST_LIMIT
  };
  return {
    token: signToken(payload, secret),
    token_type: "Bearer",
    scope: payload.scope,
    expires_at: new Date(payload.exp * 1000).toISOString()
  };
}

export function safeCompletionBody(body, upstreamModel) {
  if (!Array.isArray(body.messages) || body.messages.length === 0) {
    throw new Error("messages are required");
  }
  return {
    ...body,
    model: upstreamModel,
    max_tokens: Math.min(Math.max(Number(body.max_tokens ?? 2048), 1), 4096),
    stream: Boolean(body.stream)
  };
}

async function readJson(request, maxBytes = MAX_BODY_BYTES) {
  const chunks = [];
  let size = 0;
  for await (const chunk of request) {
    size += chunk.length;
    if (size > maxBytes) throw new Error("request body too large");
    chunks.push(chunk);
  }
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

function json(response, status, payload) {
  const body = JSON.stringify(payload);
  response.writeHead(status, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body),
    "cache-control": "no-store",
    "x-content-type-options": "nosniff"
  });
  response.end(body);
}

function clientAddress(request) {
  return request.socket.remoteAddress ?? "unknown";
}

function bearer(request) {
  const header = request.headers.authorization ?? "";
  return header.startsWith("Bearer ") ? header.slice(7).trim() : "";
}

export function createRelayServer(options = {}) {
  const tokenSecret = options.tokenSecret ?? process.env.RELAY_TOKEN_SECRET;
  const openAiKey = options.openAiKey ?? process.env.OPENAI_API_KEY;
  const upstreamUrl = options.upstreamUrl ?? "https://api.openai.com/v1/chat/completions";
  const upstreamModel = options.upstreamModel ?? process.env.RELAY_OPENAI_MODEL ?? "gpt-4o-mini";
  const upstreamFetch = options.upstreamFetch ?? fetch;
  const usagePath = options.usagePath ?? process.env.RELAY_USAGE_LOG;
  const clock = options.clock ?? (() => Date.now());
  if (!tokenSecret || tokenSecret.length < 32) {
    throw new Error("RELAY_TOKEN_SECRET must contain at least 32 characters");
  }
  if (!openAiKey) throw new Error("OPENAI_API_KEY is required");

  const enrollments = new Map();
  const tokenUsage = new Map();

  return createServer(async (request, response) => {
    response.setHeader("content-security-policy", "default-src 'none'");
    try {
      const url = new URL(request.url ?? "/", "http://relay.local");
      if (request.method === "GET" && url.pathname === "/healthz") {
        return json(response, 200, { ok: true });
      }

      if (request.method === "POST" && url.pathname === "/v1/tokens") {
        const body = await readJson(request, 8 * 1024);
        const installationId = String(body.installation_id ?? "");
        if (!/^[a-zA-Z0-9_-]{16,96}$/.test(installationId)) {
          return json(response, 400, { error: "invalid installation_id" });
        }
        if (body.scope !== "inference:chat") {
          return json(response, 400, { error: "unsupported scope" });
        }
        const day = Math.floor(clock() / 86_400_000);
        const enrollmentKey = `${clientAddress(request)}:${installationId}:${day}`;
        const issued = enrollments.get(enrollmentKey) ?? 0;
        if (issued >= ENROLLMENTS_PER_DAY) {
          return json(response, 429, { error: "enrollment quota exceeded" });
        }
        enrollments.set(enrollmentKey, issued + 1);
        const now = Math.floor(clock() / 1000);
        return json(response, 201, issueToken(installationId, tokenSecret, now));
      }

      if (request.method === "POST" && url.pathname === "/v1/chat/completions") {
        let claims;
        try {
          claims = verifyToken(bearer(request), tokenSecret, Math.floor(clock() / 1000));
        } catch (error) {
          return json(response, 401, { error: String(error.message) });
        }
        const used = tokenUsage.get(claims.sub) ?? 0;
        if (used >= claims.max_requests) {
          return json(response, 429, { error: "token request quota exceeded" });
        }
        const body = await readJson(request);
        const safeBody = safeCompletionBody(body, upstreamModel);
        const upstream = await upstreamFetch(upstreamUrl, {
          method: "POST",
          headers: {
            authorization: `Bearer ${openAiKey}`,
            "content-type": "application/json"
          },
          body: JSON.stringify(safeBody)
        });
        if (!upstream.ok) {
          const detail = await upstream.text();
          return json(response, 502, {
            error: "upstream inference failed",
            status: upstream.status,
            detail: detail.slice(0, 1024)
          });
        }
        tokenUsage.set(claims.sub, used + 1);
        const audit = JSON.stringify({
          at: new Date(clock()).toISOString(),
          installation_id: claims.sub,
          request_count: used + 1,
          model: upstreamModel
        });
        if (usagePath) await appendFile(usagePath, `${audit}\n`, { mode: 0o600 });
        response.writeHead(200, {
          "content-type": upstream.headers.get("content-type") ?? "application/json",
          "cache-control": "no-store",
          "x-jeff-relay-request-count": String(used + 1)
        });
        if (!upstream.body) return response.end();
        for await (const chunk of upstream.body) response.write(chunk);
        return response.end();
      }

      return json(response, 404, { error: "not found" });
    } catch (error) {
      return json(response, 400, { error: String(error.message) });
    }
  });
}

if (process.argv[1] === new URL(import.meta.url).pathname) {
  const port = Number(process.env.PORT ?? 8787);
  createRelayServer().listen(port, "0.0.0.0", () => {
    process.stdout.write(`jeff inference relay listening on ${port}\n`);
  });
}
