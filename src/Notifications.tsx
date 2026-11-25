import { useEffect, useState } from "react";
import { api } from "./api";
import { Icon, IconName } from "./icons";

export type NoticeKind = "update" | "folders" | "damaged" | "dupes" | "sort" | "nas";

export type Notice = {
  /** Stable while the notice says the same thing; hiding it hides this id. */
  id: string;
  kind: NoticeKind;
  title: string;
  detail?: string;
  tone?: "info" | "warn" | "good";
  actions?: { label: string; run: () => void; primary?: boolean }[];
};

const KEY = "rewind.noticesHidden";
/** When each notice first said what it says now (id -> ms). */
const SEEN_KEY = "rewind.noticesSeen";
/** The version whose "What's new" you closed. */
const NEWS_KEY = "rewind.whatsNewHidden";

function loadJson<T>(key: string, fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    return v ? (JSON.parse(v) as T) : fallback;
  } catch {
    return fallback;
  }
}
function saveJson(key: string, v: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(v));
  } catch {
    /* only remembered for this session */
  }
}

/** "just now", "5 min ago", "3 h ago", "yesterday", then the date. */
export function ago(ms: number, now = Date.now()) {
  const s = Math.max(0, (now - ms) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  const d = new Date(ms);
  const y = new Date(now);
  y.setDate(y.getDate() - 1);
  if (d.toDateString() === y.toDateString()) return "yesterday";
  return d.toLocaleDateString(undefined, { day: "numeric", month: "short", ...(d.getFullYear() !== new Date(now).getFullYear() ? { year: "numeric" } : {}) });
}
const fullDate = (ms: number) => new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

function loadHidden(): string[] {
  try {
    return JSON.parse(localStorage.getItem(KEY) || "[]");
  } catch {
    return [];
  }
}

function saveHidden(ids: string[]) {
  try {
    if (ids.length) localStorage.setItem(KEY, JSON.stringify(ids));
    else localStorage.removeItem(KEY);
  } catch {
    /* only remembered for this session */
  }
}

/** Which notices are hidden (remembered until a notice changes what it says). */
export function useNoticeState(notices: Notice[]) {
  const [hidden, setHidden] = useState<string[]>(loadHidden);
  // When each notice appeared (or last changed what it says), kept across restarts.
  const [seen, setSeen] = useState<Record<string, number>>(() => loadJson(SEEN_KEY, {}));
  const ids = notices.map((n) => n.id).join("\n");
  useEffect(() => {
    setSeen((old) => {
      const now = Date.now();
      const next: Record<string, number> = {};
      for (const n of notices) next[n.id] = old[n.id] ?? now;
      const same = Object.keys(next).length === Object.keys(old).length && Object.keys(next).every((k) => old[k] === next[k]);
      if (same) return old;
      saveJson(SEEN_KEY, next);
      return next;
    });
  }, [ids]); // eslint-disable-line react-hooks/exhaustive-deps
  const shown = notices.filter((n) => !hidden.includes(n.id));
  const set = (ids: string[]) => {
    // Forget ids no notice uses any more, so the list doesn't grow forever.
    const next = ids.filter((h) => notices.some((n) => n.id === h));
    setHidden(next);
    saveHidden(next);
  };
  return {
    seen,
    shown,
    hiddenCount: notices.length - shown.length,
    hide: (id: string) => set([...hidden, id]),
    hideAll: () => set(notices.map((n) => n.id)),
    showHidden: () => set([]),
  };
}

type Group = "all" | "library" | "nas" | "updates";
const GROUP_OF: Record<NoticeKind, Group> = { update: "updates", folders: "library", damaged: "library", dupes: "library", sort: "library", nas: "nas" };
const GROUPS: [Group, string][] = [
  ["all", "All"],
  ["library", "Library"],
  ["nas", "NAS"],
  ["updates", "Updates"],
];

const KIND_ICON: Record<NoticeKind, IconName> = { update: "download", folders: "folders", damaged: "damaged", dupes: "copy", sort: "cleanup", nas: "nas" };

function KindIcon({ kind }: { kind: NoticeKind }) {
  return (
    <span className={`notif-icon ${kind}`} aria-hidden="true">
      <Icon name={KIND_ICON[kind]} size={18} />
    </span>
  );
}

/**
 * Notifications, as a page of their own (there can be a lot of them). One row
 * per notice with its action and an × to hide it; filters along the top.
 */
export function NotificationsPage({ state }: { state: ReturnType<typeof useNoticeState> }) {
  const [group, setGroup] = useState<Group>("all");
  const { shown, hiddenCount, hide, hideAll, showHidden, seen } = state;
  // Newest first, so the timestamps read top to bottom.
  const byTime = [...shown].sort((a, b) => (seen[b.id] ?? 0) - (seen[a.id] ?? 0));
  // Ticks the "5 min ago"s along while the page is open.
  const [, tick] = useState(0);
  useEffect(() => {
    const t = window.setInterval(() => tick((n) => n + 1), 60_000);
    return () => window.clearInterval(t);
  }, []);
  // What's new in the version you have: its release notes, until you close them.
  const [news, setNews] = useState<{ version: string; date: string; notes: string } | null>(null);
  const [newsHidden, setNewsHidden] = useState<string>(() => loadJson(NEWS_KEY, ""));
  useEffect(() => {
    let alive = true;
    api
      .whatsNew()
      .then((n) => alive && n.notes && setNews(n))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);
  const showNews = !!news && newsHidden !== news.version && (group === "all" || group === "updates");
  const list = byTime.filter((n) => group === "all" || GROUP_OF[n.kind] === group);
  const count = (g: Group) => shown.filter((n) => g === "all" || GROUP_OF[n.kind] === g).length;
  return (
    <div className="page notif-page">
      <div className="notif-top">
        <h1>Notifications</h1>
        <div className="notif-filters" role="tablist" aria-label="Show">
          {GROUPS.map(([g, label]) => (
            <button key={g} role="tab" aria-selected={group === g} className={`chip ${group === g ? "on" : ""}`} onClick={() => setGroup(g)}>
              {label} <span className="chip-n">{count(g)}</span>
            </button>
          ))}
        </div>
        <span style={{ flex: 1 }} />
        {hiddenCount > 0 && (
          <button className="btn small" onClick={showHidden} title="Show hidden notices again">
            Show {hiddenCount} hidden
          </button>
        )}
        {shown.length > 0 && (
          <button className="btn small" onClick={hideAll}>
            Hide all
          </button>
        )}
      </div>
      {showNews && news && (
        <div className="notif-news">
          <KindIcon kind="update" />
          <div className="notif-text">
            <strong>
              What's new in {news.version}
              {news.date && (
                <time className="notif-time" dateTime={news.date} title={fullDate(Date.parse(news.date))}>
                  {ago(Date.parse(news.date))}
                </time>
              )}
            </strong>
            <ul>
              {news.notes
                .split(/(?<=[.!?])\s+(?=[A-Z"“])|\r?\n+/)
                .map((l) => l.replace(/^[-*•]\s*/, "").trim())
                .filter(Boolean)
                .map((l, i) => (
                  <li key={i}>{l}</li>
                ))}
            </ul>
          </div>
          <button
            className="notif-x"
            onClick={() => {
              setNewsHidden(news.version);
              saveJson(NEWS_KEY, news.version);
            }}
            title="Hide until the next version"
            aria-label="Hide what's new"
          >
            <Icon name="close" size={16} />
          </button>
        </div>
      )}
      {list.length === 0 ? (
        <div className="notif-empty">
          <strong>You're all caught up</strong>
          <span className="muted">New things to look at show up here.</span>
        </div>
      ) : (
        <div className="notif-list">
          {list.map((n) => (
            <div key={n.id} className={`notif-row ${n.tone ?? "info"}`}>
              <KindIcon kind={n.kind} />
              <div className="notif-text">
                <strong>{n.title}</strong>
                {n.detail && <small>{n.detail}</small>}
                {seen[n.id] && (
                  <time className="notif-time" dateTime={new Date(seen[n.id]).toISOString()} title={fullDate(seen[n.id])}>
                    {ago(seen[n.id])}
                  </time>
                )}
              </div>
              <div className="notif-actions">
                {n.actions?.map((a) => (
                  <button key={a.label} className={`btn small ${a.primary ? "primary" : ""}`} onClick={a.run}>
                    {a.label}
                  </button>
                ))}
              </div>
              <button className="notif-x" onClick={() => hide(n.id)} title="Hide this until it changes" aria-label={`Hide “${n.title}”`}>
                <Icon name="close" size={16} />
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
