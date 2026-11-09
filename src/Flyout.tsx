import { Select } from "./Select";
import { useState } from "react";
import { api, ClipFile, errText, fmtDuration, Settings } from "./api";
import { audioLine, folderLine, RecordButton, statusLine } from "./BufferCard";
import { useAppearance, useClips, useSettings, useStatus, useThumb } from "./hooks";

function MiniClip({ clip }: { clip: ClipFile }) {
  const thumb = useThumb(clip.path, clip.modified);
  const ago = Math.round((Date.now() - clip.modified) / 60000);
  return (
    <button className="mini-clip" onClick={() => api.openInStudio(clip.path)}>
      <div className="mini-thumb">{thumb && <img src={thumb} alt="" />}</div>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div className="card-title">{clip.name}</div>
        <div className="card-meta">
          {clip.folder} · {ago < 1 ? "just now" : ago < 60 ? `${ago} min ago` : new Date(clip.modified).toLocaleString()}
        </div>
      </div>
    </button>
  );
}

export function Flyout() {
  const status = useStatus();
  const [settings, update] = useSettings();
  useAppearance(settings);
  const [clips] = useClips();
  const [busy, setBusy] = useState<number | null>(null);
  const [msg, setMsg] = useState<{ text: string; err?: boolean } | null>(null);

  const on = !!status?.running && !!status?.enabled;
  const full = settings?.bufferSeconds ?? 120;
  const pct = status?.bufferSeconds ? (status.bufferedSeconds / status.bufferSeconds) * 100 : 0;

  const save = async (sec: number) => {
    setBusy(sec);
    setMsg(null);
    try {
      const p = await api.saveClip(sec);
      setMsg({ text: `Saved ${p.split("\\").pop()}` });
    } catch (e) {
      setMsg({ text: errText(e), err: true });
    } finally {
      setBusy(null);
    }
  };

  const options = [30, 60, full].filter((v, i, a) => v <= full && a.indexOf(v) === i);

  return (
    <div className="flyout">
      <div className="fly-row">
        <span className="brand" style={{ padding: 0 }}>
          HINDSIGHT
        </span>
        <button className="btn small" onClick={() => api.showMain()}>
          Open app
        </button>
      </div>

      <div className="fly-card">
        <div className="fly-row">
          <span style={{ fontWeight: 600, display: "flex", gap: 8, alignItems: "center" }}>
            <span className={`dot ${on ? "on" : ""}`} /> Replay buffer
          </span>
          <input
            type="checkbox"
            className="switch"
            aria-label="Replay buffer on"
            checked={!!settings?.bufferEnabled}
            onChange={(e) => update({ bufferEnabled: e.target.checked })}
          />
        </div>
        <div className="meter" aria-hidden="true">
          <div style={{ width: `${pct}%` }} />
        </div>
        <div className="buffer-meta">
          {status ? `${fmtDuration(status.bufferedSeconds)} of ${fmtDuration(status.bufferSeconds)} held · ` : ""}
          {statusLine(status)}
          <br />
          {audioLine(status) ?? ""} · {folderLine(status)}
        </div>
        <div className="save-row" style={{ gridTemplateColumns: `repeat(${options.length}, minmax(0, 1fr))` }}>
          {options.map((sec, i) => (
            <button
              key={sec}
              className={`btn ${i === options.length - 1 ? "primary" : ""}`}
              disabled={!on || busy !== null}
              onClick={() => save(sec)}
            >
              {busy === sec ? "Saving…" : i === options.length - 1 ? `Save ${fmtDuration(sec)}` : `${sec} s`}
            </button>
          ))}
        </div>
        <RecordButton status={status} hotkey={settings?.recordHotkey ?? "Alt+F9"} />
        {msg && <div className={msg.err ? "err-text" : "buffer-meta"}>{msg.text}</div>}
      </div>

      <div className="stack">
        <div className="label">Capturing</div>
        <label className="fly-row">
          <span className="muted">Monitor</span>
          <Select
            className="select"
            value={settings?.monitorMode ?? "primary"}
            onChange={(e) => update({ monitorMode: e.target.value as Settings["monitorMode"] })}
          >
            <option value="primary">Follow primary (Win+P safe)</option>
            <option value="cursor">Follow cursor</option>
            <option value="fixed">One fixed monitor</option>
          </Select>
        </label>
      </div>

      <div className="stack" style={{ minHeight: 0, flex: 1 }}>
        <div className="label">Last clips</div>
        {(clips ?? []).filter((c) => !c.external).slice(0, 3).map((c) => (
          <MiniClip key={c.path} clip={c} />
        ))}
        {clips && clips.length === 0 && <span className="muted" style={{ fontSize: 12 }}>No clips yet. Press {settings?.hotkey}.</span>}
      </div>
    </div>
  );
}
