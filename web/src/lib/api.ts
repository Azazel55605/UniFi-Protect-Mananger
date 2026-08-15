/**
 * The single seam between the UI and the server.
 *
 * Every network call goes through here. Components never call `fetch`
 * directly, so authentication handling, error shape and base URL stay in one
 * place — and the app stays portable if it is ever served from somewhere else.
 */
import type {
  AuthStatus,
  DiscoveryResult,
  Health,
  NamedCheck,
  Settings,
  SetupState,
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

  /** Live container logs. Authenticated by the page cookie, same origin. */
  logSocket: (tail = 200) => {
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    return new WebSocket(`${proto}//${location.host}/ws/logs?tail=${tail}`);
  },
};
