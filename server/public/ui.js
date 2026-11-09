// Small helpers shared by the library page and the editor.

import { icon } from "./icons.js";

export function el(tag, props = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (v === undefined || v === null || v === false) continue;
    if (k === "class") e.className = v;
    else if (k === "text") e.textContent = v;
    else if (k === "icon") e.append(icon(v));
    else if (k.startsWith("on") && typeof v === "function") e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v === true ? "" : v);
  }
  e.append(...kids.flat().filter((k) => k !== null && k !== undefined && k !== false && k !== ""));
  return e;
}

/** A pill button: `el("button")` with an optional leading icon. */
export function btn(label, { icon: ic, cls = "", ...props } = {}) {
  return el("button", { class: `btn ${cls}`.trim(), ...props }, ic ? icon(ic) : null, label);
}

/** Fill every `[data-icon]` placeholder in the page with its icon. */
export function hydrateIcons(root = document) {
  for (const n of root.querySelectorAll("[data-icon]")) {
    if (!n.firstChild) n.replaceWith(icon(n.dataset.icon));
  }
}

export function size(bytes) {
  if (bytes >= 1024 ** 4) return `${(bytes / 1024 ** 4).toFixed(2)} TB`;
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  if (bytes >= 1024 ** 2) return `${Math.round(bytes / 1024 ** 2)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

export function duration(s) {
  s = Math.max(0, Math.round(s));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** 0:12.34 — for the editor's handles. */
export function preciseTime(s) {
  s = Math.max(0, s);
  const m = Math.floor(s / 60);
  return `${m}:${(s - m * 60).toFixed(2).padStart(5, "0")}`;
}

export const dayTs = (day) => new Date(`${day}T12:00:00`).getTime();

/** "Today", "Yesterday", "Mon, Nov 2" (with the year when it isn't this year), like the desktop app. */
export function dayLabel(day) {
  const d = new Date(dayTs(day));
  const today = new Date();
  const y = new Date();
  y.setDate(today.getDate() - 1);
  if (d.toDateString() === today.toDateString()) return "Today";
  if (d.toDateString() === y.toDateString()) return "Yesterday";
  return d.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric", year: d.getFullYear() === today.getFullYear() ? undefined : "numeric" });
}

export function timeOf(c) {
  const [h, m] = c.takenAt.slice(11, 16).split(":").map(Number);
  const d = new Date();
  d.setHours(h, m, 0, 0);
  return d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
}

/** A clip's title: when it was taken, plus what made it an edit ("Today, 9:14 PM · trim"). */
export function clipTitle(c) {
  const edit = /\(((?:trim|remux|H\.264|repaired|audio|\d+ MB)(?: \(\d+\))?|trimmed|good part)\)[^()]*$/i.exec(c.name)?.[1];
  const n = c.n > 1 ? ` (${c.n})` : "";
  return `${dayLabel(c.day)}, ${timeOf(c)}${n}${edit ? ` · ${edit}` : ""}`;
}

export const codecLabel = (c) => ({ av1: "AV1", hevc: "HEVC", h264: "H.264", vp9: "VP9", mpeg2video: "MPEG-2" })[c] ?? (c || "").toUpperCase();
export const api = (id, action) => `api/clips/${encodeURIComponent(id)}/${action}`;
export const logoUrl = (game) => `api/logo?game=${encodeURIComponent(game)}`;

export function logo(game, cls = "game-logo") {
  return el("img", { class: cls, src: logoUrl(game), alt: "", loading: "lazy", decoding: "async" });
}

export async function postJson(url, body) {
  const res = await fetch(url, body === undefined ? { method: "POST" } : { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  const j = await res.json().catch(() => ({}));
  if (!res.ok) throw Object.assign(new Error(j.error || `HTTP ${res.status}`), { status: res.status });
  return j;
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * Load a clip into a <video>: the original when the browser can play it, else
 * the H.264 copy the server makes once (shown with `onStatus(text, progress)`).
 * `alive()` false stops it (the view was left). Resolves with the source used.
 */
export async function loadVideo(video, c, info, { onStatus = () => {}, alive = () => true } = {}) {
  const use = (src) => {
    onStatus("");
    video.src = src;
    return src;
  };
  if (info.directPlay) return use(api(c.id, "original"));
  if (info.web?.state === "ready") return use(api(c.id, "web.mp4"));
  if (info.web?.state !== "working" && info.web?.state !== "queued") {
    // Try the original first: desktop browsers often play MKV/HEVC/AV1. Phones usually can't.
    const ok = await new Promise((resolve) => {
      const done = (v) => {
        clearTimeout(timer);
        video.removeEventListener("error", bad);
        video.removeEventListener("loadeddata", good);
        resolve(v);
      };
      const bad = () => done(false);
      const good = () => done(!info.video || video.videoWidth > 0);
      const timer = setTimeout(bad, 8000);
      video.addEventListener("error", bad);
      video.addEventListener("loadeddata", good);
      video.src = api(c.id, "original");
    });
    if (!alive()) return null;
    if (ok) return api(c.id, "original");
    video.removeAttribute("src");
    video.load();
  }
  onStatus("Making a version this browser can play…", 0);
  let st = await postJson(api(c.id, "web")).catch(() => ({ state: "failed", error: "Can't reach the server." }));
  while (alive() && (st.state === "queued" || st.state === "working")) {
    onStatus(st.state === "queued" ? "Waiting for another conversion to finish…" : `Making a version this browser can play… ${Math.round((st.progress || 0) * 100)}%`, st.progress || 0);
    await sleep(1500);
    st = await fetch(api(c.id, "web")).then((r) => r.json()).catch(() => st);
  }
  if (!alive()) return null;
  if (st.state !== "ready") {
    onStatus(`Can't play this clip here${st.error ? `: ${st.error}` : "."}`, null, true);
    return null;
  }
  return use(api(c.id, "web.mp4"));
}

/**
 * The views' edge bar, like the desktop sidebar: the selected icon's background
 * fades in where it is, and only a thin accent bar slides to it (down the left
 * edge of the rail, or along the bottom of the tab bar on phones).
 */
export function edgeBar(rail) {
  const bar = el("span", { class: "rail-bar", "aria-hidden": "true" });
  rail.append(bar);
  rail.classList.add("has-bar");
  let first = true;
  const place = () => {
    const on = rail.querySelector(".nav.on");
    if (!on || on.offsetParent === null) return void (bar.style.opacity = "0");
    const row = getComputedStyle(rail).flexDirection === "row";
    if (first) bar.style.transition = "none";
    bar.style.opacity = "1";
    if (row) {
      const w = Math.round(on.offsetWidth * 0.4);
      bar.style.width = `${w}px`;
      bar.style.height = "3px";
      bar.style.transform = `translate(${on.offsetLeft + (on.offsetWidth - w) / 2}px, ${on.offsetTop + on.offsetHeight - 5}px)`;
    } else {
      bar.style.width = "3px";
      bar.style.height = "20px";
      bar.style.transform = `translate(0px, ${on.offsetTop + (on.offsetHeight - 20) / 2}px)`;
    }
    if (first) {
      void bar.offsetWidth;
      bar.style.transition = "";
      first = false;
    }
  };
  let frame = 0;
  const soon = () => {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(place);
  };
  new MutationObserver(soon).observe(rail, { subtree: true, attributes: true, attributeFilter: ["class"] });
  new ResizeObserver(soon).observe(rail);
  place();
}
