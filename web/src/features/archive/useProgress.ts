import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import type { RunProgress } from "@/lib/types.gen";

/**
 * Subscribes to job progress for as long as the page is open.
 *
 * The socket is the only way to see a long job move; polling would either lag
 * badly or hammer the server. When a job finishes it refreshes the queries the
 * job invalidated, so the archive list and run history update themselves.
 */
export function useProgress() {
  const [progress, setProgress] = useState<RunProgress | null>(null);
  const queryClient = useQueryClient();

  useEffect(() => {
    const socket = api.progressSocket();

    socket.onmessage = (e) => {
      const update = JSON.parse(e.data as string) as RunProgress;
      setProgress(update);

      if (update.finished) {
        queryClient.invalidateQueries({ queryKey: ["archive"] });
        queryClient.invalidateQueries({ queryKey: ["archive-runs"] });
        queryClient.invalidateQueries({ queryKey: ["index-stats"] });
        // Leave the final state on screen briefly so a job that finishes
        // quickly still tells you how it went.
        setTimeout(() => setProgress(null), 6000);
      }
    };

    return () => socket.close();
  }, [queryClient]);

  return progress;
}
