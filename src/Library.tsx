import { open } from "@tauri-apps/plugin-dialog";
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { api, ClipFile, codecLabel, errText, fmtDuration, fmtSize, Health, Probe, Upload, UploadProgress, VIDEO_EXTENSIONS, want } from "./api";
import { useNearView, useThumb } from "./hooks";
import { liveLink, progressText, ShareRequest, timeLeft } from "./Share";
import { Timeline } from "./Timeline";
import { DamagedStatus, damagedText } from "./Health";
import { DeletePreview } from "./DeletePreview";
import { GameIcon } from "./GameIcon";
import { GamePicker } from "./GamePicker";
import { OnThisDay } from "./OnThisDay";
import { HealReview } from "./HealReview";
import { NasBadge, nasStatus, useNasState } from "./NasProgress";
import { SlidingPill } from "./SlidingPill";
import { Icon } from "./icons";

/** Where each list (all clips, damaged clips) was scrolled to, kept while you're in Studio (the page itself unmounts). */
const memories: Record<string, { scroll: number; folder: string; query: string; filters?: Filters }> = {};

type ViewMode = "gallery" | "folders" | "list";
type Sort = "new" | "old" | "big";
/** where: "" any, "pc" on this PC, "nas" on the NAS; sources: "Hindsight", "Medal"…; from/to: YYYY-MM-DD; minMb: 0 = any */
type Filters = { where: "" | "pc" | "nas"; sources: string[]; from: string; to: string; minMb: number };
const NO_FILTERS: Filters = { where: "", sources: [], from: "", to: "", minMb: 0 };
/** Cards built straight away (a couple of screens), then this many more at a time. */
const FIRST_CARDS = 40;
const MORE_CARDS = 80;
/** Gallery: rows per block, how far past the screen blocks are built, and the sizes used to guess heights. */
const BLOCK_ROWS = 5;
const NEAR = 1400;
const CARD_MIN = 210;
const CARD_GAP = 6;
const DAY_HEAD = 32;
const PAGE_GAP = 22;

const TITLES: Record<string, string> = { totrim: "To trim", clips: "Clips", local: "Local clips", synced: "Synced clips", damaged: "Damaged clips" };

const remembered = <T extends string>(k: string, d: T): T => {
  try {
    return (localStorage.getItem(`rewind.${k}`) as T) || d;
  } catch {
    return d;
  }
};
const remember = (k: string, v: string) => {
  try {
    localStorage.setItem(`rewind.${k}`, v);
  } catch {
    /* only a convenience */
  }
};
const sourceOf = (c: ClipFile) => (c.location === "linked" ? c.source : c.location === "nas" ? "Other PCs" : "Hindsight");
const ymd = (ms: number) => {
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
};

/** A game's folder in Folders view: its newest clip as the cover, with count and size. */
function FolderTile({ name, items, onOpen }: { name: string; items: ClipFile[]; onOpen: () => void }) {
  const cover = items[0];
  const thumb = useThumb(cover.path, cover.modified);
  const bytes = items.reduce((n, c) => n + c.size, 0);
  return (
    <button className="folder-tile" onClick={onOpen} title={`Open ${name}`}>
      <div className="folder-cover">
        <div className="folder-back" />
        <div className="thumb">{thumb && <img src={thumb} alt="" loading="lazy" />}</div>
      </div>
      <div className="card-title folder-title">
        <GameIcon name={name} size={18} />
        {name}
      </div>
      <div className="card-meta">
        {items.length} clip{items.length === 1 ? "" : "s"} · {fmtSize(bytes)} · newest {dayLabel(cover.modified)}
      </div>
    </button>
  );
}

/** One clip in List view. */
function ClipRow({
  clip,
  selected,
  onClick,
  damage,
  nasOn,
  copying,
}: {
  clip: ClipFile;
  selected: boolean;
  onClick: (e: React.MouseEvent) => void;
  damage?: Health;
  nasOn: boolean;
  copying?: string | null;
}) {
  const [rowRef, near] = useNearView<HTMLButtonElement>();
  const thumb = useThumb(clip.path, clip.modified, near);
  const time = new Date(clip.modified).toLocaleString(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
  return (
    <button ref={rowRef} className={`clip-row ${selected ? "selected" : ""}`} onClick={onClick} title={clip.path} data-path={clip.path}>
      <div className="clip-row-thumb">{thumb && <img src={thumb} alt="" decoding="async" />}</div>
      <div className="clip-row-main">
        <div className="card-title">{clip.name}</div>
        <div className="card-meta">
          {clip.folder} · {time}
          {damage && damage.state !== "ok" && <span className="src-tag bad with-icon"><Icon name="warn" size={12} />Damaged</span>}
          {clip.location === "linked" && <span className="src-tag">{clip.source}</span>}
          <NasBadge status={nasStatus(clip, nasOn, copying)} />
        </div>
      </div>
      <div className="clip-row-size">{fmtSize(clip.size)}</div>
    </button>
  );
}

function dayLabel(ms: number) {
  const d = new Date(ms);
  const today = new Date();
  const y = new Date();
  y.setDate(today.getDate() - 1);
  if (d.toDateString() === today.toDateString()) return "Today";
  if (d.toDateString() === y.toDateString()) return "Yesterday";
  // Immich-style: "Mon, Nov 3", with the year when it isn't this year.
  return d.toLocaleDateString(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
    year: d.getFullYear() === today.getFullYear() ? undefined : "numeric",
  });
}

function containerLabel(path: string) {
  const ext = path.split(".").pop()?.toUpperCase() ?? "";
  return ext === "M4V" ? "MP4" : ext;
}

function ClipCard({
  clip,
  selected,
  onClick,
  onToggle,
  onDelete,
  onToast,
  upload,
  link,
  onShare,
  damage,
  returned,
  deleteCount = 1,
  nasOn,
  onKeepWhole,
  copying,
}: {
  /** The file the NAS sync is copying right now */
  copying?: string | null;
  /** To trim: take it off the list */
  onKeepWhole?: () => void;
  /** NAS syncing is on (clips not copied yet get a "waiting" mark) */
  nasOn: boolean;
  /** How many clips its Delete removes (all selected ones when it's part of a selection) */
  deleteCount?: number;
  upload?: UploadProgress;
  /** Its newest link that still works */
  link: Upload | null;
  onShare: () => void;
  damage?: Health;
  /** The clip you just came back from: briefly highlighted */
  returned: boolean;
  clip: ClipFile;
  selected: boolean;
  onClick: (e: React.MouseEvent) => void;
  onToggle: (e: React.MouseEvent) => void;
  onDelete: () => void;
  onToast: (t: string, err?: boolean) => void;
}) {
  // Only clips near the screen ask the drive for anything: with hundreds of
  // clips (or a slow hard drive) the ones you're looking at come first.
  const [cardRef, near] = useNearView<HTMLButtonElement>();
  const thumb = useThumb(clip.path, clip.modified, near);
  const [info, setInfo] = useState<Probe | null>(null);
  useEffect(() => {
    if (!near) return;
    let alive = true;
    const k = "p:" + clip.path + String(clip.modified);
    want(k, 1);
    api
      .probe(clip.path, String(clip.modified), false, true)
      .then((p) => alive && setInfo(p))
      .catch(() => {});
    return () => {
      alive = false;
      want(k, -1);
    };
  }, [near, clip.path, clip.modified]);

  const time = new Date(clip.modified).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  const nasState = nasStatus(clip, nasOn, copying);

  return (
    <button
      ref={cardRef}
      className={`card ${selected ? "selected" : ""} ${returned ? "returned" : ""}`}
      onClick={onClick}
      title={clip.path}
      data-path={clip.path}
    >
      <div className={`thumb ${nasState ? "has-nas" : ""}`}>
        {thumb && <img src={thumb} alt="" decoding="async" />}
        <span
          className={`check ${selected ? "on" : ""}`}
          role="checkbox"
          aria-checked={selected}
          aria-label={`Select ${clip.name}`}
          tabIndex={0}
          onClick={(e) => {
            e.stopPropagation();
            onToggle(e);
          }}
        >
          {selected && <Icon name="check" size={15} strokeWidth={2.5} />}
        </span>
        {info && <span className="badge br">{fmtDuration(info.duration)}</span>}
        <div className="card-actions">
          {/* No one-click MP4 here: it sat next to the checkbox and was easy to hit by mistake. It's in Studio. */}
          <span
            role="button"
            tabIndex={0}
            onClick={(e) => {
              e.stopPropagation();
              api.reveal(clip.path).catch((err) => onToast(errText(err), true));
            }}
            title="Show in Explorer"
          >
            Show
          </span>
          <span
            role="button"
            tabIndex={0}
            onClick={(e) => {
              e.stopPropagation();
              onShare();
            }}
            title="Upload and copy a link that plays inside Discord"
          >
            Share
          </span>
          {onKeepWhole && (
            <span
              role="button"
              tabIndex={0}
              onClick={(e) => {
                e.stopPropagation();
                onKeepWhole();
              }}
              title="It's fine as it is: take it off this list"
            >
              Keep whole
            </span>
          )}
          {/* The NAS has no Recycle Bin, so clips only there are deleted on the NAS itself. */}
          {clip.location !== "nas" && (
            <span
              role="button"
              tabIndex={0}
              onClick={(e) => {
                e.stopPropagation();
                onDelete();
              }}
              title={deleteCount > 1 ? `Move the ${deleteCount} selected clips to the Recycle Bin` : "Move to the Recycle Bin"}
            >
              {deleteCount > 1 ? `Delete ${deleteCount}` : "Delete"}
            </span>
          )}
        </div>
        {/* Status that matters at a glance stays visible; the rest shows on hover, like Immich. */}
        <div className="card-tags">
          {damage && damage.state !== "ok" && (
            <span className="src-tag bad with-icon" title={damagedText(damage)}>
              <Icon name="warn" size={12} />
              Damaged
            </span>
          )}
          {upload && (
            <span className={`src-tag ${upload.stage === "error" ? "bad" : "ok"}`} title={upload.error ?? undefined}>
              {progressText(upload)}
            </span>
          )}
          {!upload && link && (
            <span className="src-tag ok with-icon" title={`Shared on ${link.host}: the link is in Studio`}>
              <Icon name="link" size={12} />
              {timeLeft(link) ? `${timeLeft(link)} left` : "Shared"}
            </span>
          )}
          {clip.location === "linked" && <span className="src-tag">{clip.source}</span>}
        </div>
        {/* Stays put on hover: at a glance, which clips are safely on the NAS. */}
        {nasState && (
          <span className="card-nas">
            <NasBadge status={nasState} />
          </span>
        )}
        <div className="card-over">
          <div className="card-title">{clip.name}</div>
          <div className="card-meta">
            {clip.folder} · {time} · {fmtSize(clip.size)}
            {info && ` · ${codecLabel(info.codec)} ${containerLabel(clip.path)}`}
          </div>
        </div>
      </div>
    </button>
  );
}

/** "Counter-Strike 2" and "counter strike 2" are the same game. */
function gameKey(folder: string) {
  return folder.toLowerCase().replace(/[^a-z0-9]+/g, "");
}

/**
 * The clip lists. Redrawn only when something it shows changes (memo): the app
 * gets a status update every second, and redrawing 500+ cards on each one kept
 * the window busy.
 */
export const Library = memo(LibraryView);

function LibraryView({
  clips,
  onOpen,
  onToast,
  onDelete,
  hotkey,
  uploads,
  links,
  focusPath,
  onShare,
  health,
  mode = "clips",
  active = true,
  onKeepWhole,
}: {
  /** "damaged": the Damaged section (only the damaged clips); "local" / "synced": this PC's or the NAS's; "totrim": not trimmed yet. */
  mode?: "clips" | "damaged" | "local" | "synced" | "totrim";
  /** False while Studio covers it (it stays loaded, so coming back is instant) */
  active?: boolean;
  /** To trim: "Keep whole" takes clips off the list */
  onKeepWhole?: (paths: string[]) => void;
  health?: Record<string, Health>;
  onShare?: (r: ShareRequest) => void;
  /** The clip last opened in Studio: scrolled back to and highlighted */
  focusPath?: string | null;
  uploads?: Record<string, UploadProgress>;
  links?: Record<string, Upload[]>;
  clips: ClipFile[] | null;
  onOpen: (path: string) => void;
  onToast: (t: string, err?: boolean) => void;
  onChanged: () => void;
  /** Asks for confirmation, then deletes. Resolves true if deleted. */
  onDelete: (paths: string[]) => Promise<boolean>;
  hotkey: string;
}) {
  const memory = (memories[mode] ??= { scroll: 0, folder: "All", query: "" });
  const damagedMode = mode === "damaged";
  const trimMode = mode === "totrim";
  const nas = useNasState();
  const nasOn = !!nas?.enabled;
  // Filter and search survive a trip to Studio too.
  const [folder, setFolderState] = useState(memory.folder);
  const [query, setQueryState] = useState(memory.query);
  const setFolder = (f: string) => {
    memory.folder = f;
    setFolderState(f);
  };
  const setQuery = (q: string) => {
    memory.query = q;
    setQueryState(q);
  };
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const anchor = useRef<string | null>(null);
  // Gallery / Folders / List, the sort, and filters (view and sort remembered for every list).
  const [viewMode, setViewModeState] = useState<ViewMode>(() => remembered<ViewMode>("viewMode", "gallery"));
  const [sort, setSortState] = useState<Sort>(() => remembered<Sort>("sort", "new"));
  const [filters, setFiltersState] = useState<Filters>(memory.filters ?? NO_FILTERS);
  const [showFilters, setShowFilters] = useState(false);
  const setViewMode = (v: ViewMode) => {
    remember("viewMode", v);
    setViewModeState(v);
    if (v === "folders") setFolder("All");
  };
  const setSort = (v: Sort) => {
    remember("sort", v);
    setSortState(v);
  };
  const setFilters = (p: Partial<Filters>) => {
    const next = { ...filters, ...p };
    memory.filters = next;
    setFiltersState(next);
  };
  const activeFilters = (filters.where ? 1 : 0) + filters.sources.length + (filters.from || filters.to ? 1 : 0) + (filters.minMb ? 1 : 0);
  const sources = useMemo(() => {
    const m = new Map<string, number>();
    for (const c of clips ?? []) m.set(sourceOf(c), (m.get(sourceOf(c)) ?? 0) + 1);
    return [...m].sort((a, b) => b[1] - a[1]);
  }, [clips]);

  // One chip per game, even when recorders spell it differently ("VALORANT" / "Valorant").
  // Hindsight's own folder name wins as the label.
  const folders = useMemo(() => {
    const label = new Map<string, string>();
    for (const c of clips ?? []) {
      const k = gameKey(c.folder);
      if (!label.has(k) || c.location === "local") label.set(k, c.folder);
    }
    const names = [...label.values()];
    return ["All", ...names.sort((a, b) => (a === "Imported" ? 1 : b === "Imported" ? -1 : a.localeCompare(b)))];
  }, [clips]);

  /** Search and filters (not the game chip: Folders view groups by game itself). */
  const matching = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = (clips ?? []).filter((c) => {
      if (q && !`${c.name} ${c.folder} ${c.source}`.toLowerCase().includes(q)) return false;
      if (filters.where === "pc" && c.location === "nas") return false;
      if (filters.where === "nas" && !(c.onNas || c.location === "nas")) return false;
      if (filters.sources.length && !filters.sources.includes(sourceOf(c))) return false;
      const day = ymd(c.modified);
      if (filters.from && day < filters.from) return false;
      if (filters.to && day > filters.to) return false;
      if (filters.minMb && c.size < filters.minMb * 1_048_576) return false;
      return true;
    });
    if (sort === "old") list.reverse();
    if (sort === "big") list.sort((a, b) => b.size - a.size);
    return list;
  }, [clips, query, filters, sort]);

  const groups = useMemo(() => {
    const want = gameKey(folder);
    const list = matching.filter((c) => folder === "All" || gameKey(c.folder) === want);
    if (sort === "big") return list.length ? [{ day: "Biggest first", ts: 0, items: list }] : [];
    const out: { day: string; ts: number; items: ClipFile[] }[] = [];
    for (const c of list) {
      const day = dayLabel(c.modified);
      if (out[out.length - 1]?.day !== day) out.push({ day, ts: c.modified, items: [] });
      out[out.length - 1].items.push(c);
    }
    return out;
  }, [matching, folder, sort]);

  // List view: build the first rows straight away and the rest in small
  // batches, so it shows at once even with hundreds of clips. (The gallery is
  // built only near the screen, below.)
  const total = useMemo(() => groups.reduce((n, g) => n + g.items.length, 0), [groups]);
  const [limit, setLimit] = useState(() => (memory.scroll > 0 ? Infinity : FIRST_CARDS));
  const listKey = `${folder}|${query}|${sort}|${viewMode}|${JSON.stringify(filters)}`;
  const lastKey = useRef(listKey);
  useEffect(() => {
    if (lastKey.current === listKey) return;
    lastKey.current = listKey;
    setLimit(FIRST_CARDS);
  }, [listKey]);
  useEffect(() => {
    if (limit >= total) return;
    const t = window.setTimeout(() => setLimit((l) => l + MORE_CARDS), 0);
    return () => window.clearTimeout(t);
  }, [limit, total]);
  /** The days built so far: each with its whole group (for counts and "Select day") and the cards built. */
  const shownGroups = useMemo(() => {
    let left = limit;
    const out: { g: (typeof groups)[number]; items: ClipFile[] }[] = [];
    for (const g of groups) {
      if (left <= 0) break;
      const items = g.items.length <= left ? g.items : g.items.slice(0, left);
      left -= items.length;
      out.push({ g, items });
    }
    return out;
  }, [groups, limit]);

  // Gallery: only the days near the screen are built. The others are empty
  // placeholders of the right height, so the scrollbar, the date timeline and
  // coming back to a spot halfway down all still work, but leaving and
  // returning to the list (or switching lists) never builds 500+ cards at once.
  // Big days (and "Biggest first") are cut into blocks of BLOCK cards.
  const scroller = useRef<HTMLDivElement>(null);
  const daysTop = useRef<HTMLDivElement>(null);
  const [gridWidth, setGridWidth] = useState(0);
  const measured = useRef(new Map<string, number>());
  const [measureTick, setMeasureTick] = useState(0);
  // How many cards fit in a row: blocks hold whole rows, so a cut never leaves a short row.
  const cols = Math.max(1, Math.floor(((gridWidth || 1200) + CARD_GAP) / (CARD_MIN + CARD_GAP)));
  const blocks = useMemo(() => {
    const size = cols * BLOCK_ROWS;
    const out: { key: string; g: (typeof groups)[number]; items: ClipFile[]; first: boolean }[] = [];
    for (const g of groups) {
      for (let i = 0; i < g.items.length; i += size) {
        out.push({ key: `${g.day}#${size}#${i / size}`, g, items: g.items.slice(i, i + size), first: i === 0 });
      }
    }
    return out;
  }, [groups, cols]);
  /** Each block's top and height, from its measured height or an estimate from the grid's width. */
  const layout = useMemo(() => {
    const W = gridWidth || 1200;
    const cardH = ((W - CARD_GAP * (cols - 1)) / cols) * (9 / 16);
    let y = 0;
    return blocks.map((b, i) => {
      const rows = Math.ceil(b.items.length / cols);
      const guess = (b.first ? DAY_HEAD : 0) + rows * cardH + (rows - 1) * CARD_GAP;
      const h = measured.current.get(`${b.key}|${b.items.length}|${Math.round(W)}`) ?? guess;
      // The rest of a day sits a card gap below; the next day a page gap.
      if (i > 0) y += b.first ? PAGE_GAP : CARD_GAP;
      const top = y;
      y += h;
      return { top, h };
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [blocks, gridWidth, cols, measureTick]);
  /** Which blocks are built: those within a couple of screens of where you are. */
  const rangeFor = useCallback(
    (scrollTop: number, viewH: number) => {
      const start = daysTop.current && scroller.current
        ? daysTop.current.getBoundingClientRect().top - scroller.current.getBoundingClientRect().top + scroller.current.scrollTop
        : 400;
      const lo = scrollTop - start - NEAR;
      const hi = scrollTop - start + viewH + NEAR;
      let from = layout.findIndex((l) => l.top + l.h >= lo);
      if (from < 0) from = Math.max(0, layout.length - 1);
      let to = from;
      while (to + 1 < layout.length && layout[to + 1].top <= hi) to++;
      return [from, to] as const;
    },
    [layout],
  );
  const [range, setRange] = useState<readonly [number, number]>(() => [0, 0]);
  const updateRange = useCallback(() => {
    const sc = scroller.current;
    const top = sc && sc.clientHeight ? sc.scrollTop : memory.scroll;
    const next = rangeFor(top, sc?.clientHeight || 900);
    setRange((r) => (r[0] === next[0] && r[1] === next[1] ? r : next));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rangeFor]);
  useLayoutEffect(() => updateRange(), [updateRange]);
  // The grid's width sets how many cards fit in a row.
  useEffect(() => {
    const el = daysTop.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setGridWidth((w) => (Math.abs(w - el.clientWidth) > 1 ? el.clientWidth : w)));
    ro.observe(el);
    setGridWidth(el.clientWidth);
    return () => ro.disconnect();
  });

  /** Folders view: one per game (same game, any spelling), biggest collections first. */
  const gameFolders = useMemo(() => {
    const by = new Map<string, { name: string; items: ClipFile[] }>();
    for (const c of matching) {
      const k = gameKey(c.folder);
      if (!by.has(k)) by.set(k, { name: c.folder, items: [] });
      const f = by.get(k)!;
      if (c.location === "local") f.name = c.folder;
      f.items.push(c);
    }
    return [...by.values()].sort((a, b) => b.items.length - a.items.length || a.name.localeCompare(b.name));
  }, [matching]);
  const atFolderRoot = viewMode === "folders" && folder === "All";

  const visible = useMemo(() => groups.flatMap((g) => g.items.map((c) => c.path)), [groups]);

  // Coming back from Studio: same scroll position, and the clip you opened glows
  // for a moment (scrolled into view if the list moved, e.g. a new clip arrived).
  const [returnedTo, setReturnedTo] = useState<string | null>(null);
  const restored = useRef(false);
  useLayoutEffect(() => {
    const sc = scroller.current;
    if (!active) {
      restored.current = false;
      return;
    }
    if (!sc || restored.current || !clips?.length) return;
    restored.current = true;
    sc.scrollTop = memory.scroll;
    if (focusPath) {
      const card = sc.querySelector<HTMLElement>(`[data-path="${CSS.escape(focusPath)}"]`);
      if (card) {
        const r = card.getBoundingClientRect();
        const v = sc.getBoundingClientRect();
        if (r.top < v.top + 80 || r.bottom > v.bottom) card.scrollIntoView({ block: "center" });
        setReturnedTo(focusPath);
        setTimeout(() => setReturnedTo(null), 2200);
      }
    }
  }, [clips, focusPath, active]);
  // "Back" again while already on the clips (mouse back button, Alt+←, Esc, or the
  // Clips tab): forget the old spot and go to the top.
  useEffect(() => {
    if (!active) return;
    const top = () => {
      memory.scroll = 0;
      setReturnedTo(null);
      scroller.current?.scrollTo({ top: 0, behavior: "smooth" });
    };
    window.addEventListener("rewind-top", top);
    return () => window.removeEventListener("rewind-top", top);
  }, [active]);
  const scrollFrame = useRef(0);
  const onScroll = () => {
    if (scroller.current && active) memory.scroll = scroller.current.scrollTop;
    cancelAnimationFrame(scrollFrame.current);
    scrollFrame.current = requestAnimationFrame(updateRange);
  };
  // Built blocks remember their real height, so placeholders match them next time.
  useLayoutEffect(() => {
    const page = scroller.current;
    if (!page || !gridWidth) return;
    let changed = false;
    page.querySelectorAll<HTMLElement>("[data-block]").forEach((el) => {
      const k = `${el.dataset.block}|${Math.round(gridWidth)}`;
      const h = el.offsetHeight;
      if (h && Math.abs((measured.current.get(k) ?? 0) - h) > 1) {
        measured.current.set(k, h);
        changed = true;
      }
    });
    if (changed) setMeasureTick((t) => t + 1);
  });
  // The date timeline only makes sense for the gallery in date order.
  const days = useMemo(
    () => (viewMode === "list" || atFolderRoot || sort === "big" ? [] : groups.map((g) => ({ key: g.day, ts: g.ts, count: g.items.length }))),
    [groups, viewMode, atFolderRoot, sort],
  );

  // Drop selections that disappeared (deleted, filtered out).
  useEffect(() => {
    setSelected((s) => {
      const keep = new Set([...s].filter((p) => visible.includes(p)));
      return keep.size === s.size ? s : keep;
    });
  }, [visible]);

  const selecting = selected.size > 0;
  const [previewing, setPreviewing] = useState(false);
  const [healing, setHealing] = useState(false);
  // In the order they appear on the page.
  const selectedClips = useMemo(
    () => (clips ?? []).filter((c) => selected.has(c.path) && visible.includes(c.path)),
    [clips, selected, visible],
  );

  const toggle = useCallback((path: string) => {
    setSelected((s) => {
      const n = new Set(s);
      if (n.has(path)) n.delete(path);
      else n.add(path);
      return n;
    });
    anchor.current = path;
  }, []);

  const selectRange = useCallback(
    (path: string) => {
      const from = anchor.current ? visible.indexOf(anchor.current) : -1;
      const to = visible.indexOf(path);
      if (from < 0 || to < 0) return toggle(path);
      const [a, b] = from < to ? [from, to] : [to, from];
      setSelected((s) => new Set([...s, ...visible.slice(a, b + 1)]));
    },
    [visible, toggle],
  );

  const clickCard = (path: string) => (e: React.MouseEvent) => {
    if (e.shiftKey) selectRange(path);
    else if (e.ctrlKey || e.metaKey || selecting) toggle(path);
    else onOpen(path);
  };

  const deleteSelected = useCallback(async () => {
    if (!selected.size) return;
    if (await onDelete([...selected])) setSelected(new Set());
  }, [selected, onDelete]);

  // Delete removes the selection, Ctrl+A selects everything shown, Esc clears.
  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement).tagName;
      if (tag === "INPUT" || tag === "SELECT" || tag === "TEXTAREA") return;
      if (document.querySelector(".modal-backdrop")) return;
      if (e.key === "Delete" && selected.size) {
        e.preventDefault();
        deleteSelected();
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a" && visible.length) {
        e.preventDefault();
        setSelected(new Set(visible));
      } else if (e.key === "Escape" && selected.size) {
        setSelected(new Set());
      } else if (e.key === "Escape") {
        window.dispatchEvent(new Event("rewind-top"));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selected, visible, deleteSelected, active]);

  const openAny = async () => {
    const picked = await open({
      multiple: true,
      title: "Open videos from anywhere",
      filters: [{ name: "Videos", extensions: VIDEO_EXTENSIONS }],
    });
    if (!picked) return;
    const paths = Array.isArray(picked) ? picked : [picked];
    try {
      await api.addExternal(paths);
      if (paths.length === 1) onOpen(paths[0]);
    } catch (e) {
      onToast(errText(e), true);
    }
  };

  const selectedBytes = (clips ?? []).filter((c) => selected.has(c.path)).reduce((n, c) => n + c.size, 0);

  // The cards themselves, built again only when something they show changes:
  // showing or hiding the list (Settings and back) doesn't rebuild 500+ cards.
  const cards = useMemo(
    () => (
clips === null ? null : clips.length === 0 && trimMode ? (
        <div className="empty">
          <strong>Nothing left to trim</strong>
          <span>Every saved clip has been trimmed, or marked “Keep whole”. New clips show up here until you trim them.</span>
        </div>
      ) : clips.length === 0 && damagedMode ? (
        <div className="empty">
          <strong>No damaged clips</strong>
          <span>Every clip checked so far plays properly. Anything that doesn't will show up here.</span>
        </div>
      ) : clips.length === 0 ? (
        <div className="empty">
          <strong>No clips yet</strong>
          <span>
            Play something and press <span className="kbd">{hotkey}</span> to save the last moments. Or open a video from
            anywhere on your PC to edit it.
          </span>
          <button className="btn" onClick={openAny}>
            Open any video…
          </button>
        </div>
      ) : (atFolderRoot ? gameFolders.length === 0 : groups.length === 0) ? (
        <div className="empty">
          <strong>Nothing matches</strong>
          {activeFilters > 0 && (
            <button className="btn" onClick={() => setFilters(NO_FILTERS)}>
              Clear filters
            </button>
          )}
        </div>
      ) : atFolderRoot ? (
        <div className="folder-grid">
          {gameFolders.map((f) => (
            <FolderTile key={f.name} name={f.name} items={f.items} onOpen={() => setFolder(f.name)} />
          ))}
        </div>
      ) : viewMode === "list" ? (
        <div className="clip-rows">
          {shownGroups.flatMap((s) => s.items).map((c) => (
            <ClipRow key={c.path} clip={c} selected={selected.has(c.path)} onClick={clickCard(c.path)} damage={health?.[c.path]} nasOn={nasOn} copying={nas?.copying} />
          ))}
        </div>
      ) : (
        <>
          <div ref={daysTop} className="days-top" aria-hidden="true" />
          {blocks.map((blk, i) => {
            const { g, items: built, first } = blk;
            if (i < range[0] || i > range[1]) {
              return <section key={blk.key} data-day={first ? g.day : undefined} className={`day day-ph ${first ? "" : "day-more"}`} style={{ height: layout[i]?.h }} aria-hidden="true" />;
            }
            const allInDay = g.items.every((c) => selected.has(c.path));
            return (
              <section key={blk.key} data-day={first ? g.day : undefined} data-block={`${blk.key}|${built.length}`} className={`day ${first ? "" : "day-more"}`}>
                {first && (
                  <div className="day-head">
                    <h3>{g.day}</h3>
                    <span className="muted">
                      {g.items.length} clip{g.items.length === 1 ? "" : "s"}
                    </span>
                    {selecting && (
                      <button
                        className="btn small"
                        onClick={() =>
                          setSelected((s) => {
                            const n = new Set(s);
                            g.items.forEach((c) => (allInDay ? n.delete(c.path) : n.add(c.path)));
                            return n;
                          })
                        }
                      >
                        {allInDay ? "Unselect day" : "Select day"}
                      </button>
                    )}
                  </div>
                )}
                <div className="grid">
                  {built.map((c) => (
                    <ClipCard
                      nasOn={nasOn}
                      copying={nas?.copying}
                      key={c.path}
                      clip={c}
                      selected={selected.has(c.path)}
                      onClick={clickCard(c.path)}
                      onToggle={(e) => (e.shiftKey ? selectRange(c.path) : toggle(c.path))}
                      onDelete={() => onDelete(selected.has(c.path) ? [...selected] : [c.path])}
                      deleteCount={selected.has(c.path) ? selected.size : 1}
                      onToast={onToast}
                      upload={uploads?.[c.path]}
                      link={liveLink(links?.[c.path])}
                      damage={health?.[c.path]}
                      onShare={() => onShare?.({ path: c.path })}
                      returned={c.path === returnedTo}
                      onKeepWhole={onKeepWhole ? () => onKeepWhole([c.path]) : undefined}
                    />
                  ))}
                </div>
              </section>
            );
          })}
        </>
      )
    ),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [clips, trimMode, damagedMode, hotkey, atFolderRoot, gameFolders, groups.length, activeFilters, viewMode, shownGroups, blocks, layout, range, selected, selecting, health, nasOn, nas?.copying, uploads, links, returnedTo, onKeepWhole, onDelete, onToast, onShare, onOpen, selectRange, toggle],
  );

  return (
    <div className={`lib ${active ? "" : "inactive"}`} aria-hidden={!active || undefined} inert={!active || undefined}>
      {previewing && selecting && (
        <DeletePreview
          clips={selectedClips}
          onKeep={(p) =>
            setSelected((s) => {
              const n = new Set(s);
              n.delete(p);
              return n;
            })
          }
          onClose={() => setPreviewing(false)}
          onDelete={async () => {
            setPreviewing(false);
            await deleteSelected();
          }}
        />
      )}
    <div className={`page lib-page ${selecting ? "selecting" : ""}`} ref={scroller} onScroll={onScroll}>
      <div className="toolbar">
        <h1 style={{ flex: 1, display: "flex", alignItems: "center", gap: 10, minWidth: 0 }}>
          {folder !== "All" && <GameIcon name={folder} size={26} />}
          <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {viewMode === "folders" && folder !== "All" ? folder : TITLES[mode] ?? "Clips"}
          </span>
          {folder !== "All" && <LogoButton game={folder} onToast={onToast} />}
        </h1>
        <label className="search">
          <span className="muted" style={{ fontSize: 12 }}>
            Search
          </span>
          <input placeholder="clips, games…" value={query} onChange={(e) => setQuery(e.target.value)} />
        </label>
        <div className="seg" role="group" aria-label="View">
          <SlidingPill />
          {(["gallery", "folders", "list"] as ViewMode[]).map((v) => (
            <button key={v} className={viewMode === v ? "on" : ""} onClick={() => setViewMode(v)}>
              {v === "gallery" ? "Gallery" : v === "folders" ? "Folders" : "List"}
            </button>
          ))}
        </div>
        <button className={`btn ${showFilters ? "on" : ""}`} onClick={() => setShowFilters((s) => !s)} aria-expanded={showFilters}>
          Filters{activeFilters > 0 && <span className="count">{activeFilters}</span>}
        </button>
        <select className="select" value={sort} onChange={(e) => setSort(e.target.value as Sort)} aria-label="Sort">
          <option value="new">Newest first</option>
          <option value="old">Oldest first</option>
          <option value="big">Biggest first</option>
        </select>
        {mode === "clips" && (
          <button className="btn" onClick={openAny}>
            Open any video…
          </button>
        )}
        {damagedMode && !!clips?.length && (
          <button className="btn primary" onClick={() => setHealing(true)} title="Go through them one by one: watch what's kept, then heal or skip each">
            Review and heal ({clips.length})…
          </button>
        )}
      </div>
      {damagedMode && !!clips?.length && <RepairAll clips={clips} onToast={onToast} />}
      {healing && clips && health && <HealReview clips={clips} health={health} onClose={() => setHealing(false)} onToast={onToast} />}
      {showFilters && (
        <div className="lib-filters">
          {mode === "clips" && (
            <div className="fgroup">
              <span className="label">Where</span>
              <SlidingPill />
              {(
                [
                  ["", "Anywhere"],
                  ["pc", "This PC"],
                  ["nas", "NAS"],
                ] as [Filters["where"], string][]
              ).map(([v, l]) => (
                <button key={v} className={`chip small ${filters.where === v ? "on" : ""}`} onClick={() => setFilters({ where: v })}>
                  {l}
                </button>
              ))}
            </div>
          )}
          {sources.length > 1 && (
            <div className="fgroup">
              <span className="label">Made with</span>
              {sources.map(([s, n]) => (
                <button
                  key={s}
                  className={`chip small ${filters.sources.includes(s) ? "on" : ""}`}
                  onClick={() => setFilters({ sources: filters.sources.includes(s) ? filters.sources.filter((x) => x !== s) : [...filters.sources, s] })}
                >
                  {s} <small className="muted">{n}</small>
                </button>
              ))}
            </div>
          )}
          <div className="fgroup">
            <span className="label">Date</span>
            <input type="date" className="text-input date" value={filters.from} onChange={(e) => setFilters({ from: e.target.value })} aria-label="From" />
            <span className="muted">to</span>
            <input type="date" className="text-input date" value={filters.to} onChange={(e) => setFilters({ to: e.target.value })} aria-label="To" />
          </div>
          <div className="fgroup">
            <span className="label">Size</span>
            <SlidingPill />
            {[0, 100, 500, 1024].map((mb) => (
              <button key={mb} className={`chip small ${filters.minMb === mb ? "on" : ""}`} onClick={() => setFilters({ minMb: mb })}>
                {mb === 0 ? "Any" : mb >= 1024 ? "Over 1 GB" : `Over ${mb} MB`}
              </button>
            ))}
            {activeFilters > 0 && (
              <button className="btn small" style={{ marginLeft: "auto" }} onClick={() => setFilters(NO_FILTERS)}>
                Clear filters
              </button>
            )}
          </div>
        </div>
      )}
      {damagedMode && <DamagedStatus count={clips?.length ?? 0} />}
      {trimMode && !!clips?.length && <TrimStatus clips={clips} />}
      {mode === "clips" && viewMode === "gallery" && !selecting && !query && folder === "All" && activeFilters === 0 && clips && (
        <OnThisDay clips={clips} onOpenClip={onOpen} />
      )}

      {selecting ? (
        <div className="select-bar" role="toolbar" aria-label="Selected clips">
          <strong style={{ fontSize: 13, padding: "0 6px" }}>
            {selected.size} selected <span className="muted">· {fmtSize(selectedBytes)}</span>
          </strong>
          <span style={{ flex: 1 }} />
          <button className="btn small" onClick={() => setSelected(new Set(visible))}>
            Select all ({visible.length})
          </button>
          <button className="btn small" onClick={() => setSelected(new Set())}>
            Clear
          </button>
          {onKeepWhole && (
            <button
              className="btn small"
              onClick={() => {
                onKeepWhole([...selected]);
                setSelected(new Set());
              }}
              title="They're fine as they are: take them off this list"
            >
              Keep whole
            </button>
          )}
          <button className="btn small" onClick={() => setPreviewing(true)} title="Look through the selected clips before deleting them">
            Preview
          </button>
          <button className="btn small danger" onClick={deleteSelected} title="Move the selected clips to the Recycle Bin (Delete)">
            Delete {selected.size} clip{selected.size === 1 ? "" : "s"}
          </button>
        </div>
      ) : (
        folders.length > 1 && !atFolderRoot && (
          <div className="row game-row">
            <GamePicker clips={clips ?? []} folder={folder} setFolder={setFolder} />
            <span className="muted" style={{ fontSize: 12, marginLeft: "auto" }}>
              Tip: Ctrl+click or Shift+click to select several
            </span>
          </div>
        )
      )}

      {cards}
    </div>
    <Timeline scroller={scroller} days={days} />
    </div>
  );
}

/**
 * To trim: how much these clips take, and about how much trimming each to its
 * best 30 seconds would free (from their lengths, worked out in the background).
 */
function TrimStatus({ clips }: { clips: ClipFile[] }) {
  const [lens, setLens] = useState<Record<string, number>>({});
  useEffect(() => {
    let alive = true;
    const t = setTimeout(async () => {
      for (const c of clips) {
        if (!alive) return;
        try {
          const p = await api.probe(c.path, String(c.modified));
          if (alive) setLens((l) => ({ ...l, [c.path]: p.duration }));
        } catch {
          /* unreadable: left out of the estimate */
        }
      }
    }, 600);
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [clips]);
  const bytes = clips.reduce((n, c) => n + c.size, 0);
  let freed = 0;
  let known = 0;
  for (const c of clips) {
    const d = lens[c.path];
    if (!d) continue;
    known++;
    if (d > 30) freed += c.size * (1 - 30 / d);
  }
  return (
    <div className="trim-status">
      <strong>
        {clips.length} clip{clips.length === 1 ? "" : "s"} not trimmed yet · {fmtSize(bytes)}
      </strong>
      <span className="muted">
        {known === 0
          ? "Working out how much trimming would save…"
          : `Trimming each to its best 30 seconds would free about ${fmtSize(freed * (clips.length / known))}${known < clips.length ? " (estimate so far)" : ""}.`}{" "}
        Open one to trim it, or choose “Keep whole” on clips that are fine as they are.
      </span>
    </div>
  );
}

/**
 * Damaged: try to repair them all, one after another. Each repair writes a
 * "(repaired)" copy next to the clip and leaves the original as it is, so
 * nothing is lost; check the copies, then delete whichever you don't want.
 */
function RepairAll({ clips, onToast }: { clips: ClipFile[]; onToast: (t: string, err?: boolean) => void }) {
  const [at, setAt] = useState<number | null>(null);
  const stop = useRef(false);
  const todo = clips.filter((c) => !/\(repaired\)/i.test(c.name));
  const run = async () => {
    stop.current = false;
    let ok = 0;
    let failed = 0;
    for (let i = 0; i < todo.length; i++) {
      if (stop.current) break;
      setAt(i);
      try {
        await api.repairClip(todo[i].path);
        ok++;
      } catch {
        failed++;
      }
    }
    setAt(null);
    onToast(
      `Repaired ${ok} clip${ok === 1 ? "" : "s"}${failed ? `, ${failed} couldn't be` : ""}`,
      failed > 0 && ok === 0,
    );
  };
  if (!todo.length) return null;
  return (
    <div className="repair-all">
      {at == null ? (
        <>
          <span className="muted">
            Repair re-encodes each clip into a “(repaired)” copy next to it. The originals stay as they are until you delete them.
          </span>
          <button className="btn small" onClick={run}>
            Repair all {todo.length}
          </button>
        </>
      ) : (
        <>
          <span>
            Repairing {at + 1} of {todo.length}: <span className="muted">{todo[at]?.name}</span>
          </span>
          <span className="dl-bar" style={{ flex: 1, maxWidth: 240 }}>
            <span style={{ width: `${(at / todo.length) * 100}%` }} />
          </span>
          <button className="btn small" onClick={() => (stop.current = true)}>
            Stop after this one
          </button>
        </>
      )}
    </div>
  );
}

/** Pick a picture as this game's logo (for games with no logo, or the wrong one). */
function LogoButton({ game, onToast }: { game: string; onToast: (t: string, err?: boolean) => void }) {
  const [menu, setMenu] = useState(false);
  const pick = async () => {
    setMenu(false);
    const file = await open({ multiple: false, title: `Logo for ${game}`, filters: [{ name: "Pictures", extensions: ["png", "jpg", "jpeg", "webp"] }] });
    if (!file || Array.isArray(file)) return;
    try {
      await api.setGameLogo(game, file);
      onToast(`New logo for ${game}`);
    } catch (e) {
      onToast(errText(e), true);
    }
  };
  const reset = async () => {
    setMenu(false);
    try {
      await api.setGameLogo(game, null);
      onToast(`${game} uses its automatic logo again`);
    } catch (e) {
      onToast(errText(e), true);
    }
  };
  return (
    <span className="logo-btn">
      <button className="btn small" onClick={() => setMenu((m) => !m)} aria-expanded={menu} title="Change this game's logo">
        Logo…
      </button>
      {menu && (
        <span className="logo-menu" role="menu">
          <button role="menuitem" className="more-item" onClick={pick}>
            Choose a picture…
          </button>
          <button role="menuitem" className="more-item" onClick={reset}>
            Use the automatic logo
          </button>
        </span>
      )}
    </span>
  );
}
