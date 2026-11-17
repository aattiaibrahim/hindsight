/**
 * A move into Hindsight running in the background (Settings → Library → Move
 * into Hindsight), in the status bar: how far it's got, when it's waiting for a
 * game or a save, Stop, and what happened when it's over.
 */
import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { FolderInput } from "lucide-react";
import { api, MoveStatus } from "./api";

export function MoveStatusItem() {
  const [st, setSt] = useState<MoveStatus | null>(null);
  // A finished move's result shows for a little while, then goes.
  const [showDone, setShowDone] = useState(false);
  useEffect(() => {
    api
      .moveStatus()
      .then((s) => s?.running && setSt(s))
      .catch(() => {});
    const un = listen<MoveStatus>("move-status", (e) => {
      setSt(e.payload);
      if (!e.payload.running && e.payload.finished) setShowDone(true);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);
  useEffect(() => {
    if (!showDone) return;
    const t = setTimeout(() => setShowDone(false), 15_000);
    return () => clearTimeout(t);
  }, [showDone]);

  if (!st || (!st.running && !showDone)) return null;
  if (!st.running) {
    return (
      <span className="sb-item quiet sb-move" title={st.label}>
        <FolderInput size={15} aria-hidden="true" />
        <span className="sb-text">{st.finished}</span>
      </span>
    );
  }
  const pct = st.total ? Math.min(100, Math.round((st.done / st.total) * 100)) : 0;
  const text = st.stopping
    ? "Stopping after this clip…"
    : `${st.verb} clips into Hindsight · ${Math.min(st.done + 1, st.total)} of ${st.total}${st.paused ? (st.pausedFor ? ` · waiting while ${st.pausedFor} is open` : " · waiting for a clip to save") : ""}`;
  return (
    <>
      <span
        className="sb-item quiet sb-move"
        title={`From ${st.label}. ${st.paused ? "It waits while a game is running or a clip is being saved, then carries on." : "Runs in the background: the app stays usable and clips still save."}`}
      >
        <FolderInput size={15} aria-hidden="true" className={st.paused ? "" : "sb-move-on"} />
        <span className="sb-text" title={text}>{text}</span>
        <span className="sb-meter" aria-hidden="true">
          <i style={{ width: `${pct}%` }} />
        </span>
      </span>
      {st.paused && st.pausedFor && !st.stopping && (
        <button
          className="btn small"
          onClick={() => api.moveAnyway().catch(() => {})}
          title={`${st.pausedFor} is still open (maybe in the background). Copying now can cost a little performance if you're playing.`}
        >
          Move anyway
        </button>
      )}
      {!st.stopping && (
        <button className="btn small" onClick={() => api.stopMove().catch(() => {})} title="Stop after the clip it's on. What's moved stays moved; Undo in Settings puts it back.">
          Stop
        </button>
      )}
    </>
  );
}
