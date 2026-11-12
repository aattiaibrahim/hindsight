import { convertFileSrc, invoke } from "@tauri-apps/api/core";

export type Encoder = "auto" | "av1" | "hevc" | "h264";
/** A recorder's folder with no clips left in it (they all moved into Hindsight). */
export type OldFolder = { path: string; label: string; moved: number; files: number; bytes: number };
/** The picture check's state. */
export type DetectStatus = { installed: boolean; downloading: number | null; lookedAt: number; toLookAt: number; error: string | null };
/** A clip in an unsorted folder and the game it looks like. */
export type GameSuggestion = { path: string; name: string; folder: string; game: string; confidence: number; likeness: number };
/** A montage to render: clip pieces in order, an optional song. */
export type MontagePlan = {
  pieces: { path: string; start: number; end: number }[];
  music: { path: string; offset: number; volumeDb: number } | null;
  clipVolumeDb: number;
  height: number;
  fps: number;
};
/** What this PC's NVIDIA GPU can encode. */
export type GpuEncoders = { gpu: string; supported: Exclude<Encoder, "auto">[]; auto: Exclude<Encoder, "auto"> };
export type MonitorMode = "primary" | "cursor" | "fixed";

export type Settings = {
  clipsDir: string;
  encoder: Encoder;
  quality: number;
  fps: number;
  outputHeight: number;
  bufferSeconds: number;
  bufferInRam: boolean;
  captureEngine: "native" | "ffmpeg";
  monitorMode: MonitorMode;
  fixedMonitor: string;
  drawCursor: boolean;
  /** Part of the screen to record (fractions); null = all of it. */
  captureArea: CropRect | null;
  hotkey: string;
  recordHotkey: string;
  bufferEnabled: boolean;
  launchOnStartup: boolean;
  autoUpdate: boolean;
  saveSound: boolean;
  nasDir: string;
  /** Id in the confirmed NAS folder's marker file. */
  nasLibraryId: string;
  /** The paired NAS app's address, "" = not paired. */
  nasServer: string;
  nasRemoveLocal: boolean;
  linkedFolders: string[];
  /** Game key → the folder spelling picked when merging */
  folderNames: Record<string, string>;
  /** Games kept under separate spellings (keys) */
  similarDismissed: string[];
  /** Cleanup: path → size when kept; hidden until the next cleanup check */
  cleanupKept: Record<string, number>;
  benchmarkMode: boolean;
  uploadHost: UploadHost;
  litterboxTime: "1h" | "12h" | "24h" | "72h";
  autoUpload: boolean;
  postLinkToDiscord: boolean;
  streamableEmail: string;
  discordWebhookName: string;
  confirmUploads: boolean;
  healthCheck: boolean;
  /** Make every clip's thumbnail ahead of time, while no game is running */
  prepareThumbnails: boolean;
  /** Find the game in unsorted clips from the picture (downloads a model) */
  gameDetect: boolean;
  detectDismissed: string[];
  /** Clips taken off "To trim" */
  keepWhole: string[];
  /** Game logos from Steam (only the game's name is sent) */
  onlineGameLogos: boolean;
  startMinimized: boolean;
  keepOriginalAfterRemux: boolean;
  replaceOriginalOnEdit: boolean;
  externalFiles: string[];
  desktopAudio: boolean;
  /** Discord on its own track, left out of desktop audio */
  discordTrack: boolean;
  micEnabled: boolean;
  micMono: boolean;
  micDevice: string;
  /** Mic volume while recording, dB (-20..+20). */
  micGainDb: number;
  theme: Theme;
  accent: Accent;
  reduceMotion: boolean;
};

export type Theme = "tinted" | "midnight" | "graphite" | "oled";
export type Accent = "winblue" | "system" | "cyan" | "rose" | "lime" | "amber" | "violet" | "emerald" | "gold" | "pink" | "teal" | "mono" | "green";

export type Status = {
  enabled: boolean;
  running: boolean;
  monitor: string | null;
  width: number;
  height: number;
  encoder: string;
  fps: number;
  bufferedSeconds: number;
  bufferSeconds: number;
  restarts: number;
  lastError: string | null;
  audio: string[];
  audioError: string | null;
  game: string | null;
  recording: boolean;
  recordingSeconds: number;
  inRam: boolean;
  bufferMb: number;
  engine: "native" | "ffmpeg";
};

export type UpdateState = {
  current: string;
  stage: "idle" | "checking" | "available" | "downloading" | "ready" | "up-to-date" | "error" | "";
  version: string | null;
  notes: string | null;
  error: string | null;
};

export type BenchApp = "rewind" | "obs" | "nvidia" | "medal";
export type AppUse = { cpuPercent: number; ramMb: number; gpu3dPercent: number; encoderPercent: number };
export type BenchFrames = {
  frames: number;
  avgFps: number;
  low1Fps: number;
  avgFrameMs: number;
  gpuBusyMs: number | null;
  cpuBusyMs: number | null;
  syncInterval: number | null;
  presentMode: string | null;
};
export type Stat = { mean: number; sd: number; n: number; ci95: number | null };
export type BenchRun = {
  round: number;
  app: BenchApp | null;
  atSecs: number;
  frames: BenchFrames | null;
  apps: Partial<Record<BenchApp, AppUse>>;
  excluded: string | null;
};
export type Verdict = "oneRun" | "noMeasurableCost" | "costs" | "faster" | "noData";
export type Comparison = {
  app: BenchApp;
  runs: number;
  fpsCost: Stat | null;
  low1Cost: Stat | null;
  gpuMsAdded: Stat | null;
  cpuPercent: Stat | null;
  gpu3dPercent: Stat | null;
  encoderPercent: Stat | null;
  ramMb: Stat | null;
  verdict: Verdict;
};
export type Baseline = {
  runs: number;
  fps: Stat | null;
  low1: Stat | null;
  noisePercent: number | null;
  driftPercent: number | null;
  gpuBusyShare: number | null;
  cpuBusyShare: number | null;
  vsync: boolean;
  presentMode: string | null;
};
export type RecorderSettings = {
  app: BenchApp;
  width: number | null;
  height: number | null;
  fps: number | null;
  codec: string | null;
  quality: string | null;
  bufferSecs: number | null;
  source: string;
};
export type BenchReport = {
  game: string;
  started: string;
  seconds: number;
  rounds: number;
  cores: number;
  warmupSecs: number;
  minutes: number;
  runs: BenchRun[];
  baseline: Baseline;
  results: Comparison[];
  settings: RecorderSettings[];
  skipped: { app: BenchApp; reason: string }[];
  warnings: string[];
  error: string | null;
};
export type BenchProgress = { step: string; detail: string; waitingForUser: boolean; done: number; total: number };

export type PreviewAudio ={ firstTitle: string; extra: { index: number; title: string; path: string }[]; /** a track that's the others already mixed */ mix?: number | null };

/** The same clip more than once: "copy" (byte for byte) or "remux" (same recording, another format, e.g. OBS's MKV and MP4). */
export type DupeSet = { paths: string[]; kind: "copy" | "remux" | "smaller" };

export type DeleteResult ={ deleted: string[]; failed: [string, string][] };

export type ClipFile = {
  path: string;
  name: string;
  folder: string;
  size: number;
  modified: number;
  external: boolean;
  /** "local" = this PC, "nas" = only on the NAS, "linked" = another recorder's folder */
  location: "local" | "nas" | "linked";
  onNas: boolean;
  source: string;
};

export type NasState = {
  enabled: boolean;
  reachable: boolean;
  pending: number;
  copying: string | null;
  copied: number;
  lastError: string | null;
  lastSync: number | null;
  /** Waiting because a game is running or a recording is going */
  paused: boolean;
  /** This pass: clips and bytes to copy, and how many are done */
  totalFiles: number;
  doneFiles: number;
  totalBytes: number;
  doneBytes: number;
  /** Bytes still to copy (stays set while paused) */
  pendingBytes: number;
  /** Reading the NAS copy back to check it */
  checking: boolean;
  /** Freeing up space on this PC: clips done of this many (this pass) */
  freeDone: number;
  freeTotal: number;
  /** Checking a clip before freeing up its local copy (vs copying one) */
  freeing: boolean;
  /** Stopped from the status bar until Resume */
  stopped: boolean;
  /** Bytes per second this pass */
  rate: number;
  etaSecs: number | null;
};

export type ClipFolder = { label: string; path: string; exists: boolean };

export type Health = {
  state: "ok" | "damaged" | "unreadable";
  damaged: [number, number][];
  best: [number, number] | null;
  duration: number;
};
export type HealthProgress = { checked: number; total: number; damaged: number; paused: boolean; running: boolean };

export type UploadHost = "catbox" | "litterbox" | "discord" | "streamable";
export const UPLOAD_HOSTS: { id: UploadHost; label: string; hint: string }[] = [
  { id: "catbox", label: "Catbox", hint: "Free, no account, keeps the clip. Plays inside Discord. Up to 200 MB." },
  { id: "litterbox", label: "Litterbox", hint: "Free, no account, deletes the clip after a while. Up to 1 GB." },
  { id: "discord", label: "Discord webhook", hint: "Posts the clip into your channel. Compressed under Discord's 10 MB." },
  { id: "streamable", label: "Streamable", hint: "Clip page with a player. Needs a Streamable account." },
];
export type Upload = { host: UploadHost; url: string; at: number; bytes: number; expires: number | null };
export type UploadProgress = {
  path: string;
  host: UploadHost;
  stage: "preparing" | "uploading" | "done" | "error";
  sent: number;
  total: number;
  url: string | null;
  error: string | null;
  warning: string | null;
};
export type Reach = { host: string; state: "ok" | "blocked" | "unreachable" | "webhook" };

export type MovePlan = {
  label: string;
  count: number;
  bytes: number;
  folders: [string, number][];
  examples: [string, string][];
  otherDrive: boolean;
  /** Copying: the originals stay where they are */
  copy: boolean;
};
/** A folder with clips found on one of the drives (an external HDD too). */
export type FoundFolder = { path: string; label: string; drive: string; clips: number; bytes: number; shown: boolean };
export type MoveOutcome = { moved: number; failed: [string, string][] };
export type LastMove = { label: string; count: number; when: number };

/** Part of the picture to keep, as fractions of the frame. */
export type CropRect = { x: number; y: number; w: number; h: number };

/** What's in a folder picked as the NAS folder (before anything syncs). */
export type NasFolderCheck = {
  dir: string;
  reachable: boolean;
  writable: boolean;
  clips: number;
  clipBytes: number;
  games: number;
  otherFiles: number;
  otherExamples: string[];
  toCopy: number;
  toCopyBytes: number;
  knownLibrary: boolean;
  serverGuess: string;
  onThisPc: boolean;
  warnings: string[];
};

export type Probe = {
  duration: number;
  codec: string;
  width: number;
  height: number;
  fps: number;
  audioTracks: number;
  container: string;
};

export type Monitor = {
  index: number;
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  primary: boolean;
};

// Thumbnails and probes spawn ffmpeg, so only run a few at a time. Newest
// first: the clips just scrolled into view matter more than ones scrolled past.
/**
 * Thumbnails and probes, a few at a time, newest request first (the cards you
 * just scrolled to). Clips on the NAS have their own lane: reading them over the
 * network can be slow (more so while the NAS is busy checking copies), and they
 * must never hold up the clips on this PC.
 */
type Waiter = { go: () => void; key: string };
type Lane = { active: number; max: number; waiting: Waiter[] };

/**
 * Cards actually on screen (not just near it): their requests go first. Without
 * this, after a quick scroll the cards you're looking at waited behind ones 900 px
 * below (asked for more recently) and ones already scrolled past.
 */
const onScreen = new Set<string>();
export function markOnScreen(keys: string[], on: boolean) {
  for (const k of keys) {
    if (on) onScreen.add(k);
    else onScreen.delete(k);
  }
}
/** The next request to go: the newest one on screen, else the newest one. */
function next(lane: Lane) {
  for (let i = lane.waiting.length - 1; i >= 0; i--) {
    if (onScreen.has(lane.waiting[i].key)) return lane.waiting.splice(i, 1)[0].go();
  }
  lane.waiting.pop()?.go();
}
const LANES: Record<"local" | "net", Lane> = { local: { active: 0, max: 3, waiting: [] }, net: { active: 0, max: 3, waiting: [] } };
const laneOf = (path: string): Lane => (/^(\\\\|\/\/)/.test(path) ? LANES.net : LANES.local);

/** Thrown for a request whose card left the screen before its turn came. */
export const NOT_NEEDED = "not needed any more";

async function limited<T>(path: string, job: () => Promise<T>, stillWanted: () => boolean = () => true, key = ""): Promise<T> {
  const lane = laneOf(path);
  if (lane.active >= lane.max) await new Promise<void>((go) => lane.waiting.push({ go, key }));
  if (!stillWanted()) {
    next(lane); // pass the turn on
    throw new Error(NOT_NEEDED);
  }
  lane.active++;
  // A request that hangs (a network drive that stopped answering) mustn't hold its
  // turn forever: after SLOT_LIMIT_MS the next one goes ahead, and this one still
  // delivers its answer whenever it comes. Before, a few stuck ones froze every card.
  let released = false;
  const release = () => {
    if (released) return;
    released = true;
    lane.active--;
    next(lane);
  };
  const timer = setTimeout(release, SLOT_LIMIT_MS);
  let giveUp = 0;
  try {
    // A request that never answers is given up on, so the card asks again.
    return await Promise.race([
      job(),
      new Promise<never>((_, no) => (giveUp = window.setTimeout(() => no(new Error(TIMED_OUT)), REQUEST_LIMIT_MS))),
    ]);
  } finally {
    clearTimeout(giveUp);
    clearTimeout(timer);
    release();
  }
}
const SLOT_LIMIT_MS = 20_000;
const REQUEST_LIMIT_MS = 30_000;
const TIMED_OUT = "timed out";

/**
 * Who still wants a thumbnail or probe: cards add themselves while they're near
 * the screen and remove themselves when they leave. Something nobody registered
 * for (other views) always counts as wanted.
 */
const wants = new Map<string, number>();
export function want(key: string, delta: 1 | -1) {
  wants.set(key, Math.max(0, (wants.get(key) ?? 0) + delta));
}
/** Asked for by something other than a card (Studio, the To trim estimate): always wanted. */
const always = new Set<string>();
const wanted = (key: string) => always.has(key) || !wants.has(key) || (wants.get(key) ?? 0) > 0;

const thumbCache = new Map<string, Promise<string>>();
const thumbReady = new Map<string, string>();
const iconCache = new Map<string, Promise<string | null>>();
const probeCache = new Map<string, Promise<Probe>>();

// The app's player loads clips from a local server that streams only what it
// needs (Tauri's file protocol read whole 400 MB clips into memory first).
let mediaBase: string | null = null;
invoke<string | null>("media_base")
  .then((b) => (mediaBase = typeof b === "string" ? b : null))
  .catch(() => {});

/** The address the player loads a clip (or audio track) from. */
export function mediaSrc(path: string, bust?: string | number) {
  if (mediaBase) return `${mediaBase}${encodeURIComponent(path)}${bust != null ? `&v=${encodeURIComponent(String(bust))}` : ""}`;
  return `${convertFileSrc(path)}${bust != null ? `?v=${bust}` : ""}`;
}

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  setSettings: (settings: Settings) => invoke<void>("set_settings", { settings }),
  getStatus: () => invoke<Status>("get_status"),
  restartCapture: () => invoke<void>("restart_capture"),
  listMonitors: () => invoke<Monitor[]>("list_monitors"),
  nasCheck: (dir: string) => invoke<NasFolderCheck>("nas_check", { dir }),
  /** Marks the folder as the library; returns its id. */
  nasConfirm: (dir: string) => invoke<string>("nas_confirm", { dir }),
  nasPair: (dir: string, server: string, code: string) => invoke<string>("nas_pair", { dir, server, code }),
  /** A small PNG of the screen being recorded (refused while a game runs). */
  screenPreview: () => invoke<ArrayBuffer>("screen_preview"),
  saveClip: (seconds?: number) => invoke<string>("save_clip", { seconds: seconds ?? null }),
  toggleRecording: () => invoke<string | null>("toggle_recording"),
  listClips: () => invoke<ClipFile[]>("list_clips"),
  /** `tracked`: the caller registers with `want` (cards); anyone else always gets an answer. */
  probe: (path: string, key = "", now = false, tracked = false) => {
    const k = path + key;
    // Before, this deleted the cards' count for the clip, so cards still on screen
    // looked unwanted, were told "not needed" and never showed their details.
    if (!tracked) always.add("p:" + k);
    if (!probeCache.has(k)) {
      const p = now ? invoke<Probe>("probe_file", { path }) : limited(path, () => invoke<Probe>("probe_file", { path }), () => wanted("p:" + k), "p:" + k);
      p.catch(() => probeCache.delete(k));
      probeCache.set(k, p);
    }
    return probeCache.get(k)!;
  },
  thumbnail: (path: string, key = "", tracked = false) => {
    const k = path + key;
    if (!tracked) always.add("t:" + k);
    if (!thumbCache.has(k)) {
      const p = limited(path, () => invoke<string>("thumbnail", { path }), () => wanted("t:" + k), "t:" + k).then(convertFileSrc);
      p.then((src) => thumbReady.set(k, src)).catch(() => thumbCache.delete(k));
      thumbCache.set(k, p);
    }
    return thumbCache.get(k)!;
  },
  /** A thumbnail that's already loaded this session (no waiting, no flash). */
  thumbNow: (path: string, key = "") => thumbReady.get(path + key) ?? null,
  startBench: (compare: BenchApp[], seconds: number, rounds: number) =>
    invoke<void>("start_bench", { options: { compare, seconds, rounds } }),
  cancelBench: () => invoke<void>("cancel_bench"),
  benchState: () => invoke<[boolean, BenchReport | null]>("bench_state"),
  previewAudio: (path: string) => invoke<PreviewAudio>("preview_audio", { path }),
  remuxMp4: (path: string) => invoke<string>("remux_mp4", { path }),
  trim: (path: string, start: number, end: number, exact: boolean, replace = false, audio?: TrackEdit[], crop?: CropRect) =>
    invoke<string>("trim_clip", { path, start, end, exact, replace, audio: audio ?? null, crop: crop ?? null }),
  clipWaveforms: (path: string) => invoke<Waveform[]>("clip_waveforms", { path }),
  exportH264: (path: string, replace = false, audio?: TrackEdit[], crop?: CropRect) =>
    invoke<string>("export_h264", { path, replace, audio: audio ?? null, crop: crop ?? null }),
  /** MP3 of [start, end]: every track mixed (track null) or one track alone. */
  exportAudio: (path: string, start: number, end: number, track: number | null, label: string, audio?: TrackEdit[]) =>
    invoke<string>("export_audio", { path, start, end, track, label, audio: audio ?? null }),
  compress: (path: string, targetMb: number, replace = false, audio?: TrackEdit[], crop?: CropRect) =>
    invoke<string>("compress_clip", { path, targetMb, replace, audio: audio ?? null, crop: crop ?? null }),
  gpuEncoders: () => invoke<GpuEncoders>("gpu_encoders"),
  /** Sets of paths holding the very same clip */
  findDuplicates: () => invoke<DupeSet[]>("find_duplicates"),
  detectStatus: () => invoke<DetectStatus>("detect_status"),
  detectInstall: () => invoke<void>("detect_install"),
  gameSuggestions: () => invoke<GameSuggestion[]>("game_suggestions"),
  applyGameSort: (picks: [string, string][]) => invoke<MoveOutcome>("apply_game_sort", { picks }),
  dismissGameSuggestions: (paths: string[]) => invoke<void>("dismiss_game_suggestions", { paths }),
  /** Save a diagnostics report in Downloads (about one clip, if given); returns its path. */
  /** Remove this PC's copy of a clip that's identical on the NAS (the NAS copy stays). */
  freeLocalCopy: (path: string) => invoke<void>("free_local_copy", { path }),
  runDiagnostics: (clip?: string) => invoke<string>("run_diagnostics", { clip: clip ?? null }),
  renderMontage: (montage: MontagePlan) => invoke<string>("render_montage", { montage }),
  cancelMontage: () => invoke<void>("cancel_montage"),
  /** Clips changed in the app (trimmed, healed, exported) */
  editedClips: () => invoke<string[]>("edited_clips"),
  /** Windows' accent colour ("#rrggbb"), for "Follow Windows" */
  systemAccent: () => invoke<string | null>("system_accent"),
  /** A picture as a game's logo (null: back to the automatic one) */
  setGameLogo: (name: string, picture: string | null) => {
    iconCache.clear();
    return invoke<void>("set_game_logo", { name, picture });
  },
  gameIcon: (name: string) => {
    if (!iconCache.has(name)) iconCache.set(name, invoke<string | null>("game_icon", { name }).then((p) => (p ? convertFileSrc(p) : null)).catch(() => null));
    return iconCache.get(name)!;
  },
  listAudioInputs: () => invoke<string[]>("list_audio_inputs"),
  reveal: (path: string) => invoke<void>("reveal_file", { path }),
  deleteClips: (paths: string[]) => invoke<DeleteResult>("delete_clips", { paths }),
  restoreClips: (paths: string[]) => invoke<number>("restore_clips", { paths }),
  addExternal: (paths: string[]) => invoke<void>("add_external_files", { paths }),
  removeExternal: (path: string) => invoke<void>("remove_external_file", { path }),
  openInStudio: (path: string) => invoke<void>("open_in_studio", { path }),
  showMain: () => invoke<void>("show_main_window"),
  hideFlyout: () => invoke<void>("hide_flyout"),
  getUpdateState: () => invoke<UpdateState>("get_update_state"),
  checkForUpdates: () => invoke<string | null>("check_for_updates"),
  installUpdate: () => invoke<void>("install_update"),
  getNasState: () => invoke<NasState>("get_nas_state"),
  uploadClip: (path: string, host?: UploadHost) => invoke<Upload>("upload_clip", { path, host: host ?? null }),
  uploadHistory: () => invoke<Record<string, Upload[]>>("upload_history"),
  copyText: (text: string) => invoke<void>("copy_text", { text }),
  setSecret: (name: "discord_webhook" | "streamable_password", value: string) =>
    invoke<{ name: string; channelId: string } | null>("set_secret", { name, value }),
  checkSharing: () => invoke<[UploadHost, Reach[]][]>("check_sharing"),
  clipHealth: (paths: string[]) => invoke<Record<string, Health>>("clip_health", { paths }),
  healthProgress: () => invoke<HealthProgress>("health_progress"),
  rescueClip: (path: string) => invoke<string>("rescue_clip", { path }),
  /** Replace a damaged clip with its longest good part (original to the Recycle Bin). */
  healClip: (path: string) => invoke<string>("heal_clip", { path }),
  /** A full-length repaired copy, "<name> (repaired)"; the original stays. */
  repairClip: (path: string) => invoke<string>("repair_clip", { path }),
  secretsSet: () => invoke<{ discordWebhook: boolean; streamablePassword: boolean }>("secrets_set"),
  syncNasNow: () => invoke<void>("sync_nas_now"),
  /** Stop (true) or resume (false) NAS copying and freeing up. */
  stopNasSync: (stop: boolean) => invoke<void>("stop_nas_sync", { stop }),
  recorderClipFolders: () => invoke<ClipFolder[]>("recorder_clip_folders"),
  planMoveIntoRewind: (folders: string[]) => invoke<MovePlan>("plan_move_into_rewind", { folders }),
  moveIntoRewind: (folders: string[]) => invoke<MoveOutcome>("move_into_rewind", { folders }),
  /** Recorder folders that moving into Hindsight emptied */
  oldFolders: () => invoke<OldFolder[]>("old_folders"),
  removeOldFolder: (path: string) => invoke<void>("remove_old_folder", { path }),
  lastMove: () => invoke<LastMove | null>("last_move"),
  undoLastMove: () => invoke<MoveOutcome>("undo_last_move"),
  similarGameFolders: () => invoke<SimilarFolders[]>("similar_game_folders"),
  mergeGameFolders: (key: string, target: string) => invoke<MoveOutcome>("merge_game_folders", { key, target }),
  /** Stop asking about one game, or (no key) ask about every game again. */
  keepFoldersSeparate: (key: string | null) => invoke<void>("keep_folders_separate", { key }),
  findClipFolders: () => invoke<FoundFolder[]>("find_clip_folders"),
  /** Cleanup: compress to ~40%, replacing the original (Recycle Bin), keeping its date. */
  shrinkClip: (path: string) => invoke<string>("shrink_clip", { path }),
  planImport: (path: string, copy: boolean) => invoke<MovePlan>("plan_import", { path, copy }),
  importFolder: (path: string, copy: boolean) => invoke<MoveOutcome>("import_folder", { path, copy }),
};

/** One game saved under several folder spellings, on this PC and/or the NAS. */
export type SimilarFolders = {
  key: string;
  /** Most clips first */
  names: { name: string; here: number; nas: number }[];
  suggested: string;
};

export const VIDEO_EXTENSIONS = ["mkv", "mp4", "mov", "webm", "m4v", "avi", "ts", "flv"];

export function fmtDuration(sec: number) {
  if (!isFinite(sec) || sec < 0) sec = 0;
  const m = Math.floor(sec / 60);
  const s = Math.floor(sec % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

/** One audio track's loudness over time: levels[i] (0-255) covers `step` seconds. */
export type Waveform = { step: number; levels: number[] };
/** Studio's changes to one audio track: volume in dB and muted stretches (seconds of the original). */
export type TrackEdit = { gainDb: number; mutes: [number, number][] };

export function fmtDb(db: number) {
  return db === 0 ? "0 dB" : `${db > 0 ? "+" : ""}${db.toFixed(1).replace(/\.0$/, "")} dB`;
}

export function fmtTime(sec: number) {
  const m = Math.floor(sec / 60);
  const s = sec - m * 60;
  return `${m.toString().padStart(2, "0")}:${s.toFixed(2).padStart(5, "0")}`;
}

export function fmtSize(bytes: number) {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  if (bytes >= 1024 ** 2) return `${Math.round(bytes / 1024 ** 2)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

export function codecLabel(codec: string) {
  return ({ av1: "AV1", hevc: "HEVC", h264: "H.264", vp9: "VP9" } as Record<string, string>)[codec] ?? codec.toUpperCase();
}

export function errText(e: unknown) {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}

/** Rough RAM-free disk use of the buffer, for the settings hint (MB). */
export function bufferEstimateMb(s: Pick<Settings, "bufferSeconds" | "quality" | "encoder">) {
  const base = { auto: 22, av1: 22, hevc: 28, h264: 40 }[s.encoder]; // Mbps at CQ 20, busy 1440p60 game
  const mbps = base * Math.pow(0.88, s.quality - 20);
  return Math.round((mbps * s.bufferSeconds) / 8);
}
