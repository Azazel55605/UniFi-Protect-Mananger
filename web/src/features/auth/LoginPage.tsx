import { useState, type FormEvent } from "react";
import { api, ApiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

export function LoginPage({
  configured,
  onSignedIn,
}: {
  configured: boolean;
  onSignedIn: () => void;
}) {
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [cookieBlocked, setCookieBlocked] = useState(false);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    setCookieBlocked(false);
    try {
      await api.login(password);

      // The password was right, but that isn't the same as being signed in.
      // Over plain HTTP the browser accepts the response and then discards the
      // Secure cookie, so without this check the page would simply reappear
      // with no explanation — the most confusing failure in the whole app.
      const status = await api.authStatus();
      if (!status.authenticated) {
        setCookieBlocked(location.protocol !== "https:");
        if (location.protocol === "https:") {
          setError("Signed in, but the session cookie was not stored by the browser.");
        }
        return;
      }

      setPassword("");
      onSignedIn();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "Could not reach the server");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex min-h-screen items-center justify-center p-6">
      <div className="w-full max-w-sm">
        <div className="mb-8 flex items-center gap-2.5">
          <span className="tally bg-signal" aria-hidden />
          <span className="text-[13px] font-semibold tracking-[0.12em] uppercase">
            Protect
          </span>
        </div>

        {configured ? (
          <form onSubmit={submit} className="glass border border-line rounded-[3px] p-5">
            <h1 className="mb-1 text-base font-semibold">Sign in</h1>
            <p className="mb-5 text-sm text-fg-dim">
              This console manages your recording backups.
            </p>

            <label className="eyebrow mb-1.5 block" htmlFor="password">
              Password
            </label>
            <Input
              id="password"
              type="password"
              autoComplete="current-password"
              autoFocus
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />

            {error && <p className="mt-3 text-sm text-bad">{error}</p>}

            {cookieBlocked && (
              <div className="mt-4 rounded-[3px] border border-warn/40 p-3">
                <p className="text-sm text-warn">
                  Your password was correct, but this page is served over HTTP and the
                  session cookie requires HTTPS, so the browser discarded it.
                </p>
                <p className="mt-2 text-xs text-fg-dim">
                  Serve the app over HTTPS — terminating TLS at your reverse proxy is
                  enough, the connection from the proxy to the container can stay HTTP.
                  To test over HTTP instead, set{" "}
                  <code className="data text-fg">PM_COOKIE_SECURE=0</code> and restart.
                </p>
              </div>
            )}

            <Button type="submit" variant="primary" className="mt-5 w-full" disabled={busy}>
              {busy ? "Signing in…" : "Sign in"}
            </Button>
          </form>
        ) : (
          <div className="glass border border-line rounded-[3px] p-5">
            <h1 className="mb-1 text-base font-semibold">No password set</h1>
            <p className="mb-4 text-sm text-fg-dim">
              Generate a hash and pass it to the container as{" "}
              <code className="data text-fg">PM_PASSWORD_HASH</code>, then restart.
            </p>
            <pre className="data surface-solid overflow-x-auto rounded-[3px] border border-line p-3 text-fg-dim">
              docker run --rm protect-manager:local hash-password 'your-password'
            </pre>
          </div>
        )}
      </div>
    </div>
  );
}
