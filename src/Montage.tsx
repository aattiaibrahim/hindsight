import { Select } from "./Select";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, ClipFile, errText, fmtDb, fmtTime, Waveform, mediaSrc } from "./api";
import { GameIcon } from "./GameIcon";
import { useThumb } from "./hooks";
import { Slider } from "./Slider";
import { Music } from "lucide-react";

/** One piece of the montage: part of a clip. */
type Piece = { id: number; clip: ClipFile; start: number; end: number; length: number };
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
function Setup({ clips, onCreate }: { clips: ClipFile[]; onCreate: (pieces: Piece[]) => void }) {
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
  const isEdited = (c: ClipFile) => edited.has(c.path) || EDIT_MARK.test(c.name) || /^edits$/i.test(c.folder);
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
    const order = source === "random" ? shuffle(pool) : [...pool].sort((a, b) => a.modified - b.modified);
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
              ["edited", "Edited clips only", "Ones you've trimmed or exported"],
              ["unedited", "Unedited clips only", "Straight from the replay buffer"],
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
  const [render, setRender] = useState<{ progress: number; out?: string; err?: string } | null>(null);
  const [binQuery, setBinQuery] = useState("");
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
      if (playingIdx.current !== at.i || !v.src.endsWith(encodeURI(src).split("/").pop() ?? "")) {
        playingIdx.current = at.i;
        v.src = src;
      }
      v.currentTime = at.local;
      v.volume = Math.min(1, Math.pow(10, clipDb / 20));
      if (play) v.play().catch(() => {});
      const a = audio.current;
      if (a && song) {
        a.currentTime = (songOffset + t) % Math.max(1, song.duration);
        a.volume = Math.min(1, Math.pow(10, songDb / 20));
        if (play) a.play().catch(() => {});
        else a.pause();
      }
    },
    [locate, pieces, clipDb, song, songOffset, songDb],
  );
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
  useEffect(() => show(time, false), []); // eslint-disable-line react-hooks/exhaustive-deps

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

  const addClip = async (c: ClipFile) => {
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
        pieces: pieces.map((p) => ({ path: p.clip.path, start: p.start, end: p.end })),
        music: song ? { path: song.path, offset: songOffset, volumeDb: songDb } : null,
        clipVolumeDb: clipDb,
        height,
        fps,
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
        <div className="me-bin">
          <div className="label">Clips</div>
          <input className="text-input" placeholder="Search…" value={binQuery} onChange={(e) => setBinQuery(e.target.value)} />
          <div className="me-bin-list">
            {bin.map((c) => (
              <button key={c.path} className="me-bin-item" onClick={() => addClip(c)} title="Add to the end of the montage">
                <GameIcon name={c.folder} size={16} />
                <span>{c.name}</span>
                <b>+</b>
              </button>
            ))}
          </div>
        </div>
        <div className="me-monitor">
          <video ref={video} className="me-video" onTimeUpdate={onTime} playsInline />
          {song && <audio ref={audio} src={mediaSrc(song.path)} loop preload="auto" />}
          <div className="row me-transport">
            <button className="btn small" onClick={() => seek(0)} title="To the start">
              ⏮
            </button>
            <button className="btn small primary" onClick={togglePlay}>
              {playing ? "Pause" : "Play"}
            </button>
            <span className="mono">
              {fmtTime(time)} / {fmtTime(length)}
            </span>
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
          <div className="label">Export</div>
          <div className="row">
            <Select className="select" value={height} onChange={(e) => setHeight(Number(e.target.value))} aria-label="Size">
              <option value={720}>720p</option>
              <option value={1080}>1080p</option>
              <option value={1440}>1440p</option>
            </Select>
            <Select className="select" value={fps} onChange={(e) => setFps(Number(e.target.value))} aria-label="Frame rate">
              <option value={30}>30 fps</option>
              <option value={60}>60 fps</option>
            </Select>
          </div>
        </div>
      </div>

      <div className="me-timeline">
        <div className="row me-tl-bar">
          <button className="btn small" onClick={onBack}>
            ← Start over
          </button>
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
          <div style={{ width, position: "relative" }}>
            <div
              className="me-ruler"
              onPointerDown={(e) => {
                const r = e.currentTarget.getBoundingClientRect();
                seek((e.clientX - r.left) / px);
              }}
            >
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
            <div className="me-playhead" style={{ left: 36 + time * px }} />
          </div>
        </div>
      </div>
    </div>
  );
}

/** The montage creator: pick what goes in, then edit it on a timeline. */
// Kept while you pop into the clip editor and come back.
let kept: Piece[] | null = null;

export function MontageView({ clips, onOpen }: { clips: ClipFile[] | null; onOpen: (path: string) => void }) {
  const [pieces, setPiecesState] = useState<Piece[] | null>(kept);
  const setPieces = (p: Piece[] | null) => {
    kept = p;
    setPiecesState(p);
  };
  return (
    <div className="page montage-page">
      <div className="toolbar">
        <h1 style={{ flex: 1 }}>Montage</h1>
      </div>
      {!clips ? null : pieces ? (
        <Editor pieces={pieces} setPieces={setPieces} clips={clips} onBack={() => setPieces(null)} onOpen={onOpen} />
      ) : (
        <Setup clips={clips} onCreate={setPieces} />
      )}
    </div>
  );
}
