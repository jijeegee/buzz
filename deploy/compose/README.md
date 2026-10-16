# Kuzz Docker Compose deployment

This is the single-node/VPS deployment bundle. It is intentionally separate from
the root `docker-compose.yml`, which remains local development infrastructure.

## Quick start

```bash
cd deploy/compose
cp .env.example .env
$EDITOR .env       # replace every CHANGE_ME value
./run.sh start
```

For a public VPS with automatic Let's Encrypt certificates:

```bash
cd deploy/compose
KUZZ_COMPOSE_TLS=true ./run.sh start
```

The bootstrap script should eventually replace manual `.env` editing for normal
users. It is responsible for generating stable secrets and, optionally, an owner
keypair.

## Production notes

- Requires Docker Compose v2.24.4 or newer; the TLS override uses Compose's
  `!reset` tag to remove the direct relay port when Caddy terminates HTTPS.
- Default `KUZZ_IMAGE` tracks `ghcr.io/block/buzz:main` for early testing. Pin it to `ghcr.io/block/buzz:sha-<7>` or a semver release tag for production once available.
- Keep `KUZZ_RELAY_PRIVATE_KEY`, `KUZZ_GIT_HOOK_HMAC_SECRET`, database/Redis,
  and S3 secrets stable across restarts.
- `RELAY_OWNER_PUBKEY` is intentionally not prefixed with `KUZZ_`; it must be a
  64-character hex Nostr pubkey when closed relay mode is enabled.
- `KUZZ_AUTO_MIGRATE` is opt-in. Set `KUZZ_AUTO_MIGRATE=true` or run
  `kuzz-admin migrate` before starting the relay when bootstrapping a fresh
  database. Auto-migration requires an image that includes embedded SQLx
  migrations.
- The stack uses Postgres, Redis, MinIO, and a git data volume because
  those are real Kuzz dependencies today. Minimal mode can simplify this later.
- Mobile push remains off by default. To use the public gateway, keep the
  template's explicit `KUZZ_PUSH_GATEWAY_DELIVERY_URL` and set
  `KUZZ_PUSH_ENABLED=true`. To use another gateway, replace the exact HTTPS
  `/v1/deliveries/apns` URL before enabling push.
- The bundled Compose stack fixes the relay endpoint to `http://minio:9000` and
  `KUZZ_S3_ADDRESSING_STYLE=path`: Docker DNS resolves `minio`, not
  `<bucket>.minio`. It is not configurable for an external S3 provider through
  `.env`; use the Helm chart or a custom Compose configuration for providers
  such as new Railway Storage Buckets that require `virtual` addressing.

Run `./run.sh backup-hint` for the backup checklist.

## Validation

Before sharing an install link publicly, verify a fresh install with:

```bash
cd deploy/compose
cp .env.example .env
$EDITOR .env
./run.sh config
./run.sh start
curl -fsS "http://127.0.0.1:$(grep -E '^KUZZ_HTTP_PORT=' .env | cut -d= -f2-)/_liveness"
./run.sh status
```
