import { useEffect, useRef, useState } from "react";
import { fmtDb, fmtTime, TrackEdit, Waveform } from "./api";
import { Slider } from "./Slider";

/** No changes to a track. */
export const NO_EDIT: TrackEdit = { gainDb: 0, mutes: [] };

export function hasAudioEdits(edits: TrackEdit[]) {
  return edits.some((e) => Math.abs(e.gainDb) >= 0.05 || e.mutes.length > 0);
}

/** Is `t` inside one of the muted stretches? */
export function isMutedAt(e: TrackEdit | undefined, t: number) {
  return !!e?.mutes.some(([a, b]) => t >= a && t < b);
}

/** Add a stretch, merging any it overlaps or touches. */
export function addMute(mutes: [number, number][], a: number, b: number): [number, number][] {
  const all = [...mutes, [Math.min(a, b), Math.max(a, b)] as [number, number]].sort((x, y) => x[0] - y[0]);
  const out: [number, number][] = [];
  for (const m of all) {
    const last = out[out.length - 1];
    if (last && m[0] <= last[1] + 0.02) last[1] = Math.max(last[1], m[1]);
    else out.push([m[0], m[1]]);
  }
  return out;
}

/** Peaks drawn as a mirrored bar shape, in the lane's own colour. */
function Wave({ wave, duration, color }: { wave: Waveform | undefined; duration: number; color: string }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    const ro = new ResizeObserver(() => setWidth(c.clientWidth));
    ro.observe(c);
    return () => ro.disconnect();
  }, []);
  useEffect(() => {
    const c = canvas.current;
    if (!c || !width || !wave || !duration) return;
    const dpr = window.devicePixelRatio || 1;
    const h = c.clientHeight;
    c.width = Math.round(width * dpr);
    c.height = Math.round(h * dpr);
    const g = c.getContext("2d");
    if (!g) return;
    g.scale(dpr, dpr);
    g.clearRect(0, 0, width, h);
    // Canvas can't read CSS variables: let the browser resolve the colour on the element.
    g.fillStyle = getComputedStyle(c).color;
    const n = wave.levels.length;
    const perPx = duration / width / wave.step; // buckets per pixel
    const bar = 2;
    for (let x = 0; x < width; x += bar + 1) {
      const i0 = Math.floor(x * perPx);
      const i1 = Math.max(i0 + 1, Math.floor((x + bar) * perPx));
      let peak = 0;
      for (let i = i0; i < i1 && i < n; i++) peak = Math.max(peak, wave.levels[i]);
      if (i0 >= n) break;
      const bh = Math.max(1, (peak / 255) * (h - 2));
      g.fillRect(x, (h - bh) / 2, bar, bh);
    }
  }, [wave, width, duration, color]);
  return <canvas ref={canvas} className="lane-wave" style={{ color }} />;
}

type Props = {
  titles: string[];
  waves: Waveform[] | null;
  edits: TrackEdit[];
  setEdits: (e: TrackEdit[]) => void;
  duration: number;
  time: number;
  range: [number, number];
  onSeek: (t: number) => void;
  /** A track that's the others already mixed (Medal's "All Audio"): left out of saves, so no lane. */
  hidden?: number | null;
};

/**
 * One lane per audio track under the timeline: its waveform, drag across it
 * to mute a stretch, click a muted stretch's × to bring it back, and a volume
 * (dB) control for the saved file.
 */
export function AudioLanes({ titles, waves, edits, setEdits, duration, time, range, onSeek, hidden = null }: Props) {
  const [draft, setDraft] = useState<{ track: number; a: number; b: number } | null>(null);
  if (!duration || titles.length === 0) return null;
  const pct = (t: number) => `${Math.min(100, Math.max(0, (t / duration) * 100))}%`;
  const edit = (i: number) => edits[i] ?? NO_EDIT;
  const change = (i: number, e: TrackEdit) => {
    const next = titles.map((_, k) => edits[k] ?? NO_EDIT);
    next[i] = e;
    setEdits(next);
  };

  const down = (track: number) => (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
    const el = e.currentTarget;
    const r = el.getBoundingClientRect();
    const at = (x: number) => Math.min(duration, Math.max(0, ((x - r.left) / r.width) * duration));
    const x0 = e.clientX;
    const t0 = at(x0);
    let dragged = false;
    el.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      if (Math.abs(ev.clientX - x0) > 4) dragged = true;
      if (dragged) setDraft({ track, a: t0, b: at(ev.clientX) });
    };
    const up = (ev: PointerEvent) => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      setDraft(null);
      if (!dragged) return onSeek(t0); // a click just moves the playhead
      const t1 = at(ev.clientX);
      if (Math.abs(t1 - t0) < 0.05) return;
      change(track, { ...edit(track), mutes: addMute(edit(track).mutes, t0, t1) });
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  };

  return (
    <div className="lanes">
      {titles.map((title, i) => {
        if (i === hidden) return null;
        const e = edit(i);
        const allMuted = e.mutes.length === 1 && e.mutes[0][0] <= 0.01 && e.mutes[0][1] >= duration - 0.01;
        const name = title.replace(/ audio$/i, "");
        return (
          <div key={i} className="lane">
            <div className="lane-head">
              <span className={`lane-dot t${i % 3}`} />
              <span className="lane-name" title={title}>
                {name}
              </span>
              <button
                className={`btn small lane-btn ${allMuted ? "on" : ""}`}
                aria-pressed={allMuted}
                title={allMuted ? `Unmute ${name} in the saved file` : `Mute all of ${name} in the saved file`}
                onClick={() => change(i, { ...e, mutes: allMuted ? [] : [[0, duration]] })}
              >
                {allMuted ? "Muted" : "Mute all"}
              </button>
              {e.mutes.length > 0 && !allMuted && (
                <button className="btn small lane-btn" onClick={() => change(i, { ...e, mutes: [] })} title="Remove every muted stretch on this track">
                  Clear {e.mutes.length} cut{e.mutes.length === 1 ? "" : "s"}
                </button>
              )}
              <span style={{ flex: 1 }} />
              <span className="lane-gain">
                <span className="muted">Volume</span>
                <Slider
                  aria-label={`${name} volume in the saved file`}
                  title="Louder or quieter in the saved file (double-click to reset)"
                  min={-20}
                  max={20}
                  step={0.5}
                  value={e.gainDb}
                  onValue={(v) => change(i, { ...e, gainDb: v })}
                  onDoubleClick={() => change(i, { ...e, gainDb: 0 })}
                  style={{ width: 110 }}
                />
                <span className="mono">{fmtDb(e.gainDb)}</span>
              </span>
            </div>
            <div className="lane-track" onPointerDown={down(i)} title="Drag across the waveform to mute that part">
              {waves ? (
                <Wave wave={waves[i]} duration={duration} color={`var(--lane-${i % 3})`} />
              ) : (
                <span className="lane-loading muted">Reading audio…</span>
              )}
              {/* Outside the trim: dimmed, it won't be in the saved clip. */}
              <div className="lane-out" style={{ left: 0, width: pct(range[0]) }} />
              <div className="lane-out" style={{ left: pct(range[1]), right: 0 }} />
              {e.mutes.map(([a, b], k) => (
                <div key={k} className="lane-mute" style={{ left: pct(a), width: `calc(${pct(b)} - ${pct(a)})` }}>
                  <button
                    className="lane-x"
                    aria-label={`Unmute ${fmtTime(a)} to ${fmtTime(b)}`}
                    title={`Muted ${fmtTime(a)}–${fmtTime(b)}. Click to unmute.`}
                    onClick={() => change(i, { ...e, mutes: e.mutes.filter((_, j) => j !== k) })}
                  >
                    ×
                  </button>
                </div>
              ))}
              {draft?.track === i && (
                <div
                  className="lane-mute draft"
                  style={{ left: pct(Math.min(draft.a, draft.b)), width: `calc(${pct(Math.max(draft.a, draft.b))} - ${pct(Math.min(draft.a, draft.b))})` }}
                />
              )}
              <div className="lane-head-line" style={{ left: pct(time) }} />
            </div>
          </div>
        );
      })}
      <span className="lanes-hint muted">
        Drag across a track to mute that part. Mutes and volume go into the saved trim, export or compress. Boosts above the
        player's full volume are only heard in the saved file.
      </span>
    </div>
  );
}
