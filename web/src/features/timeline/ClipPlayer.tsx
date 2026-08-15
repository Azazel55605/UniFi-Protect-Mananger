import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { formatBytes } from "@/lib/format";
import { Button } from "@/components/ui/button";
import { Panel, PanelBody, PanelHeader } from "@/components/ui/panel";
import type { EventRecord } from "@/lib/types.gen";
import { formatDuration } from "./TimelinePage";
import { VideoPlayer } from "./VideoPlayer";

/**
 * Inline playback, with the detail you'd otherwise have to go to the
 * filesystem for.
 *
 * The recordings are HEVC, which the browser generally cannot play, so the
 * server transcodes on first request. That takes a few seconds and a `<video>`
 * element gives no hint why it is idle — hence the explicit notice, driven by
 * an info call that reports whether a transcode already exists.
 */
export function ClipPlayer({
  event,
  onClose,
  onPrevious,
  onNext,
}: {
  event: EventRecord;
  onClose: () => void;
  onPrevious?: () => void;
  onNext?: () => void;
}) {
  const [playing, setPlaying] = useState(false);

  const info = useQuery({
    queryKey: ["clip-info", event.id],
    queryFn: () => api.clipInfo(event.id),
  });

  useEffect(() => setPlaying(false), [event.id]);

  // Once frames are on screen the conversion is plainly over, whatever the
  // cached info call said when the clip was opened.
  const preparing = !playing && info.data ? !info.data.prepared : false;
  const when = new Date(event.start * 1000);
  const clipUrl = `/api/media/${encodeURIComponent(event.id)}`;

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
          <div className="flex items-center gap-2">
            {/* The original, not the transcode: the transcode is a viewing
                convenience, and what you'd keep is the recording itself. */}
            <a
              href={`${clipUrl}/original`}
              download={`${event.camera} ${when.toISOString().slice(0, 19).replace("T", " ")}.mp4`}
            >
              <Button size="sm" variant="default">
                Download
              </Button>
            </a>
            <Button size="sm" variant="ghost" onClick={onClose}>
              Close
            </Button>
          </div>
        }
      />
      <PanelBody className="space-y-3">
        {preparing && (
          <p className="text-xs text-warn">
            Converting this clip for the browser — the first play of a recording
            takes a few seconds. Later plays are instant.
          </p>
        )}

        <div onPlay={() => setPlaying(true)}>
          <VideoPlayer
            key={event.id}
            src={`${clipUrl}/clip`}
            poster={`${clipUrl}/thumb`}
            fps={info.data?.fps}
            onPrevious={onPrevious}
            onNext={onNext}
          />
        </div>

        <dl className="grid grid-cols-2 gap-x-6 gap-y-1.5 sm:grid-cols-4">
          <Fact label="Recorded" value={when.toLocaleString(undefined, { hourCycle: "h23" })} />
          <Fact label="Length" value={formatDuration(event.duration)} />
          <Fact
            label="Size"
            value={
              info.data?.size_bytes != null
                ? formatBytes(info.data.size_bytes)
                : event.size_bytes != null
                  ? formatBytes(event.size_bytes)
                  : "—"
            }
          />
          <Fact
            label="Video"
            value={
              info.data?.codec
                ? [
                    info.data.codec,
                    info.data.width && info.data.height
                      ? `${info.data.width}×${info.data.height}`
                      : null,
                    info.data.fps ? `${Math.round(info.data.fps)} fps` : null,
                  ]
                    .filter(Boolean)
                    .join(" · ")
                : "…"
            }
            hint={
              info.data && !info.data.direct
                ? "Converted to H.264 for playback; the download is the original"
                : undefined
            }
          />
          <Fact label="Camera" value={event.camera} />
          <Fact
            label="Detected"
            value={event.subtypes.join(", ") || event.event_type}
          />
          <Fact label="Event type" value={event.event_type} />
          <Fact label="Event id" value={event.id} mono />
        </dl>
      </PanelBody>
    </Panel>
  );
}

function Fact({
  label,
  value,
  hint,
  mono,
}: {
  label: string;
  value: string;
  hint?: string;
  mono?: boolean;
}) {
  return (
    <div title={hint}>
      <dt className="eyebrow">{label}</dt>
      <dd className={mono ? "data truncate text-fg-dim" : "truncate text-sm text-fg"}>
        {value}
      </dd>
    </div>
  );
}
