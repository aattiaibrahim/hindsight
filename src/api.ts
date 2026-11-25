import { convertFileSrc, invoke as tauriInvoke } from "@tauri-apps/api/core";

/** Every command, with failures written to the detailed log (Settings → Detailed log). */
function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return tauriInvoke<T>(cmd, args).catch((e) => {
    // "game…": work that waits while a game runs (expected, and asked every few seconds).
    const text = errText(e);
    if (cmd !== "log_ui" && !/^game\b/.test(text)) logUi(`${cmd} failed: ${text}`);
    throw e;
  });
}

/** A line for the detailed log (it's dropped when the log is off). */
export function logUi(text: string) {
  tauriInvoke("log_ui", { text }).catch(() => {});
}

if (typeof window !== "undefined") {
  window.addEventListener("error", (e) => logUi(`crash: ${e.message} at ${e.filename}:${e.lineno}`));
  window.addEventListener("unhandledrejection", (e) => logUi(`unhandled: ${errText(e.reason)}`));
}

export type Encoder = "auto" | "av1" | "hevc" | "h264";
/** A recorder's folder with no clips left in it (they all moved into Hindsight). */
export type OldFolder = { path: string; label: string; moved: number; files: number; bytes: number };
/** The picture check's state. */
export type DetectStatus = { installed: boolean; downloading: number | null; lookedAt: number; toLookAt: number; error: string | null };
/** A clip in an unsorted folder and the game it looks like. */
/** `sure`: ticked in the review. Otherwise a close call, with its likeliest games in `choices` (best first). */
/** Checking a game's folder for clips of other games. lookedAt: clips with a fingerprint, of clips. */
/** desktopLike: clips that look like the desktop or an app, not a game. notGame: OBS, Edits… (looking for clips from a game). */
export type FolderCheck = { suggestions: GameSuggestion[]; lookedAt: number; clips: number; desktopLike: number; notGame: boolean };
/** A suggestion to move a clip to Desktop (it looks like the desktop or an app, not a game). */
export const forDesktop = (s: { game: string }) => s.game.toLowerCase() === "desktop";
export type GameSuggestion = { path: string; name: string; folder: string; game: string; confidence: number; likeness: number; sure: boolean; choices: [string, number][] };
/** A montage to render: clip pieces in order, an optional song. */
export type MontagePlan = {
  pieces: { path: string; start: number; end: number; volumeDb?: number }[];
  music: { path: string; offset: number; volumeDb: number } | null;
  clipVolumeDb: number;
  height: number;
  fps: number;
  /** Between clips: a cut, a fade through black, or a crossfade */
  transition: "cut" | "fade" | "crossfade";
  transitionSecs: number;
};
/** What this PC's NVIDIA GPU can encode. */
/** encoder: "NVIDIA NVENC" | "AMD AMF" | "Intel Quick Sync" | "Windows Media Foundation" | "OpenH264 (CPU)"; native: the native engine works here (NVIDIA only). */
export type GpuEncoders = { gpu: string; encoder: string; native: boolean; supported: Exclude<Encoder, "auto">[]; auto: Exclude<Encoder, "auto"> };
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
  fixedMonitorId: string;
  drawCursor: boolean;
  /** Part of the screen to record (fractions); null = all of it. */
  captureArea: CropRect | null;
  hotkey: string;
  recordHotkey: string;
  bufferEnabled: boolean;
  launchOnStartup: boolean;
  autoUpdate: boolean;
  saveSound: boolean;
  /** New clips' file names: "{game} {date} {time}" by default (see naming.rs for the tokens) */
  fileNameTemplate: string;
  /** Add this PC's name to new clips' names */
  fileNamePc: boolean;
  nasDir: string;
  /** Id in the confirmed NAS folder's marker file. */
  nasLibraryId: string;
  /** The paired NAS app's address, "" = not paired. */
  nasServer: string;
  nasRemoveLocal: boolean;
  nasSyncWhileGaming: boolean;
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
  detectFolders?: string[];
  /** Folders that aren't games (your own): never learned from or offered by the game finder */
  notGameFolders?: string[];
  /** Clips taken off "To trim" */
  keepWhole: string[];
  /** Game logos from Steam (only the game's name is sent) */
  onlineGameLogos: boolean;
  startMinimized: boolean;
  keepOriginalAfterRemux: boolean;
  replaceOriginalOnEdit: boolean;
  detailedLog: boolean;
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
/** The NAS app adding All audio tracks to older clips (server/src/mixtracks.js). */
export type MixTracksStatus = { running: boolean; checked: number; total: number; added: number; failed: number; lastError: string | null; finishedAt: number | null };

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

export type MoveStatus = { running: boolean; verb: string; label: string; done: number; total: number; paused: boolean; pausedFor?: string | null; forced?: boolean; stopping: boolean; finished: string | null };
export type NasEvent = { at: number; kind: "ok" | "info" | "bad"; text: string; game?: string };
export type NasState = {
  /** Just started: still looking for the NAS. */
  connecting?: boolean;
  /** What happened lately, newest first (nas::NasEvent). */
  recent?: NasEvent[];
  /** The game the copying is waiting for */
  pausedFor?: string | null;
  /** Can't reach it because the NAS turned down the saved password (changed on the NAS) */
  loginRejected?: boolean;
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
  state: "ok" | "damaged" | "unreadable" | "novideo";
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
/** Catbox takes clips up to this size (bytes); bigger ones go to Litterbox. */
export const CATBOX_LIMIT = 200 * 1_000_000;
/** Why a host can't take a clip of this size, or null if it can. */
export function hostRefuses(host: UploadHost, bytes: number | undefined): string | null {
  if (host === "catbox" && bytes != null && bytes > CATBOX_LIMIT)
    return `This clip is ${Math.ceil(bytes / 1_000_000)} MB, and Catbox takes up to 200 MB. Use Litterbox (up to 1 GB), or trim it first.`;
  return null;
}
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
/** A shared folder on the NAS, from signing in. */
export type NasShare = {
  name: string;
  comment: string;
  /** `\\host\name` */
  path: string;
};

/** The login Windows has saved for a NAS (the user name only). */
export type NasSavedLogin = {
  host: string;
  user: string;
};

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
  /** The screen's hardware id: stays the same when Windows renumbers the displays. */
  id: string;
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
// Asked until it answers: right at startup the server may not be listening yet, and
// asking only once left the player on Tauri's file protocol for the whole session,
// which can't play clips on the NAS (and is limited to the app's own data folder).
// If it never starts (about 15 s of asking, or an empty answer meaning "none"), clips
// fall back to the file protocol rather than waiting forever.
let mediaBase: string | null = null;
let noMediaServer = false;
const mediaWaiters = new Set<() => void>();
const mediaSettled = () => {
  mediaWaiters.forEach((f) => f());
  mediaWaiters.clear();
};
const askMediaBase = (tries = 0) => {
  const again = () => {
    if (tries >= 12) {
      noMediaServer = true;
      mediaSettled();
    } else setTimeout(() => askMediaBase(tries + 1), Math.min(2000, 100 * 2 ** tries));
  };
  invoke<string | null>("media_base")
    .then((b) => {
      if (typeof b === "string" && b) {
        mediaBase = b;
        mediaSettled();
      } else if (b === "") {
        noMediaServer = true;
        mediaSettled();
      } else again();
    })
    .catch(again);
};
askMediaBase();
/** Is it known yet where the player loads clips from? */
export const mediaReady = () => !!mediaBase || noMediaServer;
/** Call `f` once the player's address is known (right away if it is). */
export function whenMediaReady(f: () => void) {
  if (mediaReady()) f();
  else mediaWaiters.add(f);
  return () => {
    mediaWaiters.delete(f);
  };
}

/** The address the player loads a clip (or audio track) from. */
export function mediaSrc(path: string, bust?: string | number): string | undefined {
  if (noMediaServer) return `${convertFileSrc(path)}${bust != null ? `?v=${bust}` : ""}`;
  if (!mediaBase) return undefined; // App re-renders once it's known (useMediaReady)
  return `${mediaBase}${encodeURIComponent(path)}${bust != null ? `&v=${encodeURIComponent(String(bust))}` : ""}`;
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
  /** Sign in to the NAS (saving the password in Credential Manager with `remember`) and list its shared folders. Empty password: use the saved one. */
  nasSignIn: (host: string, user: string, password: string, remember: boolean) =>
    invoke<NasShare[]>("nas_sign_in", { host, user, password, remember }),
  /** Open a share (or a folder in it, made if needed); returns the folder's path. */
  nasOpenShare: (path: string, user: string, password: string) => invoke<string>("nas_open_share", { path, user, password }),
  nasSavedLogin: (host: string) => invoke<NasSavedLogin | null>("nas_saved_login", { host }),
  nasForgetLogin: (host: string) => invoke<void>("nas_forget_login", { host }),
  /** A small PNG of the screen being recorded (refused while a game runs). */
  screenPreview: () => invoke<ArrayBuffer>("screen_preview"),
  saveClip: (seconds?: number) => invoke<string>("save_clip", { seconds: seconds ?? null }),
  toggleRecording: () => invoke<string | null>("toggle_recording"),
  listClips: () => invoke<ClipFile[]>("list_clips"),
  /** `tracked`: the caller registers with `want` (cards); anyone else always gets an answer. */
  /** Lengths already known for these clips (0 = not yet), in one call that reads no files. */
  /** Steam games named like what you typed (empty when Online game logos is off). */
  searchGames: (term: string) => invoke<string[]>("search_games", { term }),
  knownLengths: (clips: ClipFile[]) => invoke<number[]>("known_lengths", { clips: clips.map((c) => [c.path, c.size, c.modified]) }),
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
  addMixTracks: () => invoke<{ addedHere: number; failedHere: [string, string][]; nasStarted: boolean; nasError: string | null }>("add_mix_tracks"),
  mixTracksStatus: () => invoke<MixTracksStatus | null>("mix_tracks_status"),
  detectStatus: () => invoke<DetectStatus>("detect_status"),
  detectInstall: () => invoke<void>("detect_install"),
  gameSuggestions: () => invoke<GameSuggestion[]>("game_suggestions"),
  applyGameSort: (picks: [string, string][]) => invoke<MoveOutcome>("apply_game_sort", { picks }),
  /** Selected clips into one folder (new or existing), on this PC and the NAS together; undoable. */
  moveToFolder: (paths: string[], folder: string) => invoke<MoveOutcome>("move_to_folder", { paths, folder }),
  /** Rename a clip (and its NAS copy); `name` without the extension. Returns its new path. */
  renameClip: (path: string, name: string) => invoke<string>("rename_clip", { path, name }),
  /** What a clip saved now would be called with this template. */
  previewFileName: (template: string, pc: boolean) => invoke<string>("preview_file_name", { template, pc }),
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
  openLogFolder: () => invoke<void>("open_log_folder"),
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
  /** Let the game finder look through one of your own folders */
  findGameInFolder: (folder: string) => invoke<void>("find_game_in_folder", { folder }),
  /** A game's folder: its clips that look like another game, from what's been looked at (instant). */
  checkGameFolder: (folder: string) => invoke<FolderCheck>("check_game_folder", { folder }),
  /** The same after looking at every clip in it ("folder-scan-progress" (done, total)). */
  scanGameFolder: (folder: string) => invoke<FolderCheck>("scan_game_folder", { folder }),
  cancelFolderScan: () => invoke<void>("cancel_folder_scan"),
  /** A move into Hindsight running in the background (or the last one's result). */
  moveStatus: () => invoke<MoveStatus | null>("move_status"),
  stopMove: () => invoke<void>("stop_move"),
  /** Carry on with the background move while a game is open */
  moveAnyway: () => invoke<void>("move_anyway"),
  /** A copy of the clip that scrubs smoothly (keyframes close together), or null when it doesn't need one. */
  smoothCopy: (path: string) => invoke<string | null>("smooth_copy", { path }),
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
