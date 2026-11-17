import { useMemo, useState } from "react";
import { ClipFile } from "./api";
import { useNearView, useThumb } from "./hooks";

export type Memory = { label: string; years: number; clips: ClipFile[] };

const DAY = 86_400_000;

/**
 * Clips from this date in earlier years, like Immich's "On this day":
 * "1 year ago today", "2 years ago". A year with nothing on the exact day
 * falls back to that week ("This week, 1 year ago"). Newest year first.
 */
export function memoriesFor(clips: ClipFile[], now = new Date()): Memory[] {
  const out: Memory[] = [];
  const years = new Map<number, ClipFile[]>();
  for (const c of clips) {
    const d = new Date(c.modified);
    const back = now.getFullYear() - d.getFullYear();
    if (back >= 1 && back <= 20) years.set(back, [...(years.get(back) ?? []), c]);
  }
  for (const back of [...years.keys()].sort((a, b) => a - b)) {
    const list = years.get(back)!;
    const sameDay = list.filter((c) => {
      const d = new Date(c.modified);
      return d.getMonth() === now.getMonth() && d.getDate() === now.getDate();
    });
    // The same date that year, ±3 days.
    const then = new Date(now);
    then.setFullYear(now.getFullYear() - back);
    const week = list.filter((c) => Math.abs(c.modified - then.getTime()) <= 3.5 * DAY);
    const ago = `${back} year${back === 1 ? "" : "s"} ago`;
    if (sameDay.length) out.push({ label: `${ago} today`, years: back, clips: sameDay.sort((a, b) => a.modified - b.modified) });
    else if (week.length) out.push({ label: `This week, ${ago}`, years: back, clips: week.sort((a, b) => a.modified - b.modified) });
  }
  return out;
}

function MemoryCard({ m, on, onOpen }: { m: Memory; on: boolean; onOpen: () => void }) {
  const [ref, near] = useNearView<HTMLButtonElement>(".lib-page", "200px");
  const cover = m.clips[0];
  const thumb = useThumb(near ? cover.path : "", cover.modified);
  const games = [...new Set(m.clips.map((c) => c.folder))];
  return (
    <button ref={ref} className={`memory-card ${on ? "on" : ""}`} aria-expanded={on} onClick={onOpen} title={`${m.clips.length} clip${m.clips.length === 1 ? "" : "s"}: ${games.join(", ")}`}>
      {thumb && <img src={thumb} alt="" />}
      <span className="memory-shade" />
      <span className="memory-text">
        <strong>{m.label}</strong>
        <small>
          {new Date(cover.modified).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" })} · {m.clips.length} clip
          {m.clips.length === 1 ? "" : "s"}
        </small>
      </span>
    </button>
  );
}

function MemoryClip({ clip, onOpen }: { clip: ClipFile; onOpen: () => void }) {
  const thumb = useThumb(clip.path, clip.modified);
  return (
    <button className="card memory-clip" onClick={onOpen} title={clip.path}>
      <div className={`thumb ${thumb ? "" : "loading"}`}>{thumb && <img src={thumb} alt="" />}</div>
      <span className="card-title">{clip.name}</span>
      <span className="card-meta">
        {clip.folder} · {new Date(clip.modified).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}
      </span>
    </button>
  );
}

/** The "On this day" strip at the top of the library (nothing when there's nothing). */
export function OnThisDay({ clips, onOpenClip }: { clips: ClipFile[]; onOpenClip: (path: string) => void }) {
  const today = new Date().toDateString();
  const [hidden, setHidden] = useState(() => {
    try {
      return localStorage.getItem("rewind.memoriesHidden") === today;
    } catch {
      return false;
    }
  });
  const memories = useMemo(() => memoriesFor(clips), [clips]);
  const [open, setOpen] = useState<Memory | null>(null);
  if (hidden || memories.length === 0) return null;
  return (
    <section className="memories" aria-label="On this day">
      <div className="memories-head">
        <h3>On this day</h3>
        <button
          className="btn small"
          onClick={() => {
            setHidden(true);
            try {
              localStorage.setItem("rewind.memoriesHidden", today);
            } catch {
              /* only hides it for today */
            }
          }}
          title="Hide until tomorrow"
        >
          Hide for today
        </button>
      </div>
      <div className="memories-row">
        {memories.map((m) => (
          <MemoryCard key={m.label} m={m} on={open?.label === m.label} onOpen={() => setOpen((o) => (o?.label === m.label ? null : m))} />
        ))}
      </div>
      {/* That day's clips, like the gallery: each opens in Studio like any other clip. */}
      {open && (
        <div className="memory-clips">
          {open.clips.map((c) => (
            <MemoryClip key={c.path} clip={c} onOpen={() => onOpenClip(c.path)} />
          ))}
        </div>
      )}
    </section>
  );
}
