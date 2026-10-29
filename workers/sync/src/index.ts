/**
 * loki sync Worker.
 *
 * Deployed to the user's own Cloudflare account. It stores encrypted vault
 * blobs and version metadata, and nothing else: no key derivation, no
 * plaintext, no analytics. A leaked D1 dump reveals item count and timing.
 *
 * Endpoints:
 *   GET  /v1/health                    service probe, no auth
 *   GET  /v1/prelogin                  kdf salt + params, no auth
 *   POST /v1/setup                     first-device vault init (once)
 *   POST /v1/login                     exchange password-derived auth key for a token
 *   GET  /v1/vault                     vault metadata
 *   GET  /v1/items?since=N&limit=M     incremental pull
 *   PUT  /v1/items                     push a batch
 */

export interface Env {
  DB: D1Database;
  /** Secret shared by every client; guards setup and login. */
  SETUP_TOKEN?: string;
}

const API_VERSION = 1;
const SERVICE = "loki-sync";
const MAX_BATCH = 200;
const SESSION_TTL_SECONDS = 30 * 24 * 3600;

function json(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "content-type": "application/json; charset=utf-8" },
  });
}

function err(message: string, status: number): Response {
  return json({ error: message }, status);
}

/** Constant-time-ish compare; lengths are public anyway. */
function safeEqual(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

async function sha256Hex(input: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(input));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

async function issueToken(db: D1Database, deviceId: string): Promise<string> {
  const token = [...crypto.getRandomValues(new Uint8Array(32))]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  const now = Math.floor(Date.now() / 1000);
  // Store only the hash: a read of this table cannot be replayed.
  await db
    .prepare("INSERT INTO sessions (token, device_id, created_at, expires_at) VALUES (?, ?, ?, ?)")
    .bind(await sha256Hex(token), deviceId, now, now + SESSION_TTL_SECONDS)
    .run();
  return token;
}

async function authenticate(request: Request, db: D1Database): Promise<string | null> {
  const header = request.headers.get("authorization") ?? "";
  if (!header.startsWith("Bearer ")) return null;
  const token = header.slice(7);
  const hashed = await sha256Hex(token);
  const now = Math.floor(Date.now() / 1000);
  const row = await db
    .prepare("SELECT device_id FROM sessions WHERE token = ? AND expires_at > ?")
    .bind(hashed, now)
    .first<{ device_id: string }>();
  return row?.device_id ?? null;
}

async function getMeta(db: D1Database, key: string): Promise<string | null> {
  const row = await db
    .prepare("SELECT value FROM meta WHERE key = ?")
    .bind(key)
    .first<{ value: string }>();
  return row?.value ?? null;
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const path = url.pathname;

    if (path === "/v1/health") {
      // `initialized` tells a second device whether to join or set up.
      const salt = await getMeta(env.DB, "kdf_salt");
      return json({
        service: SERVICE,
        version: "1.0.0",
        api: API_VERSION,
        initialized: salt !== null,
      });
    }

    if (path === "/v1/prelogin") {
      const salt = await getMeta(env.DB, "kdf_salt");
      const params = await getMeta(env.DB, "kdf_params");
      if (!salt || !params) return err("not initialized", 404);
      return json({ kdf_salt: salt, kdf_params: params });
    }

    if (path === "/v1/setup" && request.method === "POST") {
      // Setup runs once. A second call is refused so a fresh device cannot
      // overwrite an existing vault's salt and lock everyone out.
      if (await getMeta(env.DB, "kdf_salt")) return err("already initialized", 409);

      const body = await request.json<Record<string, string>>();
      if (env.SETUP_TOKEN && !safeEqual(body.setup_token ?? "", env.SETUP_TOKEN)) {
        return err("bad setup token", 403);
      }
      for (const key of [
        "kdf_salt",
        "kdf_params",
        "auth_key",
        "protected_vault_key",
        "recovery_vault_key",
        "recovery_auth",
      ]) {
        if (!body[key]) return err(`missing ${key}`, 400);
      }

      const now = Math.floor(Date.now() / 1000);
      for (const [key, value] of Object.entries(body)) {
        if (key === "setup_token" || key === "schema_version") continue;
        await env.DB.prepare("INSERT INTO meta (key, value) VALUES (?, ?)")
          .bind(key, value)
          .run();
      }
      await env.DB.prepare("INSERT INTO meta (key, value) VALUES ('seq', '0')").run();
      await env.DB.prepare("INSERT INTO meta (key, value) VALUES ('created_at', ?)")
        .bind(String(now))
        .run();

      const token = await issueToken(env.DB, body.device_id ?? "first");
      return json({ token, expires_at: now + SESSION_TTL_SECONDS });
    }

    if (path === "/v1/login" && request.method === "POST") {
      const body = await request.json<{ auth_key?: string; device_id?: string }>();
      const stored = await getMeta(env.DB, "auth_key");
      if (!stored) return err("not initialized", 404);
      if (!body.auth_key || !safeEqual(body.auth_key, stored)) {
        return err("bad credentials", 401);
      }
      const now = Math.floor(Date.now() / 1000);
      const token = await issueToken(env.DB, body.device_id ?? "unknown");
      return json({ token, expires_at: now + SESSION_TTL_SECONDS });
    }

    // Everything below needs a token.
    const device = await authenticate(request, env.DB);
    if (!device) return err("unauthorized", 401);

    if (path === "/v1/vault" && request.method === "GET") {
      const salt = await getMeta(env.DB, "kdf_salt");
      const params = await getMeta(env.DB, "kdf_params");
      const protectedKey = await getMeta(env.DB, "protected_vault_key");
      const recoveryKey = await getMeta(env.DB, "recovery_vault_key");
      const seq = await getMeta(env.DB, "seq");
      if (!salt || !protectedKey) return err("not initialized", 404);
      return json({
        schema_version: 1,
        kdf_salt: salt,
        kdf_params: params,
        protected_vault_key: protectedKey,
        recovery_vault_key: recoveryKey,
        seq: Number(seq ?? 0),
      });
    }

    if (path === "/v1/items" && request.method === "GET") {
      const since = Number(url.searchParams.get("since") ?? "0");
      const limit = Math.min(Number(url.searchParams.get("limit") ?? "100"), MAX_BATCH);

      const { results } = await env.DB.prepare(
        "SELECT id, kind, envelope, revision, seq, deleted, updated_at FROM items WHERE seq > ? ORDER BY seq ASC LIMIT ?",
      )
        .bind(since, limit + 1)
        .all();

      const hasMore = results.length > limit;
      const page = results.slice(0, limit);
      const nextSince = page.length > 0 ? Number(page[page.length - 1].seq) : since;

      return json({
        items: page.map((r) => ({
          id: r.id,
          kind: r.kind ?? "",
          envelope: r.envelope,
          revision: Number(r.revision),
          seq: Number(r.seq),
          deleted: Number(r.deleted) === 1,
          updated_at: Number(r.updated_at),
        })),
        next_since: nextSince,
        has_more: hasMore,
      });
    }

    if (path === "/v1/items" && request.method === "PUT") {
      const body = await request.json<{ items?: unknown[] }>();
      const items = body.items ?? [];
      if (items.length > MAX_BATCH) return err("batch too large", 413);

      // One transaction per batch: the seq counter and the rows move together,
      // so a client never sees a gap in its cursor.
      const statements: D1PreparedStatement[] = [];
      let accepted = 0;
      let rejected = 0;

      for (const raw of items) {
        const item = raw as Record<string, unknown>;
        const id = String(item.id ?? "");
        if (!id) continue;

        const existing = await env.DB.prepare("SELECT revision, updated_at FROM items WHERE id = ?")
          .bind(id)
          .first<{ revision: number; updated_at: number }>();

        const remoteUpdatedAt = existing ? Number(existing.updated_at) : 0;
        const remoteRevision = existing ? Number(existing.revision) : 0;
        const incomingUpdatedAt = Number(item.updated_at ?? 0);
        const incomingRevision = Number(item.revision ?? 1);

        // Last write wins on the *plaintext* timestamp the client recorded.
        // A tie goes to the server copy, so two devices cannot ping-pong.
        const loses =
          existing &&
          (incomingUpdatedAt < remoteUpdatedAt ||
            (incomingUpdatedAt === remoteUpdatedAt && incomingRevision <= remoteRevision));
        if (loses) {
          rejected++;
          continue;
        }

        const deleted = item.deleted === true;
        const envelope = deleted ? null : (item.envelope ?? null);
        if (!deleted && !envelope) continue;

        statements.push(
          env.DB.prepare(
            `INSERT INTO items (id, kind, envelope, revision, seq, deleted, updated_at)
             VALUES (?, ?, ?, ?, (SELECT COALESCE(MAX(seq), 0) + 1 FROM items), ?, ?)
             ON CONFLICT(id) DO UPDATE SET
               kind = excluded.kind,
               envelope = excluded.envelope,
               revision = excluded.revision,
               seq = (SELECT COALESCE(MAX(seq), 0) + 1 FROM items),
               deleted = excluded.deleted,
               updated_at = excluded.updated_at`,
          ).bind(id, String(item.kind ?? ""), envelope, incomingRevision, deleted ? 1 : 0, incomingUpdatedAt),
        );
        accepted++;
      }

      if (statements.length > 0) {
        statements.push(
          env.DB.prepare(
            "INSERT INTO meta (key, value) VALUES ('seq', (SELECT COALESCE(MAX(seq), 0) FROM items)) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
          ),
        );
        await env.DB.batch(statements);
      }

      const seq = await getMeta(env.DB, "seq");
      return json({ accepted, rejected, seq: Number(seq ?? 0) });
    }

    return err("not found", 404);
  },
};
