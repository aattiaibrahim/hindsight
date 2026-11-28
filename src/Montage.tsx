import { Select } from "./Select";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { api, ClipFile, errText, fmtDb, fmtTime, Waveform, mediaSrc } from "./api";
import { dbToGain, mediaCors, setLevel } from "./boost";
import { useMediaReady } from "./hooks";
import { Pause, Play, Plus, SkipBack, SkipForward } from "lucide-react";
import { GameIcon } from "./GameIcon";
import { useThumb } from "./hooks";
import { Slider } from "./Slider";
import { Music } from "lucide-react";

/** One piece of the montage: part of a clip. */
type Piece = { id: number; clip: ClipFile; start: number; end: number; length: number; volumeDb?: number };
type Transition = "cut" | "fade" | "crossfade" | "ramp";
type RampStrength = "subtle" | "medium" | "strong";
type Source = "random" | "range" | "edited" | "unedited";
type Take = "end" | "middle" | "start";

const LENGTHS: [number, string][] = [
  [30, "30 s"],
  [60, "1 min"],
  [120, "2 min"],
  [180, "3 min"],
  [300, "5 min"],
  [600, "10 min"],
];
const PER_CLIP: [number, string][] = [
  [5, "5 s"],
  [10, "10 s"],
  [15, "15 s"],
  [20, "20 s"],
  [30, "30 s"],
  [0, "Whole clip"],
];

/** Names that say a clip came out of an edit. */
const EDIT_MARK = /\((trim|H\.264|rescued|audio|\d+ MB)( \d+)?\)|\(trim\)/i;

let nextId = 1;

/** Shuffled, then taking turns between games (one from each in turn), so a game
 *  with most of the clips doesn't fill the whole montage. */
function spread(list: ClipFile[]) {
  const key = (c: ClipFile) => c.folder.toLowerCase().replace(/[^a-z0-9]/g, "");
  const groups = new Map<string, ClipFile[]>();
  for (const c of shuffle(list)) {
    const k = key(c);
    if (!groups.has(k)) groups.set(k, []);
    groups.get(k)!.push(c);
  }
  const queues = shuffle([...groups.values()]);
  const out: ClipFile[] = [];
  while (queues.some((q) => q.length)) for (const q of queues) if (q.length) out.push(q.shift()!);
  return out;
}

function shuffle<T>(list: T[]) {
  const a = [...list];
  for (let i = a.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [a[i], a[j]] = [a[j], a[i]];
  }
  return a;
}

/** The part of a clip to use: the last N seconds (where a replay's action usually is), the middle or the start. */
function cut(length: number, seconds: number, take: Take): [number, number] {
  if (!seconds || seconds >= length) return [0, length];
  if (take === "start") return [0, seconds];
  if (take === "middle") return [(length - seconds) / 2, (length + seconds) / 2];
  return [length - seconds, length];
}

const total = (p: Piece[]) => p.reduce((n, x) => n + (x.end - x.start), 0);

/** Step 1: what kind of montage. */
function Setup({ clips, untrimmed, onCreate }: { clips: ClipFile[]; untrimmed: ClipFile[] | null; onCreate: (pieces: Piece[]) => void }) {
  const [length, setLength] = useState(120);
  const [source, setSource] = useState<Source>("random");
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [game, setGame] = useState("All");
  const [perClip, setPerClip] = useState(15);
  const [take, setTake] = useState<Take>("end");
  const [edited, setEdited] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api.editedClips().then((l) => setEdited(new Set(l))).catch(() => {});
  }, []);
  // One chip per game, whatever the folder's spelling ("VALORANT" and "Valorant").
  const key = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, "");
  const games = useMemo(() => {
    const by = new Map<string, string>();
    for (const c of clips) if (c.folder !== "Montages" && !by.has(key(c.folder))) by.set(key(c.folder), c.folder);
    return ["All", ...[...by.values()].sort()];
  }, [clips]);
  // Trimmed or not, by the same rule as To trim (so "Untrimmed" is exactly that list).
  const waiting = useMemo(() => (untrimmed ? new Set(untrimmed.map((c) => c.path)) : null), [untrimmed]);
  const isEdited = (c: ClipFile) => (waiting ? !waiting.has(c.path) : edited.has(c.path) || EDIT_MARK.test(c.name) || /^edits$/i.test(c.folder));
  const pool = clips.filter((c) => {
    if (c.folder === "Montages") return false;
    if (game !== "All" && key(c.folder) !== key(game)) return false;
    if (source === "edited" && !isEdited(c)) return false;
    if (source === "unedited" && isEdited(c)) return false;
    if (source === "range") {
      if (from && c.modified < new Date(from).getTime()) return false;
      if (to && c.modified > new Date(to).getTime() + 86_400_000) return false;
    }
    return true;
  });

  const create = async () => {
    setBusy(true);
    // A time frame plays in order; everything else is a fair mix across your games.
    const order = source === "range" ? [...pool].sort((a, b) => a.modified - b.modified) : spread(pool);
    const out: Piece[] = [];
    for (const clip of order) {
      if (total(out) >= length) break;
      const info = await api.probe(clip.path, String(clip.modified)).catch(() => null);
      if (!info || info.duration < 1) continue;
      const [start, end] = cut(info.duration, perClip, take);
      out.push({ id: nextId++, clip, start, end, length: info.duration });
    }
    // Trim the last piece so the montage lands on the length asked for.
    const over = total(out) - length;
    const last = out[out.length - 1];
    if (last && over > 0 && last.end - last.start - over >= 2) last.start += over;
    setBusy(false);
    onCreate(out);
  };

  return (
    <div className="montage-setup">
      <div className="setup-q">
        <h3>How long should it be?</h3>
        <div className="chips">
          {LENGTHS.map(([s, l]) => (
            <button key={s} className={`chip ${length === s ? "on" : ""}`} onClick={() => setLength(s)}>
              {l}
            </button>
          ))}
        </div>
      </div>
      <div className="setup-q">
        <h3>Which clips?</h3>
        <div className="setup-cards">
          {(
            [
              ["random", "Random clips", "A shuffle of your library"],
              ["range", "From a time frame", "Between two dates, in order"],
              ["edited", "Trimmed clips", "Ones you've trimmed or kept whole"],
              ["unedited", "Untrimmed clips", "The ones waiting in To trim"],
            ] as [Source, string, string][]
          ).map(([v, t, d]) => (
            <button key={v} className={`setup-card ${source === v ? "on" : ""}`} onClick={() => setSource(v)}>
              <strong>{t}</strong>
              <small>{d}</small>
            </button>
          ))}
        </div>
        {source === "range" && (
          <div className="row" style={{ gap: 8 }}>
            <input type="date" className="text-input date" value={from} onChange={(e) => setFrom(e.target.value)} aria-label="From" />
            <span className="muted">to</span>
            <input type="date" className="text-input date" value={to} onChange={(e) => setTo(e.target.value)} aria-label="To" />
          </div>
        )}
      </div>
      <div className="setup-q">
        <h3>Game</h3>
        <div className="chips">
          {games.map((g) => (
            <button key={g} className={`chip game-chip ${game === g ? "on" : ""}`} onClick={() => setGame(g)}>
              {g !== "All" && <GameIcon name={g} size={16} />}
              {g}
            </button>
          ))}
        </div>
      </div>
      <div className="setup-q">
        <h3>How much of each clip?</h3>
        <div className="chips">
          {PER_CLIP.map(([s, l]) => (
            <button key={s} className={`chip ${perClip === s ? "on" : ""}`} onClick={() => setPerClip(s)}>
              {l}
            </button>
          ))}
        </div>
        {perClip > 0 && (
          <div className="seg" role="group" aria-label="Which part">
            {(
              [
                ["end", "The end (where replays put the action)"],
                ["middle", "The middle"],
                ["start", "The start"],
              ] as [Take, string][]
            ).map(([v, l]) => (
              <button key={v} className={take === v ? "on" : ""} onClick={() => setTake(v)}>
                {l}
              </button>
            ))}
          </div>
        )}
      </div>
      <div className="row" style={{ justifyContent: "flex-end", gap: 12 }}>
        <span className="muted">
          {pool.length} clip{pool.length === 1 ? "" : "s"} to choose from. You can change everything afterwards.
        </span>
        <button className="btn primary" disabled={busy || pool.length === 0} onClick={create}>
          {busy ? "Picking clips…" : "Create montage"}
        </button>
      </div>
    </div>
  );
}

/** A clip block on the video track. */
function Block({
  p,
  px,
  selected,
  onSelect,
  onTrim,
  onDragStart,
}: {
  p: Piece;
  px: number;
  selected: boolean;
  onSelect: () => void;
  onTrim: (edge: "start" | "end", e: React.PointerEvent) => void;
  onDragStart: (e: React.PointerEvent) => void;
}) {
  const thumb = useThumb(p.clip.path, p.clip.modified);
  const w = Math.max(18, (p.end - p.start) * px);
  return (
    <div
      className={`tl-block ${selected ? "on" : ""}`}
      style={{ width: w, backgroundImage: thumb ? `url("${thumb}")` : undefined }}
      onPointerDown={(e) => {
        if ((e.target as HTMLElement).classList.contains("tl-handle")) return;
        onSelect();
        onDragStart(e);
      }}
      title={`${p.clip.name}: ${fmtTime(p.start)}–${fmtTime(p.end)}`}
    >
      <span className="tl-handle left" onPointerDown={(e) => onTrim("start", e)} />
      <span className="tl-block-name">
        <GameIcon name={p.clip.folder} size={13} />
        {w > 70 && p.clip.name}
      </span>
      <span className="tl-handle right" onPointerDown={(e) => onTrim("end", e)} />
    </div>
  );
}

function SongWave({ wave, width }: { wave: Waveform | null; width: number }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const c = ref.current;
    if (!c || !wave || !width) return;
    const dpr = window.devicePixelRatio || 1;
    const h = c.clientHeight;
    c.width = Math.round(width * dpr);
    c.height = Math.round(h * dpr);
    const g = c.getContext("2d")!;
    g.scale(dpr, dpr);
    g.fillStyle = getComputedStyle(c).color;
    const perPx = wave.levels.length / width;
    for (let x = 0; x < width; x += 3) {
      let peak = 0;
      for (let i = Math.floor(x * perPx); i < Math.floor((x + 2) * perPx) && i < wave.levels.length; i++) peak = Math.max(peak, wave.levels[i]);
      const bh = Math.max(1, (peak / 255) * (h - 4));
      g.fillRect(x, (h - bh) / 2, 2, bh);
    }
  }, [wave, width]);
  return <canvas ref={ref} className="song-wave" style={{ width }} />;
}

/** Step 2: the editor. Preview, a video track, a music track, export. */
function Editor({
  pieces,
  setPieces,
  clips,
  onBack,
  onOpen,
}: {
  pieces: Piece[];
  setPieces: (p: Piece[]) => void;
  clips: ClipFile[];
  onBack: () => void;
  onOpen: (path: string) => void;
}) {
  const [sel, setSel] = useState<number | null>(pieces[0]?.id ?? null);
  const [px, setPx] = useState(12); // pixels per second
  const [time, setTime] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [song, setSong] = useState<{ path: string; name: string; duration: number; wave: Waveform | null } | null>(null);
  const [songDb, setSongDb] = useState(-6);
  const [songOffset, setSongOffset] = useState(0);
  const [clipDb, setClipDb] = useState(0);
  const [height, setHeight] = useState(1080);
  const [fps, setFps] = useState(60);
  const [transition, setTransition] = useState<Transition>("cut");
  const [transitionSecs, setTransitionSecs] = useState(0.5);
  const [rampStrength, setRampStrength] = useState<RampStrength>("medium");
  const [render, setRender] = useState<{ progress: number; out?: string; err?: string } | null>(null);
  const [binQuery, setBinQuery] = useState("");
  const [picking, setPicking] = useState(false);
  const mediaReady = useMediaReady();
  const video = useRef<HTMLVideoElement>(null);
  const audio = useRef<HTMLAudioElement>(null);
  const playingIdx = useRef(-1);
  const length = total(pieces);

  // Where the playhead is: which piece, and how far into the clip.
  const locate = useCallback(
    (t: number) => {
      let acc = 0;
      for (let i = 0; i < pieces.length; i++) {
        const d = pieces[i].end - pieces[i].start;
        if (t < acc + d || i === pieces.length - 1) return { i, local: pieces[i].start + Math.min(d, Math.max(0, t - acc)), base: acc };
        acc += d;
      }
      return null;
    },
    [pieces],
  );
  const show = useCallback(
    (t: number, play: boolean) => {
      const v = video.current;
      const at = locate(t);
      if (!v || !at) return;
      const p = pieces[at.i];
      const src = mediaSrc(p.clip.path);
      if (!src) return; // the player's address isn't known yet
      if (playingIdx.current !== at.i || !v.src.endsWith(encodeURI(src).split("/").pop() ?? "")) {
        playingIdx.current = at.i;
        const cors = mediaCors(src);
        if (cors && v.crossOrigin !== cors) v.crossOrigin = cors;
        v.src = src;
      }
      v.currentTime = at.local;
      setLevel(v, gainOf(p), false);
      if (play) v.play().catch(() => {});
      const a = audio.current;
      if (a && song) {
        a.currentTime = (songOffset + t) % Math.max(1, song.duration);
        setLevel(a, dbToGain(songDb), false);
        if (play) a.play().catch(() => {});
        else a.pause();
      }
    },
    [locate, pieces, clipDb, song, songOffset, songDb],
  );
  // A clip's volume in the preview: the overall clips volume plus its own (above 100%
  // too: boost.ts plays it louder, as the saved montage will be).
  function gainOf(p: Piece) {
    if (clipDb <= -60 || (p.volumeDb ?? 0) <= -60) return 0;
    return dbToGain(clipDb + (p.volumeDb ?? 0));
  }
  const seek = (t: number) => {
    const c = Math.max(0, Math.min(length, t));
    setTime(c);
    show(c, playing);
  };
  const togglePlay = () => {
    if (playing) {
      video.current?.pause();
      audio.current?.pause();
      setPlaying(false);
    } else {
      setPlaying(true);
      show(time >= length - 0.05 ? 0 : time, true);
    }
  };
  // Follow the video while it plays, and step to the next piece at each piece's end.
  const onTime = () => {
    const v = video.current;
    const at = playingIdx.current;
    if (!v || at < 0 || !pieces[at]) return;
    const p = pieces[at];
    const base = pieces.slice(0, at).reduce((n, x) => n + (x.end - x.start), 0);
    setTime(base + Math.max(0, v.currentTime - p.start));
    if (v.currentTime >= p.end - 0.03 && playing) {
      if (at + 1 < pieces.length) show(base + (p.end - p.start) + 0.001, true);
      else {
        v.pause();
        audio.current?.pause();
        setPlaying(false);
      }
    }
  };
  // Show the first frame as soon as the player can load clips (it may not be ready
  // when the editor opens), and again when the montage changes under the playhead.
  useEffect(() => {
    if (mediaReady && !playing) show(Math.min(time, Math.max(0, length - 0.05)), false);
  }, [mediaReady, pieces]); // eslint-disable-line react-hooks/exhaustive-deps
  // Volumes apply straight away while it plays (they used to wait for the next play).
  useEffect(() => {
    const v = video.current;
    const at = locate(time);
    if (v && at) setLevel(v, gainOf(pieces[at.i]), false);
  }, [clipDb, pieces]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (audio.current) setLevel(audio.current, dbToGain(songDb), false);
  }, [songDb]);
  // The start of each piece on the montage's clock, for jumping clip to clip.
  const starts = useMemo(() => {
    let acc = 0;
    return pieces.map((p) => {
      const s = acc;
      acc += p.end - p.start;
      return s;
    });
  }, [pieces]);
  const jump = (dir: -1 | 1) => {
    const cur = locate(time)?.i ?? 0;
    // Back: to this clip's start, or the one before if already there.
    const i = dir < 0 ? (time - (starts[cur] ?? 0) > 0.5 ? cur : cur - 1) : cur + 1;
    if (i < 0) return seek(0);
    if (i >= pieces.length) return seek(length);
    seek(starts[i] + 0.001);
  };
  // Space plays and pauses (not while typing).
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement;
      if (e.code !== "Space" || /INPUT|TEXTAREA|SELECT/.test(t.tagName) || t.isContentEditable) return;
      e.preventDefault();
      togglePlay();
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  });

  useEffect(() => {
    const un = listen<number>("montage-progress", (e) => setRender((r) => (r && !r.out && !r.err ? { progress: e.payload } : r)));
    return () => {
      un.then((f) => f());
    };
  }, []);

  const update = (id: number, f: (p: Piece) => Piece) => setPieces(pieces.map((p) => (p.id === id ? f(p) : p)));
  const selected = pieces.find((p) => p.id === sel) ?? null;

  // Trim a block by dragging its edge.
  const trim = (p: Piece, edge: "start" | "end") => (e: React.PointerEvent) => {
    e.stopPropagation();
    setSel(p.id);
    const x0 = e.clientX;
    const s0 = p.start;
    const e0 = p.end;
    const move = (ev: PointerEvent) => {
      const d = (ev.clientX - x0) / px;
      update(p.id, (q) =>
        edge === "start" ? { ...q, start: Math.max(0, Math.min(e0 - 0.5, s0 + d)) } : { ...q, end: Math.min(q.length, Math.max(s0 + 0.5, e0 + d)) },
      );
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  // Drag a block sideways to move it.
  const track = useRef<HTMLDivElement>(null);
  const drag = (p: Piece) => (e: React.PointerEvent) => {
    const x0 = e.clientX;
    let moved = false;
    let order = pieces;
    const move = (ev: PointerEvent) => {
      if (!moved && Math.abs(ev.clientX - x0) < 6) return;
      moved = true;
      const blocks = Array.from(track.current?.querySelectorAll<HTMLElement>(".tl-block") ?? []);
      const target = blocks.findIndex((b) => {
        const r = b.getBoundingClientRect();
        return ev.clientX < r.left + r.width / 2;
      });
      const from = order.findIndex((q) => q.id === p.id);
      const to = target < 0 ? order.length - 1 : target > from ? target - 1 : target;
      if (to !== from) {
        const next = [...order];
        next.splice(to, 0, next.splice(from, 1)[0]);
        order = next;
        setPieces(next);
      }
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  // Drag the playhead (or along the ruler) to move through the montage.
  const area = useRef<HTMLDivElement>(null);
  const scrub = (e: React.PointerEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const at = (x: number) => {
      const r = area.current?.getBoundingClientRect();
      return r ? (x - r.left - 36) / px : 0;
    };
    seek(at(e.clientX));
    // At most every 40 ms: each seek reloads a frame from the clip.
    let last = 0;
    let pending = 0;
    const move = (ev: PointerEvent) => {
      clearTimeout(pending);
      const wait = 40 - (performance.now() - last);
      const go = () => {
        last = performance.now();
        seek(at(ev.clientX));
      };
      if (wait <= 0) go();
      else pending = window.setTimeout(go, wait);
    };
    const up = () => {
      clearTimeout(pending);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const addClip = async (c: ClipFile) => {
    setPicking(false);
    const info = await api.probe(c.path, String(c.modified)).catch(() => null);
    if (!info) return;
    const [start, end] = cut(info.duration, 15, "end");
    const p = { id: nextId++, clip: c, start, end, length: info.duration };
    setPieces([...pieces, p]);
    setSel(p.id);
  };

  const chooseSong = async () => {
    const f = await open({ title: "Choose a song", filters: [{ name: "Music", extensions: ["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus"] }] });
    if (typeof f !== "string") return;
    const info = await api.probe(f).catch(() => null);
    setSong({ path: f, name: f.split("\\").pop() ?? f, duration: info?.duration ?? 0, wave: null });
    api.clipWaveforms(f).then((w) => setSong((s) => (s && s.path === f ? { ...s, wave: w[0] ?? null } : s))).catch(() => {});
  };

  const exportIt = async () => {
    video.current?.pause();
    audio.current?.pause();
    setPlaying(false);
    setRender({ progress: 0 });
    try {
      const out = await api.renderMontage({
        pieces: pieces.map((p) => ({ path: p.clip.path, start: p.start, end: p.end, volumeDb: p.volumeDb ?? 0 })),
        music: song ? { path: song.path, offset: songOffset, volumeDb: songDb } : null,
        clipVolumeDb: clipDb,
        height,
        fps,
        transition,
        transitionSecs,
        ramp: rampStrength,
      });
      setRender({ progress: 1, out });
    } catch (e) {
      setRender({ progress: 0, err: errText(e) });
    }
  };

  const bin = clips.filter((c) => c.folder !== "Montages" && (!binQuery || `${c.name} ${c.folder}`.toLowerCase().includes(binQuery.toLowerCase()))).slice(0, 80);
  const width = Math.max(600, length * px + 40);
  const ticks: number[] = [];
  const step = px >= 30 ? 5 : px >= 12 ? 10 : px >= 5 ? 30 : 60;
  for (let t = 0; t <= length + step; t += step) ticks.push(t);

  return (
    <div className="montage-editor">
      <div className="me-top">
        <div className="me-monitor">
          <div className="me-screen" onClick={togglePlay}>
            <video ref={video} className="me-video" onTimeUpdate={onTime} playsInline preload="auto" />
          </div>
          {song && <audio ref={audio} crossOrigin={mediaCors(mediaSrc(song.path))} src={mediaSrc(song.path)} loop preload="auto" />}
          <div className="me-transport">
            <button className="round" onClick={() => jump(-1)} aria-label="Previous clip" title="Previous clip">
              <SkipBack size={16} aria-hidden="true" />
            </button>
            <button className="round me-play" onClick={togglePlay} aria-label={playing ? "Pause (Space)" : "Play (Space)"} title={playing ? "Pause (Space)" : "Play (Space)"}>
              {playing ? <Pause size={18} aria-hidden="true" /> : <Play size={18} aria-hidden="true" />}
            </button>
            <button className="round" onClick={() => jump(1)} aria-label="Next clip" title="Next clip">
              <SkipForward size={16} aria-hidden="true" />
            </button>
            <span className="mono me-time">
              {fmtTime(time)} / {fmtTime(length)}
            </span>
            <span className="muted me-now">{pieces[locate(time)?.i ?? -1]?.clip.name ?? ""}</span>
          </div>
        </div>
        <div className="me-inspector">
          {selected ? (
            <>
              <div className="label">Selected clip</div>
              <strong className="me-name">{selected.clip.name}</strong>
              <small className="muted">
                {selected.clip.folder} · uses {fmtTime(selected.end - selected.start)} of {fmtTime(selected.length)}
              </small>
              <label className="me-field">
                In
                <Slider value={selected.start} min={0} max={selected.length} step={0.1} onValue={(v) => update(selected.id, (q) => ({ ...q, start: Math.min(v, q.end - 0.5) }))} />
                <span className="mono">{fmtTime(selected.start)}</span>
              </label>
              <label className="me-field">
                Out
                <Slider value={selected.end} min={0} max={selected.length} step={0.1} onValue={(v) => update(selected.id, (q) => ({ ...q, end: Math.max(v, q.start + 0.5) }))} />
                <span className="mono">{fmtTime(selected.end)}</span>
              </label>
              <label className="me-field">
                Volume
                <Slider
                  value={selected.volumeDb ?? 0}
                  min={-60}
                  max={10}
                  step={1}
                  onValue={(v) => update(selected.id, (q) => ({ ...q, volumeDb: v }))}
                  onDoubleClick={() => update(selected.id, (q) => ({ ...q, volumeDb: 0 }))}
                  aria-label="This clip's volume"
                />
                <span className="mono">{(selected.volumeDb ?? 0) <= -60 ? "Off" : fmtDb(selected.volumeDb ?? 0)}</span>
              </label>
              <div className="row">
                <button className="btn small" onClick={() => onOpen(selected.clip.path)}>
                  Open in editor
                </button>
                <button
                  className="btn small danger"
                  onClick={() => {
                    setPieces(pieces.filter((p) => p.id !== selected.id));
                    setSel(null);
                  }}
                >
                  Remove
                </button>
              </div>
            </>
          ) : (
            <small className="muted">Click a clip on the timeline to trim it, or drag it to move it.</small>
          )}
          <div className="label">Sound</div>
          <label className="me-field">
            Clips
            <Slider value={clipDb} min={-60} max={10} step={1} onValue={setClipDb} onDoubleClick={() => setClipDb(0)} />
            <span className="mono">{clipDb <= -60 ? "Off" : fmtDb(clipDb)}</span>
          </label>
          {song && (
            <>
              <label className="me-field">
                Song
                <Slider value={songDb} min={-40} max={10} step={1} onValue={setSongDb} onDoubleClick={() => setSongDb(-6)} />
                <span className="mono">{fmtDb(songDb)}</span>
              </label>
              <label className="me-field">
                Starts at
                <Slider value={songOffset} min={0} max={Math.max(1, song.duration - 1)} step={0.5} onValue={setSongOffset} />
                <span className="mono">{fmtTime(songOffset)}</span>
              </label>
            </>
          )}
          <div className="label">Between clips</div>
          <div className="row">
            <Select className="select" value={transition} onChange={(e) => setTransition(e.target.value as Transition)} aria-label="Transition">
              <option value="cut">Cut</option>
              <option value="fade">Fade through black</option>
              <option value="crossfade">Crossfade</option>
              <option value="ramp">Speed ramp</option>
            </Select>
            {transition === "ramp" && (
              <Select className="select" value={rampStrength} onChange={(e) => setRampStrength(e.target.value as RampStrength)} aria-label="Speed ramp strength">
                <option value="subtle">Subtle</option>
                <option value="medium">Medium</option>
                <option value="strong">Strong</option>
              </Select>
            )}
            {transition !== "cut" && transition !== "ramp" && (
              <Select className="select" value={transitionSecs} onChange={(e) => setTransitionSecs(Number(e.target.value))} aria-label="Transition length">
                <option value={0.3}>0.3 s</option>
                <option value={0.5}>0.5 s</option>
                <option value={1}>1 s</option>
              </Select>
            )}
          </div>
          {transition !== "cut" && (
            <small className="muted">
              Shows in the exported video
              {transition === "crossfade"
                ? "; each join overlaps, so it's a little shorter"
                : transition === "ramp"
                  ? ": each clip speeds up into the join and the next slows back down from it (sound too), so it's a little shorter"
                  : ""}
              .
            </small>
          )}
          <div className="label">Export</div>
          <div className="row">
            <Select className="select" value={height} onChange={(e) => setHeight(Number(e.target.value))} aria-label="Size">
              <option value={720}>720p</option>
              <option value={1080}>1080p</option>
              <option value={1440}>1440p</option>
              <option value={2160}>4K (2160p)</option>
            </Select>
            <Select className="select" value={fps} onChange={(e) => setFps(Number(e.target.value))} aria-label="Frame rate">
              <option value={30}>30 fps</option>
              <option value={60}>60 fps</option>
              <option value={120}>120 fps</option>
            </Select>
          </div>
        </div>
      </div>

      <div className="me-timeline">
        <div className="row me-tl-bar">
          <button className="btn small" onClick={onBack} title="Leave this montage (you can save it as a draft)">
            New montage…
          </button>
          <div className="me-pick-wrap">
            <button className="btn small" onClick={() => setPicking((v) => !v)} aria-expanded={picking}>
              <Plus size={14} aria-hidden="true" /> Add clips
            </button>
            {picking && (
              <div className="me-pick" role="dialog" aria-label="Add clips">
                <input className="text-input" autoFocus placeholder="Search clips…" value={binQuery} onChange={(e) => setBinQuery(e.target.value)} />
                <div className="me-bin-list">
                  {bin.map((c) => (
                    <button key={c.path} className="me-bin-item" onClick={() => addClip(c)} title="Add to the end of the montage">
                      <GameIcon name={c.folder} size={16} />
                      <span>{c.name}</span>
                      <b>+</b>
                    </button>
                  ))}
                  {!bin.length && <small className="muted">No clips match.</small>}
                </div>
              </div>
            )}
          </div>
          <span className="muted">
            {pieces.length} clips · {fmtTime(length)}
          </span>
          <span style={{ flex: 1 }} />
          <span className="muted">Zoom</span>
          <Slider value={px} min={2} max={60} step={1} onValue={setPx} style={{ width: 120 }} aria-label="Zoom" />
          {render && !render.out && !render.err ? (
            <>
              <span className="mono">{Math.round(render.progress * 100)}%</span>
              <button className="btn small" onClick={() => api.cancelMontage()}>
                Cancel
              </button>
            </>
          ) : (
            <button className="btn primary" disabled={!pieces.length} onClick={exportIt}>
              Export montage
            </button>
          )}
        </div>
        {render?.out && (
          <div className="result">
            Saved {render.out.split("\\").pop()} in your Montages folder.
            <div className="row" style={{ marginTop: 8 }}>
              <button className="btn small" onClick={() => onOpen(render.out!)}>
                Open it
              </button>
              <button className="btn small" onClick={() => api.reveal(render.out!)}>
                Show
              </button>
            </div>
          </div>
        )}
        {render?.err && <div className="result err">{render.err}</div>}
        {render && !render.out && !render.err && (
          <div className="meter">
            <div style={{ width: `${render.progress * 100}%` }} />
          </div>
        )}
        <div className="me-scroll">
          <div style={{ width, position: "relative" }} ref={area}>
            <div className="me-ruler" onPointerDown={scrub}>
              {ticks.map((t) => (
                <span key={t} style={{ left: t * px }}>
                  {fmtTime(t).replace(/\.\d+$/, "")}
                </span>
              ))}
            </div>
            <div className="me-track">
              <span className="me-track-label">V1</span>
              <div className="me-lane" ref={track}>
                {pieces.map((p) => (
                  <Block
                    key={p.id}
                    p={p}
                    px={px}
                    selected={p.id === sel}
                    onSelect={() => setSel(p.id)}
                    onTrim={(edge, e) => trim(p, edge)(e)}
                    onDragStart={drag(p)}
                  />
                ))}
              </div>
            </div>
            <div className="me-track">
              <span className="me-track-label">A1</span>
              <div className="me-lane">
                {song ? (
                  <div className="song-block" style={{ width: Math.max(40, length * px) }} title={song.name}>
                    <SongWave wave={song.wave} width={Math.max(40, length * px)} />
                    <span className="song-name with-icon"><Music size={12} aria-hidden="true" />{song.name}</span>
                    <button className="song-x" onClick={() => setSong(null)} aria-label="Remove the song">
                      ×
                    </button>
                  </div>
                ) : (
                  <button className="btn small me-add-song" onClick={chooseSong}>
                    + Add a song
                  </button>
                )}
              </div>
            </div>
            <div className="me-playhead" style={{ left: 36 + time * px }} onPointerDown={scrub} title="Drag to move through the montage" />
          </div>
        </div>
      </div>
    </div>
  );
}

/** The montage creator: pick what goes in, then edit it on a timeline. */
// Kept while you pop into the clip editor and come back.
let kept: Piece[] | null = null;

// A montage saved to pick up later (Back → Save draft): the clips and their in and out
// points, kept on this PC.
type Draft = { savedAt: number; pieces: { path: string; start: number; end: number; length: number; volumeDb?: number }[] };
const DRAFT_KEY = "rewind.montageDraft";
function readDraft(): Draft | null {
  try {
    const d = JSON.parse(localStorage.getItem(DRAFT_KEY) ?? "null") as Draft | null;
    return d && Array.isArray(d.pieces) && d.pieces.length ? d : null;
  } catch {
    return null;
  }
}
function writeDraft(d: Draft | null) {
  try {
    if (d) localStorage.setItem(DRAFT_KEY, JSON.stringify(d));
    else localStorage.removeItem(DRAFT_KEY);
  } catch {
    /* only kept for this session then */
  }
}

export function MontageView({ clips, untrimmed, onOpen }: { clips: ClipFile[] | null; untrimmed: ClipFile[] | null; onOpen: (path: string) => void }) {
  const [pieces, setPiecesState] = useState<Piece[] | null>(kept);
  const [leaving, setLeaving] = useState(false);
  const [draft, setDraft] = useState<Draft | null>(readDraft);
  const setPieces = (p: Piece[] | null) => {
    kept = p;
    setPiecesState(p);
  };
  const saveDraft = () => {
    if (!pieces) return;
    const d: Draft = { savedAt: Date.now(), pieces: pieces.map((p) => ({ path: p.clip.path, start: p.start, end: p.end, length: p.length, volumeDb: p.volumeDb })) };
    writeDraft(d);
    setDraft(d);
  };
  // The draft's clips, as they are now (a clip deleted since is left out).
  const resume = () => {
    if (!draft || !clips) return;
    const by = new Map(clips.map((c) => [c.path, c]));
    const back = draft.pieces.flatMap((p) => {
      const clip = by.get(p.path);
      return clip ? [{ id: nextId++, clip, start: p.start, end: p.end, length: p.length, volumeDb: p.volumeDb }] : [];
    });
    if (back.length) setPieces(back);
  };
  const dLen = draft ? draft.pieces.reduce((n, p) => n + (p.end - p.start), 0) : 0;
  return (
    <div className="page montage-page">
      <div className="toolbar">
        {pieces && (
          <button className="btn back" onClick={() => setLeaving(true)} title="Back to the montage setup">
            ← Back
          </button>
        )}
        <h1 style={{ flex: 1 }}>Montage</h1>
      </div>
      {!clips ? null : pieces ? (
        <Editor pieces={pieces} setPieces={setPieces} clips={clips} onBack={() => setLeaving(true)} onOpen={onOpen} />
      ) : (
        <>
          {draft && (
            <div className="me-draft">
              <div>
                <strong>Your draft</strong>
                <small className="muted">
                  {draft.pieces.length} clips · {fmtTime(dLen).replace(/\.\d+$/, "")} · saved {new Date(draft.savedAt).toLocaleString(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" })}
                </small>
              </div>
              <button className="btn small" onClick={() => {
                writeDraft(null);
                setDraft(null);
              }}>
                Delete draft
              </button>
              <button className="btn small primary" onClick={resume}>
                Continue editing
              </button>
            </div>
          )}
          <Setup clips={clips} untrimmed={untrimmed} onCreate={setPieces} />
        </>
      )}
      {leaving && (
        <LeaveDialog
          onDraft={() => {
            saveDraft();
            setLeaving(false);
            setPieces(null);
          }}
          onDiscard={() => {
            setLeaving(false);
            setPieces(null);
          }}
          onStay={() => setLeaving(false)}
        />
      )}
    </div>
  );
}

/** Leaving the editor: keep it as a draft, throw it away, or stay. */
function LeaveDialog({ onDraft, onDiscard, onStay }: { onDraft: () => void; onDiscard: () => void; onStay: () => void }) {
  useEffect(() => {
    const key = (e: KeyboardEvent) => e.key === "Escape" && onStay();
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [onStay]);
  return createPortal(
    <div className="modal-backdrop" onClick={onStay}>
      <div className="modal" role="alertdialog" aria-modal="true" aria-labelledby="leave-title" onClick={(e) => e.stopPropagation()}>
        <h2 id="leave-title">Leave this montage?</h2>
        <p className="muted">Save it as a draft to pick up later from the montage page, or discard it. Exported montages aren't affected.</p>
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button className="btn" onClick={onStay} autoFocus>
            Keep editing
          </button>
          <button className="btn danger" onClick={onDiscard}>
            Discard
          </button>
          <button className="btn primary" onClick={onDraft}>
            Save draft
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
