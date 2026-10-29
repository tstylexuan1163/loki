# loki sync Worker

Optional sync backend. Deploy it to **your own** Cloudflare account; loki talks to
it over HTTPS and stores only ciphertext there.

## What the server can and cannot see

| It can see | It cannot see |
|---|---|
| How many items exist | Host names, addresses, usernames |
| When each item changed | Passwords, private keys, API keys |
| A random session token's hash | The master password or any derived key |

The encryption happens in the desktop app. The `envelope` column is exactly what
the app already stores locally before sync is switched on. Someone with full read
access to your D1 can count and time your edits, and nothing else.

## Deploy

```bash
cd workers/sync
npm install          # installs wrangler
npx wrangler d1 create loki-sync         # note the database_id
```

Put the id in `wrangler.toml`:

```toml
name = "loki-sync"
main = "src/index.ts"
compatibility_date = "2025-01-01"

[[d1_databases]]
binding = "DB"
database_name = "loki-sync"
database_id = "<paste the id>"
```

Apply the schema and publish:

```bash
npx wrangler d1 execute loki-sync --remote --file=./migrations/0001_init.sql
npx wrangler deploy
```

Optional but recommended — require a shared secret for the one-time setup call so
a stranger who finds your URL cannot initialise the vault first:

```bash
npx wrangler secret put SETUP_TOKEN     # any long random string
```

## Wire up the app

In loki: **设置 → 同步**, paste the `https://loki-sync.<you>.workers.dev` URL and a
token from `/v1/login`, then **立即同步**.

## Endpoints

| Method | Path | Auth | Notes |
|---|---|---|---|
| GET | `/v1/health` | no | `{ service, api, initialized }` |
| GET | `/v1/prelogin` | no | KDF salt + params |
| POST | `/v1/setup` | `SETUP_TOKEN` | Runs once; later calls get 409 |
| POST | `/v1/login` | no | Body `{ auth_key, device_id }` → token |
| GET | `/v1/vault` | bearer | Vault metadata |
| GET | `/v1/items?since=N&limit=M` | bearer | Incremental pull, max 200 |
| PUT | `/v1/items` | bearer | Batch push |

Conflict rule, applied server-side: **last write wins on `updated_at`**, and a tie
keeps the stored copy. A push carrying an older timestamp is counted in
`rejected` instead of overwriting.

Session tokens live in `sessions` as SHA-256 hashes, so a dump of that table
cannot be replayed.

## Local test harness

`tests/sync_live.rs` in the app exercises this protocol against a Node + SQLite
stand-in. It is what caught the missing `kind` column, so keep it around when
editing the schema.
