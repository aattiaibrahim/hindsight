/**
 * The NAS sync item in the status bar: what the app and the NAS are doing right
 * now (with a spinning icon while they're at it), and a panel with when it last
 * synced, the progress, Sync now / Stop, and a short history of what happened.
 */
import { useEffect, useRef, useState } from "react";
import { Cloud, CloudAlert, CloudOff, RefreshCw } from "lucide-react";
import { api, NasState } from "./api";
import { nasActivity, NasProgress } from "./NasProgress";
import { GameIcon } from "./GameIcon";

/** "just now", "4 min ago", "2 h ago", "yesterday 21:14", "Nov 12, 21:14". */
export function ago(ms: number, now = Date.now()) {
  const s = Math.max(0, (now - ms) / 1000);
  if (s < 45) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  const d = new Date(ms);
  const time = d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  if (s < 6 * 3600) return `${Math.round(s / 3600)} h ago`;
  const today = new Date(now);
  const yesterday = new Date(now - 86_400_000);
  if (d.toDateString() === today.toDateString()) return `today ${time}`;
  if (d.toDateString() === yesterday.toDateString()) return `yesterday ${time}`;
  return `${d.toLocaleDateString(undefined, { month: "short", day: "numeric" })}, ${time}`;
}

type Look = "busy" | "ok" | "off" | "bad" | "idle";

function lookOf(n: NasState, busy: boolean): Look {
  if (busy || n.connecting) return "busy";
  if (!n.reachable) return "off";
  if (n.lastError) return "bad";
  if (n.pending || n.stopped || n.paused) return "idle";
  return "ok";
}

function SyncIcon({ look }: { look: Look }) {
  const p = { size: 15, strokeWidth: 1.9, "aria-hidden": true } as const;
  if (look === "busy") return <RefreshCw {...p} className="sync-spin" />;
  if (look === "off") return <CloudOff {...p} />;
  if (look === "bad") return <CloudAlert {...p} />;
  return <Cloud {...p} />;
}

export function SyncStatus({ nas, onSyncWhilePlaying }: { nas: NasState | null; onSyncWhilePlaying?: () => void }) {
  const [open, setOpen] = useState(false);
  const [now, setNow] = useState(Date.now());
  const [asked, setAsked] = useState(false);
  const wrap = useRef<HTMLDivElement>(null);
  // Times like "3 min ago" move on while the panel is open.
  useEffect(() => {
    if (!open) return;
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), 15_000);
    const close = (e: MouseEvent) => !wrap.current?.contains(e.target as Node) && setOpen(false);
    const key = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", key);
    return () => {
      clearInterval(t);
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", key);
    };
  }, [open]);
  useEffect(() => {
    if (!asked) return;
    const t = setTimeout(() => setAsked(false), 4000);
    return () => clearTimeout(t);
  }, [asked]);

  const activity = nasActivity(nas);
  if (!nas?.enabled || !activity) return null;
  const look = lookOf(nas, activity.busy);
  const last = nas.lastSync ? ago(nas.lastSync, now) : null;
  const recent = nas.recent ?? [];
  const working = activity.busy || asked;

  return (
    <div className="sb-sync" ref={wrap}>
      <button
        className={`sb-item sb-nas ${look === "off" || look === "bad" ? "bad" : "quiet"}`}
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        title="NAS sync: what it's doing and when it last synced"
      >
        <SyncIcon look={look} />
        <span className="sb-text">{activity.text}</span>
        {activity.pct != null && (
          <span className="sb-meter" aria-hidden="true">
            <i style={{ width: `${activity.pct}%` }} />
          </span>
        )}
      </button>
      {open && (
        <div className="sb-pop sync-pop" role="dialog" aria-label="NAS sync">
          <div className="sync-head">
            <span className={`sync-badge ${look}`}>
              {nas.paused && nas.pausedFor ? <GameIcon name={nas.pausedFor} size={22} /> : <SyncIcon look={look} />}
            </span>
            <div className="sync-title">
              <strong>{activity.text}</strong>
              <small className="muted">
                {last ? `Last synced ${last}` : nas.reachable ? "Hasn't finished a sync since Hindsight started" : "Not synced since Hindsight started"}
              </small>
            </div>
          </div>
          <NasProgress n={nas} compact />
          <div className="sync-facts muted">
            <span>{nas.copied} copied this session</span>
            <span>{nas.pending ? `${nas.pending} waiting` : "Nothing waiting"}</span>
          </div>
          {nas.lastError && <div className="err-text sync-err">{nas.lastError}</div>}
          <div className="row sync-actions">
            <button
              className="btn small primary"
              disabled={working || nas.stopped || !nas.reachable}
              onClick={() => {
                setAsked(true);
                api.syncNasNow().catch(() => setAsked(false));
              }}
              title={nas.stopped ? "Resume first" : !nas.reachable ? "The NAS can't be reached right now" : "Check for new clips and copy them now"}
            >
              {working ? "Syncing…" : "Sync now"}
            </button>
            {nas.reachable && nas.paused && onSyncWhilePlaying && (
              <button className="btn small" onClick={onSyncWhilePlaying} title="Copy to the NAS while you play from now on (Settings → Library → Sync while you play)">
                Sync while playing
              </button>
            )}
            {nas.reachable && (activity.busy || nas.stopped) && (
              <button className="btn small" onClick={() => api.stopNasSync(!nas.stopped).catch(() => {})}>
                {nas.stopped ? "Resume" : "Stop"}
              </button>
            )}
          </div>
          <div className="sync-recent">
            <div className="sync-recent-head muted">Recent activity</div>
            {recent.length === 0 ? (
              <div className="muted sync-empty">Nothing yet since Hindsight started.</div>
            ) : (
              <ul>
                {recent.slice(0, 12).map((e, i) => (
                  <li key={`${e.at}-${i}`} className={e.kind}>
                    {e.game ? <GameIcon name={e.game} size={14} /> : <span className="sync-dot" aria-hidden="true" />}
                    <span className="sync-text">{e.text}</span>
                    <time className="muted" dateTime={new Date(e.at).toISOString()} title={new Date(e.at).toLocaleString()}>
                      {ago(e.at, now)}
                    </time>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
