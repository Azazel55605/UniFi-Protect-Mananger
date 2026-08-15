/**
 * The single seam between the UI and the server.
 *
 * Every network call goes through here. Components never call `fetch`
 * directly, so authentication handling, error shape and base URL stay in one
 * place — and the app stays portable if it is ever served from somewhere else.
 */
import type {
  ApiErrorBody,
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
  ErrorCode,
  EventQuery,
  Health,
  IndexStats,
  NamedCheck,
  SessionInfo,
  Settings,
  SetupState,
  StorageSample,
  StorageSnapshot,
  WatchdogConfig,
  WatchdogState,
  UpbInspection,
} from "./types.gen";

/**
 * A failure the server classified.
 *
 * `code` is what the UI branches on — a 409 alone cannot distinguish "setup
 * isn't finished" from "a job is already running", and those want different
 * screens. `message` is the sentence to show; `hint` is the next step, when
 * there is one.
 */
export class ApiError extends Error {
  readonly code: ErrorCode;
  readonly hint?: string;
  readonly checks?: NamedCheck[];
  /** Seconds until a rate-limited request will be accepted. */
  readonly retryAfter?: number;
  /** Matches the server log line. Present on server faults only. */
  readonly requestId?: string;

  constructor(readonly status: number, body: Partial<ApiErrorBody> & { message: string }) {
    super(body.message);
    this.code = body.code ?? "internal";
    this.hint = body.hint ?? undefined;
    this.checks = body.checks ?? undefined;
    this.retryAfter = body.retry_after_secs ?? undefined;
    this.requestId = body.request_id ?? undefined;
  }

  /** True when the session is gone and the app should return to the login screen. */
  get isAuthFailure() {
    return this.code === "unauthenticated" || this.status === 401;
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    credentials: "same-origin",
    headers: init?.body ? { "content-type": "application/json" } : undefined,
    ...init,
  });

  if (!res.ok) throw await asApiError(res);

  return res.status === 204 ? (undefined as T) : ((await res.json()) as T);
}

/**
 * Every API error has the same shape, but not everything that can answer this
 * app does — a reverse proxy returning its own 502 page, say. So parse the
 * shape when it is there and fall back to something honest when it isn't.
 */
async function asApiError(res: Response): Promise<ApiError> {
  const text = await res.text().catch(() => "");

  try {
    const parsed = JSON.parse(text) as Partial<ApiErrorBody>;
    if (parsed && typeof parsed.message === "string") {
      return new ApiError(res.status, parsed as ApiErrorBody);
    }
  } catch {
    // Not JSON. Falls through to the text below.
  }

  return new ApiError(res.status, {
    message: text.trim() || res.statusText || `Request failed (${res.status})`,
  });
}

export const api = {
  authStatus: () => request<AuthStatus>("/api/auth/status"),

  login: (password: string) =>
    request<void>("/api/auth/login", {
      method: "POST",
      body: JSON.stringify({ password }),
    }),

  logout: () => request<void>("/api/auth/logout", { method: "POST" }),

  sessions: () => request<SessionInfo[]>("/api/auth/sessions"),

  revokeOtherSessions: () =>
    request<SessionInfo[]>("/api/auth/sessions/revoke-others", { method: "POST" }),

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
