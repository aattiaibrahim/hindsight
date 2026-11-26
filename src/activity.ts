// What's happening to clips right now, for their cards: being changed (a save, a
// compress, a move: a shimmer), just changed (a short glow) or being deleted (the
// card shrinks and fades, then the others slide into its place).
//
// Kept outside React, so it carries on while you go to another page and back.

import { useSyncExternalStore } from "react";

export type Activity = "busy" | "done" | "leaving" | null;

const busy = new Map<string, number>(); // path -> how many jobs
/** A named job on a clip (a Studio save): what it's doing and how far it's got. */
export type Job = { path: string; label: string; pct: number | null };
const jobs = new Map<string, Job>(); // path -> its job
const done = new Map<string, number>(); // path -> timer
const leaving = new Map<string, number>(); // path -> when it started (ms)
const listeners = new Set<() => void>();
let version = 0;

const norm = (p: string) => p.replace(/\//g, "\\").toLowerCase();
const changed = () => {
  version++;
  listeners.forEach((f) => f());
};

/** How long a card takes to leave (App.css .card.leaving). */
export const LEAVE_MS = 260;
const GLOW_MS = 900;

/** A job on these clips started: their cards shimmer until `finished`. `label`
 *  ("Trimming"): shown on the card and in the status bar, with its progress. */
export function started(paths: string[], label?: string) {
  paths.forEach((p) => {
    busy.set(norm(p), (busy.get(norm(p)) ?? 0) + 1);
    if (label) jobs.set(norm(p), { path: p, label, pct: null });
  });
  changed();
}

/** How far a labelled job has got (0-100). */
export function progress(path: string, pct: number) {
  const j = jobs.get(norm(path));
  if (!j || j.pct === pct) return;
  jobs.set(norm(path), { ...j, pct: Math.max(j.pct ?? 0, Math.round(pct)) });
  changed();
}

/** The job ended. `glow`: the clips it made or changed (their cards glow once). */
export function finished(paths: string[], glow: string[] = []) {
  paths.forEach((p) => {
    const n = (busy.get(norm(p)) ?? 1) - 1;
    if (n > 0) busy.set(norm(p), n);
    else busy.delete(norm(p));
    jobs.delete(norm(p));
  });
  glow.forEach((p) => {
    const k = norm(p);
    window.clearTimeout(done.get(k));
    done.set(
      k,
      window.setTimeout(() => {
        done.delete(k);
        changed();
      }, GLOW_MS),
    );
  });
  changed();
}

/** These clips are being deleted: their cards leave. */
export function leave(paths: string[]) {
  const now = performance.now();
  paths.forEach((p) => leaving.set(norm(p), now));
  changed();
}

/** Some didn't go after all (or were put back): show them again. */
export function stay(paths: string[]) {
  paths.forEach((p) => leaving.delete(norm(p)));
  changed();
}

export const isLeaving = () => leaving.size > 0;

/** Resolves once every leaving card has finished its animation. */
export function leavesDone(): Promise<void> {
  if (!leaving.size) return Promise.resolve();
  const last = Math.max(...leaving.values());
  const wait = Math.max(0, last + LEAVE_MS - performance.now());
  return new Promise((r) => window.setTimeout(r, wait));
}

/** The list now has these clips: forget the leaving ones that are gone from it. */
export function forgetGone(present: string[]) {
  if (!leaving.size) return;
  const here = new Set(present.map(norm));
  [...leaving.keys()].forEach((k) => !here.has(k) && leaving.delete(k));
  changed();
}

function subscribe(f: () => void) {
  listeners.add(f);
  return () => listeners.delete(f);
}

export function activityOf(path: string): Activity {
  const k = norm(path);
  if (leaving.has(k)) return "leaving";
  if (busy.has(k)) return "busy";
  if (done.has(k)) return "done";
  return null;
}

/** This clip's activity, re-rendering when it changes. */
export function useActivity(path: string): Activity {
  useSyncExternalStore(subscribe, () => version);
  return activityOf(path);
}

/** This clip's named job, if it has one (re-renders as it moves on). */
export function useJob(path: string): Job | null {
  useSyncExternalStore(subscribe, () => version);
  return jobs.get(norm(path)) ?? null;
}

/** Every named job running now, for the status bar. */
export function useJobs(): Job[] {
  useSyncExternalStore(subscribe, () => version);
  return [...jobs.values()];
}

/** What a Studio save is doing, for its card: "Saving trim…" -> "Trimming". */
export function jobLabel(label: string) {
  const l = label.toLowerCase();
  if (l.startsWith("saving trim")) return "Trimming";
  if (l.startsWith("cutting")) return "Cutting";
  if (l.startsWith("saving parts")) return "Splitting";
  if (l.startsWith("saving crop")) return "Cropping";
  if (l.startsWith("saving audio")) return "Saving audio";
  if (l.startsWith("compress")) return "Compressing";
  if (l.startsWith("export")) return "Exporting";
  if (l.startsWith("remux")) return "Converting";
  return label.replace(/…$/, "");
}

// Sliding the other cards into place (FLIP): where each card was before the list
// changed, then each one moves from there to where it is now.
type Rects = Map<string, DOMRect>;

/** Where the cards on screen are now (only ones near the window: cheap). */
export function snapshot(): Rects {
  const out: Rects = new Map();
  const h = window.innerHeight;
  document.querySelectorAll<HTMLElement>(".card[data-path], .clip-row[data-path]").forEach((el) => {
    const r = el.getBoundingClientRect();
    if (r.bottom > -200 && r.top < h + 200 && r.width > 0) out.set(el.dataset.path ?? "", r);
  });
  return out;
}

/** Move cards from where `before` had them to where they are now. */
export function slide(before: Rects) {
  const m = document.documentElement.dataset.motion;
  const still = m === "reduce" || (m !== "on" && matchMedia("(prefers-reduced-motion: reduce)").matches);
  if (!before.size || still) return;
  document.querySelectorAll<HTMLElement>(".card[data-path], .clip-row[data-path]").forEach((el) => {
    const was = before.get(el.dataset.path ?? "");
    if (!was) return;
    const now = el.getBoundingClientRect();
    const dx = was.left - now.left;
    const dy = was.top - now.top;
    if (Math.abs(dx) < 1 && Math.abs(dy) < 1) return;
    el.animate([{ transform: `translate(${dx}px, ${dy}px)` }, { transform: "none" }], { duration: 280, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)" });
  });
}
