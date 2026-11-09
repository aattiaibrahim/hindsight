import pkg from "../package.json";
// Browser preview mode (dev only): when the UI runs in a normal browser
// instead of inside Hindsight, fake the backend with sample clips so the
// interface can be clicked through without touching real files.

type Clip = {
  path: string; name: string; folder: string; size: number; modified: number; external: boolean;
  location: string; onNas: boolean; source: string;
};

export function installDevMock() {
  const w = window as unknown as Record<string, unknown>;
  if (w.__TAURI_INTERNALS__) return; // real app

  const now = Date.now();
  const day = 86_400_000;
  let clips: Clip[] = [
    ["Counter-Strike 2", "Mirage AWP flick", 0.1, 61],
    ["VALORANT", "1v3 clutch", 0.2, 48],
    ["Overwatch 2", "5 kill streak", 0.3, 92],
    ["Rainbow Six Siege", "Scoped clutch", 0.4, 22],
    ["Marvel Rivals", "Team-up ult", 1.1, 75],
    ["Hunt Showdown", "Bayou extract", 1.2, 40],
    ["Payday The Heist", "Clean heist", 2.2, 18],
  ].map(([folder, name, ago, mb]) => ({
    path: `C:\\Users\\you\\Videos\\Hindsight\\${folder}\\${name}.mkv`,
    name: name as string,
    folder: folder as string,
    size: (mb as number) * 1024 * 1024,
    modified: now - (ago as number) * day,
    external: false,
    location: "local",
    onNas: (ago as number) > 1,
    source: "",
  }));
  // A clip from another PC on the NAS, and one from Medal's folder.
  clips.push(
    { path: "C:\\Users\\you\\nas\\VALORANT\\VALORANT 2026-11-02 - OTHER-PC.mkv", name: "VALORANT 2026-11-02 - OTHER-PC", folder: "VALORANT", size: 52 << 20, modified: now - 0.5 * day, external: false, location: "nas", onNas: false, source: "NAS" },
    { path: "C:\\Users\\you\\Medal\\Clips\\Valorant\\Medal clip.mp4", name: "Medal clip", folder: "Valorant", size: 30 << 20, modified: now - 0.6 * day, external: true, location: "linked", onNas: false, source: "Medal" },
  );
  // Clips from this date one and two years back, for "On this day".
  for (const [years, g, name] of [[1, "VALORANT", "Ace on Ascent"], [1, "VALORANT", "Operator flick"], [2, "Counter-Strike 2", "1v3 clutch"]] as const) {
    const d = new Date(now);
    d.setFullYear(d.getFullYear() - years);
    clips.push({
      path: `C:\\Users\\you\\Videos\\Hindsight\\${g}\\${name}.mkv`, name, folder: g, size: 60 << 20, modified: d.getTime() - 3600_000,
      external: false, location: "local", onNas: true, source: "",
    });
  }
  // A few months of older clips, so the date timeline has something to show.
  const games = ["VALORANT", "Counter-Strike 2", "Overwatch 2", "Rainbow Six Siege", "Marvel Rivals", "Hunt Showdown", "Payday The Heist"];
  for (let i = 0; i < 40; i++) {
    const ago = 3 + Math.floor(i * 3.1) + (i % 3) * 0.3;
    const g = games[i % games.length];
    clips.push({
      path: `C:\\Users\\you\\Videos\\Hindsight\\${g}\\${g} old ${i}.mkv`, name: `${g} ${i + 1}`, folder: g,
      size: (20 + (i * 7) % 90) << 20, modified: now - ago * day, external: false, location: "local", onNas: false, source: "",
    });
  }
  // ?many=N: a library of N clips, to time the app with a real-sized library.
  const many = Number(new URLSearchParams(location.search).get("many") || 0);
  const medal = new URLSearchParams(location.search).has("medal");
  for (let i = 0; i < many; i++) {
    const g = games[i % games.length];
    clips.push({
      path: `C:\\Users\\you\\Videos\\Hindsight\\${g}\\${g} big ${i}.mkv`, name: `${g} clip ${i + 1}`, folder: g,
      size: (40 + (i * 13) % 400) << 20, modified: now - (i * 0.37 + 0.2) * day, external: false, location: "local", onNas: i % 2 === 0, source: "",
    });
  }
  clips.sort((a, b) => b.modified - a.modified); // newest first, like Hindsight's own list
  const bin: Clip[] = [];
  const uploads: Record<string, unknown[]> = {};
  const secrets: Record<string, boolean> = {};
  const settings = {
    clipsDir: "C:\\Users\\you\\Videos\\Hindsight", encoder: "auto", quality: 24, fps: 60, bufferSeconds: 120,
    bufferInRam: true, captureEngine: "native", monitorMode: "primary", fixedMonitor: "", drawCursor: true, captureArea: null, hotkey: "Alt+F10",
    recordHotkey: "Alt+F9", bufferEnabled: true, launchOnStartup: false, autoUpdate: true, startMinimized: true,
    keepOriginalAfterRemux: true, replaceOriginalOnEdit: false, externalFiles: [], desktopAudio: true, discordTrack: true,
    micEnabled: true, micMono: true, micDevice: "", micGainDb: 0, theme: "tinted", accent: "winblue", keepWhole: [], reduceMotion: false,
    saveSound: true, nasDir: "\\\\nas\\clips", nasLibraryId: "mock", nasServer: "", nasRemoveLocal: false, linkedFolders: [] as string[], benchmarkMode: false, uploadHost: "catbox", litterboxTime: "72h", autoUpload: false, postLinkToDiscord: false, streamableEmail: "", discordWebhookName: "", confirmUploads: true, healthCheck: true, prepareThumbnails: true, gameDetect: true, detectDismissed: [] as string[],
    folderNames: {} as Record<string, string>, similarDismissed: [] as string[], cleanupKept: {} as Record<string, number>,
  };
  // Same game under two folder names (preview of the merge prompt).
  let similar = [
    { key: "tomclancysrainbowsixsiege", suggested: "Tom Clancy's Rainbow Six  Siege", names: [{ name: "Tom Clancy's Rainbow Six  Siege", here: 0, nas: 29 }, { name: "Tom Clancys Rainbow Six Siege", here: 27, nas: 0 }] },
    { key: "valorant", suggested: "VALORANT", names: [{ name: "VALORANT", here: 91, nas: 0 }, { name: "Valorant", here: 0, nas: 6 }] },
  ];
  const status = {
    enabled: true, running: true, monitor: "Display 1 · 1920×1080", width: 1920, height: 1080, encoder: "av1",
    fps: 60, bufferedSeconds: 120, bufferSeconds: 120, restarts: 0, lastError: null,
    audio: ["Desktop: Headphones", "Mic (mono): USB Mic"], audioError: null, game: null, recording: false,
    recordingSeconds: 0, inRam: true, bufferMb: 410, engine: "native",
  };
  const pendingUpdate = new URLSearchParams(location.search).has("update");
  if (pendingUpdate) Object.assign(status, { recording: true, recordingSeconds: 151, game: "VALORANT" });
  const listeners = new Map<string, number[]>();
  const callbacks = new Map<number, (e: unknown) => void>();
  let nextCb = 1;
  // Screenshot helpers (preview only): ?demo=studio opens the first clip,
  // ?demo=settings-<tab> opens that Settings tab.
  const demo = new URLSearchParams(location.search).get("demo") ?? "";
  if (demo.startsWith("settings-")) {
    try {
      localStorage.setItem("rewind.settingsTab", demo.slice(9));
    } catch {
      /* fine */
    }
  }
  if (demo === "performance") settings.benchmarkMode = true;
  if (demo) {
    setTimeout(() => {
      const send = (event: string, payload: unknown) =>
        (listeners.get(event) ?? []).forEach((id) => callbacks.get(id)?.({ event, id: 0, payload }));
      if (demo === "studio") send("open-in-studio", clips[0].path);
      if (demo.startsWith("settings-")) send("show-view", "settings");
      if (demo === "performance") send("show-view", "performance");
    }, 900);
  }  const emit = (event: string) =>
    (listeners.get(event) ?? []).forEach((id) => callbacks.get(id)?.({ event, id: 0, payload: null }));
  // Like the real app: the buffer's status arrives every second.
  setInterval(() => {
    (listeners.get("engine-status") ?? []).forEach((id) => callbacks.get(id)?.({ event: "engine-status", id: 0, payload: { ...status } }));
  }, 1000);

  const handlers: Record<string, (a: Record<string, unknown>) => unknown> = {
    get_settings: () => ({ ...settings }),
    set_settings: (a) => {
      Object.assign(settings, a.settings as object);
      return null;
    },
    get_status: () => status,
    // ?update in the URL: pretend an update is downloaded while a recording runs.
    get_update_state: () =>
      pendingUpdate
        ? { current: "0.1.12", stage: "ready", version: "0.1.13", notes: "Sample release notes.", error: null }
        : { current: pkg.version, stage: "up-to-date", version: null, notes: null, error: null },
    "plugin:opener|open_url": () => null,
    get_nas_state: () =>
      settings.nasDir
        ? new URLSearchParams(location.search).has("freeing")
          ? { enabled: true, reachable: true, pending: 0, copying: "OBS/OBS 2026-07-19 04-09-26.mp4", copied: 0, lastError: null, lastSync: Date.now() - 20_000, paused: false, totalFiles: 0, doneFiles: 0, totalBytes: 0, doneBytes: 0, pendingBytes: 0, checking: true, rate: 0, etaSecs: null, freeDone: 119, freeTotal: 298, freeing: true, stopped: false }
          : { enabled: true, reachable: true, pending: 1, copying: null, copied: 3, lastError: null, lastSync: Date.now() - 20_000, paused: false, totalFiles: 0, doneFiles: 0, totalBytes: 0, doneBytes: 0, pendingBytes: 0, checking: false, rate: 0, etaSecs: null, freeDone: 0, freeTotal: 0, freeing: false, stopped: false }
        : { enabled: false, reachable: false, pending: 0, copying: null, copied: 0, lastError: null, lastSync: null, paused: false, totalFiles: 0, doneFiles: 0, totalBytes: 0, doneBytes: 0, pendingBytes: 0, checking: false, rate: 0, etaSecs: null, freeDone: 0, freeTotal: 0, freeing: false, stopped: false },
    sync_nas_now: () => null,
    stop_nas_sync: () => null,
    "plugin:dialog|open": () => "\\\\nas\\clips",
    nas_check: (a) => ({
      dir: a.dir, reachable: true, writable: true, clips: 412, clipBytes: 268e9, games: 9, otherFiles: 0, otherExamples: [],
      toCopy: 38, toCopyBytes: 21.4e9, knownLibrary: true, serverGuess: "http://nas:8420", onThisPc: false, warnings: [],
    }),
    nas_confirm: () => "mock",
    nas_pair: (a) => {
      if (String(a.code).replace(/\s/g, "") !== "123456") throw "That code isn't right.";
      return "Paired with the NAS app (412 clips there).";
    },
    run_diagnostics: () => "C:\\Users\\you\\Downloads\\Hindsight diagnostics 2026-11-06 12-00-00.txt",
    find_duplicates: () => [],
    edited_clips: () => [],
    system_accent: () => "#4cc2ff",
    media_base: () => null,
    old_folders: () => [{ path: "C:\\Medal", label: "Medal", moved: 42, files: 3, bytes: 18_400_000 }],
    remove_old_folder: () => null,
    set_game_logo: () => null,
    detect_status: () => ({ installed: true, downloading: null, lookedAt: 51, toLookAt: 53, error: null }),
    game_suggestions: () =>
      clips.filter((c) => c.folder === "Hunt Showdown").slice(0, 3).map((c, i) => ({ path: c.path, name: c.name, folder: "OBS", game: ["VALORANT", "Counter-Strike 2", "Rainbow Six Siege"][i], confidence: 0.92 - i * 0.07, likeness: 0.94 })),
    apply_game_sort: (a) => ({ moved: (a.picks as unknown[]).length, failed: [] }),
    dismiss_game_suggestions: () => null,
    render_montage: async () => (await new Promise((r) => setTimeout(r, 1200)), "C:\\Users\\you\\Videos\\Hindsight\\Montages\\Montage 2026-11-06 12-00-00.mp4"),
    cancel_montage: () => null,
    game_icon: () => null,
    gpu_encoders: () => ({ gpu: "NVIDIA GeForce RTX 3070", supported: ["hevc", "h264"], auto: "hevc" }),
    // Uploads (preview only): pretend to prepare and upload, with progress events, then give a fake link.
    upload_clip: async (a) => {
      const path = a.path as string, host = (a.host as string) ?? settings.uploadHost;
      const send = (stage: string, sent = 0, total = 0, url: string | null = null) =>
        (listeners.get("upload-progress") ?? []).forEach((id) => callbacks.get(id)?.({ event: "upload-progress", id: 0, payload: { path, host, stage, sent, total, url, error: null } }));
      send("preparing");
      await new Promise((r) => setTimeout(r, 600));
      for (let i = 1; i <= 5; i++) { send("uploading", i * 2e6, 1e7); await new Promise((r) => setTimeout(r, 250)); }
      const up = { host, url: "https://files.catbox.moe/preview.mp4", at: Date.now(), bytes: 1e7, expires: null };
      (uploads[path] ??= []).push(up);
      send("done", 1e7, 1e7, up.url);
      return up;
    },
    upload_history: () => uploads,
    clip_health: (a) => Object.fromEntries((a.paths as string[]).filter((p) => p.includes("Bayou")).map((p) => [p, { state: "damaged", damaged: [[0, 70]], best: [70, 180.2], duration: 180.2 }])),
    health_progress: () => ({ checked: 31, total: 49, damaged: 1, paused: false, running: true }),
    rescue_clip: (a) => (a.path as string).replace(/\.(\w+)$/, " (rescued).$1"),
    heal_clip: async (a) => (await new Promise((r) => setTimeout(r, 900)), a.path as string),
    export_audio: (a) => (a.path as string).replace(/\.(\w+)$/, " (audio).mp3"),
    copy_text: () => null,
    set_secret: (a) => { secrets[a.name as string] = !!a.value; return null; },
    check_sharing: () => [["catbox", [{ host: "catbox.moe", state: "ok" }, { host: "files.catbox.moe", state: "blocked" }]], ["litterbox", [{ host: "litter.catbox.moe", state: "ok" }]], ["discord", [{ host: "discord.com", state: "ok" }]], ["streamable", [{ host: "streamable.com", state: "ok" }]]],
    secrets_set: () => ({ discordWebhook: !!secrets.discord_webhook, streamablePassword: !!secrets.streamable_password }),
    plan_move_into_rewind: () => ({
      label: "Medal", count: 42, bytes: 3.2e9, otherDrive: false,
      folders: [["VALORANT", 30], ["Tom Clancys Rainbow Six Siege", 12]],
      examples: [["Valorant_20261028_213344.mp4", "VALORANT\\VALORANT 2026-10-28 21-33-44.mp4"]],
    }),
    move_into_rewind: () => ({ moved: 42, failed: [] }),
    last_move: () => null,
    find_clip_folders: async () => {
      await new Promise((r) => setTimeout(r, 900));
      return [
        { path: "E:\\Old PC\\Medal", label: "Medal", drive: "E:", clips: 214, bytes: 61.4e9, shown: false },
        { path: "E:\\recording", label: "recording", drive: "E:", clips: 37, bytes: 18.9e9, shown: false },
        { path: "D:\\Videos\\Captures", label: "Captures", drive: "D:", clips: 12, bytes: 2.1e9, shown: true },
      ];
    },
    plan_import: (a) => ({
      label: (a.path as string).split("\\").pop(), count: 37, bytes: 18.9e9, otherDrive: true, copy: !!a.copy,
      folders: [["VALORANT", 20], ["recording", 17]],
      examples: [["2024-01-01 10-00-00.mkv", "recording\\recording 2024-01-01 10-00-00.mkv"]],
    }),
    import_folder: async () => (await new Promise((r) => setTimeout(r, 900)), { moved: 37, failed: [] }),
    shrink_clip: async (a) => (await new Promise((r) => setTimeout(r, 900)), (a.path as string).replace(/\.\w+$/, ".mp4")),
    similar_game_folders: () => similar.filter((g) => !settings.similarDismissed.includes(g.key)),
    merge_game_folders: async (a) => {
      await new Promise((r) => setTimeout(r, 700));
      similar = similar.filter((g) => g.key !== a.key);
      settings.folderNames[a.key as string] = a.target as string;
      (listeners.get("clips-changed") ?? []).forEach((id) => callbacks.get(id)?.({ event: "clips-changed", id: 0, payload: null }));
      return { moved: 27, failed: [] };
    },
    keep_folders_separate: (a) => {
      if (a.key) settings.similarDismissed.push(a.key as string);
      else settings.similarDismissed = [];
      (listeners.get("settings-changed") ?? []).forEach((id) => callbacks.get(id)?.({ event: "settings-changed", id: 0, payload: settings }));
      return null;
    },
    undo_last_move: () => ({ moved: 42, failed: [] }),
    recorder_clip_folders: () => [
      { label: "Medal", path: "C:\\Medal", exists: true },
      { label: "OBS", path: "C:\\Users\\you\\Videos\\OBS", exists: true },
      { label: "NVIDIA", path: "C:\\Users\\you\\Videos\\NVIDIA", exists: false },
    ],
    install_update: () => {
      throw "Preview mode: nothing to install.";
    },
    list_clips: () => clips,
    list_monitors: () => [],
    screen_preview: () => fetch("/thumb-cs2a.jpg").then((r) => r.arrayBuffer()),
    list_audio_inputs: () => [],
    // ?medal: a Medal clip, "All Audio" (the others mixed) plus Game and Mic.
    probe_file: () => ({ duration: 45, codec: "av1", width: 1920, height: 1080, fps: 60, audioTracks: medal ? 3 : 2, container: "matroska" }),
    // Thumbnails matched to each game (frames picked from real clips for the README screenshots).
    thumbnail: (a) => {
      const p = a.path as string;
      const by: [string, string[]][] = [["VALORANT", ["valorant"]], ["Counter-Strike", ["cs2a", "cs2b"]], ["Overwatch", ["ow1", "ow2"]], ["Rainbow", ["r6"]], ["Marvel", ["marvel"]], ["Hunt", ["hunt"]], ["Payday", ["payday"]]];
      const list = by.find(([k]) => p.toLowerCase().includes(k.toLowerCase()))?.[1] ?? ["cs2a"];
      return `/thumb-${list[p.length % list.length]}.jpg`;
    },
    // A made-up 45 s game track (gunfire bursts) and mic (talking in stretches).
    clip_waveforms: () => {
      const n = 45 * 20;
      const game = Array.from({ length: n }, (_, i) => {
        const burst = [6, 7, 14, 22, 23, 31, 38].some((s) => i / 20 >= s && i / 20 < s + 1.2);
        return Math.round(burst ? 200 + Math.random() * 50 : 95 + Math.sin(i / 9) * 20 + Math.random() * 25);
      });
      const mic = Array.from({ length: n }, (_, i) => {
        const talk = [2, 10, 18, 27, 35].some((s) => i / 20 >= s && i / 20 < s + 4);
        return Math.round(talk ? 120 + Math.random() * 100 : Math.random() * 20);
      });
      if (medal) return [{ step: 0.05, levels: game.map((g, i) => Math.min(255, g + mic[i])) }, { step: 0.05, levels: game }, { step: 0.05, levels: mic }];
      return [{ step: 0.05, levels: game }, { step: 0.05, levels: mic }];
    },
    preview_audio: () =>
      medal
        ? {
            firstTitle: "All Audio",
            extra: [
              { index: 1, title: "Game", path: "C:\\Users\\you\\preview\\game.m4a" },
              { index: 2, title: "Mic", path: "C:\\Users\\you\\preview\\mic.m4a" },
            ],
            mix: 0,
          }
        : {
            firstTitle: "Desktop audio",
            extra: [{ index: 1, title: "Microphone", path: "C:\\Users\\you\\preview\\mic.m4a" }],
          },
    delete_clips: (a) => {
      const paths = a.paths as string[];
      bin.push(...clips.filter((c) => paths.includes(c.path)));
      clips = clips.filter((c) => !paths.includes(c.path));
      setTimeout(() => emit("clips-changed"));
      return { deleted: paths, failed: [] };
    },
    restore_clips: (a) => {
      const paths = a.paths as string[];
      const back = bin.filter((c) => paths.includes(c.path));
      clips = [...clips, ...back].sort((x, y) => y.modified - x.modified);
      setTimeout(() => emit("clips-changed"));
      return back.length;
    },
    bench_state: () => [false, sampleBenchReport()],
    start_bench: () => null,
    cancel_bench: () => null,
    "plugin:event|listen": (a) => {
      const ev = a.event as string;
      listeners.set(ev, [...(listeners.get(ev) ?? []), a.handler as number]);
      return a.handler;
    },
    "plugin:event|unlisten": (a) => {
      const ev = a.event as string;
      listeners.set(ev, (listeners.get(ev) ?? []).filter((id) => id !== (a.eventId as number)));
      return null;
    },
  };

  w.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
    transformCallback: (cb: (e: unknown) => void) => {
      const id = nextCb++;
      callbacks.set(id, cb);
      return id;
    },
    unregisterCallback: (id: number) => callbacks.delete(id),
    // Every sample clip plays the same generated test video (public/preview-sample.mkv:
    // AV1 1080p60 with a keyframe every second, like Hindsight's own clips).
    convertFileSrc: (p: string) =>
      p.endsWith("mic.m4a") ? "/preview-mic.m4a" : p.startsWith("C:\\Users\\you\\") ? "/preview-sample.mkv" : p,
    invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
      const h = handlers[cmd];
      if (!h) {
        console.warn("[preview] no mock for", cmd);
        return null;
      }
      return h(args);
    },
  };
  w.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
  console.info("[preview] Hindsight UI running with fake sample clips");
}

// A believable 3-round result so the Performance page can be laid out in preview.
function sampleBenchReport() {
  const st = (mean: number, sd: number, n = 3) => ({ mean, sd, n, ci95: n > 1 ? ((n === 2 ? 12.706 : 4.303) * sd) / Math.sqrt(n) : null });
  const fr = (fps: number, gpu = 0.96) => ({
    frames: Math.round(fps * 20), avgFps: fps, low1Fps: fps * 0.71, avgFrameMs: 1000 / fps,
    gpuBusyMs: (1000 / fps) * gpu, cpuBusyMs: (1000 / fps) * 0.55, syncInterval: 0, presentMode: "Hardware: Independent Flip",
  });
  const orders = [["obs", "rewind", "medal", "nvidia"], ["rewind", "nvidia", "obs", "medal"], ["nvidia", "medal", "rewind", "obs"]];
  const hit: Record<string, number> = { rewind: 0.004, obs: 0.021, nvidia: 0.009, medal: 0.017 };
  const runs: unknown[] = [];
  let t = 0;
  let base = 282;
  const push = (round: number, app: string | null, fps: number, excluded: string | null = null) => {
    runs.push({ round, app, atSecs: t, frames: fr(fps), apps: {}, excluded });
    t += 34;
  };
  push(1, null, base);
  orders.forEach((order, ri) =>
    order.forEach((app, ai) => {
      if (app === "medal" && ri === 2) {
        push(ri + 1, app, 0, "Medal wasn't recording for the whole run.");
      } else push(ri + 1, app, base * (1 - hit[app]) + ((ai * 7 + ri * 3) % 5) - 2);
      base -= 0.35;
      push(ri + 1, null, base + ((ai * 5 + ri) % 3) - 1);
    }),
  );
  return {
    game: "VALORANT-Win64-Shipping", started: "2026-10-30 21:10", seconds: 20, rounds: 3, cores: 16, warmupSecs: 20, minutes: 17.4,
    runs,
    baseline: { runs: 13, fps: st(280.0, 1.6, 13), low1: st(198.8, 2.9, 13), noisePercent: 0.5, driftPercent: -1.5, gpuBusyShare: 0.96, cpuBusyShare: 0.55, vsync: false, presentMode: "Hardware: Independent Flip" },
    results: [
      { app: "rewind", runs: 3, fpsCost: st(0.4, 0.5), low1Cost: st(0.6, 1.2), gpuMsAdded: st(0.01, 0.01), cpuPercent: st(0.14, 0.03), gpu3dPercent: st(1.4, 0.2), encoderPercent: st(8.9, 0.4), ramMb: st(688, 4), verdict: "noMeasurableCost" },
      { app: "obs", runs: 3, fpsCost: st(2.1, 0.4), low1Cost: st(2.8, 1.1), gpuMsAdded: st(0.07, 0.01), cpuPercent: st(0.78, 0.06), gpu3dPercent: st(2.3, 0.3), encoderPercent: st(14.2, 0.6), ramMb: st(651, 9), verdict: "costs" },
      { app: "nvidia", runs: 3, fpsCost: st(0.9, 0.6), low1Cost: st(1.0, 1.4), gpuMsAdded: st(0.03, 0.01), cpuPercent: st(0.27, 0.04), gpu3dPercent: st(1.1, 0.2), encoderPercent: st(10.1, 0.5), ramMb: st(412, 6), verdict: "noMeasurableCost" },
      { app: "medal", runs: 2, fpsCost: st(1.7, 0.3, 2), low1Cost: st(2.2, 0.9, 2), gpuMsAdded: st(0.06, 0.02, 2), cpuPercent: st(1.9, 0.2, 2), gpu3dPercent: st(2.0, 0.3, 2), encoderPercent: st(11.0, 0.4, 2), ramMb: st(540, 12, 2), verdict: "noMeasurableCost" },
    ],
    settings: [
      { app: "rewind", width: 1920, height: 1080, fps: 60, codec: "AV1 (NVENC)", quality: "CQ 24", bufferSecs: 120, source: "Hindsight settings" },
      { app: "obs", width: 1920, height: 1080, fps: 60, codec: "AV1 (NVENC)", quality: "CQP 16", bufferSecs: 180, source: 'OBS profile "Untitled"' },
      { app: "nvidia", width: 1920, height: 1080, fps: 60, codec: null, quality: null, bufferSecs: 180, source: "NVIDIA Instant Replay settings (codec and bitrate aren't stored where Hindsight can read them)" },
      { app: "medal", width: null, height: null, fps: null, codec: "HEVC", quality: "7Mbps", bufferSecs: null, source: "Medal settings (resolution, FPS and clip length aren't stored where Hindsight can read them)" },
    ],
    skipped: [],
    warnings: [
      "Hindsight couldn't read Medal's resolution and frame rate. Check they match Hindsight's (1920×1080 at 60 fps).",
      "1 of 25 runs failed a check and were left out (see All runs).",
    ],
    error: null,
  };
}

