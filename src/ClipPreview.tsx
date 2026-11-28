import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { fmtDuration, mediaSrc } from "./api";
import { useThumb } from "./hooks";
import { Slider } from "./Slider";
import { useSpoiler } from "./spoiler";

/**
 * A clip's thumbnail that plays a muted preview while the pointer rests on it
 * (from about a third in, where the action usually is), and opens the player
 * when clicked. Nothing loads until you hover, so a long list stays light.
 */
export function HoverPreview({ path, className = "", onOpen }: { path: string; className?: string; onOpen: () => void }) {
  const thumb = useThumb(path);
  // A spoiler stays still (and blurred) until clicked; that click only shows it.
  const spoiler = useSpoiler(path);
  const [hover, setHover] = useState(false);
  const [playing, setPlaying] = useState(false);
  const timer = useRef(0);
  useEffect(() => () => clearTimeout(timer.current), []);
  return (
    <button
      type="button"
      className={`hover-preview ${thumb ? "" : "loading"} ${className}`}
      aria-label="Play this clip"
      title="Hover to preview, click to play"
      onPointerEnter={() => {
        // A short wait, so sweeping the pointer across the list doesn't start every clip.
        timer.current = window.setTimeout(() => setHover(true), 220);
      }}
      onPointerLeave={() => {
        clearTimeout(timer.current);
        setHover(false);
        setPlaying(false);
      }}
      onClick={(e) => {
        e.preventDefault(); // inside a row's label: don't tick its box
        e.stopPropagation();
        if (spoiler.hidden) spoiler.reveal();
        else onOpen();
      }}
    >
      {thumb && <img src={thumb} alt="" />}
      {hover && !spoiler.hidden && (
        <video
          src={mediaSrc(path)}
          muted
          autoPlay
          loop
          playsInline
          preload="metadata"
          className={playing ? "on" : ""}
          onLoadedMetadata={(e) => {
            const v = e.currentTarget;
            if (v.duration > 6) v.currentTime = v.duration * 0.33;
          }}
          onPlaying={() => setPlaying(true)}
        />
      )}
      <span className="hover-play" aria-hidden="true">
        <svg viewBox="0 0 24 24" width="16" height="16">
          <path d="M8 5.5v13l10.5-6.5z" fill="currentColor" />
        </svg>
      </span>
    </button>
  );
}

/**
 * A clip, big, in the app: play and pause, a scrubber to move through it, sound
 * on or off. Space plays or pauses, ← → jump 5 seconds, Esc closes.
 */
export function ClipPlayer({ path, title, subtitle, onClose }: { path: string; title: string; subtitle?: string; onClose: () => void }) {
  const video = useRef<HTMLVideoElement>(null);
  const [t, setT] = useState(0);
  const [dur, setDur] = useState(0);
  const [paused, setPaused] = useState(false);
  const [muted, setMuted] = useState(false);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      const v = video.current;
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      } else if (!v) return;
      else if (e.key === " " || e.key === "k") {
        e.preventDefault();
        if (v.paused) v.play().catch(() => {});
        else v.pause();
      } else if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        e.preventDefault();
        v.currentTime = Math.max(0, Math.min(v.duration || 0, v.currentTime + (e.key === "ArrowLeft" ? -5 : 5)));
      } else if (e.key === "m") setMuted((m) => !m);
    };
    // Capture: the dialog behind it doesn't get these keys.
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
  }, [onClose]);
  const toggle = () => {
    const v = video.current;
    if (!v) return;
    if (v.paused) v.play().catch(() => {});
    else v.pause();
  };
  return createPortal(
    <div
      className="player-backdrop"
      onClick={(e) => {
        e.stopPropagation();
        onClose();
      }}
    >
      <div className="player" role="dialog" aria-modal="true" aria-label={`Playing ${title}`} onClick={(e) => e.stopPropagation()}>
        <div className="player-head">
          <span className="player-title">
            <strong title={title}>{title}</strong>
            {subtitle && <small className="muted">{subtitle}</small>}
          </span>
          <button className="round player-x" aria-label="Close" title="Close (Esc)" onClick={onClose}>
            <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true">
              <path d="M6 6l12 12M18 6L6 18" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
            </svg>
          </button>
        </div>
        <div className="player-stage" onClick={toggle}>
          {failed ? (
            <p className="muted player-fail">This clip can't be played here. Show it in its folder to open it in another player.</p>
          ) : (
            <video
              ref={video}
              src={mediaSrc(path)}
              autoPlay
              playsInline
              muted={muted}
              onTimeUpdate={(e) => setT(e.currentTarget.currentTime)}
              onLoadedMetadata={(e) => setDur(e.currentTarget.duration || 0)}
              onPlay={() => setPaused(false)}
              onPause={() => setPaused(true)}
              onError={() => setFailed(true)}
            />
          )}
        </div>
        <div className="player-bar">
          <button className="round player-btn" aria-label={paused ? "Play" : "Pause"} title={paused ? "Play (Space)" : "Pause (Space)"} onClick={toggle}>
            <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
              {paused ? <path d="M8 5.5v13l10.5-6.5z" fill="currentColor" /> : <path d="M7 5h3.5v14H7zM13.5 5H17v14h-3.5z" fill="currentColor" />}
            </svg>
          </button>
          <span className="player-time">{fmtDuration(t)}</span>
          <Slider
            className="player-scrub"
            aria-label="Position in the clip"
            min={0}
            max={Math.max(dur, 0.1)}
            step={0.05}
            value={Math.min(t, dur)}
            onValue={(v) => {
              setT(v);
              if (video.current) video.current.currentTime = v;
            }}
          />
          <span className="player-time muted">{fmtDuration(dur)}</span>
          <button className="round player-btn" aria-label={muted ? "Sound on" : "Mute"} title={muted ? "Sound on (M)" : "Mute (M)"} onClick={() => setMuted((m) => !m)}>
            <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M11 5 6 9H3v6h3l5 4z" fill="currentColor" stroke="none" />
              {muted ? <path d="m16 9 5 6m0-6-5 6" /> : <path d="M15.5 8.5a5 5 0 0 1 0 7M18.5 5.5a9 9 0 0 1 0 13" />}
            </svg>
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
