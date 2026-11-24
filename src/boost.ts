// Playing a clip louder than 100% while you edit.
//
// A <video>/<audio> element can't go above volume 1, so a + dB change was only heard
// once saved. When a level above 1 is asked for, the element is routed through Web
// Audio (a gain node) instead. That needs the file served with CORS: the media server
// sends it for the app, and the element is given crossOrigin="anonymous" (mediaCors).
// An element is routed only when a boost is needed, so normal playback stays as it was.

let ctx: AudioContext | null = null;
const gains = new WeakMap<HTMLMediaElement, GainNode>();

/** crossOrigin for a player's src: the media server's files can be read by Web Audio. */
export function mediaCors(src: string | undefined): "anonymous" | undefined {
  return src && /^http:\/\/127\.0\.0\.1:\d+\//.test(src) ? "anonymous" : undefined;
}

function routable(el: HTMLMediaElement) {
  const src = el.currentSrc || el.src;
  if (!src) return false;
  try {
    if (new URL(src).origin === location.origin) return true;
  } catch {
    return false;
  }
  return el.crossOrigin === "anonymous" && !!mediaCors(src);
}

/**
 * Set a player's loudness: `level` is a plain multiplier (1 = as recorded, 2 = +6 dB),
 * `muted` silences it. Above 1 it goes through Web Audio, when the file allows it.
 */
export function setLevel(el: HTMLMediaElement, level: number, muted: boolean) {
  level = Math.max(0, Number.isFinite(level) ? level : 0);
  let g = gains.get(el);
  if (!g && level > 1.001 && routable(el)) {
    try {
      ctx ??= new AudioContext();
      const src = ctx.createMediaElementSource(el);
      g = ctx.createGain();
      src.connect(g).connect(ctx.destination);
      gains.set(el, g);
    } catch {
      g = undefined; // plays capped at 100%, as before
    }
  }
  if (g && ctx) {
    if (ctx.state === "suspended") ctx.resume().catch(() => {});
    if (el.volume !== 1) el.volume = 1;
    if (el.muted !== muted) el.muted = muted;
    const want = muted ? 0 : level;
    if (Math.abs(g.gain.value - want) > 1e-4) g.gain.setTargetAtTime(want, ctx.currentTime, 0.015);
  } else {
    const v = Math.min(1, level);
    if (Math.abs(el.volume - v) > 1e-4) el.volume = v;
    if (el.muted !== muted) el.muted = muted;
  }
}

/** dB → multiplier; -60 dB or lower is off. */
export const dbToGain = (db: number) => (db <= -60 ? 0 : Math.pow(10, db / 20));
