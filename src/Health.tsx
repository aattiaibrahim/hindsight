import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { api, ClipFile, errText, fmtTime, Health, HealthProgress } from "./api";
import { Icon } from "./icons";

/** Health results for the clips on screen (only ones already checked). */
export function useHealth(clips: ClipFile[] | null) {
  const [health, setHealth] = useState<Record<string, Health>>({});
  // Asked again only when the clips really changed, not every time the list is re-read
  // (that happens whenever the window gets focus).
  const key = clips?.map((c) => `${c.path}|${c.modified}`).join("\n") ?? "";
  useEffect(() => {
    if (!key) return;
    const paths = key.split("\n").map((l) => l.slice(0, l.lastIndexOf("|")));
    const load = () => api.clipHealth(paths).then(setHealth).catch(() => {});
    load();
    // New results arrive as the background check goes (it re-emits clips-changed on damage).
    const t = setInterval(load, 60_000);
    const un = listen("clips-changed", load);
    return () => {
      clearInterval(t);
      un.then((f) => f());
    };
  }, [key]);
  return health;
}

export function damagedText(h: Health) {
  if (h.state === "unreadable") return "This file can't be read at all.";
  const parts = h.damaged.map(([a, b]) => `${fmtTime(a).replace(/\.\d+$/, "")}–${fmtTime(Math.min(b, h.duration)).replace(/\.\d+$/, "")}`);
  return `Damaged: ${parts.join(", ")} won't play properly.`;
}

/** Studio: what's wrong, and a one-click rescue of the good part. */
export function HealthNotice({
  path,
  health,
  busy: editing = false,
  onHeal,
  onRescued,
}: {
  path: string;
  health: Health | undefined;
  busy?: boolean;
  /** Replace the clip with its good part */
  onHeal?: () => void;
  onRescued: (p: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  if (!health || health.state === "ok") return null;
  const best = health.best;
  const rescue = async () => {
    setBusy(true);
    setErr(null);
    try {
      onRescued(await api.rescueClip(path));
    } catch (e) {
      setErr(errText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="health-notice">
      <strong className="with-icon"><Icon name="warn" size={16} />{damagedText(health)}</strong>
      <small>
        The file was already like this when it was saved (it's common with NVIDIA and OBS MP4s after a driver hiccup or a
        crash). Hindsight's own clips are saved as MKV, which survives crashes, so they're far less likely to be hit.
      </small>
      {best && onHeal && (
        <button
          className="btn small primary"
          disabled={busy || editing}
          onClick={onHeal}
          title="Shows the part that would be kept. After you've watched it you can cut the clip down to it (nothing is re-encoded; the original goes to the Recycle Bin)."
        >
          Review and heal…
        </button>
      )}
      {best && (
        <button className="btn small" disabled={busy || editing} onClick={rescue} title="Copies the good part into a new file. Nothing is re-encoded and the original stays.">
          {busy ? "Saving…" : "Save the good part as a copy"}
        </button>
      )}
      {err && <small className="err-text">{err}</small>}
    </div>
  );
}

/** How far the background check has got, as one plain line. */
function statusText(p: HealthProgress | null, on = true) {
  if (!on) return "Off.";
  if (!p || !p.total) return "Starting…";
  if (p.checked >= p.total)
    return `All ${p.total} clips checked${p.damaged ? `, ${p.damaged} damaged (marked in your library)` : ", none damaged"}.`;
  return `${p.checked} of ${p.total} checked${p.damaged ? `, ${p.damaged} damaged so far` : ""}${p.paused ? " · paused while you play" : ""}.`;
}

function useHealthProgress() {
  const [p, setP] = useState<HealthProgress | null>(null);
  useEffect(() => {
    const load = () => api.healthProgress().then(setP).catch(() => {});
    load();
    const t = setInterval(load, 5000);
    return () => clearInterval(t);
  }, []);
  return p;
}

/** Top of the Damaged section: what it is, and where the check is up to. */
export function DamagedStatus({ count }: { count: number }) {
  const p = useHealthProgress();
  return (
    <p className="muted damaged-status">
      {count > 0
        ? "These clips won't play properly all the way through. Review and heal shows each one's good part so you can decide, one clip at a time, whether to cut it down to that part (nothing is re-encoded, originals go to the Recycle Bin)."
        : "Hindsight checks your clips in the background, one at a time, only while you're not gaming or recording."}{" "}
      <span className="damaged-progress">{statusText(p)}</span>
    </p>
  );
}

/** Settings → Library: the background check's switch and progress. */
export function HealthSetting({
  on,
  set,
  Toggle,
}: {
  on: boolean;
  set: (v: boolean) => void;
  Toggle: (p: { label: string; checked: boolean; onChange: (v: boolean) => void }) => React.ReactElement;
}) {
  const p = useHealthProgress();
  const status = statusText(p, on);
  return (
    <label className="setting">
      <span>
        Check clips for damage
        <small>
          Finds clips that won't play properly and offers to heal them (keep only the good part). Runs only while no game is in front, on
          the graphics card's video decoder, one clip at a time, and stops the moment a game starts.
        </small>
        <small>{status}</small>
      </span>
      <Toggle label="Check clips for damage" checked={on} onChange={set} />
    </label>
  );
}
