import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";

/**
 * How far an ffmpeg job on `path` has got (0–99), from the "edit-progress" events
 * lib.rs sends for edits, Shrink, repair, heal and Change to .mp4. null until the
 * first report, and again when `on` goes false.
 */
export function useEditProgress(path: string | null | undefined, on: boolean): number | null {
  const [pct, setPct] = useState<number | null>(null);
  useEffect(() => {
    setPct(null);
    if (!on || !path) return;
    const un = listen<{ path: string; pct: number }>("edit-progress", (e) => {
      if (e.payload.path === path) setPct((p) => Math.max(p ?? 0, e.payload.pct));
    });
    return () => {
      un.then((f) => f());
    };
  }, [path, on]);
  return pct;
}

/** (done, total) from a "move-progress" / "dupes-progress" style event while `on`. */
export function useCountProgress(event: string, on: boolean): [number, number] | null {
  const [p, setP] = useState<[number, number] | null>(null);
  useEffect(() => {
    setP(null);
    if (!on) return;
    const un = listen<[number, number]>(event, (e) => setP(e.payload));
    return () => {
      un.then((f) => f());
    };
  }, [event, on]);
  return p;
}

/** A thin bar: filled to `pct`, or a moving stripe while it isn't known yet. */
export function ProgressBar({ pct }: { pct: number | null }) {
  return (
    <span className={`job-bar ${pct == null ? "unknown" : ""}`} role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct ?? undefined}>
      <span style={pct == null ? undefined : { width: `${pct}%` }} />
    </span>
  );
}
