/**
 * Speed ramps at the joins, as the app sees them: the same curve as the exported
 * video (src-tauri/src/ramp.rs), for the new length and the live preview.
 */
export type JoinStyle = "cut" | "subtle" | "medium" | "strong";

export const RAMPS: Record<Exclude<JoinStyle, "cut">, { peak: number; side: number; label: string }> = {
  subtle: { peak: 3, side: 0.5, label: "Speed ramp, subtle" },
  medium: { peak: 5, side: 0.75, label: "Speed ramp, medium" },
  strong: { peak: 8, side: 1, label: "Speed ramp, strong" },
};

export type Sides = { len: number; a: number; b: number; peak: number };

/** A piece's ramps: `a` at its start (after a join), `b` at its end (before one). */
export function sides(len: number, rampIn: boolean, rampOut: boolean, style: JoinStyle): Sides {
  if (style === "cut") return { len, a: 0, b: 0, peak: 1 };
  const s = RAMPS[style];
  const limit = (len * 0.9) / (rampIn && rampOut ? 2 : 1);
  const each = (on: boolean) => {
    const r = on ? Math.min(s.side, limit) : 0;
    return r < 0.15 ? 0 : r;
  };
  return { len, a: each(rampIn), b: each(rampOut), peak: s.peak };
}

/** How long the piece plays, ramps included. */
export function outLen({ len, a, b, peak }: Sides) {
  const k = 1 - 1 / peak;
  return len - (a + b) * (k / 2);
}

/** Pieces of these lengths joined with this style: the clip's new length. */
export function joinedLen(lens: number[], style: JoinStyle) {
  return lens.reduce((n, l, i) => n + outLen(sides(l, i > 0, i < lens.length - 1, style)), 0);
}

/** The speed at `t` seconds into a piece (1 away from its ramps). */
export function speedAt(t: number, { len, a, b, peak }: Sides) {
  const k = 1 - 1 / peak;
  let slow = 1;
  if (a > 0 && t < a) slow = 1 - k * (0.5 + Math.cos((Math.PI * t) / a) / 2);
  else if (b > 0 && t > len - b) slow = 1 - k * (0.5 - Math.cos((Math.PI * (t - (len - b))) / b) / 2);
  return 1 / Math.max(slow, 1 / peak);
}
