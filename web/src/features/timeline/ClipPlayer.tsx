import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Panel, PanelBody, PanelHeader } from "@/components/ui/panel";
import type { EventRecord } from "@/lib/types.gen";

/**
 * Inline playback.
 *
 * The recordings are HEVC, which the browser generally cannot play, so the
 * server transcodes on first request. That takes a few seconds and the video
 * element gives no hint why it is idle — hence the explicit notice, driven by
 * an info call that says whether a transcode already exists.
 */
export function ClipPlayer({
  event,
  onClose,
}: {
  event: EventRecord;
  onClose: () => void;
}) {
  const info = useQuery({
    queryKey: ["clip-info", event.id],
    queryFn: () => api.clipInfo(event.id),
  });

  const preparing = info.data ? !info.data.prepared : false;
  const when = new Date(event.start * 1000);

  return (
    <Panel className="border-signal/40">
      <PanelHeader
        label={`${event.camera} · ${when.toLocaleTimeString(undefined, {
          hour: "2-digit",
          minute: "2-digit",
          second: "2-digit",
          hourCycle: "h23",
        })}`}
        aside={
          <div className="flex items-center gap-3">
            {info.data?.codec && (
              <span className="data text-[11px] text-fg-faint">
                {info.data.direct
                  ? info.data.codec
                  : `${info.data.codec} → h264`}
              </span>
            )}
            <a
              href={`/api/media/${encodeURIComponent(event.id)}/original`}
              download
              className="data text-[11px] text-signal-strong hover:underline"
            >
              original
            </a>
            <Button size="sm" variant="ghost" onClick={onClose}>
              Close
            </Button>
          </div>
        }
      />
      <PanelBody className="space-y-2">
        {preparing && (
          <p className="text-xs text-warn">
            Converting this clip for the browser — the first play of a recording
            takes a few seconds. Later plays are instant.
          </p>
        )}
        <video
          key={event.id}
          controls
          autoPlay
          preload="metadata"
          className="w-full rounded-[3px] bg-ink-deep"
          src={`/api/media/${encodeURIComponent(event.id)}/clip`}
        />
        <p className="data text-[11px] text-fg-faint">
          {event.subtypes.join(", ") || event.event_type} · {Math.round(event.duration)}s
        </p>
      </PanelBody>
    </Panel>
  );
}
