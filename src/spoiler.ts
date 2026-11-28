import { useCallback, useSyncExternalStore } from "react";

/**
 * Clips marked as a spoiler: blurred everywhere in the app until clicked. A click
 * shows it until you leave the page (App clears `revealed` on each page change,
 * except into Studio, so a clip shown in the library stays shown when opened).
 * Marks come from Settings (`spoilers`), keyed by the file name in lower case, like
 * the backend's, so they stay through moves and NAS copies.
 */
let marked = new Set<string>();
const revealed = new Set<string>();
let version = 0;
const listeners = new Set<() => void>();

export const spoilerKey = (path: string) => (path.split(/[\\/]/).pop() ?? path).toLowerCase();

function changed() {
  version++;
  listeners.forEach((l) => l());
}

export function setMarked(list: string[]) {
  const next = new Set(list.map((k) => k.toLowerCase()));
  if (next.size === marked.size && [...next].every((k) => marked.has(k))) return;
  marked = next;
  changed();
}

export const isMarked = (path: string) => !!path && marked.has(spoilerKey(path));
export const isHidden = (path: string) => isMarked(path) && !revealed.has(spoilerKey(path));

export function reveal(path: string) {
  revealed.add(spoilerKey(path));
  changed();
}

/** Blur every marked clip again (leaving a page). */
export function hideAll() {
  if (!revealed.size) return;
  revealed.clear();
  changed();
}

function subscribe(l: () => void) {
  listeners.add(l);
  return () => listeners.delete(l);
}

/** Whether a clip is marked, and still hidden; `reveal` shows it until you leave the page. */
export function useSpoiler(path: string) {
  useSyncExternalStore(subscribe, () => version);
  const show = useCallback(() => reveal(path), [path]);
  return { marked: isMarked(path), hidden: isHidden(path), reveal: show };
}

// Blurred copies of thumbnails, made once each.
const blurs = new Map<string, Promise<string | null>>();

/** A blurred, darkened copy of an image (a blob URL), or null if it can't be made. */
export function blurred(src: string): Promise<string | null> {
  let p = blurs.get(src);
  if (!p) {
    p = new Promise((resolve) => {
      const img = new Image();
      img.crossOrigin = "anonymous";
      img.onload = () => {
        try {
          const w = 320;
          const h = Math.max(1, Math.round((img.naturalHeight / Math.max(1, img.naturalWidth)) * w));
          const c = document.createElement("canvas");
          c.width = w;
          c.height = h;
          const g = c.getContext("2d");
          if (!g) return resolve(null);
          // Drawn a bit larger than the canvas, so the blur doesn't fade in from the edges.
          g.filter = "blur(18px) brightness(0.6)";
          g.drawImage(img, -24, -24, w + 48, h + 48);
          c.toBlob((b) => resolve(b ? URL.createObjectURL(b) : null), "image/jpeg", 0.8);
        } catch {
          resolve(null);
        }
      };
      img.onerror = () => resolve(null);
      img.src = src;
    });
    blurs.set(src, p);
  }
  return p;
}
