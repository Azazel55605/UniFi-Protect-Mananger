/**
 * The single seam between the UI and the server.
 *
 * Every network call goes through here. Components never call `fetch`
 * directly, so authentication handling, error shape and base URL stay in one
 * place — and the app stays portable if it is ever served from somewhere else.
 */
import type {
  ArchiveOverview,
  ArchiveRun,
  AuthStatus,
  CameraInfo,
  CameraMonth,
  ClipInfo,
  Schedule,
  StartArchiveRequest,
  DiscoveryResult,
  EventPage,
  EventQuery,
  Health,
  IndexStats,
  NamedCheck,
  Settings,
  SetupState,
  StorageSample,
  StorageSnapshot,
  WatchdogConfig,
  WatchdogState,
  UpbInspection,
} from "./types.gen";

export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
    readonly checks?: NamedCheck[],
  ) {
    super(message);
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    credentials: "same-origin",
    headers: init?.body ? { "content-type": "application/json" } : undefined,
    ...init,
  });

  if (!res.ok) {
    const text = await res.text();
    // Settings validation answers with the failing checks; keep them typed so
    // the wizard can show which step is at fault instead of a generic message.
    try {
      const parsed = JSON.parse(text);
      if (Array.isArray(parsed)) {
        throw new ApiError(res.status, "Some settings are not valid", parsed);
      }
    } catch (e) {
      if (e instanceof ApiError) throw e;
    }
    throw new ApiError(res.status, text || res.statusText);
  }

  return res.status === 204 ? (undefined as T) : ((await res.json()) as T);
}

export const api = {
  authStatus: () => request<AuthStatus>("/api/auth/status"),

  login: (password: string) =>
    request<void>("/api/auth/login", {
      method: "POST",
      body: JSON.stringify({ password }),
    }),

  logout: () => request<void>("/api/auth/logout", { method: "POST" }),

  health: () => request<Health>("/api/health"),

  setup: () => request<SetupState>("/api/setup"),

  discover: () => request<DiscoveryResult>("/api/setup/discover"),

  saveSettings: (settings: Settings) =>
    request<SetupState>("/api/settings", {
      method: "PUT",
      body: JSON.stringify(settings),
    }),

  inspect: () => request<UpbInspection>("/api/upb/inspect"),

  events: (q: EventQuery) => {
    const params = new URLSearchParams();
    for (const [k, v] of Object.entries(q)) {
      if (v !== undefined && v !== null && v !== "") params.set(k, String(v));
    }
    return request<EventPage>(`/api/events?${params}`);
  },

  cameras: () => request<CameraInfo[]>("/api/cameras"),

  indexStats: () =>
    request<{ stats: IndexStats; event_types: string[] }>("/api/index/stats"),

  archive: () => request<ArchiveOverview>("/api/archive"),

  archiveRuns: () => request<ArchiveRun[]>("/api/archive/runs"),

  startArchive: (body: StartArchiveRequest) =>
    request<{ run_id: number }>("/api/archive/runs", {
      method: "POST",
      body: JSON.stringify(body),
    }),

  restore: (target: CameraMonth) =>
    request<{ run_id: number }>("/api/archive/restore", {
      method: "POST",
      body: JSON.stringify(target),
    }),

  verifyArchive: (target: CameraMonth) =>
    request<{ run_id: number }>("/api/archive/verify", {
      method: "POST",
      body: JSON.stringify(target),
    }),

  setPinned: (camera: string, month: string, pinned: boolean) =>
    request<void>("/api/archive/pin", {
      method: "POST",
      body: JSON.stringify({ camera, month, pinned }),
    }),

  clipInfo: (id: string) =>
    request<ClipInfo>(`/api/media/${encodeURIComponent(id)}/info`),

  storage: () => request<StorageSnapshot>("/api/storage"),

  storageHistory: (days = 30) =>
    request<StorageSample[]>(`/api/storage/history?days=${days}`),

  watchdog: () => request<WatchdogState>("/api/watchdog"),

  saveWatchdog: (c: WatchdogConfig) =>
    request<WatchdogState>("/api/watchdog/config", {
      method: "PUT",
      body: JSON.stringify(c),
    }),

  schedule: () => request<Schedule>("/api/schedule"),

  saveSchedule: (s: Schedule) =>
    request<Schedule>("/api/schedule", { method: "PUT", body: JSON.stringify(s) }),

  /** Live progress for archive, restore and verify jobs. */
  progressSocket: () => {
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    return new WebSocket(`${proto}//${location.host}/ws/progress`);
  },

  syncIndex: () =>
    request<{ events: number; cameras: number; clips_checked: number }>(
      "/api/index/sync",
      { method: "POST" },
    ),

  /** Live container logs. Authenticated by the page cookie, same origin. */
  logSocket: (tail = 200) => {
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    return new WebSocket(`${proto}//${location.host}/ws/logs?tail=${tail}`);
  },
};
