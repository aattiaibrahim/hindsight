import { useEffect, useRef, useState } from "react";
import { api, codecLabel, errText, fmtDuration, Status } from "./api";
import { audioLine, folderLine, RecordButton } from "./BufferCard";
import { nasActivity, NasBadge, NasProgress, useNasState } from "./NasProgress";

/**
 * One thin line along the bottom of the window: the buffer, what it records,
 * the NAS, and the save / record buttons. Replaces the big card that used to
 * sit in the sidebar; its details open from the buffer part.
 */
export function StatusBar({
  status,
  hotkey,
  recordHotkey,
  version,
  customArea = false,
  onFitScreen,
}: {
  status: Status | null;
  hotkey: string;
  recordHotkey: string;
  version?: string;
  /** Only part of the screen is being recorded (Settings → Recording area) */
  customArea?: boolean;
  onFitScreen?: () => void;
}) {
  const nas = useNasState();
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const wrap = useRef<HTMLDivElement>(null);
  const on = !!status?.running;
  const bufferOn = on && !!status?.enabled;
  const problem = (status?.lastError && !on && status.enabled) || (on && status?.audioError);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => !wrap.current?.contains(e.target as Node) && setOpen(false);
    const key = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", key);
    };
  }, [open]);

  const save = async () => {
    setBusy(true);
    setErr(null);
    try {
      await api.saveClip();
    } catch (e) {
      setErr(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const bufferText = !status
    ? "Starting…"
    : bufferOn
      ? `Buffer on · ${fmtDuration(status.bufferedSeconds)} of ${fmtDuration(status.bufferSeconds)}`
      : status.enabled === false
        ? "Buffer paused"
        : "Buffer starting…";
  const activity = nasActivity(nas);
  const nasText = activity?.text ?? null;

  return (
    <footer className="statusbar">
      <div className="sb-buffer" ref={wrap}>
        <button className={`sb-item ${problem ? "bad" : ""}`} onClick={() => setOpen((o) => !o)} aria-expanded={open} title="Buffer details">
          <span className={`dot ${bufferOn ? "on" : problem ? "err" : ""}`} />
          {bufferText}
        </button>
        {open && (
          <div className="sb-pop" role="dialog" aria-label="Buffer details">
            <div className="meter" aria-hidden="true">
              <div style={{ width: `${bufferOn && status?.bufferSeconds ? (status.bufferedSeconds / status.bufferSeconds) * 100 : 0}%` }} />
            </div>
            <div className="buffer-meta">
              {status && bufferOn ? (status.inRam ? `${status.bufferMb} MB in RAM` : "Held on disk") : "Not recording right now"}
              <br />
              {audioLine(status) ?? "No audio"}
              <br />
              {folderLine(status)}
            </div>
            <NasProgress n={nas} compact />
            {status?.lastError && !on && status.enabled && <div className="err-text">{status.lastError}</div>}
            {on && status?.audioError && <div className="err-text">{status.audioError}</div>}
          </div>
        )}
      </div>
      {on && (
        <span className="sb-item quiet" title={customArea ? `Recording only part of the screen: ${status!.width} × ${status!.height}` : undefined}>
          {codecLabel(status!.encoder)} · {status!.height}p{status!.fps}
          {customArea && " · part of the screen"}
        </span>
      )}
      {/* A changed recording area is easy to forget (clips come out an odd size): one click back to the whole screen. */}
      {on && customArea && onFitScreen && (
        <button className="btn small" onClick={onFitScreen} title="Record the whole screen again (Settings → Recording area)">
          Fit to screen
        </button>
      )}
      {nasText && (
        <span className={`sb-item quiet sb-nas ${nas && !nas.reachable && !nas.connecting ? "bad" : ""}`} title={nas?.copying ? `${nas.copying}${nas.lastError ? `\n${nas.lastError}` : ""}` : (nas?.lastError ?? undefined)}>
          {nas?.connecting ? <span className="sb-spin" aria-hidden="true" /> : <NasBadge status={nas?.copying ? "copying" : nas?.pending ? "not-synced" : "synced"} />}
          {nasText}
          {activity?.pct != null && (
            <span className="sb-meter" aria-hidden="true">
              <i style={{ width: `${activity.pct}%` }} />
            </span>
          )}
        </span>
      )}
      {/* Stop copying or freeing up (e.g. changed your mind), and pick it up again later. */}
      {nas?.enabled && nas.reachable && (activity?.busy || nas.stopped) && (
        <button
          className="btn small sb-stop"
          onClick={() => api.stopNasSync(!nas.stopped).catch(() => {})}
          title={nas.stopped ? "Carry on copying to the NAS (and freeing up space if that's on)" : "Stop after the current clip. Nothing is lost: it picks up where it left off when you resume."}
        >
          {nas.stopped ? "Resume" : "Stop"}
        </button>
      )}
      <span style={{ flex: 1 }} />
      {err && <span className="sb-item bad">{err}</span>}
      {version && <span className="sb-item quiet">v{version} beta</span>}
      <RecordButton status={status} hotkey={recordHotkey} className="small" />
      <button className="btn small primary" disabled={!bufferOn || busy} onClick={save} title={`Save the last ${status ? fmtDuration(status.bufferSeconds) : "moments"} (${hotkey})`}>
        {busy ? "Saving…" : `Save clip · ${hotkey}`}
      </button>
    </footer>
  );
}
