import { ApiError } from "@/lib/api";
import { cn } from "@/lib/utils";

/**
 * One way of showing a failure, everywhere.
 *
 * Before this, each page rendered `error.message` in whatever red it had to
 * hand, and threw away everything else the server said. The server now sends a
 * hint with most errors and a request id with faults, and both are worth more
 * than the sentence: the hint is usually the whole fix, and the id is the only
 * way to connect what a person saw to what the logs recorded.
 */
export function ErrorNotice({
  error,
  className,
}: {
  error: unknown;
  className?: string;
}) {
  if (!error) return null;

  const api = error instanceof ApiError ? error : null;
  const message =
    api?.message ??
    (error instanceof Error ? error.message : null) ??
    "Something went wrong.";

  // A failure to reach the server at all is the most common thing here and the
  // least usefully described by its own message ("Failed to fetch").
  const offline = !api && error instanceof TypeError;

  return (
    <div
      role="alert"
      className={cn(
        "rounded-[3px] border border-bad/40 bg-bad/5 px-3 py-2.5 text-sm",
        className,
      )}
    >
      <p className="text-bad">{offline ? "Can't reach the server." : message}</p>

      {(api?.hint || offline) && (
        <p className="mt-1 text-xs text-fg-dim">
          {offline
            ? "Check the container is running, then try again."
            : api?.hint}
        </p>
      )}

      {api?.checks && api.checks.length > 0 && (
        // Which settings failed, rather than only that some did — the wizard
        // has several steps and "some settings are not valid" does not say
        // which one to go back to.
        <ul className="mt-2 space-y-1">
          {api.checks.map((c) => (
            <li key={c.name} className="text-xs text-fg-dim">
              <span className="data text-fg">{c.name}</span> — {c.detail}
            </li>
          ))}
        </ul>
      )}

      {api?.requestId && (
        <p className="data mt-1.5 text-[10px] text-fg-faint">
          request {api.requestId} — this id appears in the container log
        </p>
      )}
    </div>
  );
}
