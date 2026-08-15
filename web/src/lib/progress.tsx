import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import type { RunProgress } from "./types.gen";

/**
 * One progress socket for the whole app.
 *
 * It lives above the router so a job stays visible when you navigate away —
 * archiving a few months takes long enough that being pinned to one page to
 * watch it would be its own annoyance. A socket per component would also mean
 * several connections racing to report the same job.
 */
const ProgressContext = createContext<RunProgress | null>(null);

export function ProgressProvider({ children }: { children: ReactNode }) {
  const [progress, setProgress] = useState<RunProgress | null>(null);
  const queryClient = useQueryClient();
  const clearTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    let socket: WebSocket | null = null;
    let retry: number | undefined;
    let closed = false;
    let attempt = 0;

    const connect = () => {
      socket = api.progressSocket();

      socket.onopen = () => {
        attempt = 0;
      };

      socket.onmessage = (e) => {
        const update = JSON.parse(e.data as string) as RunProgress;
        window.clearTimeout(clearTimer.current);
        setProgress(update);

        if (update.finished) {
          queryClient.invalidateQueries({ queryKey: ["archive"] });
          queryClient.invalidateQueries({ queryKey: ["archive-runs"] });
          queryClient.invalidateQueries({ queryKey: ["index-stats"] });
          // Hold the final state briefly, so a job that finishes in a second
          // still gets to say how it went.
          clearTimer.current = window.setTimeout(() => setProgress(null), 8000);
        }
      };

      // A dropped socket means losing sight of a running job, so reconnect —
      // but back off, so a server that is down or refusing the connection
      // isn't retried every few seconds for as long as the tab is open.
      socket.onclose = () => {
        if (closed) return;
        attempt += 1;
        retry = window.setTimeout(connect, Math.min(30_000, 2 ** attempt * 1000));
      };
    };

    connect();
    return () => {
      closed = true;
      window.clearTimeout(retry);
      window.clearTimeout(clearTimer.current);
      socket?.close();
    };
  }, [queryClient]);

  return <ProgressContext.Provider value={progress}>{children}</ProgressContext.Provider>;
}

export function useProgress() {
  return useContext(ProgressContext);
}

/** Percentages for the current camera-month and for the run as a whole. */
export function useProgressPercent(progress: RunProgress | null) {
  return useMemo(() => {
    if (!progress) return { file: 0, overall: 0 };
    return {
      file:
        progress.files_total > 0
          ? Math.round((progress.files_done / progress.files_total) * 100)
          : 0,
      overall:
        progress.overall_total > 0
          ? Math.round((progress.overall_done / progress.overall_total) * 100)
          : 0,
    };
  }, [progress]);
}
