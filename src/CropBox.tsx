import { useRef } from "react";

/** Part of the picture to keep, as fractions of the frame (0..1). */
export type Crop = { x: number; y: number; w: number; h: number };

export const WHOLE: Crop = { x: 0, y: 0, w: 1, h: 1 };

export const isWhole = (c: Crop | null) => !c || (c.x <= 0.002 && c.y <= 0.002 && c.w >= 0.998 && c.h >= 0.998);

/** Shapes people crop to; `null` is free. */
export const CROP_PRESETS: [string, number | null, string][] = [
  ["Free", null, "Any shape"],
  ["16:9", 16 / 9, "Widescreen, YouTube"],
  ["9:16", 9 / 16, "TikTok, Shorts, Reels"],
  ["1:1", 1, "Square"],
  ["4:3", 4 / 3, "Classic"],
];

const MIN = 0.05;
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** The biggest box of `ratio` (width / height in pixels) that fits, in the middle. */
export function fitRatio(ratio: number, videoAr: number): Crop {
  // In fractions: (w * W) / (h * H) = ratio, so w / h = ratio / videoAr.
  const k = ratio / videoAr;
  const [w, h] = k >= 1 ? [1, 1 / k] : [k, 1];
  return { x: (1 - w) / 2, y: (1 - h) / 2, w, h };
}

/** Output size in pixels, to show next to the box. */
export function cropPixels(c: Crop, width: number, height: number) {
  const even = (v: number) => Math.max(2, Math.floor(Math.round(v) / 2) * 2);
  return [even(c.w * width), even(c.h * height)];
}

type Handle = "move" | "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";
const HANDLES: Handle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

/**
 * A box over the video: drag it to move, drag its edges or corners to resize.
 * With a shape picked (`ratio`), it keeps that shape. Outside the box is dimmed.
 */
export function CropBox({
  value,
  onChange,
  ratio,
  videoAr,
  editing,
  label,
}: {
  value: Crop;
  onChange: (c: Crop) => void;
  ratio: number | null;
  videoAr: number;
  editing: boolean;
  label?: string;
}) {
  const root = useRef<HTMLDivElement>(null);

  const start = (handle: Handle) => (e: React.PointerEvent) => {
    if (!editing || e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const rect = root.current!.getBoundingClientRect();
    const c0 = value;
    const p0 = [e.clientX, e.clientY];
    const k = ratio ? ratio / videoAr : null; // w / h in fractions when the shape is locked
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      const dx = (ev.clientX - p0[0]) / rect.width;
      const dy = (ev.clientY - p0[1]) / rect.height;
      if (handle === "move") {
        onChange({ ...c0, x: clamp(c0.x + dx, 0, 1 - c0.w), y: clamp(c0.y + dy, 0, 1 - c0.h) });
        return;
      }
      let l = c0.x, t = c0.y, r = c0.x + c0.w, b = c0.y + c0.h;
      if (handle.includes("w")) l = clamp(l + dx, 0, r - MIN);
      if (handle.includes("e")) r = clamp(r + dx, l + MIN, 1);
      if (handle.includes("n")) t = clamp(t + dy, 0, b - MIN);
      if (handle.includes("s")) b = clamp(b + dy, t + MIN, 1);
      if (k) {
        // Keep the shape: the dragged side leads, the other follows, anchored
        // on the opposite side (or the middle, for a plain edge).
        let w = r - l, h = b - t;
        if (handle === "n" || handle === "s") w = h * k;
        else h = w / k;
        // Shrink to fit the frame.
        const cx = handle === "n" || handle === "s" ? (c0.x + c0.w / 2) : handle.includes("w") ? r : l;
        const cy = handle === "e" || handle === "w" ? (c0.y + c0.h / 2) : handle.includes("n") ? b : t;
        const maxW = handle === "n" || handle === "s" ? 2 * Math.min(cx, 1 - cx) : handle.includes("w") ? cx : 1 - cx;
        const maxH = handle === "e" || handle === "w" ? 2 * Math.min(cy, 1 - cy) : handle.includes("n") ? cy : 1 - cy;
        const s = Math.min(1, maxW / w, maxH / h);
        w *= s;
        h *= s;
        l = handle === "n" || handle === "s" ? cx - w / 2 : handle.includes("w") ? cx - w : cx;
        t = handle === "e" || handle === "w" ? cy - h / 2 : handle.includes("n") ? cy - h : cy;
        r = l + w;
        b = t + h;
      }
      onChange({ x: l, y: t, w: r - l, h: b - t });
    };
    const up = () => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", up);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
  };

  const pc = (v: number) => `${v * 100}%`;
  return (
    <div ref={root} className={`crop-layer ${editing ? "editing" : ""}`}>
      <div
        className="crop-box"
        style={{ left: pc(value.x), top: pc(value.y), width: pc(value.w), height: pc(value.h) }}
        onPointerDown={start("move")}
      >
        {editing && (
          <>
            <div className="crop-grid" />
            {HANDLES.map((h) => (
              <div key={h} className={`crop-h ${h}`} onPointerDown={start(h)} />
            ))}
          </>
        )}
        {label && <span className="crop-size mono">{label}</span>}
      </div>
    </div>
  );
}
