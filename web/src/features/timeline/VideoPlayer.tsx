import { useCallback, useEffect, useRef, useState } from "react";
import {
  ChevronLeft,
  ChevronsLeft,
  ChevronsRight,
  ChevronRight,
  Maximize2,
  Minimize2,
  Pause,
  Play,
  Volume2,
  VolumeX,
} from "lucide-react";
import { cn } from "@/lib/utils";

/**
 * The video element with our own transport, instead of the browser's.
 *
 * The native controls are the one piece of browser-styled UI in the console,
 * and they can't carry what review actually needs: stepping a single frame,
 * moving to the next clip in the day, or a scrub bar that shares the timeline's
 * language. Playback itself is still the plain `<video>` element — only the
 * controls are ours.
 */

const SPEEDS = [0.5, 1, 1.5, 2, 4];

export function VideoPlayer({
  src,
  poster,
  fps,
  onPrevious,
  onNext,
}: {
  src: string;
  poster?: string;
  /** Real frame rate, so a frame step is exactly one frame. */
  fps?: number | null;
  onPrevious?: () => void;
  onNext?: () => void;
}) {
  const shellRef = useRef<HTMLDivElement>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const scrubRef = useRef<HTMLDivElement>(null);
  const scrubbing = useRef(false);

  const [playing, setPlaying] = useState(false);
  const [time, setTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [buffered, setBuffered] = useState(0);
  const [muted, setMuted] = useState(false);
  const [volume, setVolume] = useState(1);
  const [speed, setSpeed] = useState(1);
  const [fullscreen, setFullscreen] = useState(false);
  const [hoverAt, setHoverAt] = useState<number | null>(null);

  const frame = fps && fps > 0 ? 1 / fps : 1 / 30;

  const seek = useCallback((to: number) => {
    const v = videoRef.current;
    if (!v || !Number.isFinite(v.duration)) return;
    v.currentTime = Math.max(0, Math.min(v.duration, to));
    setTime(v.currentTime);
  }, []);

  const toggle = useCallback(() => {
    const v = videoRef.current;
    if (!v) return;
    if (v.paused) void v.play();
    else v.pause();
  }, []);

  /** Stepping only makes sense on a paused frame, so it pauses first. */
  const step = useCallback(
    (frames: number) => {
      const v = videoRef.current;
      if (!v) return;
      v.pause();
      seek(v.currentTime + frames * frame);
    },
    [frame, seek],
  );

  const toggleFullscreen = useCallback(() => {
    if (document.fullscreenElement) void document.exitFullscreen();
    else void shellRef.current?.requestFullscreen();
  }, []);

  // Playback rate lives on the element, so the button's label would otherwise
  // be a claim about nothing.
  useEffect(() => {
    if (videoRef.current) videoRef.current.playbackRate = speed;
  }, [speed, src]);

  useEffect(() => {
    const onChange = () => setFullscreen(!!document.fullscreenElement);
    document.addEventListener("fullscreenchange", onChange);
    return () => document.removeEventListener("fullscreenchange", onChange);
  }, []);

  // Keyboard, scoped to the player: shortcuts here must not fire while the
  // user is typing in the search box on the same page.
  const onKeyDown = (e: React.KeyboardEvent) => {
    const handled = () => {
      e.preventDefault();
      e.stopPropagation();
    };
    switch (e.key) {
      case " ":
      case "k":
        handled();
        toggle();
        break;
      case "ArrowLeft":
        handled();
        seek(time - (e.shiftKey ? 1 : 5));
        break;
      case "ArrowRight":
        handled();
        seek(time + (e.shiftKey ? 1 : 5));
        break;
      case "j":
        handled();
        seek(time - 10);
        break;
      case "l":
        handled();
        seek(time + 10);
        break;
      case ",":
        handled();
        step(-1);
        break;
      case ".":
        handled();
        step(1);
        break;
      case "m":
        handled();
        setMuted((m) => !m);
        break;
      case "f":
        handled();
        toggleFullscreen();
        break;
      case "Home":
        handled();
        seek(0);
        break;
      case "End":
        handled();
        seek(duration);
        break;
    }
  };

  // Scrubbing follows the pointer even when it leaves the bar, which is how
  // every scrub bar behaves and how nobody expects it to stop.
  const scrubTo = (clientX: number) => {
    const rect = scrubRef.current?.getBoundingClientRect();
    if (!rect || !duration) return;
    seek(((clientX - rect.left) / rect.width) * duration);
  };

  useEffect(() => {
    const move = (e: PointerEvent) => scrubbing.current && scrubTo(e.clientX);
    const up = () => (scrubbing.current = false);
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
  });

  const pct = duration > 0 ? (time / duration) * 100 : 0;
  const bufferedPct = duration > 0 ? (buffered / duration) * 100 : 0;

  return (
    <div
      ref={shellRef}
      tabIndex={0}
      onKeyDown={onKeyDown}
      className={cn(
        "group relative overflow-hidden rounded-[3px] bg-ink-deep outline-none",
        "focus-visible:ring-2 focus-visible:ring-signal",
        fullscreen && "flex h-screen w-screen flex-col justify-center rounded-none",
      )}
    >
      <video
        ref={videoRef}
        src={src}
        poster={poster}
        autoPlay
        playsInline
        preload="metadata"
        onClick={toggle}
        onPlay={() => setPlaying(true)}
        onPause={() => setPlaying(false)}
        onTimeUpdate={(e) => setTime(e.currentTarget.currentTime)}
        onDurationChange={(e) => setDuration(e.currentTarget.duration || 0)}
        onProgress={(e) => {
          const v = e.currentTarget;
          if (v.buffered.length) setBuffered(v.buffered.end(v.buffered.length - 1));
        }}
        onVolumeChange={(e) => {
          setMuted(e.currentTarget.muted);
          setVolume(e.currentTarget.volume);
        }}
        onEnded={() => setPlaying(false)}
        muted={muted}
        className={cn("w-full cursor-pointer", fullscreen ? "max-h-full" : "max-h-[60vh]")}
      />

      {/* Transport. Always visible rather than fading on idle: this is a
          review tool, not a cinema, and hunting for controls that vanished
          mid-frame is its own small irritation. */}
      <div className="glass-overlay border-t border-line px-3 pt-2 pb-2.5">
        <div
          ref={scrubRef}
          role="slider"
          aria-label="Seek"
          aria-valuemin={0}
          aria-valuemax={Math.round(duration)}
          aria-valuenow={Math.round(time)}
          onPointerDown={(e) => {
            scrubbing.current = true;
            scrubTo(e.clientX);
          }}
          onPointerMove={(e) => {
            const rect = e.currentTarget.getBoundingClientRect();
            setHoverAt(((e.clientX - rect.left) / rect.width) * duration);
          }}
          onPointerLeave={() => setHoverAt(null)}
          className="relative -mx-1 cursor-pointer px-1 py-2"
        >
          <div className="relative h-1.5 w-full overflow-hidden rounded-[2px] bg-raised">
            {/* What the browser has actually fetched, so a stalled network
                looks like a stall rather than a broken player. */}
            <div
              className="absolute inset-y-0 left-0 bg-line-bright"
              style={{ width: `${bufferedPct}%` }}
            />
            <div className="absolute inset-y-0 left-0 bg-signal" style={{ width: `${pct}%` }} />
          </div>
          <div
            className="absolute top-1/2 h-3 w-3 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-ink bg-signal"
            style={{ left: `${pct}%` }}
          />
          {hoverAt !== null && duration > 0 && (
            <span
              className="data pointer-events-none absolute -top-5 -translate-x-1/2 rounded-[2px] bg-ink px-1 text-[10px] text-fg"
              style={{ left: `${(hoverAt / duration) * 100}%` }}
            >
              {timecode(hoverAt)}
            </span>
          )}
        </div>

        <div className="flex items-center gap-1">
          <Control label="Previous clip" onClick={onPrevious} disabled={!onPrevious}>
            <ChevronsLeft size={15} />
          </Control>
          <Control label="Step back one frame" onClick={() => step(-1)}>
            <ChevronLeft size={15} />
          </Control>
          <Control label={playing ? "Pause" : "Play"} onClick={toggle} primary>
            {playing ? <Pause size={15} /> : <Play size={15} />}
          </Control>
          <Control label="Step forward one frame" onClick={() => step(1)}>
            <ChevronRight size={15} />
          </Control>
          <Control label="Next clip" onClick={onNext} disabled={!onNext}>
            <ChevronsRight size={15} />
          </Control>

          <span className="data ml-2 text-[11px] text-fg">
            {timecode(time)}
            <span className="text-fg-faint"> / {timecode(duration)}</span>
          </span>

          <div className="ml-auto flex items-center gap-1">
            <button
              onClick={() => setSpeed(SPEEDS[(SPEEDS.indexOf(speed) + 1) % SPEEDS.length]!)}
              title="Playback speed"
              className="data rounded-[2px] px-1.5 py-1 text-[11px] text-fg-dim hover:bg-raised hover:text-fg"
            >
              {speed}×
            </button>

            <div className="flex items-center gap-1">
              <Control label={muted ? "Unmute" : "Mute"} onClick={() => setMuted((m) => !m)}>
                {muted || volume === 0 ? <VolumeX size={15} /> : <Volume2 size={15} />}
              </Control>
              <input
                type="range"
                min={0}
                max={1}
                step={0.05}
                value={muted ? 0 : volume}
                onChange={(e) => {
                  const v = Number(e.target.value);
                  setVolume(v);
                  setMuted(v === 0);
                  if (videoRef.current) videoRef.current.volume = v;
                }}
                aria-label="Volume"
                className="h-1 w-16 accent-signal"
              />
            </div>

            <Control
              label={fullscreen ? "Leave fullscreen" : "Fullscreen"}
              onClick={toggleFullscreen}
            >
              {fullscreen ? <Minimize2 size={15} /> : <Maximize2 size={15} />}
            </Control>
          </div>
        </div>
      </div>
    </div>
  );
}

function Control({
  label,
  onClick,
  disabled,
  primary,
  children,
}: {
  label: string;
  onClick?: () => void;
  disabled?: boolean;
  primary?: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      title={label}
      aria-label={label}
      className={cn(
        "grid h-7 w-7 place-items-center rounded-[2px] transition-colors",
        primary
          ? "bg-signal text-signal-contrast hover:bg-signal/85"
          : "text-fg-dim hover:bg-raised hover:text-fg",
        disabled && "pointer-events-none opacity-30",
      )}
    >
      {children}
    </button>
  );
}

/** `m:ss` for short clips, `h:mm:ss` when a clip is long enough to need it. */
export function timecode(seconds: number) {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const whole = Math.floor(seconds);
  const h = Math.floor(whole / 3600);
  const m = Math.floor((whole % 3600) / 60);
  const s = whole % 60;
  return h > 0
    ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`
    : `${m}:${String(s).padStart(2, "0")}`;
}
