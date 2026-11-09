import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { api, ClipFile, errText, fmtTime, Health, mediaSrc } from "./api";
import { useThumb } from "./hooks";
import { ProgressBar, useEditProgress } from "./Progress";
import { Icon } from "./icons";

const t = (s: number) => fmtTime(s).replace(/\.\d+$/, "");

/** What healing a clip keeps, in words: "Keeps 1:10–3:00 (1:50 of 3:00)". */
export function healText(h: Health) {
  if (!h.best) return "Nothing undamaged is long enough to keep.";
  const [a, b] = h.best;
  return `Keeps ${t(a)}–${t(b)} (${t(b - a)} of ${t(h.duration)}). The damaged part is cut off.`;
}

/** Damaged stretches in red and the part healing keeps in green, along the clip. */
function HealBar({ h }: { h: Health }) {
  const pct = (s: number) => `${Math.min(100, Math.max(0, (s / (h.duration || 1)) * 100))}%`;
  return (
    <div className="heal-bar" aria-hidden="true">
      {h.best && <span className="keep" style={{ left: pct(h.best[0]), width: `calc(${pct(h.best[1])} - ${pct(h.best[0])})` }} />}
      {h.damaged.map(([a, b], k) => (
        <span key={k} className="bad" style={{ left: pct(a), width: `calc(${pct(Math.min(b, h.duration))} - ${pct(a)})` }} />
      ))}
    </div>
  );
}

/**
 * Go through damaged clips one at a time: the part that would be kept plays,
 * "Heal" swaps the clip for that part (original to the Recycle Bin), "Skip"
 * leaves it. ← / → move between clips. There's deliberately no "heal all":
 * every clip has to be looked at, and Heal only unlocks once the kept part
 * has played for a few seconds, so nobody loses a clip they didn't check.
 */
export function HealReview({
  clips,
  health,
  onClose,
  onToast,
  onHealed,
}: {
  clips: ClipFile[];
  health: Record<string, Health>;
  onClose: () => void;
  onToast: (text: string, err?: boolean) => void;
  /** After each clip is healed (its path stays the same) */
  onHealed?: (path: string) => void;
}) {
  // A snapshot: the library list shrinks as clips are healed, the review keeps its place.
  const [list] = useState(() => clips.filter((c) => health[c.path]));
  const [healed, setHealed] = useState<Set<string>>(new Set());
  const [i, setI] = useState(0);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [repaired, setRepaired] = useState<Record<string, string>>({});
  const [repairing, setRepairing] = useState(false);
  // Clips whose kept part has been watched long enough to unlock Heal.
  const [seen, setSeen] = useState<Set<string>>(new Set());
  const n = list.length;
  const clip = list[Math.min(i, n - 1)];
  // Repairing and healing re-encode the clip: show how far they've got.
  const repairPct = useEditProgress(clip?.path, repairing);
  const healPct = useEditProgress(clip?.path, busy);
  const h = clip ? health[clip.path] : undefined;
  const go = useCallback(
    (d: number) => {
      setErr(null);
      setI((x) => Math.min(n - 1, Math.max(0, x + d)));
    },
    [n],
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
      else if (e.key === "ArrowLeft") go(-1);
      else if (e.key === "ArrowRight") go(1);
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [go, onClose]);

  const start = useRef<number | null>(null);
  if (!clip || !h) return null;
  const done = healed.has(clip.path);
  // Network folders have no Recycle Bin: healing there would lose the original for good.
  const remote = clip.location === "nas" || clip.path.startsWith("\\\\");
  const left = list.filter((c) => !healed.has(c.path)).length;

  const heal = async () => {
    setBusy(true);
    setErr(null);
    try {
      await api.healClip(clip.path);
      setHealed((s) => new Set(s).add(clip.path));
      onHealed?.(clip.path);
      // On to the next one still to do.
      const next = list.findIndex((c, k) => k > i && !healed.has(c.path));
      if (next >= 0) setI(next);
      else if (left <= 1) {
        onToast(`Healed ${healed.size + 1} clip${healed.size === 0 ? "" : "s"}. The originals are in the Recycle Bin.`);
        onClose();
      }
    } catch (e) {
      setErr(errText(e));
    } finally {
      setBusy(false);
    }
  };

  return createPortal(
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal del-preview" role="dialog" aria-modal="true" aria-label="Heal damaged clips" onClick={(e) => e.stopPropagation()}>
        <div className="row" style={{ alignItems: "baseline" }}>
          <h2 style={{ flex: 1 }}>Heal damaged clips</h2>
          <span className="muted mono" style={{ fontSize: 12 }}>
            {i + 1} / {n}
          </span>
        </div>
        <div
          className="del-stage"
          onPointerDown={(e) => {
            start.current = e.pointerType === "mouse" && (e.target as HTMLElement).tagName === "VIDEO" ? null : e.clientX;
          }}
          onPointerUp={(e) => {
            if (start.current == null) return;
            const dx = e.clientX - start.current;
            start.current = null;
            if (Math.abs(dx) >= 60) go(dx < 0 ? 1 : -1);
          }}
        >
          <Slide key={clip.path} clip={clip} range={h.best} onSeen={() => setSeen((s) => (s.has(clip.path) ? s : new Set(s).add(clip.path)))} />
          {done && <span className="heal-done with-icon"><Icon name="check" size={15} />Healed</span>}
          <button className="del-nav prev round" aria-label="Previous clip (←)" disabled={i === 0} onClick={() => go(-1)}>
            ‹
          </button>
          <button className="del-nav next round" aria-label="Next clip (→)" disabled={i >= n - 1} onClick={() => go(1)}>
            ›
          </button>
        </div>
        <HealBar h={h} />
        <div className="row del-meta">
          <span style={{ flex: 1, minWidth: 0 }}>
            <strong className="del-name" title={clip.path}>
              {clip.name}
            </strong>
            <small className="muted">
              {done
                ? "Healed. The original is in the Recycle Bin."
                : remote
                  ? "This clip is on the NAS, which has no Recycle Bin, so it can't be healed in place. Open it and use “Save the good part as a copy”."
                  : healText(h)}
            </small>
          </span>
        </div>
        {err && <small className="err-text">{err}</small>}
        {repaired[clip.path] && (
          <small className="muted">
            Saved a repaired copy: <b>{repaired[clip.path].split("\\").pop()}</b>. It keeps the whole clip; the damaged moment is
            smoothed over rather than perfect. Compare it in the library, then keep whichever you like. The original is untouched.
          </small>
        )}
        {(repairing || busy) && <ProgressBar pct={repairing ? repairPct : healPct} />}
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <span className="muted" style={{ fontSize: 12, marginRight: "auto" }}>
            {seen.has(clip.path) || done
              ? "The preview plays the part that's kept. ← → or swipe to move."
              : "Watch the part that's kept first: Heal unlocks after a few seconds."}
          </span>
          <button className="btn" onClick={onClose}>
            {healed.size ? "Done" : "Close"}
          </button>
          <button className="btn" disabled={busy || i >= n - 1} onClick={() => go(1)}>
            Skip
          </button>
          <button
            className="btn"
            disabled={busy || repairing || done || remote || !!repaired[clip.path]}
            title="Make a full-length copy with the broken frames filled in from the ones around them (the original stays)"
            onClick={async () => {
              setRepairing(true);
              setErr(null);
              try {
                const out = await api.repairClip(clip.path);
                setRepaired((r) => ({ ...r, [clip.path]: out }));
              } catch (e) {
                setErr(errText(e));
              } finally {
                setRepairing(false);
              }
            }}
          >
            {repairing ? `Repairing… ${repairPct ?? 0}%` : repaired[clip.path] ? "Repaired copy saved" : "Try repair (keep it all)"}
          </button>
          <button className="btn primary" disabled={busy || done || remote || !h.best || !seen.has(clip.path)} onClick={heal}>
            {busy ? `Healing… ${healPct ?? 0}%` : "Heal this clip"}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

/** The clip, muted, looping over just the part that would be kept. */
function Slide({ clip, range, onSeen }: { clip: ClipFile; range: [number, number] | null; onSeen: () => void }) {
  const thumb = useThumb(clip.path, clip.modified);
  const [failed, setFailed] = useState(false);
  if (failed) return thumb ? <img className="del-media" src={thumb} alt="" /> : <div className="del-media" />;
  return (
    <video
      className="del-media"
      src={mediaSrc(clip.path)}
      poster={thumb ?? undefined}
      autoPlay
      muted
      playsInline
      controls
      onLoadedMetadata={(e) => {
        if (range) e.currentTarget.currentTime = range[0];
      }}
      onTimeUpdate={(e) => {
        const v = e.currentTarget;
        if (range && (v.currentTime < range[0] - 0.5 || v.currentTime >= range[1] - 0.1)) v.currentTime = range[0];
        // Watched about 3 s of what's kept (or all of a shorter part).
        if (range && v.currentTime >= Math.min(range[0] + 3, range[1] - 0.2)) onSeen();
      }}
      onError={() => setFailed(true)}
    />
  );
}
