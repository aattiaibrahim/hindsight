import { open } from "@tauri-apps/plugin-dialog";
import { RecordingArea } from "./RecordingArea";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useState } from "react";
import { Accent, api, bufferEstimateMb, errText, fmtDb, GpuEncoders, Monitor, Settings, Theme } from "./api";
import { ConfirmDialog } from "./ConfirmUpdate";
import { LibrarySettings } from "./LibrarySettings";
import { SharingSettings } from "./Share";
import { Slider } from "./Slider";
import { useUpdate } from "./hooks";
import { SlidingPill } from "./SlidingPill";
import { About } from "./About";

function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <input type="checkbox" className="switch" aria-label={label} checked={checked} onChange={(e) => onChange(e.target.checked)} />
  );
}

function hotkeyFrom(e: React.KeyboardEvent): string | null {
  if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return null;
  let key = e.code;
  if (key.startsWith("Key")) key = key.slice(3);
  else if (key.startsWith("Digit")) key = key.slice(5);
  const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Super"].filter(Boolean);
  // A bare letter would fire while typing; require a modifier unless it's an F-key.
  if (!mods.length && !/^F\d+$/.test(key)) return null;
  return [...mods, key].join("+");
}

// Preview colours for each theme card (bg, panel, line).
const THEMES: { id: Theme; label: string; c: [string, string, string] }[] = [
  { id: "tinted", label: "Tinted", c: ["#000000", "#1a1d21", "#2a2e33"] },
  { id: "midnight", label: "Midnight", c: ["#000000", "#181818", "#2a2a2a"] },
  { id: "graphite", label: "Graphite", c: ["#0e0e0e", "#1f1f1f", "#313131"] },
  { id: "oled", label: "OLED black", c: ["#000000", "#0a0a0a", "#1c1c1c"] },
];

const ACCENTS: { id: Accent; label: string; c: string }[] = [
  { id: "winblue", label: "Windows blue", c: "#4cc2ff" },
  { id: "system", label: "Follow Windows", c: "#4cc2ff" },
  { id: "cyan", label: "Ice", c: "#38d6ff" },
  { id: "rose", label: "Crimson", c: "#ff4d6d" },
  { id: "lime", label: "Volt", c: "#c6f432" },
  { id: "amber", label: "Ember", c: "#ff9a1f" },
  { id: "violet", label: "Violet", c: "#a78bfa" },
  { id: "emerald", label: "Emerald", c: "#2ee59d" },
  { id: "gold", label: "Gold", c: "#ffd23f" },
  { id: "pink", label: "Hot pink", c: "#ff5fc8" },
  { id: "teal", label: "Teal", c: "#20c9c0" },
  { id: "mono", label: "Mono", c: "#f2f4f7" },
  { id: "green", label: "NVIDIA green", c: "#76b900" },
];

const QUALITY_PRESETS: [string, number][] = [
  ["Small", 30],
  ["Balanced", 26],
  ["High", 22],
  ["Max", 18],
];

function HotkeyInput({ value, label, onPick }: { value: string; label: string; onPick: (hk: string) => void }) {
  const [listening, setListening] = useState(false);
  return (
    <div className="setting">
      <span>
        {label}
        <small>{listening ? "Press the keys now… (Esc to cancel)" : "Click, then press a combo"}</small>
      </span>
      <input
        className="text-input"
        aria-label={label}
        readOnly
        value={listening ? "…" : value}
        onFocus={() => setListening(true)}
        onBlur={() => setListening(false)}
        onKeyDown={(e) => {
          e.preventDefault();
          if (e.key === "Escape") return (e.target as HTMLInputElement).blur();
          const hk = hotkeyFrom(e);
          if (hk) {
            onPick(hk);
            (e.target as HTMLInputElement).blur();
          }
        }}
      />
    </div>
  );
}

/** What each "Which monitor" choice does when screens change (checked every second). */
const MONITOR_MODE_HINT: Record<Settings["monitorMode"], string> = {
  primary:
    "Records whichever screen Windows calls the main display. After Win+P, or plugging a screen in or out, it moves to the new main display within about a second.",
  cursor: "Records the screen your mouse is on, switching within about a second when you move to another screen.",
  fixed:
    "Stays on the screen you pick below, whatever Win+P does. If that screen is unplugged it records the main display until it's back.",
};

const TABS = [
  ["general", "General"],
  ["appearance", "Appearance"],
  ["recording", "Recording"],
  ["audio", "Audio"],
  ["library", "Library"],
  ["sharing", "Sharing"],
  ["integrations", "Stream Deck"],
  ["advanced", "Advanced"],
  ["about", "About"],
] as const;
type TabId = (typeof TABS)[number][0];

export function SettingsView({
  settings,
  update,
  onError,
  onInstall,
  installing,
}: {
  settings: Settings;
  update: (p: Partial<Settings>) => Promise<void>;
  onError: (msg: string) => void;
  onInstall: () => void;
  installing: boolean;
}) {
  const [monitors, setMonitors] = useState<Monitor[]>([]);
  const [mics, setMics] = useState<string[]>([]);
  const [enc, setEnc] = useState<GpuEncoders | null>(null);
  const upd = useUpdate();
  const [checking, setChecking] = useState(false);
  const [tab, setTab] = useState<TabId>(() => {
    try {
      const t = localStorage.getItem("rewind.settingsTab") as TabId | null;
      return t && TABS.some(([id]) => id === t) ? t : "general";
    } catch {
      return "general";
    }
  });
  const pickTab = (t: TabId) => {
    setTab(t);
    try {
      localStorage.setItem("rewind.settingsTab", t);
    } catch {
      /* private mode: just don't remember */
    }
  };
  const [askRamOff, setAskRamOff] = useState(false);
  const keepRam = useCallback(() => setAskRamOff(false), []);
  const checkNow = async () => {
    setChecking(true);
    try {
      await api.checkForUpdates();
    } catch (e) {
      onError(errText(e));
    } finally {
      setChecking(false);
    }
  };
  const updateLine = (() => {
    switch (upd?.stage) {
      case "checking":
        return "Checking…";
      case "downloading":
        return `Downloading ${upd.version}…`;
      case "ready":
        return `Version ${upd.version} is downloaded. It installs when Hindsight restarts.`;
      case "available":
        return `Version ${upd.version} is available.`;
      case "up-to-date":
        return "You're up to date.";
      case "error":
        return `Couldn't check: ${upd.error}`;
      default:
        return "Checks GitHub after launch and every 6 hours.";
    }
  })();
  useEffect(() => {
    api.listMonitors().then(setMonitors).catch(() => {});
    api.listAudioInputs().then(setMics).catch(() => {});
    api.gpuEncoders().then(setEnc).catch(() => {});
  }, []);

  const set = (p: Partial<Settings>) => update(p).catch((e) => onError(String(e)));
  const s = settings;
  const screen =
    (s.monitorMode === "fixed" && monitors.find((m) => m.name === s.fixedMonitor)) ||
    monitors.find((m) => m.primary) ||
    monitors[0];
  const screenH = screen?.height ?? 0;
  const resolutionHint =
    s.outputHeight === 0 || s.outputHeight === screenH
      ? "Full screen resolution, captured entirely on the GPU."
      : screenH && s.outputHeight > screenH
        ? "Upscaled: no extra detail and bigger files, but YouTube gives 1440p+ uploads better quality."
        : "Scaled down: smaller files, and about 1–5% CPU for the resize.";

  const pickFolder = async () => {
    const dir = await open({ directory: true, title: "Where should clips be saved?", defaultPath: s.clipsDir });
    if (typeof dir === "string") set({ clipsDir: dir });
  };

  return (
    <div className="page">
      <h1>Settings</h1>
      <div className="tabs" role="tablist" aria-label="Settings sections">
        <SlidingPill />
        {TABS.map(([id, label]) => (
          <button key={id} role="tab" aria-selected={tab === id} className={`tab ${tab === id ? "on" : ""}`} onClick={() => pickTab(id)}>
            {label}
          </button>
        ))}
      </div>
      <div className="settings-grid" key={tab}>
        <About version={upd?.current ?? null} hidden={tab !== "about"} />
        <section className="group" hidden={tab !== "advanced"}>
          <h2>Advanced</h2>
          <label className="setting">
            <span>
              Benchmark mode
              <small>
                Shows the Performance page, which measures how much FPS Hindsight, OBS, NVIDIA Instant Replay and Medal cost
                your game.
              </small>
            </span>
            <Toggle label="Benchmark mode" checked={s.benchmarkMode} onChange={(v) => set({ benchmarkMode: v })} />
          </label>
        </section>
        <section className="group" hidden={tab !== "appearance"}>
          <h2>Appearance</h2>
          <div className="themes" role="radiogroup" aria-label="Theme">
            {THEMES.map((t) => (
              <button
                key={t.id}
                role="radio"
                aria-checked={s.theme === t.id}
                className={`theme-card ${s.theme === t.id ? "on" : ""}`}
                onClick={() => set({ theme: t.id })}
              >
                <div
                  className="theme-preview"
                  style={{ "--p-bg": t.c[0], "--p-panel": t.c[1], "--p-line": t.c[2] } as React.CSSProperties}
                >
                  <i />
                  <i />
                </div>
                {t.label}
              </button>
            ))}
          </div>
          <div className="setting">
            <span>
              Accent colour
              <small>
                {ACCENTS.find((a) => a.id === s.accent)?.label ?? "Windows blue"}
                {s.accent === "system" ? ": changes when you change your accent colour in Windows" : ""}
              </small>
            </span>
            <div className="swatches" role="radiogroup" aria-label="Accent colour">
              {ACCENTS.map((a) => (
                <button
                  key={a.id}
                  role="radio"
                  aria-checked={s.accent === a.id}
                  aria-label={a.label}
                  title={a.label}
                  className={`swatch ${s.accent === a.id ? "on" : ""} ${a.id === "system" ? "system" : ""}`}
                  style={{ "--c": a.id === "system" ? "var(--system-accent, #4cc2ff)" : a.c } as React.CSSProperties}
                  onClick={() => set({ accent: a.id })}
                />
              ))}
            </div>
          </div>
          <label className="setting">
            <span>
              Reduce motion
              <small>Turns off fades and hover effects. They're already light: they only run while the window is open.</small>
            </span>
            <Toggle label="Reduce motion" checked={s.reduceMotion} onChange={(v) => set({ reduceMotion: v })} />
          </label>
        </section>

        <section className="group" hidden={tab !== "general"}>
          <h2>General</h2>
          <label className="setting">
            <span>Launch with Windows</span>
            <Toggle label="Launch with Windows" checked={s.launchOnStartup} onChange={(v) => set({ launchOnStartup: v })} />
          </label>
          <label className="setting">
            <span>
              Start in the tray when launched with Windows
              <small>The replay buffer still starts right away</small>
            </span>
            <Toggle label="Start in tray" checked={s.startMinimized} onChange={(v) => set({ startMinimized: v })} />
          </label>
          <label className="setting">
            <span>Replay buffer</span>
            <Toggle label="Replay buffer on" checked={s.bufferEnabled} onChange={(v) => set({ bufferEnabled: v })} />
          </label>
          <label className="setting">
            <span>
              Play a sound when a clip is saved
              <small>
                A short chime when a clip or recording is saved, and a low tone if saving fails. Desktop audio records
                everything you hear, so the chime is also in later clips.
              </small>
            </span>
            <Toggle label="Save sound" checked={s.saveSound} onChange={(v) => set({ saveSound: v })} />
          </label>
          <HotkeyInput label="Save clip hotkey" value={s.hotkey} onPick={(hk) => set({ hotkey: hk })} />
          <HotkeyInput label="Start / stop recording hotkey" value={s.recordHotkey} onPick={(hk) => set({ recordHotkey: hk })} />
        </section>

        <section className="group" hidden={tab !== "general"}>
          <h2>Updates</h2>
          <div className="setting">
            <span>
              <span className="version-line">
                Hindsight <strong className="mono">{upd?.current ?? "…"}</strong>
              </span>
              <small>{updateLine}</small>
            </span>
            <span className="row" style={{ flexWrap: "nowrap", flexShrink: 0 }}>
              {(upd?.stage === "ready" || upd?.stage === "available") && (
                <button className="btn small primary" disabled={installing} onClick={onInstall}>
                  {installing ? "Updating…" : "Restart to update"}
                </button>
              )}
              <button className="btn small" disabled={checking || upd?.stage === "downloading"} onClick={checkNow}>
                {checking ? "Checking…" : "Check now"}
              </button>
            </span>
          </div>
          {upd?.notes && (upd.stage === "ready" || upd.stage === "available") && (
            <p className="update-notes">
              <strong>What's new in {upd.version}:</strong> {upd.notes}
            </p>
          )}
          <label className="setting">
            <span>
              Update automatically
              <small>
                Downloads in the background and installs when Hindsight closes. Never restarts on its own during a recording
                or while a game is open.
              </small>
            </span>
            <Toggle label="Update automatically" checked={s.autoUpdate} onChange={(v) => set({ autoUpdate: v })} />
          </label>
        </section>

        <section className="group" hidden={tab !== "recording"}>
          <h2>Recording</h2>
          <label className="setting">
            <span>
              Encoder
              <small>
                All run on your NVIDIA GPU (NVENC){enc?.gpu ? `: ${enc.gpu.replace(/^NVIDIA /, "")}` : ""}. Auto picks the best
                one it has. AV1 needs an RTX 40 series or newer.
              </small>
              {enc && s.encoder !== "auto" && enc.supported.length > 0 && !enc.supported.includes(s.encoder) && (
                <small className="err-text">Your GPU can't record {s.encoder.toUpperCase()}, so it records {enc.auto.toUpperCase()} instead.</small>
              )}
            </span>
            <select className="select" value={s.encoder} onChange={(e) => set({ encoder: e.target.value as Settings["encoder"] })}>
              <option value="auto">Auto{enc ? ` (${({ av1: "AV1", hevc: "HEVC", h264: "H.264" } as const)[enc.auto]})` : ""}</option>
              {(
                [
                  ["av1", "AV1 (smallest files)", "needs RTX 40 or newer"],
                  ["hevc", "HEVC / H.265", "needs GTX 950 or newer"],
                  ["h264", "H.264 (plays everywhere)", ""],
                ] as const
              ).map(([v, label, need]) => {
                const can = !enc || enc.supported.length === 0 || enc.supported.includes(v);
                return (
                  <option key={v} value={v} disabled={!can}>
                    {label}
                    {can ? "" : ` · ${need}`}
                  </option>
                );
              })}
            </select>
          </label>
          <label className="setting">
            <span>
              Clip resolution
              <small>{resolutionHint}</small>
            </span>
            <select className="select" value={s.outputHeight} onChange={(e) => set({ outputHeight: Number(e.target.value) })}>
              <option value={0}>Same as screen{screenH ? ` (${screenH}p)` : ""}</option>
              {[2160, 1440, 1080, 720].map((h) => (
                <option key={h} value={h}>
                  {h === 2160 ? "4K (2160p)" : `${h}p`}
                  {screenH && h > screenH ? " · upscaled" : ""}
                </option>
              ))}
            </select>
          </label>
          <div className="setting">
            <span>
              Quality
              <small>Higher means sharper but bigger files. Fine-tune with the slider (CQ {s.quality}).</small>
            </span>
            <div className="row" style={{ flexWrap: "wrap", justifyContent: "flex-end" }}>
              <SlidingPill />
              {QUALITY_PRESETS.map(([label, cq]) => (
                <button
                  key={label}
                  className={`chip ${s.quality === cq ? "on" : ""}`}
                  style={{ height: 30 }}
                  onClick={() => set({ quality: cq })}
                >
                  {label}
                </button>
              ))}
            </div>
          </div>
          <label className="setting">
            <span>
              Fine-tune: CQ {s.quality}
              <small>Lower = sharper and bigger. 20–26 is a good range.</small>
            </span>
            <Slider min={15} max={35} value={s.quality} aria-label="Quality" onValue={(v) => set({ quality: v })} />
          </label>
          <label className="setting">
            <span>Frame rate</span>
            <select className="select" value={s.fps} onChange={(e) => set({ fps: Number(e.target.value) })}>
              {[30, 60, 120, 144].map((f) => (
                <option key={f} value={f}>
                  {f} fps
                </option>
              ))}
            </select>
          </label>
          <label className="setting">
            <span>
              Keep the replay buffer in RAM
              <small>
                {s.bufferInRam
                  ? `No SSD wear. Uses about ${bufferEstimateMb(s)} MB of memory while gaming.`
                  : `Uses about ${bufferEstimateMb(s)} MB on disk and rewrites it constantly.`}
              </small>
            </span>
            {/* Turning RAM off moves the buffer to the SSD and rewrites it non-stop: ask first. */}
            <Toggle
              label="Buffer in RAM"
              checked={s.bufferInRam}
              onChange={(v) => (v ? set({ bufferInRam: true }) : setAskRamOff(true))}
            />
          </label>
          {askRamOff && (
            <ConfirmDialog
              title="Move the replay buffer to your SSD?"
              confirmLabel="Use disk"
              cancelLabel="Keep it in RAM"
              danger
              onConfirm={() => {
                setAskRamOff(false);
                set({ bufferInRam: false });
              }}
              onCancel={keepRam}
            >
              <ul className="modal-list wrap">
                <li>
                  Hindsight would write about {bufferEstimateMb(s)} MB to disk over and over the whole time it runs, which
                  wears out your SSD over the years.
                </li>
                <li>The capture restarts, so what's in the replay buffer now is cleared.</li>
                <li>Only worth it for very long buffers that don't fit in your memory.</li>
              </ul>
            </ConfirmDialog>
          )}
          {s.bufferInRam && bufferEstimateMb(s) > 4000 && (
            <div className="err-text" style={{ padding: "4px 0" }}>
              That's a lot of memory for one buffer. Consider a shorter replay, or turn RAM off for very long buffers.
            </div>
          )}
          <label className="setting">
            <span>
              Replay length
              <small>How far back a saved clip reaches</small>
            </span>
            <select className="select" value={s.bufferSeconds} onChange={(e) => set({ bufferSeconds: Number(e.target.value) })}>
              {[15, 30, 60, 120, 180, 300, 600, 1200].map((v) => (
                <option key={v} value={v}>
                  {v < 60 ? `${v} seconds` : `${v / 60} min`}
                </option>
              ))}
            </select>
          </label>
        </section>

        <section className="group" hidden={tab !== "integrations"}>
          <h2>Stream Deck &amp; other tools</h2>
          <div className="setting">
            <span>
              Hindsight plugin for Stream Deck
              <small>Buttons for Save clip, Record (lights up red), Replay buffer and Quality.</small>
            </span>
            <button
              className="btn small"
              onClick={() =>
                openUrl("https://github.com/aattiaibrahim/hindsight/releases/latest/download/Hindsight.streamDeckPlugin").catch((e) =>
                  onError(errText(e)),
                )
              }
            >
              Get plugin
            </button>
          </div>
          <div className="setting">
            <span>
              Without the plugin
              <small>
                Stream Deck "Hotkey" action: {s.hotkey} saves a clip, {s.recordHotkey} records. Or run{" "}
                <code>Rewind.exe --save-clip</code>, <code>--toggle-record</code> or <code>--toggle-buffer</code>.
              </small>
            </span>
          </div>
        </section>

        <section className="group" hidden={tab !== "audio"}>
          <h2>Audio</h2>
          <label className="setting">
            <span>
              Desktop audio
              <small>Game and app sound from your default speakers or headset (track 1)</small>
            </span>
            <Toggle label="Desktop audio" checked={s.desktopAudio} onChange={(v) => set({ desktopAudio: v })} />
          </label>
          {s.desktopAudio && (
            <label className="setting">
              <span>
                Discord on its own track
                <small>
                  Discord voice and sounds go to a separate track (track 3) and are left out of desktop audio, so in the
                  editor you can mute your friends and keep the game, or the other way round. Exports and shares still mix
                  everything together.
                </small>
              </span>
              <Toggle label="Discord on its own track" checked={s.discordTrack} onChange={(v) => set({ discordTrack: v })} />
            </label>
          )}
          <label className="setting">
            <span>
              Microphone
              <small>Recorded on its own track (track 2), so you can turn it down later</small>
            </span>
            <Toggle label="Microphone" checked={s.micEnabled} onChange={(v) => set({ micEnabled: v })} />
          </label>
          {s.micEnabled && (
            <>
              <label className="setting">
                <span>Mic device</span>
                <select className="select" value={s.micDevice} onChange={(e) => set({ micDevice: e.target.value })}>
                  <option value="">Windows default</option>
                  {mics.map((m) => (
                    <option key={m} value={m}>
                      {m}
                    </option>
                  ))}
                </select>
              </label>
              <label className="setting">
                <span>
                  Mono microphone
                  <small>Your voice in both ears instead of one side</small>
                </span>
                <Toggle label="Mono microphone" checked={s.micMono} onChange={(v) => set({ micMono: v })} />
              </label>
              <label className="setting">
                <span>
                  Mic volume
                  <small>
                    Louder or quieter than Windows gives it, for new clips and recordings. Changes apply straight away. You can
                    still adjust each clip in the editor.
                  </small>
                </span>
                <span className="row" style={{ gap: 10 }}>
                  <Slider
                    aria-label="Mic volume"
                    min={-20}
                    max={20}
                    step={0.5}
                    value={s.micGainDb}
                    onValue={(v) => set({ micGainDb: v })}
                    style={{ width: 160 }}
                  />
                  <span className="muted" style={{ fontSize: 12, minWidth: 54, textAlign: "right" }}>
                    {fmtDb(s.micGainDb)}
                  </span>
                  {s.micGainDb !== 0 && (
                    <button className="btn small" onClick={() => set({ micGainDb: 0 })}>
                      Reset
                    </button>
                  )}
                </span>
              </label>
            </>
          )}
          <span className="muted" style={{ fontSize: 11, paddingTop: 8, lineHeight: 1.5 }}>
            Exports and Discord-size compresses mix both tracks into one so they play everywhere.
          </span>
        </section>

        <section className="group" hidden={tab !== "recording"}>
          <h2>Display &amp; capture</h2>
          <label className="setting">
            <span>
              Capture engine
              <small>
                {s.captureEngine === "native"
                  ? "Captures and encodes entirely on your NVIDIA GPU, with audio and video on one clock."
                  : "Older FFmpeg-based capture. Use only if Native gives you trouble."}
              </small>
            </span>
            <select
              className="select"
              value={s.captureEngine}
              onChange={(e) => set({ captureEngine: e.target.value as Settings["captureEngine"] })}
            >
              <option value="native">Native (recommended)</option>
              <option value="ffmpeg">Compatibility</option>
            </select>
          </label>
          <label className="setting">
            <span>
              Which monitor
              <small>{MONITOR_MODE_HINT[s.monitorMode]}</small>
            </span>
            <select
              className="select"
              value={s.monitorMode}
              onChange={(e) => set({ monitorMode: e.target.value as Settings["monitorMode"] })}
            >
              <option value="primary">Follow main display (Win+P safe)</option>
              <option value="cursor">Follow my cursor</option>
              <option value="fixed">Always one monitor</option>
            </select>
          </label>
          {s.monitorMode === "fixed" && (
            <label className="setting">
              <span>Monitor</span>
              <select className="select" value={s.fixedMonitor} onChange={(e) => set({ fixedMonitor: e.target.value })}>
                <option value="">Choose…</option>
                {monitors.map((m) => (
                  <option key={m.name} value={m.name}>
                    {m.name.replace("\\\\.\\", "")} · {m.width}×{m.height}
                    {m.primary ? " (primary)" : ""}
                  </option>
                ))}
              </select>
            </label>
          )}
          <div className="setting stacked">
            <span>
              Recording area
              <small>
                Record only part of the screen, like OBS: drag the box on the live preview, then press Use this area. The
                preview only runs on this page, and pauses while a game is open.
                {s.captureArea ? " Clips are the size of the area." : ""}
              </small>
            </span>
            {tab === "recording" && <RecordingArea area={s.captureArea} screen={screen} onSave={(a) => set({ captureArea: a })} />}
          </div>
          <label className="setting">
            <span>Show mouse cursor in clips</span>
            <Toggle label="Capture cursor" checked={s.drawCursor} onChange={(v) => set({ drawCursor: v })} />
          </label>
          <div className="setting">
            <span>
              Detected monitors
              <small>
                {monitors.map((m) => `${m.name.replace("\\\\.\\", "")} ${m.width}×${m.height}${m.primary ? " (main)" : ""}`).join(" · ") ||
                  "—"}
              </small>
            </span>
            <button className="btn small" onClick={() => api.listMonitors().then(setMonitors)}>
              Refresh
            </button>
          </div>
        </section>

        <section className="group" hidden={tab !== "library"}>
          <h2>Files</h2>
          <div className="setting">
            <span>
              Clips folder
              <small style={{ wordBreak: "break-all" }}>{s.clipsDir}</small>
            </span>
            <div className="row">
              <button className="btn small" onClick={() => api.reveal(s.clipsDir)}>
                Open
              </button>
              <button className="btn small" onClick={pickFolder}>
                Change…
              </button>
            </div>
          </div>
          <div className="setting">
            <span>
              Folders
              <small>Clips from a detected game go in a folder named after it. Everything else goes in “Desktop”.</small>
            </span>
          </div>
          <div className="setting">
            <span>
              Saved clip format
              <small>MKV survives crashes. Remux any file to MP4 when you want it.</small>
            </span>
            <span className="muted" style={{ fontSize: 12 }}>
              MKV
            </span>
          </div>
          <label className="setting">
            <span>
              Keep the original after remuxing to MP4
              <small>Off sends the original to the Recycle Bin</small>
            </span>
            <Toggle
              label="Keep original after remux"
              checked={s.keepOriginalAfterRemux}
              onChange={(v) => set({ keepOriginalAfterRemux: v })}
            />
          </label>
          <label className="setting">
            <span>
              Replace the original when I save an edit
              <small>Trims, exports and compresses overwrite the file. The old one goes to the Recycle Bin.</small>
            </span>
            <Toggle
              label="Replace original on edit"
              checked={s.replaceOriginalOnEdit}
              onChange={(v) => set({ replaceOriginalOnEdit: v })}
            />
          </label>
          <div className="setting">
            <span>
              Videos opened from other folders
              <small>{s.externalFiles.length} file(s) shown under “Imported”</small>
            </span>
            <button
              className="btn small"
              disabled={!s.externalFiles.length}
              onClick={() => set({ externalFiles: [] })}
              title="Removes them from the list only. Files are not deleted."
            >
              Clear list
            </button>
          </div>
        </section>

        <LibrarySettings s={s} set={set} Toggle={Toggle} hidden={tab !== "library"} />
        <SharingSettings s={s} set={set} Toggle={Toggle} hidden={tab !== "sharing"} onError={onError} />
      </div>
    </div>
  );
}
