import { listen } from "@tauri-apps/api/event";
import { requestNasSignIn } from "./NasConnect";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ShareConfirm, ShareRequest, useUploads } from "./Share";
import { useHealth } from "./Health";
import type { UploadProgress } from "./api";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import "./App.css";
import { api, ClipFile, errText, forDesktop, GameSuggestion } from "./api";
import { finished, leave, started, stay } from "./activity";
import { StatusBar } from "./StatusBar";
import { Icon, IconName } from "./icons";
import { Cleanup } from "./Cleanup";
import { ConfirmDelete } from "./ConfirmDelete";
import { ConfirmUpdate, updateInterrupts } from "./ConfirmUpdate";
import { MergeDialog, spellings, useSimilarFolders } from "./FolderMerge";
import { Notice, NotificationsPage, useNoticeState } from "./Notifications";
import { GameSortDialog, useGameSuggestions } from "./GameSort";
import { useNasState } from "./NasProgress";
import type { MoveOutcome, OldFolder } from "./api";
import { fmtSize } from "./api";
import { OldFolderDialog } from "./OldFolder";
import { useAppearance, useClips, useMediaReady, useSettings, useStatus, useUpdate } from "./hooks";
import { Library } from "./Library";
import { Performance } from "./Performance";
import logo from "./logo.svg";
import { SettingsView } from "./SettingsView";
import { Studio } from "./Studio";
import { SlidingPill } from "./SlidingPill";
import { MontageView } from "./Montage";

type View = "clips" | "local" | "synced" | "totrim" | "damaged" | "cleanup" | "montage" | "notifications" | "studio" | "performance" | "settings";
/** The clip lists (Studio goes back to whichever you came from). */
type ListView = "clips" | "local" | "synced" | "totrim" | "damaged";
const LIST_VIEWS: ListView[] = ["clips", "local", "synced", "totrim", "damaged"];
const isList = (v: View): v is ListView => v === "clips" || v === "local" || v === "synced" || v === "totrim" || v === "damaged";

/** Files an edit made (trims, exports, repairs…), not clips waiting for one. */
/** Replay buffer lengths (Hindsight's, and other recorders' usual ones), in seconds. */
const BUFFER_LENGTHS = [60, 120, 180, 300, 600, 1200];
/**
 * Is this length a whole buffer save, not cut down yet? A save is as long as the buffer
 * it came from (2, 3, 5 minutes…), or a few seconds less (it comes out at 176-179 s
 * for a 3-minute buffer). Anything else was trimmed, here or in another app. 0 =
 * length not known yet: counts as untrimmed until it is.
 */
export function isWholeSave(seconds: number, buffer?: number) {
  if (seconds <= 0) return true;
  return [...BUFFER_LENGTHS, ...(buffer ? [buffer] : [])].some((l) => seconds >= l - 6 && seconds <= l + 2);
}

/** Hindsight's recordings ("Valorant Recording 2026-…"), any length. */
const RECORDING = /(^|\s)Recording(\s|$)/i;

const MADE_BY_EDIT = /\((trim|remux|H\.264|repaired|audio|\d+ MB)( \(\d+\))?\)|\((trimmed|good part)\)/i;
const EDIT_FOLDERS = new Set(["edits", "exports", "montages"]);
type ToastAction = { label: string; run: () => void };
type Toast = { id: number; text: string; sub?: string; err?: boolean; action?: ToastAction };
type PendingDelete = { paths: string[]; resolve: (deleted: boolean) => void };

/** Hindsight's own game folders (not other recorders', not the NAS) every clip of which is in `paths`. */
function wholeFoldersOf(paths: string[], clips: ClipFile[]): string[] {
  const picked = new Set(paths);
  const by = new Map<string, ClipFile[]>();
  for (const c of clips) if (c.location === "local" && !c.external) (by.get(c.folder) ?? by.set(c.folder, []).get(c.folder)!).push(c);
  return [...by.entries()].filter(([, list]) => list.length > 0 && list.every((c) => picked.has(c.path))).map(([f]) => f);
}

export default function App() {
  const [view, setView] = useState<View>("clips");
  const [studioPath, setStudioPath] = useState<string | null>(null);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const nextId = useRef(1);
  const status = useStatus();
  const [settings, update] = useSettings();
  useAppearance(settings);
  const [clips, refresh] = useClips();
  // Everything that plays (Studio, previews) re-renders once the media server's address is known.
  useMediaReady();
  const uploads = useUploads();
  const health = useHealth(clips);
  const [shareReq, setShareReq] = useState<ShareRequest | null>(null);
  const upd = useUpdate();
  const [installing, setInstalling] = useState(false);
  // Hiding an update notice is remembered by the notification center.
  const [pendingDelete, setPendingDelete] = useState<PendingDelete | null>(null);
  const [deleting, setDeleting] = useState(false);

  const toast = useCallback((text: string, err = false, sub?: string, action?: ToastAction) => {
    const id = nextId.current++;
    setToasts((t) => [...t.slice(-2), { id, text, err, sub, action }]);
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), err || action ? 8000 : 4500);
  }, []);
  const dropToast = (id: number) => setToasts((t) => t.filter((x) => x.id !== id));

  // Same game under different folder names (this PC vs the NAS): a notice once,
  // a banner until it's dealt with, and the merge dialog.
  const [similar] = useSimilarFolders();
  const [mergeOpen, setMergeOpen] = useState(false);
  const onMerged = useCallback(
    (target: string, out: MoveOutcome) => {
      if (out.failed.length) {
        const [file, why] = out.failed[0];
        toast(`${out.failed.length} file${out.failed.length === 1 ? "" : "s"} couldn't be moved into “${target}”`, true, `${file.split("\\").pop()}: ${why}`);
      } else {
        const undo = () =>
          api
            .undoLastMove()
            .then(() => toast("Put the folders back as they were"))
            .catch((e) => toast(errText(e), true));
        toast(`Merged into “${target}”`, false, "On this PC and the NAS. New clips go there too.", { label: "Undo", run: undo });
      }
    },
    [toast],
  );
  // Share: ask first (unless turned off in Settings → Sharing), then upload.
  // Share always asks first: nothing uploads until Upload is pressed in the dialog.
  const requestShare = useCallback((r: ShareRequest) => setShareReq(r), []);

  const showBanner =
    upd && (upd.stage === "ready" || upd.stage === "available") && upd.version;
  const [askUpdate, setAskUpdate] = useState(false);
  const interrupts = updateInterrupts(status);
  const installNow = async () => {
    setInstalling(true);
    try {
      await api.installUpdate();
    } catch (e) {
      toast(errText(e), true);
      setInstalling(false);
      setAskUpdate(false);
    }
  };
  // Restarting would stop a recording or pause capture mid-game: say so and ask first.
  const requestInstall = () => (interrupts.length ? setAskUpdate(true) : installNow());
  const closeAskUpdate = useCallback(() => setAskUpdate(false), []);

  const openInStudio = useCallback((path: string) => {
    setStudioPath(path);
    setView("studio");
  }, []);

  /** Ask first; resolves true if the clips were deleted. */
  const requestDelete = useCallback(
    (paths: string[]) => new Promise<boolean>((resolve) => setPendingDelete({ paths, resolve })),
    [],
  );

  const confirmDelete = async () => {
    if (!pendingDelete) return;
    const { paths, resolve } = pendingDelete;
    setDeleting(true);
    // Let go of a clip that's open in Studio, or Windows won't let it be moved.
    if (studioPath && paths.includes(studioPath)) {
      setStudioPath(null);
      await new Promise((r) => setTimeout(r, 200));
    }
    leave(paths);
    try {
      const res = await api.deleteClips(paths);
      stay(res.failed.map(([p]) => p));
      const n = res.deleted.length;
      if (n) {
        const restore = () =>
          api
            .restoreClips(res.deleted)
            .then((k) => toast(`Restored ${k} clip${k === 1 ? "" : "s"}`))
            .catch((e) => toast(errText(e), true));
        toast(`Deleted ${n} clip${n === 1 ? "" : "s"}`, false, undefined, { label: "Undo", run: restore });
      }
      if (res.failed.length) {
        const [path, why] = res.failed[0];
        toast(
          `Couldn't delete ${res.failed.length} clip${res.failed.length === 1 ? "" : "s"}`,
          true,
          `${path.split("\\").pop()}: ${why}`,
        );
      }
      refresh();
      resolve(n > 0);
    } catch (e) {
      stay(paths);
      toast(errText(e), true);
      resolve(false);
    } finally {
      setDeleting(false);
      setPendingDelete(null);
    }
  };

  const cancelDelete = useCallback(() => {
    setPendingDelete((p) => {
      p?.resolve(false);
      return null;
    });
  }, []);

  useEffect(() => {
    const a = listen<string>("clip-saved", (e) =>
      toast("Clip saved", false, e.payload.split("\\").pop(), { label: "Trim", run: () => openInStudio(e.payload) }),
    );
    const b = listen<string>("clip-error", (e) => toast(e.payload, true));
    const c = listen<string>("open-in-studio", (e) => openInStudio(e.payload));
    const d = listen<View>("show-view", (e) => setView(e.payload));
    // Uploads (also automatic ones): say when the link is ready, or why it failed.
    const u = listen<UploadProgress>("upload-progress", (e) => {
      const p = e.payload;
      const name = p.path.split("\\").pop();
      if (p.stage === "done" && p.url) {
        const url = p.url;
        toast(p.warning ? "Link copied, but it won't open on this network" : "Link copied", !!p.warning, p.warning ?? `${name} · ${url.replace(/^https?:\/\//, "").split("?")[0]}`, {
          label: "Open",
          run: () => openUrl(url).catch(() => {}),
        });
      } else if (p.stage === "error") {
        toast(p.error ?? "Upload failed", true, name);
      }
    });
    return () => {
      [a, b, c, d, u].forEach((p) => p.then((f) => f()));
    };
  }, [toast, openInStudio]);

  // Studio has no tab of its own: it opens when you click a clip. Performance
  // only shows in Benchmark mode (Settings → Advanced).
  // Damaged clips (found by the background health check), for their own section.
  const damagedClips = useMemo(
    () => (clips ? clips.filter((c) => health[c.path] && (health[c.path].state === "damaged" || health[c.path].state === "unreadable")) : null),
    [clips, health],
  );

  // Duplicates on this PC (worked out ahead of time, so this is quick).
  const [dupes, setDupes] = useState(0);
  // Only when the clips really changed: the list is re-read every time the window
  // gets focus, and a search each time read the whole library again for nothing.
  const dupesKey = useMemo(() => (clips ? clips.map((c) => `${c.path}|${c.size}`).join("\n") : null), [clips]);
  useEffect(() => {
    if (dupesKey === null) return;
    const t = setTimeout(() => api.findDuplicates().then((s) => setDupes(s.length)).catch(() => {}), 4000);
    return () => clearTimeout(t);
  }, [dupesKey]);
  const nasNow = useNasState();
  const [sortList] = useGameSuggestions(!!settings?.gameDetect);
  const [sortOpen, setSortOpen] = useState(false);
  // A game folder's check: its finds, reviewed in the same dialog (instead of the finder's list).
  const [folderFinds, setFolderFinds] = useState<{ list: GameSuggestion[]; heading: string } | null>(null);
  // Recorder folders that moving into Hindsight emptied: offer to remove them.
  const [oldFolders, setOldFolders] = useState<OldFolder[]>([]);
  const [oldAsk, setOldAsk] = useState<OldFolder | null>(null);
  useEffect(() => {
    if (!clips) return;
    const t = setTimeout(() => api.oldFolders().then((l) => setOldFolders(Array.isArray(l) ? l : [])).catch(() => {}), 1500);
    return () => clearTimeout(t);
  }, [clips, settings?.linkedFolders]);
  // Everything that used to be a banner or a pop-up, in the notification center.
  const notices: Notice[] = [];
  if (showBanner && upd?.version)
    notices.push({
      id: `update:${upd.version}`,
      kind: "update",
      title: `Hindsight ${upd.version} is ${upd.stage === "ready" ? "ready" : "available"}`,
      detail: status?.recording ? "Restarting stops and saves your recording." : "It also installs by itself the next time Hindsight closes.",
      tone: "good",
      actions: [{ label: "Restart to update", run: () => requestInstall(), primary: true }],
    });
  if (similar.length)
    notices.push({
      id: `similar:${similar.map((s) => s.key).join(",")}`,
      kind: "folders",
      title: similar.length === 1 ? "One game is saved under different folder names" : `${similar.length} games are saved under different folder names`,
      detail: spellings(similar[0]) + (similar.length > 1 ? ", and more" : ""),
      actions: [{ label: "Review", run: () => setMergeOpen(true), primary: true }],
    });
  if (damagedClips?.length)
    notices.push({
      id: `damaged:${damagedClips.length}`,
      kind: "damaged",
      title: `${damagedClips.length} damaged clip${damagedClips.length === 1 ? "" : "s"}`,
      detail: "They won't play properly all the way through. You can try to repair them or keep their good part.",
      tone: "warn",
      actions: [{ label: "Open Damaged", run: () => setView("damaged"), primary: true }],
    });
  if (dupes)
    notices.push({
      id: `dupes:${dupes}`,
      kind: "dupes",
      title: `${dupes} clip${dupes === 1 ? " is" : "s are"} saved twice on this PC`,
      detail: "Checked byte for byte. Copies on the NAS never count.",
      actions: [{ label: "Open Cleanup", run: () => setView("cleanup"), primary: true }],
    });
  // Clips offered for Desktop (Discord, the desktop…) are in Cleanup's list, but don't make a notice or the badge.
  const gameSorts = sortList.filter((s) => !forDesktop(s));
  const sureSorts = gameSorts.filter((s) => s.sure).length;
  const closeSorts = gameSorts.length - sureSorts;
  if (gameSorts.length)
    notices.push({
      id: `sort:${sureSorts}:${closeSorts}`,
      kind: "sort",
      title: sureSorts
        ? `Found the game for ${sureSorts} unsorted clip${sureSorts === 1 ? "" : "s"}${closeSorts ? ` (and ${closeSorts} to check)` : ""}`
        : `${closeSorts} unsorted clip${closeSorts === 1 ? "" : "s"} might be a game you play`,
      detail: [...new Set(gameSorts.map((s) => s.game))].slice(0, 3).join(", ") + " and more, from what's on screen. Check them before they move.",
      actions: [
        { label: "Review", run: () => setSortOpen(true), primary: true },
        { label: "Open Cleanup", run: () => setView("cleanup") },
      ],
    });
  for (const f of oldFolders)
    notices.push({
      id: `old:${f.path}`,
      kind: "folders",
      title: `Your ${f.label} folder has no clips left in it`,
      detail: `${f.moved ? `All ${f.moved} of its clips moved into Hindsight. ` : ""}${f.files ? `Only ${f.files} other file${f.files === 1 ? "" : "s"} (${fmtSize(f.bytes)}) ${f.files === 1 ? "is" : "are"} left in it.` : "It's empty."}`,
      // Look inside first: what's left may be worth keeping.
      actions: [
        { label: "Open folder", run: () => api.reveal(f.path).catch((e) => toast(errText(e), true)) },
        { label: "Delete folder…", run: () => setOldAsk(f), primary: true },
      ],
    });
  if (nasNow?.enabled && !nasNow.reachable)
    notices.push(
      nasNow.loginRejected
        ? {
            id: "nas:login-rejected",
            kind: "nas",
            title: "The NAS turned down the saved password",
            detail: "It was probably changed on the NAS. Sign in again; clips keep saving on this PC meanwhile.",
            tone: "warn",
            actions: [
              {
                label: "Sign in again",
                primary: true,
                run: () => {
                  setView("settings");
                  requestNasSignIn();
                },
              },
            ],
          }
        : {
            id: `nas:${nasNow.lastError ?? ""}`,
            kind: "nas",
            title: "Can't reach the NAS",
            detail: "Clips keep saving on this PC and copy over once it's back.",
            tone: "warn",
            actions: [{ label: "NAS settings", run: () => setView("settings") }],
          },
    );
  const noticeState = useNoticeState(notices);

  // To trim: saved clips nobody has trimmed or edited yet (and not marked "Keep whole").
  const [edited, setEdited] = useState<Set<string>>(new Set());
  useEffect(() => {
    api.editedClips().then((l) => setEdited(new Set(l.map((p) => p.toLowerCase())))).catch(() => {});
  }, [clips]);
  const keepWhole = settings?.keepWhole;
  // Each clip's length, from the cache (instant): a clip shorter than its buffer was trimmed.
  const [lengths, setLengths] = useState<Map<string, number>>(new Map());
  useEffect(() => {
    if (!clips?.length) return;
    let alive = true;
    api
      .knownLengths(clips)
      .then((l) => alive && setLengths(new Map(clips.map((c, i) => [c.path, l[i] ?? 0]))))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [clips]);
  const bufferLength = settings?.bufferSeconds;
  const trimClips = useMemo(() => {
    if (!clips) return null;
    // Matched by "<game folder>\<file>", not the whole path: a clip whose copy here
    // was freed up (it's only on the NAS now) is still the same clip, so it stays on
    // To trim, and one trimmed or kept whole before it was freed stays off it.
    const tail = (p: string) => p.replace(/\//g, "\\").toLowerCase().split("\\").slice(-2).join("\\");
    const keep = new Set((keepWhole ?? []).map(tail));
    const done = new Set([...edited].map(tail));
    return clips.filter(
      (c) =>
        !EDIT_FOLDERS.has(c.folder.toLowerCase()) &&
        !MADE_BY_EDIT.test(c.name) &&
        !done.has(tail(c.path)) &&
        !keep.has(tail(c.path)) &&
        // Shorter than the buffer it was saved from: already trimmed (here or in another app).
        // A recording (started and stopped by hand) has no buffer length: it stays until trimmed.
        (RECORDING.test(c.name) || isWholeSave(lengths.get(c.path) ?? 0, bufferLength)),
    );
  }, [clips, edited, keepWhole, lengths, bufferLength]);
  const keepClipsWhole = useCallback(
    (paths: string[]) => {
      if (!settings) return;
      update({ keepWhole: [...new Set([...(settings.keepWhole ?? []), ...paths])] });
      toast(paths.length === 1 ? "Kept whole" : `Kept ${paths.length} clips whole`, false, "Taken off To trim.", {
        label: "Undo",
        run: () => update({ keepWhole: (settings.keepWhole ?? []).filter((p) => !paths.includes(p)) }),
      });
    },
    [settings, update, toast],
  );

  // Library → select clips → Move to…: one folder, PC and NAS together, with Undo.
  const moveClipsTo = useCallback(
    async (paths: string[], folder: string) => {
      started(paths); // their cards shimmer while they move
      try {
        const out = await api.moveToFolder(paths, folder);
        finished(paths);
        refresh();
        const failed = out.failed.length;
        toast(
          out.moved ? `Moved ${out.moved} clip${out.moved === 1 ? "" : "s"} to ${folder}` : `Nothing moved: they're already in ${folder}`,
          failed > 0,
          failed ? `${failed} file${failed === 1 ? "" : "s"} couldn't be moved: ${out.failed[0][1]}` : undefined,
          out.moved
            ? {
                label: "Undo",
                run: () =>
                  api
                    .undoLastMove()
                    .then(() => refresh())
                    .catch((e) => toast(errText(e), true)),
              }
            : undefined,
        );
        return true;
      } catch (e) {
        finished(paths);
        toast(errText(e), true);
        return false;
      }
    },
    [refresh, toast],
  );

  // With a NAS: the clips on this PC, and the ones on the NAS (synced from here, or from other PCs).
  const syncing = !!settings?.nasDir;
  const localClips = useMemo(() => (clips ? clips.filter((c) => c.location !== "nas") : null), [clips]);
  const syncedClips = useMemo(() => (clips ? clips.filter((c) => c.onNas || c.location === "nas") : null), [clips]);
  // Which list Studio goes back to.
  const [listView, setListView] = useState<ListView>("clips");
  // Lists opened so far (they stay loaded) and the save hotkey they show.
  const [openedLists, setOpenedLists] = useState<Set<ListView>>(() => new Set(["clips"]));
  useEffect(() => {
    setOpenedLists((s) => (s.has(listView) ? s : new Set([...s, listView])));
  }, [listView]);
  const hotkey = settings?.hotkey ?? "Alt+F10";
  useEffect(() => {
    if (isList(view)) setListView(view);
    if ((view === "local" || view === "synced") && !syncing) setView("clips");
  }, [view, syncing]);
  /** [view, label, sub-item under Clips] */
  const nav: [View, string, boolean?][] = [
    ["clips", "Clips"],
    ...(syncing
      ? ([
          ["local", "Local clips", true],
          ["synced", "NAS clips", true],
        ] as [View, string, boolean][])
      : []),
    ["totrim", "To trim"],
    ["damaged", "Damaged"],
    ["cleanup", "Cleanup"],
    ["montage", "Montage"],
    ["notifications", "Notifications"],
    ...(settings?.benchmarkMode ? ([["performance", "Performance"]] as [View, string][]) : []),
    ["settings", "Settings"],
  ];
  const navOn = (id: View) => view === id || (id === listView && view === "studio");
  const backToClips = useCallback(() => setView(listView), [listView]);
  // Mouse back / forward (buttons 4 and 5) and Alt+← / Alt+→, like a browser:
  // back = Studio → your clips, and on the clips → the top;
  // forward = from the clips, reopen the clip you just left.
  useEffect(() => {
    const back = () => {
      if (document.querySelector(".modal-backdrop")) return;
      if (view === "studio") setView(listView);
      else if (isList(view)) window.dispatchEvent(new Event("rewind-top"));
      else setView("clips");
    };
    const forward = () => {
      if (document.querySelector(".modal-backdrop")) return;
      if (isList(view) && studioPath) setView("studio");
    };
    const onMouse = (e: MouseEvent) => {
      if (e.button === 3 || e.button === 4) {
        e.preventDefault();
        if (e.button === 3) back();
        else forward();
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.altKey && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
        e.preventDefault();
        if (e.key === "ArrowLeft") back();
        else forward();
      }
    };
    window.addEventListener("mouseup", onMouse);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mouseup", onMouse);
      window.removeEventListener("keydown", onKey);
    };
  }, [view, studioPath, listView]);
  // The open clip's copy on this PC was freed up (it's safely on the NAS): carry on
  // with the NAS copy, same game folder and file name, instead of a dead player.
  const [movedFrom, setMovedFrom] = useState<{ from: string; to: string } | null>(null);
  useEffect(() => {
    if (!studioPath || !clips || clips.some((c) => c.path === studioPath)) return;
    const tail = (p: string) => p.replace(/\//g, "\\").split("\\").slice(-2).join("\\").toLowerCase();
    const want = tail(studioPath);
    const twin = clips.find((c) => c.location === "nas" && tail(c.path) === want);
    if (twin) {
      setMovedFrom({ from: studioPath, to: twin.path });
      setStudioPath(twin.path);
    }
  }, [clips, studioPath]);
  // Turning Benchmark mode off while on its page, or the open clip going away
  // (deleted): go back to the clips.
  useEffect(() => {
    if ((view === "performance" && settings && !settings.benchmarkMode) || (view === "studio" && !studioPath)) setView("clips");
  }, [view, settings, studioPath]);

  return (
    <div className={`shell side-${settings?.sidebar ?? "icons"}`}>
      {/* A slim rail of icons; each one's name shows on hover or keyboard focus. Settings →
          Appearance can open it with the names when the cursor is on it, or keep it open. */}
      <nav className="nav" aria-label="Hindsight">
        <SlidingPill selector=".nav-item.on" glideFar />
        <div className="brand" title={`Hindsight${upd?.current ? ` ${upd.current}` : ""} (beta)`}>
          <img src={logo} alt="Hindsight" width={28} height={28} />
        </div>
        {nav.map(([id, label, sub]) => (
          <button
            key={id}
            className={`nav-item ${sub ? "nav-sub" : ""} ${navOn(id) ? "on" : ""} ${id === "settings" ? "nav-bottom" : ""}`}
            onClick={() => (id === view && isList(id) ? window.dispatchEvent(new Event("rewind-top")) : setView(id))}
            aria-label={label}
            aria-current={navOn(id) ? "page" : undefined}
          >
            <Icon name={id as IconName} size={20} />
            <span className="nav-label" role="tooltip">
              {label}
            </span>
            {id === "totrim" && !!trimClips?.length && (
              <span className="nav-badge quiet" title={`${trimClips.length} clip${trimClips.length === 1 ? "" : "s"} not trimmed yet`}>
                {trimClips.length}
              </span>
            )}
            {id === "cleanup" && gameSorts.length > 0 && (
              <span className="nav-badge quiet" title={`${gameSorts.length} clip${gameSorts.length === 1 ? "" : "s"} to sort into game folders`}>
                {gameSorts.length}
              </span>
            )}
            {id === "notifications" && noticeState.shown.length > 0 && (
              <span className="nav-badge notif-count" title={`${noticeState.shown.length} notification${noticeState.shown.length === 1 ? "" : "s"}`}>
                {noticeState.shown.length}
              </span>
            )}
            {id === "damaged" && !!damagedClips?.length && (
              <span className="nav-badge" title={`${damagedClips.length} damaged clip${damagedClips.length === 1 ? "" : "s"}`}>
                {damagedClips.length}
              </span>
            )}
          </button>
        ))}
      </nav>
      <main className="main">
        {/* Every clip list you've opened stays loaded (hidden) behind the others,
            Studio and Settings, so switching back to it is instant. */}
        {LIST_VIEWS.filter((v) => v === listView || openedLists.has(v)).map((v) => (
          <Library
            key={v}
            active={isList(view) && listView === v}
            clips={v === "local" ? localClips : v === "synced" ? syncedClips : v === "damaged" ? damagedClips : v === "totrim" ? trimClips : clips}
            mode={v}
            onOpen={openInStudio}
            onToast={toast}
            onChanged={refresh}
            onDelete={requestDelete}
            hotkey={hotkey}
            uploads={uploads.progress}
            links={uploads.history}
            focusPath={listView === v ? studioPath : null}
            onShare={requestShare}
            health={health}
            onKeepWhole={v === "totrim" ? keepClipsWhole : undefined}
            onMoveTo={v === "damaged" ? undefined : moveClipsTo}
            sortList={sortList}
            onSortReview={() => setSortOpen(true)}
            onReviewList={(list, heading) => {
              setFolderFinds({ list, heading });
              setSortOpen(true);
            }}
          />
        ))}
        {view === "cleanup" && settings && (
          <Cleanup
            clips={clips}
            settings={settings}
            update={update}
            onOpen={openInStudio}
            onDelete={requestDelete}
            onToast={toast}
            onChanged={refresh}
            sortList={sortList}
            onSortReview={() => setSortOpen(true)}
          />
        )}
        {view === "montage" && <MontageView clips={clips} untrimmed={trimClips} onOpen={openInStudio} />}
        {view === "notifications" && <NotificationsPage state={noticeState} />}
        {view === "studio" && (
          <Studio
            path={studioPath}
            movedFrom={movedFrom && movedFrom.to === studioPath ? movedFrom.from : null}
            clips={clips}
            onOpen={openInStudio}
            onDone={refresh}
            onDelete={requestDelete}
            settings={settings}
            update={update}
            onBack={backToClips}
            uploads={uploads.progress}
            links={uploads.history}
            onShare={requestShare}
            health={health[studioPath ?? ""]}
            onSavedElsewhere={(text, out, err) =>
              toast(text, !!err, out ? out.split("\\").pop() : undefined, out ? { label: "Open", run: () => openInStudio(out) } : undefined)
            }
            onRescued={(p) => {
              refresh();
              openInStudio(p);
              toast("Rescued the good part", false, p.split("\\").pop());
            }}
          />
        )}
        {view === "performance" && <Performance />}
        {view === "settings" && settings && (
          <SettingsView
            settings={settings}
            update={update}
            onError={(m) => toast(m, true)}
            onInstall={requestInstall}
            installing={installing}
          />
        )}
      </main>
      <StatusBar
        status={status}
        hotkey={settings?.hotkey ?? "Alt+F10"}
        recordHotkey={settings?.recordHotkey ?? "Alt+F9"}
        version={upd?.current}
        customArea={!!settings?.captureArea}
        onSyncWhilePlaying={() => update({ nasSyncWhileGaming: true }).then(() => toast("Syncing to the NAS while you play (turn it off in Settings → Library)")).catch((e) => toast(errText(e), true))}
        onFitScreen={() => update({ captureArea: null }).then(() => toast("Recording the whole screen again")).catch((e) => toast(errText(e), true))}
      />
      {shareReq && settings && (
        <ShareConfirm
          req={shareReq}
          settings={settings}
          onClose={() => setShareReq(null)}
          onOpenSettings={() => {
            try {
              localStorage.setItem("rewind.settingsTab", "sharing");
            } catch {
              /* only a convenience */
            }
            setView("settings");
          }}
          onError={(m) => toast(m, true)}
        />
      )}
      {askUpdate && upd?.version && (
        <ConfirmUpdate
          version={upd.version}
          reasons={interrupts.length ? interrupts : ["Hindsight closes and reopens on the new version."]}
          busy={installing}
          onConfirm={installNow}
          onCancel={closeAskUpdate}
        />
      )}
      {sortOpen && clips && (
        <GameSortDialog
          list={folderFinds?.list ?? sortList}
          heading={folderFinds?.heading}
          clips={clips}
          onClose={() => {
            setSortOpen(false);
            setFolderFinds(null);
          }}
          onDone={(moved, failed) => {
            setSortOpen(false);
            setFolderFinds(null);
            refresh();
            toast(
              `Moved ${moved} clip${moved === 1 ? "" : "s"} into their game folders`,
              failed > 0,
              failed ? `${failed} couldn't be moved` : undefined,
              {
                label: "Undo",
                run: () =>
                  api
                    .undoLastMove()
                    .then(() => refresh())
                    .catch((e) => toast(errText(e), true)),
              },
            );
          }}
        />
      )}
      {oldAsk && (
        <OldFolderDialog
          folder={oldAsk}
          onClose={() => setOldAsk(null)}
          onDone={() => {
            setOldAsk(null);
            setOldFolders((l) => l.filter((f) => f.path !== oldAsk.path));
            toast(`Moved the old ${oldAsk.label} folder to the Recycle Bin`, false, "Restore it from there if you need it.");
          }}
        />
      )}
      {mergeOpen && (
        <MergeDialog list={similar} onClose={() => setMergeOpen(false)} onMerged={onMerged} onError={(m) => toast(m, true)} />
      )}
      {pendingDelete && (
        <ConfirmDelete
          paths={pendingDelete.paths}
          busy={deleting}
          onConfirm={confirmDelete}
          onCancel={cancelDelete}
          nas={pendingDelete.paths.some((p) => clips?.find((c) => c.path === p && (c.onNas || c.location === "nas")))}
          wholeFolders={wholeFoldersOf(pendingDelete.paths, clips ?? [])}
        />
      )}
      <div className="toasts" aria-live="polite">
        {toasts.map((t) => (
          <div key={t.id} className={`toast ${t.err ? "err" : ""}`}>
            <div className="toast-bar" />
            <div className="toast-body">
              <span>{t.text}</span>
              {t.sub && <small>{t.sub}</small>}
            </div>
            {t.action && (
              <button
                className="btn small"
                onClick={() => {
                  t.action!.run();
                  dropToast(t.id);
                }}
              >
                {t.action.label}
              </button>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

// Make the flyout close itself on Escape.
export function useEscapeToHide() {
  useEffect(() => {
    const f = (e: KeyboardEvent) => e.key === "Escape" && api.hideFlyout();
    window.addEventListener("keydown", f);
    return () => window.removeEventListener("keydown", f);
  }, []);
}
