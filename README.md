# UniFi Protect Backup Manager

A management layer for a [`unifi-protect-backup`](https://github.com/ep1cman/unifi-protect-backup)
deployment: view recent clips and their events, archive older ones to long-term
storage on a schedule, and see whether the pipeline is actually working.

> **Status: early.** The app shell, setup flow and system health are working.
> The event feed, archiving and playback are not built yet — see the
> [roadmap](#roadmap).

## What works today

- Password login with an `HttpOnly; Secure; SameSite=Strict` session cookie,
  and sessions that survive a restart
- Backup-container discovery **by image**, so no container name is hardcoded —
  and a container that is recreated is re-found rather than breaking the app
- **Interactive setup**: the database path, clip prefix and camera directories
  are read from the backup container and its filesystem, and you confirm them
- Camera directories identified by evidence (dated folders holding clips),
  not by excluding a hardcoded list of known non-camera names
- Health checks: Docker socket, container state, clip-directory readability,
  and a warning if the backup service's retention is shorter than your live
  window — which would delete footage before it is ever archived
- Live container logs streamed over an authenticated WebSocket
- Light and dark themes (following the system by default) and five accent
  colours, applied before first paint so opening the app never flickers

## Running it

```bash
# 1. Build the frontend
cd web && pnpm install && pnpm run build && cd ..

# 2. Generate a password hash
cargo run -- hash-password 'your-password'

# 3. Run against your Docker socket
PM_PASSWORD_HASH='<the hash>' \
PM_BACKUP_DIR=/path/to/unifi-protect-backup/output \
PM_STATE_DIR=./.state \
PM_COOKIE_SECURE=0 \
cargo run
```

Then open <http://localhost:8642>.

For frontend work, run `pnpm run dev` in `web/` alongside the server: Vite
proxies `/api` and `/ws` to it, so the app stays same-origin in development and
cookies behave exactly as they will in production.

`PM_COOKIE_SECURE=0` is for local HTTP only. In deployment TLS terminates at the
reverse proxy and the cookie must keep its `Secure` flag.

### In Docker

```bash
docker build -t protect-manager:local .
docker run --rm protect-manager:local hash-password 'your-password'
cp deploy/docker-compose.example.yml docker-compose.yml   # then edit the paths
```

Every path in the compose file is a placeholder. Container paths (`/backup`,
`/archive`) are fixed; you map your own directories onto them.

`CLIP_GID` must be the group that owns the clip files, or the app cannot read
them. Find it with `stat -c %g <a-clip-file>`. `/api/health` reports a mismatch
explicitly — naming both the directory's owner and the app's own ids — rather
than showing an empty list.

## Roadmap

| | | |
|---|---|---|
| ✅ | **Shell, setup and health** | Login, interactive setup, health checks, live container logs |
| ⬜ | **Event feed** | Read the backup service's event database, parse detection types out of clip filenames, filter by camera and type |
| ⬜ | **Archiving** | Pack old clips into per-camera monthly archives, verify them before deleting sources, run on a schedule, browse and restore archives |
| ⬜ | **Capacity dashboard** | Pool usage, growth over time, live vs archived split |
| ⬜ | **Timeline and playback** | Scrubbable timeline with thumbnails; clips are HEVC, so playback transcodes on demand |
| ⬜ | **Mobile layouts** | Desktop-first for now; the timeline needs a different treatment on a phone |

## Configuration

| Variable | Default | Purpose |
|---|---|---|
| `PM_PASSWORD_HASH` | — | Argon2 PHC string. Without it every authenticated route refuses. |
| `PM_BIND` | `0.0.0.0:8642` | Listen address |
| `PM_BACKUP_DIR` | `/backup` | Backup root, as mounted in this container |
| `PM_UPB_IMAGE` | `ghcr.io/ep1cman/unifi-protect-backup` | Image used to discover the container |
| `PM_UPB_CONTAINER` | — | Explicit container id/name, bypassing discovery |
| `PM_COOKIE_SECURE` | `true` | Set `0` only for local HTTP development |
| `PM_SESSION_TTL_SECS` | 14 days | Session lifetime |
| `PM_STATE_DIR` | `/var/lib/protect-manager` | Our database and caches |
| `PM_STATIC_DIR` | `web/dist` | Where the built frontend is served from |
| `PM_LOG` | `protect_manager=info` | `tracing` filter |

Everything else is configured in the app's setup flow rather than here:
deploying this for the first time should not require hand-editing config.

## API

| Endpoint | Purpose |
|---|---|
| `GET /api/auth/status` | Whether you're signed in, and whether a password is configured |
| `POST /api/auth/login` | `{"password": "..."}` → session cookie |
| `POST /api/auth/logout` | Revoke the session |
| `GET /api/health` | Docker, container and clip-directory checks |
| `GET /api/setup` | Saved settings, re-validated against the filesystem |
| `GET /api/setup/discover` | Containers, derived paths and camera candidates |
| `PUT /api/settings` | Save settings; rejects a configuration that fails validation |
| `GET /api/upb/containers` | Containers matching the backup image |
| `GET /api/upb/inspect` | Container detail and derived configuration |
| `GET /ws/logs?tail=N` | Live container logs (WebSocket) |

## Subcommands

| Command | Purpose |
|---|---|
| `hash-password <password>` | Generate a value for `PM_PASSWORD_HASH` |
| `check-hash` | Report the structure of the configured hash and diagnose a mangled one |
| `verify-password <password>` | Test a password against the configured hash |

`check-hash` and `verify-password` read `PM_PASSWORD_HASH` from the environment,
so run them inside the deployed container to see what the server actually got.

Everything except the auth endpoints requires a session.

## Development

```bash
cargo test                    # Rust unit tests
cargo clippy --all-targets
cd web && pnpm run typecheck  # frontend
```

`crates/protect-api-types` is the API contract. `cargo test -p protect-api-types`
regenerates `web/src/lib/types.gen.ts`, which is committed — so a change to a
response shape that breaks the frontend breaks `tsc` rather than surfacing as
`undefined` at runtime. Never edit the generated file.

## Why some of this looks the way it does

A few decisions that are easy to mistake for accidents:

- **The backup container is found by image, not by name.** Compose deployments
  that don't set `container_name` get a generated one, and it changes whenever
  the container is recreated. A stale id falls back to discovery.
- **Camera directories are detected by evidence** — folders named `YYYY-MM-DD`
  containing clips — rather than by excluding a list of known non-camera names.
  A blocklist only ever describes one person's layout, and would silently
  swallow a camera whose name happened to collide with it.
- **Only two environment variables are read from the backup container**
  (`SQLITE_PATH` and `TZ`). Its environment also holds NVR credentials in
  plaintext, so the allowlist is deliberate: nothing else is read, stored,
  logged or sent to the browser.
- **Settings are re-validated on every request** rather than trusting what was
  saved. Mounts vanish and permissions change; storing "validated: true" would
  make the app confidently wrong.

## Licence

MIT — see [LICENSE](LICENSE).
