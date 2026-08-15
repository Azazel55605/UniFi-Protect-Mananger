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
- An event index rebuilt on a timer from the backup service's own database,
  with camera names and detection types recovered from clip paths — neither is
  stored upstream
- Every clip classified as available, archived, missing, awaiting backfill or
  never captured, so a gap in the footage is visible rather than silent
- Backup lag on the dashboard: time since the newest event that produced a
  clip, which catches a backup service that is running but no longer capturing
- A watchdog for the backup service's known failure — still recording events,
  no longer downloading them — with an optional webhook and an opt-in restart
- Archiving that packs each camera-month into an uncompressed `.tar`, reads it
  back and compares every file against its hash, and only then removes the
  originals — with a dry run that writes nothing
- A schedule that catches up after downtime instead of skipping, records every
  attempt, and can POST to a webhook when a run fails
- An archive browser grouped by camera: what exists, what's due, archives made
  outside this app, archives that have gone missing, on-demand verification,
  and restore
- A running job stays visible from any page, so archiving a few months doesn't
  pin you to one screen
- Capacity: filesystem usage, how fast footage is growing, roughly how long the
  space lasts, and where it is going per camera
- A timeline of any day as a strip of marks that zooms down to seconds, with
  search, sorting, thumbnails and inline playback; selecting a clip zooms the
  strip to it, and the original recording is always downloadable untouched
- A purpose-built player: frame-accurate stepping, jump to the next or previous
  clip of the day, playback speed, and keyboard shortcuts

### Player shortcuts

| Key | Does |
|---|---|
| `space` / `k` | Play or pause |
| `←` / `→` | Back or forward 5s (hold `shift` for 1s) |
| `j` / `l` | Back or forward 10s |
| `,` / `.` | Step one frame — exact, using the clip's real frame rate |
| `m` | Mute |
| `f` | Fullscreen |
| `home` / `end` | Start or end of the clip |
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
| ✅ | **Event feed** | Indexes the backup service's event database, reads camera names and detection types out of clip filenames, filters by camera, type, detection and clip state |
| ✅ | **Archiving** | Packs old clips into per-camera monthly archives, verifies every file before deleting anything, runs on a schedule, and browses, verifies and restores archives |
| ✅ | **Capacity dashboard** | Filesystem usage, growth over time, live vs archived split, per-camera breakdown |
| ✅ | **Timeline and playback** | A day as a strip of marks, thumbnails, and inline playback — clips are HEVC, so they are transcoded on demand and cached |
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
| `PM_ARCHIVE_DIR` | `/archive` | Where `.tar` archives are written |
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
| `GET /api/events` | Query the index: `camera_id`, `event_type`, `subtype`, `status`, `from`, `to`, `limit`, `offset` |
| `GET /api/cameras` | Known cameras with event counts |
| `GET /api/index/stats` | Index totals, clip states, backup lag, available filters |
| `POST /api/index/sync` | Rebuild the index now instead of waiting for the timer |
| `GET /api/archive` | Archives, what's due, and anything missing |
| `GET/POST /api/archive/runs` | Run history; start an archive or a dry run |
| `POST /api/archive/restore` | Unpack a camera-month back to live |
| `POST /api/archive/verify` | Re-read an existing archive |
| `POST /api/archive/pin` | Hold a month back from scheduled archiving, or release it |
| `GET/PUT /api/schedule` | The archive schedule |
| `GET /api/watchdog` | Stall assessment, config and recent watchdog activity |
| `PUT /api/watchdog/config` | Configure the watchdog |
| `GET /api/storage` | Filesystem usage, live vs archived split, growth rate |
| `GET /api/storage/history?days=N` | Sampled usage history for the trend |
| `GET /api/media/{id}/info` | Codec, and whether playback needs preparing first |
| `GET /api/media/{id}/thumb` | Cached still from the clip |
| `GET /api/media/{id}/clip` | A clip the browser can play, transcoding if needed |
| `GET /api/media/{id}/original` | The recording itself, untouched |
| `GET /ws/progress` | Live job progress (WebSocket) |
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
- **The event index is read whole and rewritten, not merged.** The upstream
  schema has no index on time, so reading "only what's new" scans the same
  rows anyway — and backup rows are written *after* their event, so a
  high-water mark would permanently miss clips that arrived late.
- **A stall is detected by comparing two clocks, not by a timeout.** The
  backup service writes an event row when it *sees* an event and a backup row
  when it *downloads* one. Events arriving without downloads is a stall; no
  events at all is a quiet night. A "no clips for N hours" rule cannot tell
  those apart and would restart the service every quiet night.
- **Two containers share the clip directory, and age is what keeps them
  apart.** The backup service writes there; this app deletes from there. Only
  months the backup service has finished with are ever archived — and a month
  written to within the last hour is held back regardless, because packing a
  file mid-write would archive a truncated clip and then remove the original.
  If the backup service's backfill window reaches into archivable months,
  `/api/health` says so.
- **Nothing is deleted until the archive holding it has been read back and
  compared byte for byte.** Header-only checks catch truncation but pass a
  structurally valid archive containing wrong bytes, which is the one failure
  that matters when the originals are about to go.
- **An archive that already exists is never overwritten**, and its sources are
  never deleted on the strength of it — we only remove originals for an archive
  this app wrote and verified in the same run.
- **Clips are transcoded on demand, not in advance.** The recordings are HEVC,
  which Firefox cannot play at all on Linux and Chromium only plays with
  platform hardware support. The live window holds thousands of clips and
  almost none are ever watched, so converting them all would spend hours of
  CPU producing files nobody opens. The first play of a recording takes a few
  seconds; the result is cached, and evicted when the clip is archived.
- **Capacity comes from the filesystem, not a storage appliance's API.**
  `statvfs` works on any host, needs no credentials, and can't go stale against
  a vendor's API version. It reports the filesystem as mounted rather than the
  pool beneath it — which is the number that decides whether archiving fits.
- **Archives are plain uncompressed tar** and stay readable with `tar -xf`.
  Clips are already compressed video, so gzip would cost CPU for nothing, and
  the archive should outlive this application.
- **Restored months are pinned.** A restored month is older than the live
  window by definition, so the next scheduled run would immediately archive it
  again and undo the restore.
- **The upstream database is only ever opened read-only, and never on the
  request path.** It uses a rollback journal, so a reader can block while it
  writes; that is fine on a background timer and not fine while someone waits
  for a page.

## Licence

MIT — see [LICENSE](LICENSE).
