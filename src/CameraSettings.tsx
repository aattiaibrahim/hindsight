import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { api, CameraInfo, CropRect, Settings } from "./api";
import { Select } from "./Select";

/** Space between a snapped box and the edge, as a fraction of the clip's width. */
const MARGIN = 0.02;

/**
 * Settings → Recording: a camera drawn into clips and recordings, like a facecam in
 * OBS. Where it goes is set on the live preview below (RecordingArea + CameraBox).
 */
export function CameraSettings({
  s,
  set,
  native,
  Toggle,
}: {
  s: Settings;
  set: (p: Partial<Settings>) => void;
  /** The Native capture engine is in use (Compatibility restarts capture when the camera moves) */
  native: boolean;
  Toggle: (p: { checked: boolean; onChange: (v: boolean) => void; label: string }) => React.ReactNode;
}) {
  const [cams, setCams] = useState<CameraInfo[] | null>(null);
  const load = () => api.listCameras().then(setCams).catch(() => setCams([]));
  useEffect(() => {
    if (s.cameraEnabled) load();
  }, [s.cameraEnabled]);
  // None picked yet: the first one is used (as the recorder does).
  const picked =
    cams?.find((c) => c.id === s.cameraId) ?? cams?.find((c) => c.name === s.cameraName) ?? (!s.cameraId && !s.cameraName ? cams?.[0] : undefined);
  return (
    <>
      <label className="setting">
        <span>
          Camera
          <small>
            Shows your camera in clips and recordings, like a facecam in OBS. Place it on the preview below. The camera is on (its
            light too) while Hindsight records.
            {!native && " With Compatibility capture, moving or resizing it restarts capture (a second's gap in the replay buffer)."}
          </small>
        </span>
        <Toggle
          label="Camera"
          checked={s.cameraEnabled}
          onChange={(v) => {
            set({ cameraEnabled: v });
            if (v) load();
          }}
        />
      </label>
      {s.cameraEnabled && (
        <>
          <label className="setting">
            <span>
              Which camera
              {cams?.length === 0 && <small className="err-text">No camera found. Plug one in, then press Look again.</small>}
            </span>
            <span className="row" style={{ flexWrap: "nowrap" }}>
              <Select
                className="select"
                value={picked?.id ?? ""}
                onChange={(e) => {
                  const c = cams?.find((x) => x.id === e.target.value);
                  if (c) set({ cameraId: c.id, cameraName: c.name });
                }}
              >
                {!picked && <option value="">{cams === null ? "Looking…" : s.cameraName ? `${s.cameraName} (not connected)` : "Choose…"}</option>}
                {cams?.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.name}
                  </option>
                ))}
              </Select>
              <button className="btn small" onClick={load}>
                Look again
              </button>
            </span>
          </label>
          <label className="setting">
            <span>
              Background
              <small>
                {s.cameraBackground === "remove"
                  ? "Only you, over the game: what's behind you is cut away (no box)."
                  : s.cameraBackground === "blur"
                    ? "What's behind you is blurred, you stay sharp."
                    : "Blur what's behind you, or remove it so only you show. Found on this PC by a small AI model (downloaded once, about 0.5 MB); nothing leaves your PC."}
              </small>
              <BackgroundNote on={s.cameraBackground !== "off"} />
            </span>
            <Select className="select" value={s.cameraBackground} onChange={(e) => set({ cameraBackground: e.target.value as Settings["cameraBackground"] })}>
              <option value="off">As it is</option>
              <option value="blur">Blur</option>
              <option value="remove">Remove</option>
            </Select>
          </label>
          <label className="setting">
            <span>
              Mirror
              <small>Flip it left to right, like a selfie.</small>
            </span>
            <Toggle label="Mirror the camera" checked={s.cameraMirror} onChange={(v) => set({ cameraMirror: v })} />
          </label>
          {s.cameraBackground !== "remove" && (
            <label className="setting">
              <span>
                Thin border
                <small>A light outline so it stands out from the game.</small>
              </span>
              <Toggle label="Border around the camera" checked={s.cameraBorder} onChange={(v) => set({ cameraBorder: v })} />
            </label>
          )}
          <label className="setting">
            <span>
              Crop to fill
              <small>A narrower 4:3 box with the sides of the picture cut off, so you fill more of it.</small>
            </span>
            <Toggle label="Crop the camera to fill" checked={s.cameraFill} onChange={(v) => set({ cameraFill: v })} />
          </label>
        </>
      )}
    </>
  );
}

/** The background effect getting ready (its one-time download), or why it can't run. */
function BackgroundNote({ on }: { on: boolean }) {
  const [note, setNote] = useState<string | null>(null);
  useEffect(() => {
    if (!on) {
      setNote(null);
      return;
    }
    let stop = false;
    const tick = () =>
      api
        .cameraNote()
        .then((n) => !stop && setNote(n))
        .catch(() => {});
    tick();
    const t = window.setInterval(tick, 1000);
    return () => {
      stop = true;
      clearInterval(t);
    };
  }, [on]);
  return note ? <small className={note.includes("isn't available") || note.includes("stopped") ? "err-text" : "accent"}>{note}</small> : null;
}

/**
 * The camera on the live screen preview: drag it anywhere, or snap it to a corner,
 * and change its size. Positions are fractions of the recorded part of the screen.
 */
export function CameraBox({
  s,
  set,
  area,
  tools,
}: {
  s: Settings;
  set: (p: Partial<Settings>) => void;
  area: CropRect;
  /** Where the corner buttons and size slider go (under the preview) */
  tools: HTMLElement | null;
}) {
  const [img, setImg] = useState<string | null>(null);
  const [aspect, setAspect] = useState(s.cameraFill ? 4 / 3 : 16 / 9);
  const [why, setWhy] = useState("");
  const [drag, setDrag] = useState<{ x: number; y: number } | null>(null);
  const start = useRef<{ px: number; py: number; x: number; y: number; w: number; h: number } | null>(null);
  const url = useRef<string | null>(null);

  // The box as it will look, a few times a second while this page is open.
  useEffect(() => {
    let stop = false;
    let t = 0;
    const tick = async () => {
      if (stop) return;
      if (document.hidden) {
        t = window.setTimeout(tick, 1000);
        return;
      }
      try {
        const bytes = await api.cameraPreview();
        if (stop) return;
        const u = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
        if (url.current) URL.revokeObjectURL(url.current);
        url.current = u;
        setImg(u);
        setWhy("");
        t = window.setTimeout(tick, 120);
      } catch (e) {
        if (stop) return;
        const w = String(e);
        setWhy(w === "starting" ? "Starting the camera…" : w);
        t = window.setTimeout(tick, w === "starting" ? 300 : 2000);
      }
    };
    tick();
    return () => {
      stop = true;
      clearTimeout(t);
      if (url.current) URL.revokeObjectURL(url.current);
      url.current = null;
    };
  }, []);

  // Fractions of the recorded part → fractions of the whole preview.
  const size = s.cameraSize;
  // The box's height as a fraction of the recorded part: its width × the part's shape ÷ the box's shape.
  const view = useRef<HTMLDivElement>(null);
  const partAspect = () => {
    const el = view.current?.parentElement;
    if (!el) return 16 / 9;
    return (el.clientWidth * area.w) / Math.max(1, el.clientHeight * area.h);
  };
  const hFrac = Math.min(1, (size * partAspect()) / aspect);
  const pos = drag ?? { x: Math.min(s.cameraX, 1 - size), y: Math.min(s.cameraY, 1 - hFrac) };
  const snap = (corner: string) =>
    set({
      cameraX: corner.endsWith("l") ? MARGIN : 1 - size - MARGIN,
      cameraY: corner.startsWith("t") ? MARGIN * partAspect() : 1 - hFrac - MARGIN * partAspect(),
    });

  return (
    <>
      <div
        ref={view}
        className={`cam-box ${drag ? "dragging" : ""}`}
        style={{
          left: `${(area.x + pos.x * area.w) * 100}%`,
          top: `${(area.y + pos.y * area.h) * 100}%`,
          width: `${size * area.w * 100}%`,
          height: `${hFrac * area.h * 100}%`,
        }}
        title="Drag to move the camera"
        onPointerDown={(e) => {
          if (e.button !== 0) return;
          e.stopPropagation();
          e.currentTarget.setPointerCapture(e.pointerId);
          const p = view.current?.parentElement;
          if (!p) return;
          start.current = { px: e.clientX, py: e.clientY, x: pos.x, y: pos.y, w: p.clientWidth * area.w, h: p.clientHeight * area.h };
          setDrag(pos);
        }}
        onPointerMove={(e) => {
          const st = start.current;
          if (!st) return;
          const x = Math.min(Math.max(st.x + (e.clientX - st.px) / st.w, 0), 1 - size);
          const y = Math.min(Math.max(st.y + (e.clientY - st.py) / st.h, 0), 1 - hFrac);
          setDrag({ x, y });
        }}
        onPointerUp={() => {
          if (start.current && drag) set({ cameraX: drag.x, cameraY: drag.y });
          start.current = null;
          setDrag(null);
        }}
        onPointerCancel={() => {
          start.current = null;
          setDrag(null);
        }}
      >
        {img ? (
          <img
            src={img}
            alt="Your camera"
            draggable={false}
            onLoad={(e) => {
              const i = e.currentTarget;
              if (i.naturalWidth && i.naturalHeight) setAspect(i.naturalWidth / i.naturalHeight);
            }}
          />
        ) : (
          <span className="cam-box-wait">{why || "Starting the camera…"}</span>
        )}
      </div>
      {why && img && <div className="cam-box-err">{why}</div>}
      {tools && createPortal(<CameraTools size={size} onSnap={snap} onSize={(v) => set({ cameraSize: v, cameraX: Math.min(s.cameraX, 1 - v) })} />, tools)}
    </>
  );
}

function CameraTools({ size, onSnap, onSize }: { size: number; onSnap: (c: string) => void; onSize: (v: number) => void }) {
  const [v, setV] = useState(size);
  useEffect(() => setV(size), [size]);
  return (
    <>
      <span className="muted small">Camera</span>
      <span className="transport-pill crop-tools" role="group" aria-label="Snap the camera to a corner">
        {[
          ["tl", "Top left"],
          ["tr", "Top right"],
          ["bl", "Bottom left"],
          ["br", "Bottom right"],
        ].map(([c, label]) => (
          <button key={c} className="btn pill" onClick={() => onSnap(c)}>
            {label}
          </button>
        ))}
      </span>
      <label className="cam-size">
        <span className="muted small">Size</span>
        <input
          type="range"
          min={8}
          max={50}
          step={1}
          value={Math.round(v * 100)}
          onChange={(e) => setV(Number(e.target.value) / 100)}
          onPointerUp={() => onSize(v)}
          onKeyUp={() => onSize(v)}
          aria-label="Camera size"
        />
      </label>
    </>
  );
}
