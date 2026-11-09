import { useEffect, useMemo, useState } from "react";
import { api, ClipFile, errText, fmtSize, Settings, DupeSet } from "./api";
import { ConfirmDialog } from "./ConfirmUpdate";
import { useThumb } from "./hooks";

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
        {(busy || err) && <small className={err ? "err-text" : "muted"}>{err ?? busy}</small>}
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
            run("Shrinking… this can take a minute", onShrink)();
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

/** The same clip saved more than once: pick the copy to keep, the rest go to the Recycle Bin. */
function Duplicates({ clips, onDelete, onChanged }: { clips: ClipFile[]; onDelete: (paths: string[]) => Promise<boolean>; onChanged: () => void }) {
  const [sets, setSets] = useState<DupeSet[] | null>(null);
  const [keep, setKeep] = useState<Record<number, string>>({});
  const byPath = useMemo(() => new Map(clips.map((c) => [c.path, c])), [clips]);
  useEffect(() => {
    let alive = true;
    api.findDuplicates().then((s) => alive && setSets(s)).catch(() => alive && setSets([]));
    return () => {
      alive = false;
    };
  }, [clips]);
  if (!sets) return <p className="muted">Looking for duplicates…</p>;
  const found = sets
    .map((s) => ({ kind: s.kind, clips: s.paths.map((p) => byPath.get(p)).filter((c): c is ClipFile => !!c) }))
    .filter((s) => s.clips.length > 1);
  if (!found.length) return null;
  // What removing all but the kept copy would free.
  const extra = found.reduce((n, s) => {
    const kept = bestCopy(s.clips, s.kind);
    return n + s.clips.reduce((m, c) => m + (c.path === kept.path ? 0 : c.size), 0);
  }, 0);
  const remuxes = found.filter((s) => s.kind === "remux").length;
  const smaller = found.filter((s) => s.kind === "smaller").length;
  return (
    <section className="dupes">
      <h2>Duplicates</h2>
      <p className="muted" style={{ margin: 0 }}>
        {found.length} clip{found.length === 1 ? " is" : "s are"} saved more than once, using {fmtSize(extra)} extra.
        {remuxes > 0 &&
          ` ${remuxes === found.length ? "They're" : `${remuxes} of them are`} the same recording in two formats (OBS keeps the original when it remuxes to MP4): same name, length and tracks.`}
        {smaller > 0 &&
          ` ${smaller === found.length ? (smaller === 1 ? "It's" : "They're") : `${smaller} ${smaller === 1 ? "is" : "are"}`} the same recording next to a smaller re-encode of it (an export, say): the original is kept unless you pick the smaller copy.`}{" "}
        Pick the copy to keep; the others go to the Recycle Bin.
      </p>
      {found.map(({ kind, clips: set }, i) => {
        const chosen = keep[i] ?? bestCopy(set, kind).path;
        return (
          <div className="dupe-set" key={set[0].path}>
            <div className="dupe-head">
              <strong>{set[0].name}</strong>
              <span className="muted">
                {kind === "remux"
                  ? `Same recording as ${set.map((c) => extOf(c.path)).join(" and ")}`
                  : kind === "smaller"
                    ? `Same recording · the smaller copy is ${Math.round((Math.min(...set.map((c) => c.size)) / Math.max(...set.map((c) => c.size))) * 100)}% of the size`
                    : `${set.length} copies · ${fmtSize(set[0].size)} each`}
              </span>
            </div>
            {set.map((c) => (
              <label key={c.path} className={`dupe-copy ${chosen === c.path ? "on" : ""}`}>
                <input type="radio" name={`dupe-${i}`} checked={chosen === c.path} onChange={() => setKeep((k) => ({ ...k, [i]: c.path }))} />
                <span className="dupe-where">
                  {kind === "remux"
                    ? `${extOf(c.path)} · ${fmtSize(c.size)}`
                    : kind === "smaller"
                      ? `${extOf(c.path)} · ${fmtSize(c.size)}${c.size === Math.max(...set.map((x) => x.size)) ? " · original" : " · re-encoded"}`
                      : placeOf(c)}
                </span>
                <small className="muted dupe-path" title={c.path}>
                  {c.path}
                </small>
              </label>
            ))}
            <div className="row" style={{ justifyContent: "flex-end" }}>
              <button
                className="btn small danger"
                onClick={async () => {
                  const gone = set.filter((c) => c.path !== chosen).map((c) => c.path);
                  if (await onDelete(gone)) onChanged();
                }}
              >
                Keep the selected copy, remove {set.length - 1}
              </button>
            </div>
          </div>
        );
      })}
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

  return (
    <div className="page cleanup-page">
      <div className="toolbar">
        <h1 style={{ flex: 1 }}>Cleanup</h1>
        <button className="btn primary" onClick={() => update({ cleanupKept: {} })} title="List every clip again, including the ones you kept">
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
      <Duplicates clips={all} onDelete={onDelete} onChanged={onChanged} />
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
