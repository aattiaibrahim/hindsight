import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState } from "react";
import { api, ClipFolder, errText, FoundFolder, fmtSize, LastMove, MovePlan, NasState, OldFolder, Settings } from "./api";
import { ConfirmDialog } from "./ConfirmUpdate";
import { NasProgress, useNasState } from "./NasProgress";
import { useDetectStatus } from "./GameSort";
import { HealthSetting } from "./Health";
import { NasSetup } from "./NasSetup";
import { OldFolderDialog } from "./OldFolder";
import { MixTracksSetting } from "./MixTracksSetting";

type MoveJob = {
  folders: string[];
  plan: MovePlan | null;
  running: boolean;
  done: number;
  total: number;
  /** A folder found on a drive, copied or moved in (not one of the linked folders) */
  importOf?: { path: string; copy: boolean };
};

function outcomeText(verb: string, o: { moved: number; failed: [string, string][] }) {
  const base = `${verb} ${o.moved} clip${o.moved === 1 ? "" : "s"}.`;
  if (!o.failed.length) return base;
  const [file, why] = o.failed[0];
  return `${base} ${o.failed.length} couldn't be: ${file.split("\\").pop()} (${why})${o.failed.length > 1 ? " and others" : ""}.`;
}

function ago(ms: number) {
  const s = Math.round((Date.now() - ms) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  return new Date(ms).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
}

function nasLine(s: Settings, n: NasState | null) {
  if (!s.nasDir) return "Off. Clips stay on this PC only.";
  if (!n || !n.enabled) return "Starting…";
  if (!n.reachable) return `Can't reach it right now. Clips keep saving here and copy over once it's back. (${n.lastError ?? ""})`;
  if (n.paused)
    return `Paused while you play${n.pending ? ` (${n.pending} clip${n.pending === 1 ? "" : "s"} waiting)` : ""}. Copying starts again once the game is closed.`;
  if (n.copying) return n.checking ? "Checking the copy matches the original…" : "Copying…";
  const parts = [n.pending ? `${n.pending} clip${n.pending === 1 ? "" : "s"} waiting to copy` : "Everything is copied"];
  if (n.lastSync) parts.push(`checked ${ago(n.lastSync)}`);
  if (n.lastError) parts.push(`problem: ${n.lastError}`);
  return parts.join(" · ");
}

/** Settings → Library: the NAS folder and other recorders' folders. */
export function LibrarySettings({
  s,
  set,
  Toggle,
  hidden,
}: {
  hidden?: boolean;
  s: Settings;
  set: (p: Partial<Settings>) => void;
  Toggle: (p: { label: string; checked: boolean; onChange: (v: boolean) => void }) => React.ReactElement;
}) {
  const nas = useNasState();
  const [found, setFound] = useState<ClipFolder[]>([]);
  // Recorder folders with no clips left: offered for deleting right on their row.
  const [empties, setEmpties] = useState<OldFolder[]>([]);
  const [emptyAsk, setEmptyAsk] = useState<OldFolder | null>(null);
  const refreshEmpties = () => api.oldFolders().then((l) => setEmpties(Array.isArray(l) ? l : [])).catch(() => {});
  useEffect(() => {
    refreshEmpties();
    const un = listen("clips-changed", () => refreshEmpties());
    return () => {
      un.then((f) => f());
    };
  }, [s.linkedFolders.join("|")]); // eslint-disable-line react-hooks/exhaustive-deps
  const emptyOf = (p: string) => empties.find((f) => f.path.toLowerCase() === p.toLowerCase());
  const deleteButton = (p: string) => {
    const f = emptyOf(p);
    return f ? (
      <button className="btn small" onClick={() => setEmptyAsk(f)} title="No clips are left in this folder. Sends it to the Recycle Bin.">
        Delete folder…
      </button>
    ) : null;
  };
  // A folder picked for the NAS, being checked before anything syncs.
  const [checking, setChecking] = useState<string | null>(null);
  useEffect(() => {
    api.recorderClipFolders().then(setFound).catch(() => {});
  }, []);

  const pickNas = async () => {
    const dir = await open({ directory: true, title: "Pick the NAS folder for clips (e.g. \\\\nas\\clips)", defaultPath: s.nasDir || undefined });
    if (typeof dir === "string") setChecking(dir);
  };
  const linked = (p: string) => s.linkedFolders.some((f) => f.toLowerCase() === p.toLowerCase());
  const toggleLinked = (p: string, on: boolean) =>
    set({ linkedFolders: on ? [...s.linkedFolders, p] : s.linkedFolders.filter((f) => f.toLowerCase() !== p.toLowerCase()) });
  const addFolder = async () => {
    const dir = await open({ directory: true, title: "Show the clips in this folder in Hindsight" });
    if (typeof dir === "string" && !linked(dir)) set({ linkedFolders: [...s.linkedFolders, dir] });
  };
  const custom = s.linkedFolders.filter((f) => !found.some((k) => k.path.toLowerCase() === f.toLowerCase()));

  // "Move into Hindsight": plan first (nothing touched), confirm, then move with progress.
  const [job, setJob] = useState<MoveJob | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [last, setLast] = useState<LastMove | null>(null);
  const refreshLast = useCallback(() => {
    api.lastMove().then(setLast).catch(() => {});
  }, []);
  useEffect(() => {
    refreshLast();
    const un = listen<[number, number]>("move-progress", (e) =>
      setJob((j) => (j ? { ...j, done: e.payload[0], total: e.payload[1] } : j)),
    );
    return () => {
      un.then((f) => f());
    };
  }, [refreshLast]);
  const askMove = async (folders: string[]) => {
    setNote(null);
    setJob({ folders, plan: null, running: false, done: 0, total: 0 });
    try {
      const plan = await api.planMoveIntoRewind(folders);
      if (!plan.count) {
        setJob(null);
        setNote("Nothing to move: no finished clips in that folder.");
      } else setJob({ folders, plan, running: false, done: 0, total: plan.count });
    } catch (e) {
      setJob(null);
      setNote(errText(e));
    }
  };
  // Clips on other drives (an external HDD, an old Medal folder): look, then show, copy or move.
  const [drives, setDrives] = useState<FoundFolder[] | null>(null);
  const [looking, setLooking] = useState(false);
  const lookOnDrives = async () => {
    setLooking(true);
    setNote(null);
    try {
      setDrives(await api.findClipFolders());
    } catch (e) {
      setNote(errText(e));
    } finally {
      setLooking(false);
    }
  };
  const askImport = async (path: string, copy: boolean) => {
    setNote(null);
    setJob({ folders: [], plan: null, running: false, done: 0, total: 0, importOf: { path, copy } });
    try {
      const plan = await api.planImport(path, copy);
      if (!plan.count) {
        setJob(null);
        setNote("Nothing to bring in: no finished clips in that folder.");
      } else setJob({ folders: [], plan, running: false, done: 0, total: plan.count, importOf: { path, copy } });
    } catch (e) {
      setJob(null);
      setNote(errText(e));
    }
  };

  const doMove = async () => {
    if (!job?.plan) return;
    setJob({ ...job, running: true });
    try {
      if (job.importOf) {
        setNote(outcomeText(job.importOf.copy ? "Copied" : "Moved", await api.importFolder(job.importOf.path, job.importOf.copy)));
        api.findClipFolders().then(setDrives).catch(() => {});
        return;
      }
      setNote(outcomeText("Moved", await api.moveIntoRewind(job.folders)));
    } catch (e) {
      setNote(errText(e));
    } finally {
      setJob(null);
      refreshLast();
    }
  };
  const undo = async () => {
    try {
      setNote(outcomeText("Put back", await api.undoLastMove()));
    } catch (e) {
      setNote(errText(e));
    } finally {
      refreshLast();
    }
  };
  const closeMove = useCallback(() => setJob((j) => (j?.running ? j : null)), []);
  const moveButton = (path: string) =>
    linked(path) && (
      <button className="btn small" disabled={!!job} onClick={() => askMove([path])} title="Move these clips into Hindsight's game folders">
        Move into Hindsight…
      </button>
    );

  return (
    <section className="group" hidden={hidden}>
      <h2>Library &amp; NAS</h2>
      <div className="setting">
        <span>
          NAS folder
          <small style={{ wordBreak: "break-all" }}>{s.nasDir || "Not set"}</small>
          <small>
            Clips save to this PC first (instant, works offline), then copy to the NAS in the background into the same game
            folders, only once you've closed your game so it never costs frames. Every copy is read back from the NAS and
            checked byte for byte before it counts. Several PCs can share one NAS: file names
            include each PC's name.
          </small>
          <small className={nas && s.nasDir && !nas.reachable && nas.enabled ? "err-text" : ""}>{nasLine(s, nas)}</small>
          <NasProgress n={nas} />
        </span>
        <div className="row" style={{ flexWrap: "nowrap", flexShrink: 0 }}>
          {s.nasDir && (
            <button className="btn small" onClick={() => api.syncNasNow()} title="Copy anything waiting right now">
              Sync now
            </button>
          )}
          {s.nasDir && (
            <button className="btn small" onClick={() => set({ nasDir: "" })}>
              Turn off
            </button>
          )}
          <button className="btn small" onClick={pickNas}>
            {s.nasDir ? "Change…" : "Choose…"}
          </button>
        </div>
      </div>
      {emptyAsk && (
        <OldFolderDialog
          folder={emptyAsk}
          onClose={() => setEmptyAsk(null)}
          onDone={() => {
            setEmptyAsk(null);
            refreshEmpties();
          }}
        />
      )}
      {checking && (
        <NasSetup
          dir={checking}
          server={s.nasServer}
          onStart={async (p) => {
            await api.setSettings({ ...(await api.getSettings()), ...p });
          }}
          onClose={() => setChecking(null)}
        />
      )}
      {s.nasDir && (
        <label className="setting">
          <span>
            Free up space on this PC
            <small>
              Once a clip's NAS copy is checked identical to it, the copy here goes to the Recycle Bin. Off keeps both.
            </small>
          </span>
          <Toggle label="Remove local copy" checked={s.nasRemoveLocal} onChange={(v) => set({ nasRemoveLocal: v })} />
        </label>
      )}

      {s.nasDir && s.linkedFolders.length > 0 && (
        <div className="setting sub">
          <span>
            Clips from other recorders aren't copied
            <small>
              The {s.linkedFolders.length} other recorder folder{s.linkedFolders.length === 1 ? "" : "s"} below (Medal, NVIDIA,
              OBS…) only show in your library, they stay where they are. Move them into Hindsight's folders first and they'll
              copy to the NAS like your own clips{s.nasRemoveLocal ? ", then be removed from this PC" : ""}.
            </small>
          </span>
          <button className="btn small" disabled={!!job} onClick={() => askMove(s.linkedFolders)}>
            Move into Hindsight…
          </button>
        </div>
      )}

      <DetectSetting s={s} set={set} Toggle={Toggle} />

      <label className="setting">
        <span>
          Prepare thumbnails ahead of time
          <small>
            Makes every clip's thumbnail and length in the background, so the library opens instantly even with clips on a
            slow hard drive. They're kept on this PC's app folder. Runs only while no game is running.
          </small>
        </span>
        <Toggle label="Prepare thumbnails ahead of time" checked={s.prepareThumbnails} onChange={(v) => set({ prepareThumbnails: v })} />
      </label>

      <MixTracksSetting nasPaired={!!s.nasServer} />

      <label className="setting">
        <span>
          Online game logos
          <small>
            Shows each game's official icon from Steam. Only the game's folder name is sent to Steam's store search, once per
            game, never while you're playing. Games that aren't on Steam (and everything, with this off) use the icon of the
            game's own program on this PC.
          </small>
        </span>
        <Toggle label="Online game logos" checked={s.onlineGameLogos} onChange={(v) => set({ onlineGameLogos: v })} />
      </label>

      <HealthSetting on={s.healthCheck} set={(v) => set({ healthCheck: v })} Toggle={Toggle} />
      <div className="setting">
        <span>
          Clips from other recorders
          <small>
            Shows their clips in your library, grouped by game, without moving or copying anything. Move them into Hindsight's
            folders to tidy everything into one place.
          </small>
        </span>
        <span className="row" style={{ flexWrap: "nowrap", flexShrink: 0 }}>
          {s.linkedFolders.length > 1 && (
            <button
              className="btn small primary"
              disabled={!!job}
              onClick={() => askMove(s.linkedFolders)}
              title="Move every shown folder's clips into Hindsight's game folders in one go"
            >
              Move all into Hindsight…
            </button>
          )}
          <button className="btn small" onClick={addFolder}>
            Add folder…
          </button>
        </span>
      </div>
      {/* A recorder's folder that's gone isn't worth a row (unless it's still switched on, so it can be switched off). */}
      {found.filter((f) => f.exists || linked(f.path)).map((f) => (
        <div className="setting sub" key={f.path}>
          <span>
            {f.label}
            <small style={{ wordBreak: "break-all" }}>
              {f.exists ? f.path : `${f.path} (not found)`}
              {emptyOf(f.path) ? " · no clips left in it" : ""}
            </small>
          </span>
          <span className="row" style={{ flexWrap: "nowrap", flexShrink: 0 }}>
            {deleteButton(f.path) ?? moveButton(f.path)}
            <Toggle label={`Show ${f.label} clips`} checked={linked(f.path)} onChange={(v) => toggleLinked(f.path, v)} />
          </span>
        </div>
      ))}
      {custom.map((p) => (
        <div className="setting sub" key={p}>
          <span>
            {p.split("\\").filter(Boolean).pop()}
            <small style={{ wordBreak: "break-all" }}>
              {p}
              {emptyOf(p) ? " · no clips left in it" : ""}
            </small>
          </span>
          <span className="row" style={{ flexWrap: "nowrap", flexShrink: 0 }}>
            {deleteButton(p) ?? moveButton(p)}
            <button className="btn small" onClick={() => toggleLinked(p, false)} title="Stops showing them. Files are not touched.">
              Remove
            </button>
          </span>
        </div>
      ))}
      <div className="setting">
        <span>
          Clips on your drives
          <small>
            Looks through every drive, an external HDD too, for folders with clips, like an old Medal folder or a
            "Recordings" folder. Shows how many clips and how much space each has before anything is copied or moved.
          </small>
        </span>
        <button className="btn small" disabled={looking || !!job} onClick={lookOnDrives}>
          {looking ? "Looking…" : drives ? "Look again" : "Look for clips…"}
        </button>
      </div>
      {drives && !drives.length && (
        <div className="setting sub">
          <span>
            <small>No clip folders found on your drives. Use Add folder… above to pick one yourself.</small>
          </span>
        </div>
      )}
      {drives?.map((f) => (
        <div className="setting sub" key={f.path}>
          <span>
            {f.label} <span className="muted">on {f.drive}</span>
            <small style={{ wordBreak: "break-all" }}>
              {f.path} · {f.clips} clip{f.clips === 1 ? "" : "s"} · {fmtSize(f.bytes)}
            </small>
          </span>
          <span className="row" style={{ flexWrap: "nowrap", flexShrink: 0 }}>
            {!f.shown && !linked(f.path) && (
              <button className="btn small" onClick={() => toggleLinked(f.path, true)} title="Show them in your library where they are. Nothing is copied.">
                Show
              </button>
            )}
            <button className="btn small" disabled={!!job} onClick={() => askImport(f.path, true)} title="Copy them into Hindsight's game folders; the originals stay on the drive">
              Copy in…
            </button>
            <button className="btn small" disabled={!!job} onClick={() => askImport(f.path, false)} title="Move them into Hindsight's game folders">
              Move in…
            </button>
          </span>
        </div>
      ))}
      {s.similarDismissed.length > 0 && (
        <div className="setting sub">
          <span>
            {s.similarDismissed.length} game{s.similarDismissed.length === 1 ? "" : "s"} kept under separate folder names
            <small>You chose Keep separate. Ask again to see the merge option for {s.similarDismissed.length === 1 ? "it" : "them"}.</small>
          </span>
          <button className="btn small" onClick={() => api.keepFoldersSeparate(null).catch((e) => setNote(errText(e)))}>
            Ask again
          </button>
        </div>
      )}
      {last && (
        <div className="setting sub">
          <span>
            Last move: {last.count} clip{last.count === 1 ? "" : "s"} from {last.label}
            <small>{new Date(last.when).toLocaleString()} · Undo puts them back where they were, with their old names.</small>
          </span>
          <button className="btn small" disabled={!!job} onClick={undo}>
            Undo
          </button>
        </div>
      )}
      {note && (
        <div className="setting sub">
          <span>
            <small>{note}</small>
          </span>
        </div>
      )}
      {job?.plan && (
        <ConfirmDialog
          title={`${job.plan.copy ? "Copy" : "Move"} ${job.plan.count} ${job.plan.label} clip${job.plan.count === 1 ? "" : "s"} (${fmtSize(job.plan.bytes)}) into Hindsight?`}
          confirmLabel={
            job.running
              ? `${job.plan.copy ? "Copying" : "Moving"} ${Math.min(job.done + 1, job.total)} of ${job.total}…`
              : `${job.plan.copy ? "Copy" : "Move"} ${fmtSize(job.plan.bytes)}`
          }
          busy={job.running}
          onConfirm={doMove}
          onCancel={closeMove}
        >
          <ul className="modal-list wrap">
            <li>
              Into {job.plan.folders.slice(0, 4).map(([f, n]) => `${f} (${n})`).join(", ")}
              {job.plan.folders.length > 4 ? ` and ${job.plan.folders.length - 4} more folders` : ""}, renamed like Hindsight's
              clips, e.g. “{job.plan.examples[0]?.[0]}” → “{job.plan.examples[0]?.[1]}”.
            </li>
            {job.plan.copy ? (
              <>
                <li>The originals stay where they are; Hindsight gets its own copy, checked by size. This needs {fmtSize(job.plan.bytes)} free on this PC.</li>
                <li>You can undo it afterwards: the copies are removed, the originals were never touched.</li>
              </>
            ) : (
              <>
                <li>
                  {job.plan.otherDrive
                    ? "It's on another drive, so each clip is copied, checked, and only then removed from the old folder. This can take a while."
                    : "Same drive, so it's instant: nothing is copied."}
                </li>
                <li>{job.plan.label} won't find these clips in its own app anymore.</li>
                <li>You can undo it afterwards: everything goes back where it was.</li>
              </>
            )}
          </ul>
        </ConfirmDialog>
      )}
      {!found.length && !custom.length && (
        <div className="setting sub">
          <span>
            <small>No Medal, OBS or NVIDIA folders found. Use Add folder… for any other recorder.</small>
          </span>
        </div>
      )}
    </section>
  );
}

/** Settings → Library: finding the game in unsorted clips from the picture. */
function DetectSetting({
  s,
  set,
  Toggle,
}: {
  s: Settings;
  set: (p: Partial<Settings>) => void;
  Toggle: (p: { label: string; checked: boolean; onChange: (v: boolean) => void }) => React.ReactElement;
}) {
  const st = useDetectStatus();
  const [dl, setDl] = useState<DetectStep | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    const un = listen<DetectStep>("detect-progress", (e) => setDl(e.payload));
    return () => {
      un.then((f) => f());
    };
  }, []);
  const install = async () => {
    setErr(null);
    setDl({ stage: "downloading", done: 0, total: 0 });
    try {
      await api.detectInstall();
      setDl({ stage: "ready", done: 1, total: 1 });
      setTimeout(() => setDl((d) => (d?.stage === "ready" ? null : d)), 4000);
    } catch (e) {
      setErr(errText(e));
      setDl(null);
    }
  };
  const busy = !!dl && dl.stage !== "ready";
  const mb = (b: number) => (b / 1_048_576).toFixed(0);
  const stepText = !dl
    ? null
    : dl.stage === "downloading"
      ? dl.total
        ? `Downloading… ${mb(dl.done)} of ${mb(dl.total)} MB (${Math.round((dl.done / dl.total) * 100)}%)`
        : "Starting the download…"
      : dl.stage === "checking"
        ? "Checking the files…"
        : dl.stage === "installing"
          ? "Installing: starting the model once to check it works…"
          : "Installed. Looking through your unsorted clips now.";
  const status = !s.gameDetect
    ? "Off."
    : !st?.installed
      ? "Needs a one-time download first."
      : st.toLookAt
        ? `Looked at ${st.lookedAt} of ${st.toLookAt} clips${st.lookedAt < st.toLookAt ? " (only while you're not gaming)" : ""}.`
        : "Starting…";
  return (
    <div className="setting">
      <span>
        Find the game in unsorted clips
        <small>
          For clips in folders that don't say which game they're from (OBS, Desktop, Imports, other recorders), Hindsight looks at a
          few frames and compares them with your sorted clips, then suggests a game for you to check. It runs on this PC only,
          while you're not gaming. Needs a one-time download of an image model (about 106 MB).
        </small>
        {dl ? (
          <span className="dl-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={dl.total ? Math.round((dl.done / dl.total) * 100) : 0} aria-label="Download">
            <span className={`dl-bar ${dl.stage !== "downloading" ? "full" : ""} ${dl.stage === "checking" || dl.stage === "installing" ? "working" : ""}`}>
              <span style={{ width: `${dl.stage === "downloading" ? (dl.total ? (dl.done / dl.total) * 100 : 0) : 100}%` }} />
            </span>
            <small>{stepText}</small>
          </span>
        ) : (
          <small>{status}</small>
        )}
        {err && <small className="err-text">{err}</small>}
      </span>
      <span className="row" style={{ flexWrap: "nowrap", flexShrink: 0 }}>
        {s.gameDetect && st && !st.installed && (
          <button className="btn small primary" disabled={busy} onClick={install}>
            {busy ? (dl?.stage === "downloading" ? "Downloading…" : "Installing…") : "Download (106 MB)"}
          </button>
        )}
        <Toggle label="Find the game in unsorted clips" checked={s.gameDetect} onChange={(v) => set({ gameDetect: v })} />
      </span>
    </div>
  );
}

type DetectStep = { stage: "downloading" | "checking" | "installing" | "ready"; done: number; total: number };
