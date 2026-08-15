import { useEffect, useRef, useState } from "react";
import { api } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Panel, PanelHeader } from "@/components/ui/panel";
import { Tally } from "@/components/ui/tally";

/** Keeps memory bounded on a stream that never ends. */
const MAX_LINES = 2000;

export function LogsPage() {
  const [lines, setLines] = useState<string[]>([]);
  const [connected, setConnected] = useState(false);
  const socketRef = useRef<WebSocket | null>(null);
  const boxRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);

  useEffect(() => {
    const socket = api.logSocket();
    socketRef.current = socket;

    socket.onopen = () => setConnected(true);
    socket.onclose = () => setConnected(false);
    socket.onmessage = (e) =>
      setLines((prev) => {
        const next = [...prev, e.data as string];
        return next.length > MAX_LINES ? next.slice(-MAX_LINES) : next;
      });

    return () => socket.close();
  }, []);

  // Follow the tail only when the reader is already at the bottom, so scrolling
  // back through output isn't yanked away by the next line.
  useEffect(() => {
    const el = boxRef.current;
    if (el && pinnedRef.current) el.scrollTop = el.scrollHeight;
  }, [lines]);

  return (
    <Panel className="flex h-[calc(100vh-7.5rem)] flex-col">
      <PanelHeader
        label="Backup service output"
        aside={
          <div className="flex items-center gap-3">
            <div className="flex items-center gap-1.5">
              <Tally state={connected ? "live" : "idle"} />
              <span className="data text-[11px] text-fg-faint">
                {connected ? "streaming" : "disconnected"}
              </span>
            </div>
            <Button size="sm" variant="ghost" onClick={() => setLines([])}>
              Clear
            </Button>
          </div>
        }
      />
      <div
        ref={boxRef}
        onScroll={(e) => {
          const el = e.currentTarget;
          pinnedRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
        className="data surface-solid flex-1 overflow-y-auto p-3 leading-relaxed"
      >
        {lines.length === 0 ? (
          <p className="text-fg-faint">Waiting for output…</p>
        ) : (
          lines.map((line, i) => (
            <div key={i} className="whitespace-pre-wrap break-words text-fg-dim">
              {line}
            </div>
          ))
        )}
      </div>
    </Panel>
  );
}
