import { Select } from "./Select";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  ArrowLeftFromLine,
  ArrowRightFromLine,
  ChevronLeft,
  Maximize,
  Minimize,
  PanelRight,
  Crop as CropIcon,
  Pause,
  Play,
  RectangleHorizontal,
  RotateCcw,
  RotateCw,
  Undo2,
  Volume1,
  Volume2,
  VolumeX,
} from "lucide-react";
import { api, ClipFile, codecLabel, errText, fmtSize, fmtTime, Health, PreviewAudio, Probe, Settings, TrackEdit, Upload, UploadProgress, Waveform, mediaSrc } from "./api";
import { AudioLanes, hasAudioEdits, isMutedAt } from "./AudioLanes";
import { useThumb } from "./hooks";
import { SharePanel, ShareRequest } from "./Share";
import { DiagnosticsButton } from "./About";
import { HealReview } from "./HealReview";
import { HealthNotice } from "./Health";
import { StudioNas } from "./NasProgress";
import { Slider } from "./Slider";
import { Crop, CROP_PRESETS, CropBox, cropPixels, fitRatio, isWhole, WHOLE } from "./CropBox";

type Job = { label: string; pct?: number } | null;

function loadNumber(key: string, fallback: number) {
  try {
    const v = localStorage.getItem(key);
    const n = v === null ? NaN : Number(v);
    return Number.isFinite(n) ? n : fallback;
  } catch {
    return fallback;
  }
}

function loadJson<T>(key: string, fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    return v ? (JSON.parse(v) as T) : fallback;
  } catch {
    return fallback;
  }
}

function saveJson(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* not remembered */
  }
}

function save(key: string, value: number) {
  try {
    localStorage.setItem(key, String(value));
  } catch {
    /* storage unavailable: volume just isn't remembered */
  }
}

/** Speaker with 0, 1 or 2 sound waves, or a cross when muted. */
function SpeakerIcon({ level }: { level: number }) {
  const C = level === 0 ? VolumeX : level > 0.5 ? Volume2 : Volume1;
  return <C size={18} strokeWidth={2} aria-hidden="true" />;
}
/** Below this width the side panel moves under the video. */
const NARROW_AT = 1180;
type SideTab = "details" | "audio" | "share" | "save";
const SIDE_TABS: [SideTab, string][] = [
  ["save", "Save as"],
  ["share", "Share"],
  ["audio", "Audio"],
  ["details", "Details"],
];
type Result = { text: string; path?: string; err?: boolean } | null;

// The player's icons, from the same set as the rest of the app.
const ic = { strokeWidth: 2, "aria-hidden": true } as const;
const Icon = {
  play: <Play {...ic} fill="currentColor" />,
  pause: <Pause {...ic} fill="currentColor" />,
  fullscreen: <Maximize {...ic} />,
  exitFullscreen: <Minimize {...ic} />,
  theater: <RectangleHorizontal {...ic} />,
  theaterOff: <PanelRight {...ic} />,
  back5: <RotateCcw {...ic} />,
  fwd5: <RotateCw {...ic} />,
  markIn: <ArrowRightFromLine {...ic} />,
  markOut: <ArrowLeftFromLine {...ic} />,
  reset: <Undo2 {...ic} />,
};

/** Tick spacing for the ruler: a labelled tick every `major` seconds, small ones every `minor`. */
export function rulerSteps(duration: number): { major: number; minor: number } {
  const steps: [number, number][] = [
    [1, 0.5], [2, 1], [5, 1], [10, 2], [15, 5], [30, 10], [60, 10], [120, 30], [300, 60], [600, 120], [900, 300], [1800, 600],
  ];
  for (const [major, minor] of steps) if (duration / major <= 7) return { major, minor };
  return { major: 3600, minor: 900 };
}

function rulerLabel(t: number) {
  const m = Math.floor(t / 60);
  const s = Math.round(t % 60);
  return `${m}:${String(s).padStart(2, "0")}`;
}

/** Time ruler under the timeline: small ticks plus labelled bigger ones, spaced to fit the clip. */
function Ruler({ duration, onPointerDown }: { duration: number; onPointerDown: (e: React.PointerEvent) => void }) {
  if (!duration) return <div className="ruler" />;
  const { major, minor } = rulerSteps(duration);
  const ticks: { t: number; big: boolean }[] = [];
  const n = Math.floor(duration / minor + 1e-6);
  for (let i = 0; i <= n; i++) {
    const t = i * minor;
    ticks.push({ t, big: Math.abs(t / major - Math.round(t / major)) < 1e-6 });
  }
  return (
    <div className="ruler" onPointerDown={onPointerDown} aria-hidden="true">
      {ticks.map(({ t, big }) => (
        <span key={t} className={`tick ${big ? "big" : ""}`} style={{ left: `${(t / duration) * 100}%` }}>
          {/* Skip a label that would crowd the clip's end. */}
          {big && (duration - t) / duration > 0.04 && <em>{rulerLabel(t)}</em>}
        </span>
      ))}
      <span className="tick big end" style={{ left: "100%" }}>
        <em>{rulerLabel(duration)}</em>
      </span>
    </div>
  );
}

/** The app window, or null in the browser preview. */
function appWindow() {
  try {
    return getCurrentWindow();
  } catch {
    return null;
  }
}

function StripItem({ clip, current, onOpen }: { clip: ClipFile; current: boolean; onOpen: (p: string) => void }) {
  const thumb = useThumb(clip.path, clip.modified);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    // Scroll only the strip sideways: scrollIntoView would also scroll the
    // editor down to the strip, hiding the top of the video in a short window.
    const el = ref.current;
    const strip = el?.parentElement;
    if (!current || !el || !strip) return;
    const l = el.offsetLeft - strip.offsetLeft;
    if (l < strip.scrollLeft) strip.scrollLeft = l;
    else if (l + el.offsetWidth > strip.scrollLeft + strip.clientWidth) strip.scrollLeft = l + el.offsetWidth - strip.clientWidth;
  }, [current]);
  return (
    <button
      ref={ref}
      className={`strip-item ${current ? "on" : ""}`}
      onClick={() => !current && onOpen(clip.path)}
      title={clip.path}
      aria-current={current ? "true" : undefined}
    >
      <div className="strip-thumb">{thumb && <img src={thumb} alt="" loading="lazy" />}</div>
      <span>{clip.name}</span>
    </button>
  );
}

/** Thumbnails of recent clips, to jump between them. */
function ClipStrip({ clips, current, onOpen }: { clips: ClipFile[]; current: string; onOpen: (p: string) => void }) {
  if (clips.length < 2) return null;
  return (
    <div className="strip" role="list" aria-label="Clips">
      {clips.map((c) => (
        <StripItem key={c.path} clip={c} current={c.path === current} onOpen={onOpen} />
      ))}
    </div>
  );
}

function PickerCard({ clip, onOpen }: { clip: ClipFile; onOpen: (p: string) => void }) {
  const thumb = useThumb(clip.path, clip.modified);
  const when = new Date(clip.modified).toLocaleString(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
  return (
    <button className="card" onClick={() => onOpen(clip.path)} title={clip.path}>
      <div className="thumb">
        {thumb && <img src={thumb} alt="" loading="lazy" />}
        <div className="thumb-play" aria-hidden="true">{Icon.play}</div>
      </div>
      <div className="card-title">{clip.name}</div>
      <div className="card-meta">
        {clip.folder} · {when} · {fmtSize(clip.size)}
      </div>
    </button>
  );
}

export function Studio({
  path,
  clips,
  onOpen,
  onDone,
  onDelete,
  settings,
  update,
  onBack,
  uploads,
  links,
  onShare,
  health,
  onRescued,
  movedFrom = null,
}: {
  /** This clip was open from that path until its copy on this PC was freed up: keep your place. */
  movedFrom?: string | null;
  health?: Health;
  onRescued?: (path: string) => void;
  onShare?: (r: ShareRequest) => void;
  uploads?: Record<string, UploadProgress>;
  links?: Record<string, Upload[]>;
  path: string | null;
  clips: ClipFile[] | null;
  onBack: () => void;
  onOpen: (path: string) => void;
  onDone: () => void;
  onDelete: (paths: string[]) => Promise<boolean>;
  settings: Settings | null;
  update: (p: Partial<Settings>) => Promise<void>;
}) {
  // A clip only on the NAS can't be replaced: a network folder has no Recycle Bin.
  const onNetwork = !!path && (path.startsWith("\\\\") || clips?.find((c) => c.path === path)?.location === "nas");
  // A clip in your NAS folder can be replaced: the original is set aside on the NAS for 30 days.
  const nasRoot = (settings?.nasDir ?? "").replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
  const inNas = !!path && !!nasRoot && path.replace(/\//g, "\\").toLowerCase().startsWith(nasRoot + "\\");
  const noReplace = onNetwork && !inNas;
  const replace = !!settings?.replaceOriginalOnEdit && !noReplace;
  const [reload, setReload] = useState(0);
  const video = useRef<HTMLVideoElement>(null);
  const bar = useRef<HTMLDivElement>(null);
  const wrap = useRef<HTMLDivElement>(null);
  const [fullscreen, setFullscreen] = useState(false);
  const [ar, setAr] = useState(16 / 9);
  // Shown in the player until the first frame is decoded (no black box while a clip loads).
  const openClip = (clips ?? []).find((c) => c.path === path);
  const poster = useThumb(path ?? "", openClip?.modified ?? 0);
  // Theater hides the editing panel, so it lasts for this visit only (it used to
  // be remembered, and one stray "T" left every clip opening without its tools).
  const [theater, setTheater] = useState(false);
  // In a narrow window the side panel moves under the video, as tabs, so the
  // video keeps the full width.
  const studioRef = useRef<HTMLDivElement>(null);
  const sideRef = useRef<HTMLElement>(null);
  const [narrow, setNarrow] = useState(false);
  const [sideTab, setSideTab] = useState<SideTab>("save");
  useEffect(() => {
    const el = studioRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setNarrow(el.clientWidth < NARROW_AT));
    ro.observe(el);
    return () => ro.disconnect();
  });
  const group = (id: SideTab) => `side-group ${narrow && sideTab !== id ? "off" : ""}`;
  useEffect(() => {
    try {
      localStorage.removeItem("rewind.theater");
    } catch {
      /* nothing to forget */
    }
  }, []);
  // Overlay controls: shown while the pointer moves over the video, hidden after a moment of stillness.
  const [hover, setHover] = useState(false);
  const [active, setActive] = useState(false);
  const idle = useRef<number | undefined>(undefined);
  const poke = useCallback(() => {
    setActive(true);
    window.clearTimeout(idle.current);
    idle.current = window.setTimeout(() => setActive(false), 2200);
  }, []);
  useEffect(() => () => window.clearTimeout(idle.current), []);
  const [info, setInfo] = useState<Probe | null>(null);
  const [duration, setDuration] = useState(0);
  const [time, setTime] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [volume, setVolumeState] = useState(() => loadNumber("rewind.volume", 1));
  const [muted, setMutedState] = useState(() => loadNumber("rewind.muted", 0) === 1);
  const setVolume = useCallback((v: number) => {
    const clamped = Math.round(Math.min(1, Math.max(0, v)) * 100) / 100;
    setVolumeState(clamped);
    if (clamped > 0) setMutedState(false); // turning it up unmutes, like most players
    save("rewind.volume", clamped);
  }, []);
  const toggleMute = useCallback(() => setMutedState((m) => !m), []);
  // A brief "Volume 60%" over the video when the wheel or ↑ ↓ change it.
  const [volFlash, setVolFlash] = useState<number | null>(null);
  const flashTimer = useRef<number | undefined>(undefined);
  const nudgeVolume = useCallback(
    (by: number) => {
      const v = Math.round(Math.min(1, Math.max(0, (muted ? 0 : volume) + by)) * 100) / 100;
      setVolume(v);
      if (v === 0) setMutedState(true);
      setVolFlash(v);
      window.clearTimeout(flashTimer.current);
      flashTimer.current = window.setTimeout(() => setVolFlash(null), 900);
    },
    [muted, volume, setVolume],
  );
  useEffect(() => save("rewind.muted", muted ? 1 : 0), [muted]);
  // The video element only plays the first audio track; other tracks (the
  // mic) play from their own <audio> elements, kept in sync with the video.
  const [tracks, setTracks] = useState<PreviewAudio | null>(null);
  const extraAudio = useRef<Map<number, HTMLAudioElement>>(new Map());
  const [trackVol, setTrackVol] = useState<Record<string, number>>(() => loadJson("rewind.trackVolume", {}));
  const [trackMuted, setTrackMuted] = useState<Record<string, boolean>>({});
  const tv = (title: string) => trackVol[title] ?? 1;
  const setTrack = (title: string, v: number) => {
    const next = { ...trackVol, [title]: Math.round(Math.min(1, Math.max(0, v)) * 100) / 100 };
    setTrackVol(next);
    saveJson("rewind.trackVolume", next);
    if (v > 0) setTrackMuted((m) => ({ ...m, [title]: false }));
  };
  // Audio edits for the saved file (per track: volume in dB, muted stretches), heard in the preview too.
  const [audioEdits, setAudioEdits] = useState<TrackEdit[]>([]);
  const [waves, setWaves] = useState<Waveform[] | null>(null);
  const audioEdited = hasAudioEdits(audioEdits);
  const editsForSave = audioEdited ? audioEdits : undefined;
  const [audioOnly, setAudioOnly] = useState("all");
  const [healOpen, setHealOpen] = useState(false);
  // Keep every element's volume in step (also after it remounts for a new clip).
  const applyVolumes = useRef(() => {});
  applyVolumes.current = () => {
    const allMuted = muted || volume === 0;
    const v = video.current;
    const now = v?.currentTime ?? 0;
    const gain = (i: number) => Math.pow(10, (audioEdits[i]?.gainDb ?? 0) / 20);
    if (v) {
      const first = tracks?.firstTitle ?? "Game";
      v.volume = Math.min(1, volume * tv(first) * gain(0));
      v.muted = allMuted || !!trackMuted[first] || isMutedAt(audioEdits[0], now);
    }
    tracks?.extra.forEach((t) => {
      const a = extraAudio.current.get(t.index);
      if (a) {
        a.volume = Math.min(1, volume * tv(t.title) * gain(t.index));
        a.muted = allMuted || !!trackMuted[t.title] || isMutedAt(audioEdits[t.index], now);
      }
    });
  };
  useEffect(() => applyVolumes.current());
  // Muted stretches start and stop on the frame while playing, not at the next time update.
  useEffect(() => {
    if (!playing || !audioEdits.some((e) => e.mutes.length)) return;
    let raf = 0;
    const tick = () => {
      applyVolumes.current();
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing, audioEdits]);
  // Mirror play / pause / seek / speed from the video onto the extra tracks,
  // and nudge them back if they drift more than ~60 ms.
  const syncExtra = useCallback((hard = false) => {
    const v = video.current;
    if (!v) return;
    extraAudio.current.forEach((a) => {
      if (hard || Math.abs(a.currentTime - v.currentTime) > 0.06) a.currentTime = v.currentTime;
      a.playbackRate = v.playbackRate;
      if (v.paused && !a.paused) a.pause();
      if (!v.paused && a.paused) a.play().catch(() => {});
    });
  }, []);
  const [range, setRange] = useState<[number, number]>([0, 0]);
  // Which file the player failed on. Tied to the path, so an error from the
  // previous clip can't show up on the next one.
  const [playErrorFor, setPlayErrorFor] = useState<string | null>(null);
  const playError = !!path && playErrorFor === path;
  const retried = useRef<string | null>(null);
  const [job, setJob] = useState<Job>(null);
  const [result, setResult] = useState<Result>(null);
  const [targetMb, setTargetMb] = useState(10);
  // Crop: the box over the video (fractions of the frame), whether it's being
  // edited, and its locked shape (null = free).
  const [crop, setCrop] = useState<Crop | null>(null);
  const [cropping, setCropping] = useState(false);
  const [cropRatio, setCropRatio] = useState<number | null>(null);

  // Where you were in the clip, for when it carries on from its NAS copy.
  const snap = useRef<{ path: string | null; time: number; playing: boolean; range: [number, number]; edits: TrackEdit[]; crop: Crop | null } | null>(null);
  const resume = useRef<{ path: string; time: number; playing: boolean } | null>(null);
  useEffect(() => {
    const was = movedFrom && snap.current?.path === movedFrom ? snap.current : null;
    setInfo(null);
    setPlayErrorFor(null);
    setResult(null);
    setTime(was?.time ?? 0);
    setRange(was?.range ?? [0, 0]);
    setDuration(0);
    setTracks(null);
    setAudioEdits(was?.edits ?? []);
    setWaves(null);
    setCrop(was?.crop ?? null);
    setCropping(false);
    // Kept through a retry of the same clip (the player reloads once on a load hiccup).
    if (was && path) resume.current = { path, time: was.time, playing: was.playing };
    else if (resume.current?.path !== path) resume.current = null;
    if (!path) return;
    let alive = true;
    // On a hard drive, reading the file for the waveform and the extra audio
    // tracks at the same time as the video made opening a clip take seconds.
    // The picture comes first; the rest follows once it shows (or soon after).
    let started = false;
    const rest = () => {
      if (started || !alive) return;
      started = true;
      api
        .clipWaveforms(path)
        .then((w) => alive && setWaves(w))
        .catch(() => alive && setWaves([]));
      api
        .previewAudio(path)
        .then((t) => {
          if (!alive) return;
          setTracks(t);
          // A track that's the others already mixed (Medal's "All Audio") starts muted
          // here, or the game and mic play twice. Saves leave it out too.
          const mixTitle = t.mix == null ? null : t.mix === 0 ? t.firstTitle : t.extra.find((x) => x.index === t.mix)?.title;
          if (mixTitle) setTrackMuted((m) => ({ ...m, [mixTitle]: true }));
        })
        // Only the preview of the other tracks is lost; editing them still works.
        .catch(() => alive && setTracks({ firstTitle: "Game audio", extra: [] }));
    };
    const v = video.current;
    const onReady = () => rest();
    v?.addEventListener("loadeddata", onReady);
    const fallback = window.setTimeout(rest, 1500);
    api
      .probe(path, String(reload), true)
      .then((p) => {
        // Probes can queue behind thumbnails: ignore one for a clip that's no longer open.
        if (!alive) return;
        setInfo(p);
        setDuration((d) => d || p.duration);
        setRange((r) => (r[1] ? r : [0, p.duration]));
      })
      .catch(() => {});
    return () => {
      alive = false;
      v?.removeEventListener("loadeddata", onReady);
      window.clearTimeout(fallback);
    };
  }, [path, reload]);
  useEffect(() => {
    // Once the file is gone (freed up) the player has already reset: keep the last good place.
    if (path && clips && !clips.some((c) => c.path === path)) return;
    snap.current = { path, time: video.current?.currentTime ?? time, playing, range, edits: audioEdits, crop };
  });

  // Seeking: one seek at a time, always to the newest spot. Setting currentTime
  // while a seek is still decoding cancels it, so a fast drag used to show only
  // the odd keyframe; waiting for each seek to land shows ~40 real frames a second.
  const pendingSeek = useRef<number | null>(null);
  const scrubbing = useRef(false);
  const resumeAfterScrub = useRef(false);
  const seek = useCallback((t: number) => {
    const v = video.current;
    if (!v) return;
    const to = Math.max(0, Math.min(t, v.duration || t));
    setTime(to);
    if (v.seeking) pendingSeek.current = to;
    else {
      pendingSeek.current = null;
      v.currentTime = to;
    }
  }, []);
  const beginScrub = () => {
    const v = video.current;
    if (!v || scrubbing.current) return;
    scrubbing.current = true;
    resumeAfterScrub.current = !v.paused;
    v.pause();
  };
  const endScrub = () => {
    scrubbing.current = false;
    const v = video.current;
    // Resume now if the last seek already landed; otherwise onSeeked resumes once it does.
    if (v && resumeAfterScrub.current && !v.seeking && pendingSeek.current == null) {
      resumeAfterScrub.current = false;
      v.play().catch(() => {});
    }
  };
  const onSeeked = () => {
    const v = video.current;
    if (!v) return;
    if (pendingSeek.current != null) {
      const to = pendingSeek.current;
      pendingSeek.current = null;
      v.currentTime = to;
      return;
    }
    syncExtra(true);
    if (!scrubbing.current && resumeAfterScrub.current) {
      resumeAfterScrub.current = false;
      v.play().catch(() => {});
    }
  };

  const toggle = useCallback(() => {
    const v = video.current;
    if (!v) return;
    if (v.paused) {
      if (v.currentTime >= range[1] - 0.05) v.currentTime = range[0];
      v.play();
    } else v.pause();
  }, [range]);

  // Fullscreen: Hindsight's window goes fullscreen and the player covers it.
  // (Not the browser Fullscreen API, which embedded web views can refuse.)
  // Esc, F or the button leave it.
  const toggleFullscreen = useCallback(() => setFullscreen((f) => !f), []);
  const wasFullscreen = useRef(false);
  useEffect(() => {
    if (fullscreen === wasFullscreen.current) return;
    wasFullscreen.current = fullscreen;
    appWindow()?.setFullscreen(fullscreen).catch(() => {});
  }, [fullscreen]);
  useEffect(
    () => () => {
      if (wasFullscreen.current) appWindow()?.setFullscreen(false).catch(() => {});
    },
    [],
  );
  const toggleTheater = useCallback(() => setTheater((t: boolean) => !t), []);

  // Minimized, hidden to the tray, or covered: pause right where it is.
  useEffect(() => {
    const pause = () => video.current?.pause();
    const onVis = () => document.hidden && pause();
    document.addEventListener("visibilitychange", onVis);
    const un = listen("main-hidden", pause);
    return () => {
      document.removeEventListener("visibilitychange", onVis);
      un.then((f) => f());
    };
  }, []);

  // Keyboard: Space play, I / O set points, arrows step.
  useEffect(() => {
    if (!path) return;
    const onKey = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement).tagName === "INPUT" || (e.target as HTMLElement).tagName === "SELECT") return;
      const v = video.current;
      if (!v) return;
      if (e.code === "Space") {
        e.preventDefault();
        toggle();
      } else if (e.key === "i" || e.key === "I") setRange(([, b]) => [Math.min(v.currentTime, b - 0.1), b]);
      else if (e.key === "o" || e.key === "O") setRange(([a]) => [a, Math.max(v.currentTime, a + 0.1)]);
      else if (e.key === "ArrowLeft") seek(v.currentTime - (e.shiftKey ? 1 / 60 : 5));
      else if (e.key === "ArrowRight") seek(v.currentTime + (e.shiftKey ? 1 / 60 : 5));
      else if (e.key === "ArrowUp") {
        e.preventDefault();
        nudgeVolume(0.1);
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        nudgeVolume(-0.1);
      } else if (e.key === "m" || e.key === "M") toggleMute();
      else if ((e.key === "f" || e.key === "F") && !e.ctrlKey && !e.altKey) toggleFullscreen();
      else if (e.key === "Escape" && fullscreen) setFullscreen(false);
      else if ((e.key === "Escape" || e.key === "Enter") && cropping) setCropping(false);
      else if (e.key === "Escape" && !document.querySelector(".modal-backdrop")) onBack();
      else if ((e.key === "t" || e.key === "T") && !e.ctrlKey && !e.altKey && !fullscreen) toggleTheater();
      else if ((e.key === "c" || e.key === "C") && !e.ctrlKey && !e.altKey && !fullscreen) setCropping((v) => !v);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [path, toggle, seek, nudgeVolume, toggleMute, toggleFullscreen, toggleTheater, fullscreen, onBack, cropping]);

  // Dragging on the timeline: handles move in/out, anywhere else scrubs.
  const drag = (kind: "in" | "out" | "seek") => (e: React.PointerEvent) => {
    e.stopPropagation();
    const el = bar.current;
    if (!el || !duration) return;
    el.setPointerCapture(e.pointerId);
    const at = (x: number) => {
      const r = el.getBoundingClientRect();
      return Math.max(0, Math.min(1, (x - r.left) / r.width)) * duration;
    };
    const apply = (x: number) => {
      const t = at(x);
      if (kind === "in") setRange(([, b]) => [Math.min(t, b - 0.1), b]);
      else if (kind === "out") setRange(([a]) => [a, Math.max(t, a + 0.1)]);
      seek(t);
    };
    beginScrub();
    apply(e.clientX);
    const move = (ev: PointerEvent) => apply(ev.clientX);
    const up = (ev: PointerEvent) => {
      // Land exactly where the mouse let go.
      apply(ev.clientX);
      endScrub();
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", up);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
  };

  // The fullscreen progress bar: click or drag to jump.
  const scrubProgress = (e: React.PointerEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    el.setPointerCapture(e.pointerId);
    const to = (x: number) => {
      const r = el.getBoundingClientRect();
      seek(Math.max(0, Math.min(1, (x - r.left) / r.width)) * duration);
    };
    beginScrub();
    to(e.clientX);
    const move = (ev: PointerEvent) => to(ev.clientX);
    const up = (ev: PointerEvent) => {
      to(ev.clientX);
      endScrub();
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", up);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
  };

  const run = async (label: string, fn: () => Promise<string>, replaces = false) => {
    setJob({ label });
    setResult(null);
    // How far the save has got, from ffmpeg (see edit-progress in lib.rs).
    const un = listen<{ path: string; pct: number }>("edit-progress", (e) => {
      if (e.payload.path === path) setJob((j) => (j ? { ...j, pct: Math.max(j.pct ?? 0, e.payload.pct) } : j));
    });
    try {
      const out = await fn();
      onDone();
      if (replaces) {
        // The edit now lives where the original was: show it.
        video.current?.pause();
        onOpen(out);
        setReload((k) => k + 1);
        setResult({
          text: inNas
            ? "Replaced the original on the NAS with your edit (the old file is in the NAS app's Trash for 30 days)."
            : "Replaced the original with your edit (old file is in the Recycle Bin).",
        });
        return;
      }
      setResult({ text: `Saved ${out.split("\\").pop()}`, path: out });
    } catch (e) {
      setResult({ text: errText(e), err: true });
    } finally {
      un.then((f) => f());
      setJob(null);
    }
  };

  if (!path) {
    const recent = (clips ?? []).slice(0, 12);
    return (
      <div className="page">
        <h1>Studio</h1>
        {recent.length === 0 ? (
          <div className="empty">
            <strong>No clips yet</strong>
            <span>Save a clip with your hotkey, then edit it here.</span>
          </div>
        ) : (
          <>
            <p className="muted" style={{ margin: 0 }}>Pick a clip to edit. Recent clips:</p>
            <div className="grid">
              {recent.map((c) => (
                <PickerCard key={c.path} clip={c} onOpen={onOpen} />
              ))}
            </div>
          </>
        )}
      </div>
    );
  }

  const [a, b] = range;
  const pct = (t: number) => (duration ? `${Math.min(100, Math.max(0, (t / duration) * 100))}%` : "0%");
  const name = path.split("\\").pop() ?? path;
  const busy = !!job;
  const wholeFile = a <= 0.01 && duration - b <= 0.05;
  const cropped = !isWhole(crop);
  const cropForSave = cropped ? crop! : undefined;
  const pickRatio = (r: number | null) => {
    setCropRatio(r);
    setCrop(r ? fitRatio(r, ar || 16 / 9) : (crop ?? WHOLE));
  };
  const cropLabel = () => {
    if (!crop || !info?.width || !info?.height) return undefined;
    const [w, h] = cropPixels(crop, info.width, info.height);
    return `${w} × ${h}`;
  };
  const hasPreviewAudio = !!tracks && (tracks.extra.length > 0 || (info?.audioTracks ?? 0) > 0);
  const trackTitles = Array.from({ length: info?.audioTracks ?? 0 }, (_, i) =>
    i === 0 ? (tracks?.firstTitle ?? "Game audio") : (tracks?.extra[i - 1]?.title ?? (i === 1 ? "Microphone" : `Track ${i + 1}`)),
  );

  return (
    <div className="studio" ref={studioRef}>
      <div className="studio-top">
        <button className="btn back" onClick={onBack} title="Back to your clips (Esc)">
          <ChevronLeft size={16} strokeWidth={2} aria-hidden="true" />
          Clips
        </button>
        <div className="studio-title" style={{ flex: 1 }} title={path}>
          {name}
        </div>
        <StudioNas clip={clips?.find((c) => c.path === path)} />
        {/* One button both ways, with its key, so nobody has to know about T. */}
        <button className="btn" onClick={toggleTheater} aria-pressed={theater} title={theater ? "Show the editing tools again (T)" : "Hide the editing tools for a bigger video (T)"}>
          {theater ? Icon.theaterOff : Icon.theater}
          {theater ? "Show editing tools" : "Hide editing tools"}
          <span className="kbd">T</span>
        </button>
        <button
          className={`btn primary ${job ? "saving" : ""}`}
          style={job?.pct != null ? ({ "--pct": `${job.pct}%` } as React.CSSProperties) : undefined}
          disabled={busy || (wholeFile && !audioEdited && !cropped)}
          title="Cuts exactly at your start and end frames, with your audio changes. Only the moment before the first keyframe is re-encoded (under a second for Hindsight's clips); the rest of the video is copied untouched, so it's quick and keeps full quality."
          onClick={() =>
            run(wholeFile ? (cropped ? "Saving crop…" : "Saving audio changes…") : "Saving trim…", () => api.trim(path, a, b, true, replace, editsForSave, cropForSave), replace)
          }
        >
          {job
            ? `${job.label.replace("…", "")}${job.pct != null ? ` ${job.pct}%` : "…"}`
            : wholeFile && cropped ? "Save crop" : wholeFile && audioEdited ? "Save audio changes" : "Save trim"}
        </button>
      </div>
      <div className={`studio-body ${theater ? "theater" : ""} ${narrow ? "narrow" : ""}`}>
        <div className="stage">
          {/* The box takes the video's own shape, so its rounded corners are the picture's corners. */}
          <div className="video-box">
          <div
            ref={wrap}
            style={{ "--ar": ar } as React.CSSProperties}
            className={`video-wrap ${fullscreen ? "fs" : ""} ${hover && (active || !playing) ? "ui" : ""} ${fullscreen && playing && !active ? "idle" : ""}`}
            onPointerMove={() => {
              setHover(true);
              poke();
            }}
            onPointerEnter={() => setHover(true)}
            onPointerLeave={() => setHover(false)}
            // The mouse wheel over the video turns the volume up or down (5% a notch).
            onWheel={(e) => {
              if (e.ctrlKey) return;
              nudgeVolume(e.deltaY < 0 ? 0.05 : -0.05);
            }}
          >
            <video
              key={`${path}#${reload}`}
              ref={video}
              poster={poster ?? undefined}
              // The version tag makes the player load the file fresh after an edit replaced it
              // in place (same name); otherwise WebView2 can reuse its cached copy of the old file.
              src={mediaSrc(path, `${(clips ?? []).find((c) => c.path === path)?.modified ?? 0}-${reload}`)}
              onLoadedMetadata={(e) => {
                // Carrying on from the NAS copy: back to where you were, still playing.
                const r = resume.current;
                if (r && r.path === path) {
                  resume.current = null;
                  const v = e.currentTarget;
                  v.currentTime = r.time;
                  if (r.playing) v.play().catch(() => {});
                }
                const d = e.currentTarget.duration;
                const { videoWidth: vw, videoHeight: vh } = e.currentTarget;
                if (vw && vh) setAr(vw / vh);
                if (isFinite(d) && d > 0) {
                  setDuration(d);
                  // The player's own length wins over anything set before it loaded.
                  setRange((r) => (r[1] && r[1] <= d + 0.05 ? r : [0, d]));
                }
              }}
              onTimeUpdate={(e) => {
                const v = e.currentTarget;
                // While dragging (or a seek is queued) the playhead follows the mouse, not the decoder.
                if (scrubbing.current || pendingSeek.current != null) return;
                setTime(v.currentTime);
                if (!v.paused && v.currentTime >= range[1]) v.pause();
                syncExtra();
              }}
              onPlay={() => {
                setPlaying(true);
                syncExtra(true);
              }}
              onPause={() => {
                setPlaying(false);
                syncExtra();
              }}
              onSeeked={onSeeked}
              onRateChange={() => syncExtra()}
              onWaiting={() => extraAudio.current.forEach((a) => a.pause())}
              onPlaying={() => syncExtra(true)}
              onError={() => {
                // A one-off load hiccup (e.g. switching clips quickly): try once more first.
                if (retried.current !== path) {
                  retried.current = path;
                  setReload((k) => k + 1);
                } else setPlayErrorFor(path);
              }}
            />
            {(cropping || cropped) && !fullscreen && !playError && (
              <CropBox value={crop ?? WHOLE} onChange={setCrop} ratio={cropRatio} videoAr={ar || 16 / 9} editing={cropping} label={cropping ? cropLabel() : undefined} />
            )}
            {!playError && !cropping && (
              <div
                className={`vid-overlay ${playing ? "playing" : "paused"}`}
                onClick={(e) => e.target === e.currentTarget && toggle()}
                onDoubleClick={(e) => e.target === e.currentTarget && toggleFullscreen()}
              >
                <button
                  className="vid-center"
                  aria-label={playing ? "Pause" : "Play"}
                  onClick={toggle}
                  onDoubleClick={(e) => e.stopPropagation()}
                >
                  {playing ? Icon.pause : Icon.play}
                </button>
                <div className="vid-bar">
                  {fullscreen && (
                    <>
                      <button className="vid-btn" aria-label={playing ? "Pause" : "Play"} onClick={toggle}>
                        {playing ? Icon.pause : Icon.play}
                      </button>
                      <span className="mono vid-time">
                        {fmtTime(time)} / {fmtTime(duration)}
                      </span>
                      <div className="vid-progress" onPointerDown={scrubProgress}>
                        <div style={{ width: duration ? `${(time / duration) * 100}%` : 0 }} />
                      </div>
                      <button className="vid-btn" aria-label={muted ? "Unmute (M)" : "Mute (M)"} title={muted ? "Unmute (M)" : "Mute (M)"} onClick={toggleMute}>
                        <SpeakerIcon level={muted ? 0 : volume} />
                      </button>
                      <Slider
                        className="vid-volume"
                        aria-label="Volume"
                        title="Volume (scroll the wheel over the video, or ↑ ↓)"
                        value={muted ? 0 : Math.round(volume * 100)}
                        onValue={(v) => setVolume(v / 100)}
                        style={{ width: 110 }}
                      />
                    </>
                  )}
                  {!fullscreen && <span style={{ flex: 1 }} />}
                  {!fullscreen && (
                    <button
                      className="vid-btn"
                      aria-label={theater ? "Exit theater mode" : "Theater mode"}
                      title={theater ? "Exit theater mode (T)" : "Theater mode (T)"}
                      onClick={toggleTheater}
                    >
                      {theater ? Icon.theaterOff : Icon.theater}
                    </button>
                  )}
                  <button
                    className="vid-btn"
                    aria-label={fullscreen ? "Exit fullscreen" : "Fullscreen"}
                    title={fullscreen ? "Exit fullscreen (F or Esc)" : "Fullscreen (F)"}
                    onClick={toggleFullscreen}
                  >
                    {fullscreen ? Icon.exitFullscreen : Icon.fullscreen}
                  </button>
                </div>
              </div>
            )}
            {volFlash != null && (
              <div className="vol-flash" aria-live="polite">
                <SpeakerIcon level={volFlash} />
                <span className="mono">{Math.round(volFlash * 100)}%</span>
              </div>
            )}
            {playError && (
              <div className="video-msg">
                <strong style={{ color: "var(--text)" }}>The preview can't play this file</strong>
                <span>Edits still work. To preview it, try again, or make an MP4 or H.264 copy:</span>
                <div className="row">
                  <button
                    className="btn"
                    onClick={() => {
                      setPlayErrorFor(null);
                      setReload((k) => k + 1);
                    }}
                  >
                    Try again
                  </button>
                  <button className="btn" disabled={busy} onClick={() => run("Remuxing…", () => api.remuxMp4(path))}>
                    Remux to MP4
                  </button>
                  <button className="btn" disabled={busy} onClick={() => run("Exporting…", () => api.exportH264(path))}>
                    Export H.264
                  </button>
                </div>
              </div>
            )}
          </div>
          </div>
          <div className="transport">
            <span className="transport-pill">
              <button className="round icon" aria-label="Back 5 seconds" title="Back 5 s (←)" onClick={() => seek(time - 5)}>
                {Icon.back5}
              </button>
              <button className="round play" aria-label={playing ? "Pause" : "Play"} title="Play / pause (Space)" onClick={toggle}>
                {playing ? Icon.pause : Icon.play}
              </button>
              <button className="round icon" aria-label="Forward 5 seconds" title="Forward 5 s (→)" onClick={() => seek(time + 5)}>
                {Icon.fwd5}
              </button>
            </span>
            <span className="mono time-readout">
              {fmtTime(time)} <span className="muted">/ {fmtTime(duration)}</span>
            </span>
            <div className="volume" role="group" aria-label="Volume">
              <button
                className="round"
                aria-label={muted || volume === 0 ? "Unmute" : "Mute"}
                aria-pressed={muted || volume === 0}
                title="Mute (M)"
                onClick={() => (volume === 0 ? setVolume(0.5) : toggleMute())}
              >
                <SpeakerIcon level={muted ? 0 : volume} />
              </button>
              <Slider
                aria-label="Volume"
                title="Volume (↑ / ↓)"
                value={muted ? 0 : Math.round(volume * 100)}
                onValue={(v) => setVolume(v / 100)}
                style={{ width: 96 }}
              />
              <span className="mono" style={{ width: 38, textAlign: "right" }}>
                {muted ? "Muted" : `${Math.round(volume * 100)}%`}
              </span>
            </div>
            <span style={{ flex: 1 }} />
            {cropping ? (
              <span className="transport-pill crop-tools" role="group" aria-label="Crop shape">
                {CROP_PRESETS.map(([name, r, hint]) => (
                  <button key={name} className={`btn pill ${cropRatio === r ? "on" : ""}`} aria-pressed={cropRatio === r} title={hint} onClick={() => pickRatio(r)}>
                    {name}
                  </button>
                ))}
                <button className="round icon" aria-label="Reset crop" title="Keep the whole picture" onClick={() => { setCrop(null); setCropRatio(null); }}>
                  {Icon.reset}
                </button>
                <button className="btn primary crop-done" title="Done (Enter)" onClick={() => setCropping(false)}>
                  Done
                </button>
              </span>
            ) : (
              <button
                className={`btn pill ${cropped ? "on" : ""}`}
                title={cropped ? "Change the crop. Save trim, Export H.264 and Compress use it." : "Crop the picture, for example to 9:16 for TikTok or Shorts (C)"}
                disabled={!!playError}
                onClick={() => {
                  if (fullscreen) toggleFullscreen();
                  video.current?.pause();
                  setCropping(true);
                }}
              >
                <CropIcon size={15} strokeWidth={2} aria-hidden="true" />
                {cropped ? "Cropped" : "Crop"}
              </button>
            )}
            <span className="transport-pill">
              <button className="btn pill" title="Start the trim here (I)" onClick={() => setRange(([, y]) => [Math.min(time, y - 0.1), y])}>
                {Icon.markIn}
                Start
              </button>
              <button className="btn pill" title="End the trim here (O)" onClick={() => setRange(([x]) => [x, Math.max(time, x + 0.1)])}>
                {Icon.markOut}
                End
              </button>
              <button className="round icon" aria-label="Reset trim" title="Use the whole clip" onClick={() => setRange([0, duration])}>
                {Icon.reset}
              </button>
            </span>
          </div>
          <div className="timeline" ref={bar} onPointerDown={drag("seek")}>
            <div className="tl-range" style={{ left: pct(a), width: `calc(${pct(b)} - ${pct(a)})` }}>
              <div className="tl-handle in" onPointerDown={drag("in")} />
              <div className="tl-handle out" onPointerDown={drag("out")} />
            </div>
            <div className="tl-head" style={{ left: pct(time) }} />
          </div>
          <Ruler duration={duration} onPointerDown={drag("seek")} />
          {trackTitles.length > 0 && (
            <AudioLanes
              titles={trackTitles}
              waves={waves}
              edits={audioEdits}
              setEdits={setAudioEdits}
              duration={duration}
              time={time}
              range={range}
              onSeek={seek}
              hidden={tracks?.mix ?? null}
            />
          )}
          <div className="tl-foot">
            <span className="mono">
              Start {fmtTime(a)} · End {fmtTime(b)} · Length {fmtTime(Math.max(0, b - a))}
            </span>
            <span>Space play · I / O set points · ← → 5 s · Shift+← → one frame · ↑ ↓ volume · M mute · C crop · T hide or show tools · F fullscreen</span>
          </div>
          {!theater && <ClipStrip clips={(clips ?? []).slice(0, 30)} current={path} onOpen={onOpen} />}
        </div>

        <aside className="side" ref={sideRef}>
          {narrow && (
            <div className="side-tabs" role="tablist" aria-label="Editing tools">
              {SIDE_TABS.filter(([id]) => id !== "audio" || hasPreviewAudio).map(([id, label]) => (
                <button key={id} role="tab" aria-selected={sideTab === id} className={`tab ${sideTab === id ? "on" : ""}`} onClick={() => setSideTab(id)}>
                  {label}
                </button>
              ))}
            </div>
          )}
          <div className={group("details")}>
          <div className="label">File</div>
          <div className="facts">
            <span>Video</span>
            <span>{info ? `${codecLabel(info.codec)} · ${info.height}p${Math.round(info.fps)}` : "…"}</span>
            <span>Container</span>
            <span>{info ? info.container.split(",")[0].toUpperCase() : "…"}</span>
            <span>Audio</span>
            <span>{info ? (info.audioTracks ? `${info.audioTracks} track(s)` : "None") : "…"}</span>
            <span>Size</span>
            <span>{fmtSize(clips?.find((c) => c.path === path)?.size ?? 0)}</span>
          </div>
          </div>

          {hasPreviewAudio && tracks && (
            <div className={group("audio")}>
              <div className="label">Preview audio</div>
              <div className="stack">
                {/* A track that's the others already mixed isn't listed: it's never heard or saved. */}
                {[{ index: 0, title: tracks.firstTitle, path: "" }, ...tracks.extra].filter((t) => t.index !== tracks.mix).map((t) => {
                  const off = !!trackMuted[t.title] || tv(t.title) === 0;
                  return (
                    <div key={t.index} className="row track-row">
                      <button
                        className="round small-round"
                        aria-label={off ? `Unmute ${t.title}` : `Mute ${t.title}`}
                        aria-pressed={off}
                        onClick={() =>
                          tv(t.title) === 0 ? setTrack(t.title, 1) : setTrackMuted((m) => ({ ...m, [t.title]: !m[t.title] }))
                        }
                      >
                        <SpeakerIcon level={off ? 0 : tv(t.title)} />
                      </button>
                      <span className="track-name" title={t.title}>
                        {t.title.replace(" audio", "")}
                      </span>
                      <Slider
                        aria-label={`${t.title} volume`}
                        value={off ? 0 : Math.round(tv(t.title) * 100)}
                        onValue={(v) => setTrack(t.title, v / 100)}
                        style={{ flex: 1, width: 0 }}
                      />
                      <span className="mono" style={{ width: 34, textAlign: "right", fontSize: 11 }}>
                        {off ? "Off" : `${Math.round(tv(t.title) * 100)}%`}
                      </span>
                    </div>
                  );
                })}
                <span className="muted" style={{ fontSize: 11, lineHeight: 1.5 }}>
                  Only changes what you hear here. To change the saved file, use the track's Volume under the timeline.
                </span>
              </div>
              {tracks.extra.map((t) => (
                <audio
                  key={`${t.path}#${reload}`}
                  ref={(el) => {
                    if (el) extraAudio.current.set(t.index, el);
                    else extraAudio.current.delete(t.index);
                  }}
                  src={mediaSrc(t.path)}
                  preload="auto"
                  onLoadedData={() => syncExtra(true)}
                />
              ))}
            </div>
          )}
          <div className={group("details")}>
          <HealthNotice
            path={path}
            health={health}
            busy={busy}
            onHeal={() => {
              video.current?.pause();
              setHealOpen(true);
            }}
            onRescued={(p) => onRescued?.(p)}
          />
          {healOpen && health && clips?.find((c) => c.path === path) && (
            <HealReview
              clips={clips.filter((c) => c.path === path)}
              health={{ [path]: health }}
              onClose={() => setHealOpen(false)}
              onToast={(text, err) => setResult({ text, err })}
              onHealed={() => {
                onDone();
                setReload((k) => k + 1);
              }}
            />
          )}
          </div>
          <div className={group("share")}>
          <div className="label">Share</div>
          <SharePanel path={path} settings={settings} progress={uploads?.[path]} links={links?.[path]} onShare={(r) => onShare?.(r)} />
          </div>
          <div className={group("save")}>
          <div className="label">This file only</div>
          <div className="stack">
            {!path.toLowerCase().endsWith(".mp4") && (
              <button
                className="btn block"
                disabled={busy}
                title="Same video and sound, just in an .mp4 file instead of .mkv. Instant and lossless, but the video stays AV1/HEVC, which some phones and older apps can't play."
                onClick={() => run("Remuxing…", () => api.remuxMp4(path))}
              >
                Change to .mp4 (instant, lossless)
              </button>
            )}
            <button
              className="btn block"
              disabled={busy}
              title="Re-encodes the video as H.264 and mixes the audio into one track, so it plays on every phone, browser, editor and Discord. Slower and a little bigger, with your audio changes."
              onClick={() => run("Exporting H.264…", () => api.exportH264(path, replace, editsForSave, cropForSave), replace)}
            >
              Export H.264 (plays everywhere)
            </button>
            <div className="row">
              <button
                className="btn block"
                style={{ flex: 1 }}
                disabled={busy || !info?.audioTracks}
                title="Just the sound of your trimmed part as an MP3, with your audio changes"
                onClick={() => {
                  const k = audioOnly === "all" ? null : Number(audioOnly);
                  const label = k == null ? "audio" : (trackTitles[k] ?? `track ${k + 1}`).replace(/ audio$/i, "");
                  run("Exporting audio…", () => api.exportAudio(path, a, b, k, label, editsForSave));
                }}
              >
                Audio only (MP3)
              </button>
              <Select className="select" aria-label="Which audio" value={audioOnly} onChange={(e) => setAudioOnly(e.target.value)}>
                <option value="all">All tracks</option>
                {trackTitles.map((t, k) => (
                  <option key={k} value={String(k)}>
                    {t.replace(/ audio$/i, "")}
                  </option>
                ))}
              </Select>
            </div>
            <div className="row">
              <button
                className="btn block"
                style={{ flex: 1 }}
                disabled={busy}
                onClick={() => run(`Compressing to ${targetMb} MB…`, () => api.compress(path, targetMb, replace, editsForSave, cropForSave), replace)}
              >
                Compress to
              </button>
              <Select
                className="select"
                aria-label="Target size"
                value={targetMb}
                onChange={(e) => setTargetMb(Number(e.target.value))}
              >
                <option value={10}>10 MB</option>
                <option value={25}>25 MB</option>
                <option value={50}>50 MB</option>
                <option value={100}>100 MB</option>
              </Select>
            </div>
            <button className="btn block" onClick={() => api.reveal(path)}>
              Show in Explorer
            </button>
            <DiagnosticsButton clip={path} label="Report a problem with this clip…" />
            <button
              className="btn block"
              style={{ color: "#ffb3b3" }}
              disabled={busy}
              onClick={() => {
                video.current?.pause();
                onDelete([path]);
              }}
            >
              Delete this clip…
            </button>
          </div>
          <label className="row" style={{ fontSize: 13, color: "var(--text-2)", cursor: "pointer" }}>
            <input
              type="checkbox"
              className="switch"
              checked={replace}
              disabled={noReplace}
              onChange={(e) => update({ replaceOriginalOnEdit: e.target.checked })}
            />
            Replace the original with my edit
          </label>
          <span className="muted" style={{ fontSize: 12, lineHeight: 1.5 }}>
            {noReplace
              ? "This clip is on a network folder, which has no Recycle Bin, so edits are always saved as new files next to it."
              : replace && inNas
              ? "Trims, exports and compresses replace this clip on the NAS and keep its date. The old version goes to the NAS app's Trash for 30 days."
              : replace
              ? "Trims, exports and compresses overwrite this file and keep its date. The old version goes to the Recycle Bin."
              : "The original is kept. Edits are saved as new files next to it."}
          </span>
          </div>
          {job && (
            <div className="result job-progress">
              {job.label.replace("…", "")}
              {job.pct != null ? ` ${job.pct}%` : "…"}
              <span className="job-bar"><span style={{ width: `${job.pct ?? 0}%` }} /></span>
            </div>
          )}
          {result && (
            <div className={`result ${result.err ? "err" : ""}`}>
              {result.text}
              {result.path && (
                <div className="row" style={{ marginTop: 8 }}>
                  {!/\.mp3$/i.test(result.path) && (
                    <button className="btn small" onClick={() => onOpen(result.path!)}>
                      Open it
                    </button>
                  )}
                  <button className="btn small" onClick={() => api.reveal(result.path!)}>
                    Show
                  </button>
                </div>
              )}
            </div>
          )}
        </aside>
      </div>
    </div>
  );
}
