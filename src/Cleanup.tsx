import { useEffect, useMemo, useState } from "react";
import { api, ClipFile, errText, fmtSize, Settings, DupeSet } from "./api";
import { ConfirmDialog } from "./ConfirmUpdate";
import { useThumb } from "./hooks";
import { Icon } from "./icons";
import { ProgressBar, useCountProgress, useEditProgress } from "./Progress";

const LIMIT = 40;

function when(ms: number) {
  return new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

function Row({
  clip,
  onOpen,
  onKeep,
  onShrink,
  onDelete,
  onFree,
}: {
  clip: ClipFile;
  onOpen: () => void;
  onKeep: () => Promise<void>;
  onShrink: () => Promise<void>;
  onDelete: () => Promise<boolean>;
  /** Remove this PC's copy (only for clips that are also on the NAS) */
  onFree: () => Promise<void>;
}) {
  const [ask, setAsk] = useState<"shrink" | "free" | null>(null);
  const thumb = useThumb(clip.path, clip.modified);
  const [busy, setBusy] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const nasOnly = clip.location === "nas";
  // Shrinking re-encodes the whole clip: show how far it's got.
  const pct = useEditProgress(clip.path, !!busy && busy.startsWith("Shrinking"));
  const run = (label: string, f: () => Promise<unknown>) => async () => {
    setBusy(label);
    setErr(null);
    try {
      await f();
    } catch (e) {
      setErr(errText(e));
    } finally {
      setBusy(null);
    }
  };
  return (
    <div className="clean-row">
      <button className="clean-thumb" onClick={onOpen} title="Open in Studio">
        {thumb && <img src={thumb} alt="" loading="lazy" />}
      </button>
      <div className="clean-main">
        <div className="card-title">{clip.name}</div>
        <div className="card-meta">
          {clip.folder} · {when(clip.modified)}
          {clip.location === "linked" && ` · ${clip.source}`}
          {nasOnly && " · only on the NAS"}
          {clip.onNas && " · also on the NAS"}
        </div>
        {(busy || err) && (
          <small className={err ? "err-text" : "muted"}>
            {err ?? (busy?.startsWith("Shrinking") ? `Shrinking… ${pct ?? 0}%` : busy)}
          </small>
        )}
        {busy?.startsWith("Shrinking") && !err && <ProgressBar pct={pct} />}
      </div>
      <div className="clean-size">{fmtSize(clip.size)}</div>
      <div className="row" style={{ flexWrap: "nowrap" }}>
        <button className="btn small" disabled={!!busy} onClick={run("Keeping…", onKeep)} title="Hide it until the next cleanup check">
          Keep
        </button>
        {clip.onNas ? (
          // Already safe on the NAS: free the space here instead of lowering its quality everywhere.
          <button className="btn small" disabled={!!busy} onClick={() => setAsk("free")} title="It's on the NAS: remove only this PC's copy (checked identical first)">
            Remove from this PC
          </button>
        ) : (
          <button
            className="btn small"
            disabled={!!busy || nasOnly}
            onClick={() => setAsk("shrink")}
            title={nasOnly ? "It's only on the NAS: use Cleanup in the NAS app" : "Re-encode it smaller (lower quality); asks first"}
          >
            Shrink
          </button>
        )}
        <button
          className="btn small danger"
          disabled={!!busy || nasOnly}
          onClick={run("Deleting…", onDelete)}
          title={nasOnly ? "It's only on the NAS: use Cleanup in the NAS app (it has a 30-day trash)" : "Move it to the Recycle Bin"}
        >
          Delete
        </button>
      </div>
      {ask === "shrink" && (
        <ConfirmDialog
          title={`Shrink ${clip.name}?`}
          confirmLabel="Shrink it"
          onCancel={() => setAsk(null)}
          onConfirm={() => {
            setAsk(null);
            run("Shrinking", onShrink)();
          }}
        >
          <ul className="modal-list wrap">
            <li>It's re-encoded to about 40% of its size ({fmtSize(clip.size * 0.4)}), so it will look a little worse. This can't be undone on the new file.</li>
            <li>The original goes to the Recycle Bin, so you can still get it back from there.</li>
          </ul>
        </ConfirmDialog>
      )}
      {ask === "free" && (
        <ConfirmDialog
          title={`Remove ${clip.name} from this PC?`}
          confirmLabel="Remove from this PC"
          onCancel={() => setAsk(null)}
          onConfirm={() => {
            setAsk(null);
            run("Checking the NAS copy…", onFree)();
          }}
        >
          <ul className="modal-list wrap">
            <li>Hindsight first reads the NAS copy and checks it's identical, byte for byte. If it isn't, nothing is removed.</li>
            <li>This PC's copy goes to the Recycle Bin. The NAS copy is never touched, and you can still watch it in the library.</li>
          </ul>
        </ConfirmDialog>
      )}
    </div>
  );
}

/** Where a copy lives, in words. */
function placeOf(c: ClipFile) {
  if (c.location === "nas") return "NAS";
  if (c.location === "linked") return c.source || "Other recorder";
  return c.onNas ? "Hindsight (also on the NAS)" : "Hindsight";
}

/** Which copy to keep by default: Hindsight's own, then the NAS, then the oldest name.
 *  For a remux (the same recording as MKV and MP4), the MP4: it plays everywhere.
 *  For a smaller re-encode, the original (the biggest): it's the full quality. */
function bestCopy(set: ClipFile[], kind: DupeSet["kind"] = "copy") {
  if (kind === "smaller") return [...set].sort((a, b) => b.size - a.size)[0];
  const rank = (c: ClipFile) => (c.location === "local" ? 0 : c.location === "nas" ? 1 : 2);
  const mp4 = (c: ClipFile) => (kind === "remux" && /\.mp4$/i.test(c.path) ? 0 : 1);
  return [...set].sort((a, b) => mp4(a) - mp4(b) || rank(a) - rank(b) || a.path.length - b.path.length)[0];
}

const extOf = (p: string) => (p.split(".").pop() ?? "").toUpperCase();

/** The kinds of duplicate, in the order they're shown, and what's kept by default. */
const DUPE_GROUPS: { kind: DupeSet["kind"]; title: string; keeps: string }[] = [
  { kind: "remux", title: "Same recording in two formats", keeps: "keeps the MP4, which plays everywhere" },
  { kind: "smaller", title: "Next to a smaller re-encode", keeps: "keeps the original, at full quality" },
  { kind: "copy", title: "Exact copies", keeps: "keeps Hindsight's own copy" },
];

type Dupe = { id: string; kind: DupeSet["kind"]; clips: ClipFile[] };

/** A copy's label: its format for a remux, original or re-encode for a smaller copy, where it lives for an exact copy. */
function copyLabel(c: ClipFile, d: Dupe) {
  if (d.kind === "remux") return extOf(c.path);
  if (d.kind === "smaller") return c.size === Math.max(...d.clips.map((x) => x.size)) ? `Original ${extOf(c.path)}` : `Re-encode ${extOf(c.path)}`;
  return placeOf(c);
}

function DupeThumb({ clip }: { clip: ClipFile }) {
  const src = useThumb(clip.path, clip.modified);
  return <div className={`dupe-thumb ${src ? "" : "loading"}`}>{src && <img src={src} alt="" />}</div>;
}

/** The same clip saved more than once. A summary with one button removes every
 *  extra copy (the better one of each is kept); each group opens to change which
 *  copy is kept, and "Review one by one" walks through them side by side. Nothing
 *  is removed until that one button is pressed (and confirmed). */
function Duplicates({ clips, onDelete, onChanged, run = 0 }: { clips: ClipFile[]; onDelete: (paths: string[]) => Promise<boolean>; onChanged: () => void; run?: number }) {
  const [sets, setSets] = useState<DupeSet[] | null>(null);
  const progress = useCountProgress("dupes-progress", sets === null);
  const [keep, setKeep] = useState<Record<string, string>>({});
  const [both, setBoth] = useState<Set<string>>(new Set());
  const [open, setOpen] = useState<Set<DupeSet["kind"]>>(new Set());
  const [review, setReview] = useState<number | null>(null);
  const byPath = useMemo(() => new Map(clips.map((c) => [c.path, c])), [clips]);
  // Searched again only when the clips really changed (the list is re-read whenever
  // the window gets focus) or on Run cleanup check.
  const key = clips.map((c) => `${c.path}|${c.size}`).join("\n");
  useEffect(() => {
    let alive = true;
    setSets(null);
    setReview(null);
    api.findDuplicates().then((s) => alive && setSets(s)).catch(() => alive && setSets([]));
    return () => {
      alive = false;
    };
  }, [key, run]);
  if (!sets) {
    const pct = progress && progress[1] > 0 ? Math.round((progress[0] / progress[1]) * 100) : null;
    return (
      <div className="job-progress muted">
        {progress && progress[1] > 0 ? `Looking for duplicates… ${progress[0]} of ${progress[1]} clips checked (${pct}%)` : "Looking for duplicates…"}
        <ProgressBar pct={pct} />
      </div>
    );
  }
  const found: Dupe[] = sets
    .map((s) => ({ id: [...s.paths].sort().join("\n"), kind: s.kind, clips: s.paths.map((p) => byPath.get(p)).filter((c): c is ClipFile => !!c) }))
    .filter((s) => s.clips.length > 1);
  if (!found.length) return null;
  const kept = (d: Dupe) => keep[d.id] ?? bestCopy(d.clips, d.kind).path;
  const extras = (d: Dupe) => d.clips.filter((c) => c.path !== kept(d));
  const freed = (d: Dupe) => extras(d).reduce((n, c) => n + c.size, 0);
  const todo = found.filter((d) => !both.has(d.id));
  const gone = todo.flatMap((d) => extras(d).map((c) => c.path));
  const total = todo.reduce((n, d) => n + freed(d), 0);
  const toggle = <T,>(s: Set<T>, v: T) => {
    const n = new Set(s);
    if (n.has(v)) n.delete(v);
    else n.add(v);
    return n;
  };
  const removeAll = async () => {
    if (gone.length && (await onDelete(gone))) onChanged();
  };

  const summary = (
    <div className="dupe-summary">
      <div>
        <h2>Duplicates</h2>
        <div className="dupe-total">
          {found.length} clip{found.length === 1 ? "" : "s"} saved more than once · {fmtSize(total)} extra
        </div>
        <p className="muted">
          The better copy of each is kept. The others go to the Recycle Bin, or the NAS app's Trash for 30 days if they're on the NAS.
          {both.size > 0 && ` You're keeping both copies of ${both.size} ${both.size === 1 ? "clip" : "clips"}.`}
        </p>
      </div>
      <div className="row dupe-actions">
        {review === null && (
          <button className="btn" onClick={() => setReview(0)}>
            Review one by one
          </button>
        )}
        <button className="btn primary" disabled={!gone.length} onClick={removeAll}>
          Remove {gone.length} cop{gone.length === 1 ? "y" : "ies"}
        </button>
      </div>
    </div>
  );

  if (review !== null) {
    const i = Math.min(review, found.length - 1);
    const d = found[i];
    const chosen = kept(d);
    const group = DUPE_GROUPS.find((g) => g.kind === d.kind);
    const next = () => setReview(i + 1 >= found.length ? null : i + 1);
    return (
      <section className="dupes">
        {summary}
        <div className="dupe-review">
          <div className="dupe-review-head">
            <span className="muted">
              {i + 1} of {found.length} · {group?.title.toLowerCase()}
            </span>
            <div className="row">
              <button className="btn small" disabled={i === 0} onClick={() => setReview(i - 1)}>
                Back
              </button>
              <button
                className="btn small"
                onClick={() => {
                  setBoth((s) => toggle(s, d.id));
                  next();
                }}
              >
                {both.has(d.id) ? "Don't keep both" : "Keep both"}
              </button>
              <button
                className="btn small primary"
                onClick={() => {
                  setBoth((s) => {
                    const n = new Set(s);
                    n.delete(d.id);
                    return n;
                  });
                  next();
                }}
              >
                Keep {copyLabel(byPath.get(chosen) ?? d.clips[0], d)}
              </button>
              <button className="btn small" onClick={() => setReview(null)}>
                Done
              </button>
            </div>
          </div>
          <strong className="dupe-name">{d.clips[0].name}</strong>
          <div className="dupe-cards">
            {d.clips.map((c) => {
              const on = c.path === chosen && !both.has(d.id);
              return (
                <button key={c.path} className={`dupe-card ${on ? "on" : ""}`} onClick={() => setKeep((k) => ({ ...k, [d.id]: c.path }))}>
                  <DupeThumb clip={c} />
                  <span className="dupe-card-label">
                    {copyLabel(c, d)} <small className={on ? "accent" : "muted"}>{both.has(d.id) ? "keep" : c.path === chosen ? "keep" : "remove"}</small>
                  </span>
                  <small className="muted">
                    {fmtSize(c.size)} · {when(c.modified)} · {placeOf(c)}
                  </small>
                  <small className="muted dupe-path" title={c.path}>
                    {c.path}
                  </small>
                </button>
              );
            })}
          </div>
          <p className="muted small">Click a copy to keep it instead. Nothing is removed until you press Remove.</p>
        </div>
      </section>
    );
  }

  return (
    <section className="dupes">
      {summary}
      <div className="dupe-groups">
        {DUPE_GROUPS.map((g) => {
          const items = found.filter((d) => d.kind === g.kind);
          if (!items.length) return null;
          const isOpen = open.has(g.kind);
          const size = items.filter((d) => !both.has(d.id)).reduce((n, d) => n + freed(d), 0);
          return (
            <div key={g.kind} className="dupe-group">
              <button className="dupe-group-head" aria-expanded={isOpen} onClick={() => setOpen((s) => toggle(s, g.kind))}>
                <Icon name="chevron" size={16} className={`dupe-chev ${isOpen ? "open" : ""}`} />
                <span>{g.title}</span>
                <span className="muted">
                  {items.length} · {g.keeps}
                </span>
                <span className="dupe-size">{fmtSize(size)}</span>
              </button>
              {isOpen &&
                items.map((d) => {
                  const chosen = kept(d);
                  const off = both.has(d.id);
                  return (
                    <div key={d.id} className={`dupe-row ${off ? "off" : ""}`}>
                      <span className="dupe-row-name" title={d.clips[0].path}>
                        {d.clips[0].name}
                      </span>
                      <span className="dupe-tags">
                        {d.clips.map((c) => (
                          <button
                            key={c.path}
                            className={`dupe-tag ${!off && c.path === chosen ? "on" : ""} ${!off && c.path !== chosen ? "gone" : ""}`}
                            title={`${off ? "Kept" : c.path === chosen ? "Kept" : "Removed"}: ${c.path}. Click to keep this one.`}
                            onClick={() => setKeep((k) => ({ ...k, [d.id]: c.path }))}
                          >
                            {copyLabel(c, d)}
                          </button>
                        ))}
                      </span>
                      <span className="muted dupe-size">{off ? "kept both" : fmtSize(freed(d))}</span>
                      <button className="btn small" onClick={() => setBoth((s) => toggle(s, d.id))}>
                        {off ? "Remove extra" : "Keep both"}
                      </button>
                    </div>
                  );
                })}
            </div>
          );
        })}
      </div>
    </section>
  );
}

/**
 * Cleanup: the biggest clips, so you can free up space. Delete (Recycle Bin), Shrink
 * (re-encode smaller, original to the Recycle Bin) or Keep (hidden until the next
 * cleanup check).
 */
export function Cleanup({
  clips,
  settings,
  update,
  onOpen,
  onDelete,
  onToast,
  onChanged,
}: {
  clips: ClipFile[] | null;
  settings: Settings;
  update: (p: Partial<Settings>) => void;
  onOpen: (path: string) => void;
  onDelete: (paths: string[]) => Promise<boolean>;
  onToast: (t: string, err?: boolean) => void;
  onChanged: () => void;
}) {
  const kept = settings.cleanupKept ?? {};
  const all = clips ?? [];
  const total = all.reduce((n, c) => n + c.size, 0);
  const keptCount = all.filter((c) => kept[c.path] === c.size).length;
  const biggest = useMemo(
    () => [...(clips ?? [])].filter((c) => kept[c.path] !== c.size).sort((a, b) => b.size - a.size).slice(0, LIMIT),
    [clips, kept],
  );
  const shown = biggest.reduce((n, c) => n + c.size, 0);
  const here = all.filter((c) => c.location !== "nas").reduce((n, c) => n + c.size, 0);
  // "Run cleanup check" also looks for duplicates again.
  const [checkRun, setCheckRun] = useState(0);

  return (
    <div className="page cleanup-page">
      <div className="toolbar">
        <h1 style={{ flex: 1 }}>Cleanup</h1>
        <button
          className="btn primary"
          onClick={() => {
            update({ cleanupKept: {} });
            setCheckRun((n) => n + 1);
          }}
          title="List every clip again, including the ones you kept, and look for duplicates again"
        >
          Run cleanup check
        </button>
      </div>
      <p className="muted" style={{ margin: 0, maxWidth: 760, lineHeight: 1.5 }}>
        Your clips use <strong>{fmtSize(total)}</strong>
        {here !== total && <> ({fmtSize(here)} on this PC)</>}.{" "}
        {biggest.length
          ? `These ${biggest.length} biggest ones take ${fmtSize(shown)}. `
          : "Nothing left to look at: every big clip has been kept or cleaned up. "}
        Delete sends a clip to the Recycle Bin (never for clips that are only on the NAS), Shrink re-encodes it to about 40%
        of the size after asking, Remove from this PC frees a clip that's safely on the NAS, and Keep hides it until your next
        cleanup check. Nothing is deleted if it couldn't really go to the Recycle Bin.
        {keptCount > 0 && ` ${keptCount} kept since the last check.`}
      </p>
      <Duplicates clips={all} onDelete={onDelete} onChanged={onChanged} run={checkRun} />
      {biggest.length > 0 && <h2 style={{ marginBottom: 0 }}>Biggest clips</h2>}
      <div className="clean-list">
        {biggest.map((c) => (
          <Row
            key={c.path}
            clip={c}
            onOpen={() => onOpen(c.path)}
            onKeep={async () => update({ cleanupKept: { ...kept, [c.path]: c.size } })}
            onShrink={async () => {
              const before = c.size;
              await api.shrinkClip(c.path);
              onToast(`Shrank ${c.name} (it was ${fmtSize(before)})`);
              onChanged();
            }}
            onFree={async () => {
              await api.freeLocalCopy(c.path);
              onToast(`Removed ${c.name} from this PC`, false);
              onChanged();
            }}
            onDelete={async () => {
              const done = await onDelete([c.path]);
              if (done) onChanged();
              return done;
            }}
          />
        ))}
      </div>
    </div>
  );
}
