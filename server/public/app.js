import { montage } from "./montage.js";
import { editor } from "./editor.js";
import { icon } from "./icons.js";
import { facet, filterClips, groupByDay, sortBy, sortClips } from "./group.js";
import { api, btn, clipTitle, codecLabel, dayLabel, dayTs, duration, el, hydrateIcons, logo, logoUrl, postJson, size } from "./ui.js";

const $ = (id) => document.getElementById(id);
const scroller = $("scroller");
const search = $("search");

const remembered = (k, d) => {
  try {
    return localStorage.getItem(`hs.${k}`) || d;
  } catch {
    return d;
  }
};
const remember = (k, v) => {
  try {
    localStorage.setItem(`hs.${k}`, v);
  } catch {
    /* only a convenience */
  }
};

const VIEWS = ["home", "game", "totrim", "notices", "montage", "cleanup", "trash", "settings", "editor"];
let view = "home";
let game = ""; // the game page's game
let order = remembered("sort", "new");
let pcPick = null; // the game page's PC filter ("" = clips without a PC name, null = all)
const EMPTY = { pcs: [], sources: [], kind: "", from: "", to: "", length: "", damaged: false };
let filters = { ...EMPTY };
let clips = [];
let etag = null;
let canChange = false;
let about = {};
let noticeList = [];

hydrateIcons();

// ---- library ----

async function load() {
  try {
    const res = await fetch("api/clips", { cache: "no-cache" });
    const tag = res.headers.get("etag");
    if (tag && tag === etag) return;
    const data = await res.json();
    etag = tag;
    clips = data.clips;
    canChange = data.canChange;
    about = { release: data.release, version: data.version, scannedAt: data.scannedAt };
    $("brand").title = `Hindsight on the NAS · ${data.release} (${data.version})`;
    updateCounts();
    redraw();
  } catch {
    if (!clips.length) $("home-list").replaceChildren(el("div", { class: "empty" }, el("strong", { text: "Can't reach the server" }), "Check that the NAS is on."));
  }
}

function redraw() {
  if (view === "home") renderHome();
  if (view === "game") renderGame();
  if (view === "totrim") renderToTrim();
}

const activeFilters = () => filters.pcs.length + filters.sources.length + !!filters.kind + !!(filters.from || filters.to) + !!filters.length + !!filters.damaged;

function setFilters(p) {
  filters = { ...filters, ...p };
  syncUrl();
  renderHome();
}

function updateCounts() {
  const n = clips.filter((c) => c.toTrim && !c.damaged).length;
  $("count-totrim").hidden = !n;
  $("count-totrim").textContent = n > 99 ? "99+" : String(n);
}

// ---- cards ----

function thumbImg(c) {
  const img = el("img", { src: api(c.id, "thumb.jpg"), alt: "", loading: "lazy", decoding: "async" });
  img.addEventListener("error", () => img.classList.add("broken"));
  return img;
}

function tags(c) {
  return [
    c.damaged && el("span", { class: "tag bad", title: c.why ? `Can't be read: ${c.why}` : "Can't be read" }, icon("alert", 12), "Damaged"),
    c.recording && el("span", { class: "tag rec", text: "REC" }),
    c.source && el("span", { class: "tag", text: c.source }),
  ];
}

/** "VALORANT · GAMING-PC": which game, and which PC it came from. */
function cardMeta(c, withGame = true) {
  return el("div", { class: "card-meta" }, el("span", { text: [withGame ? c.game : null, c.pc].filter(Boolean).join(" · ") || size(c.size) }));
}

function card(c, { withGame = true, queue = null, extra = null } = {}) {
  const b = el(
    "button",
    { class: "card", title: c.name, onclick: () => openClip(c, queue) },
    el("div", { class: "thumb" }, thumbImg(c), c.duration ? el("span", { class: "badge", text: duration(c.duration) }) : null, el("div", { class: "card-tags" }, ...tags(c))),
    el("div", { class: "card-text" }, el("div", { class: "card-title", text: clipTitle(c) }), cardMeta(c, withGame)),
  );
  return extra ? el("div", { class: "card" }, b, extra) : b;
}

// ---- home: one row per game ----

function renderHome() {
  renderFilters();
  const box = $("home-list");
  if (!clips.length) {
    box.replaceChildren(el("div", { class: "empty" }, el("strong", { text: "No clips yet" }), el("span", { text: "Clips show up here once Hindsight copies them to the NAS." })));
    return timeline.set([]);
  }
  const searching = search.value.trim() || activeFilters();
  $("home-title").textContent = searching ? "Results" : "Home";
  if (searching) {
    const found = sortBy(filterClips(clips, { search: search.value, ...filters }), "new");
    if (!found.length) {
      box.replaceChildren(el("div", { class: "empty" }, el("strong", { text: "Nothing matches" }), activeFilters() ? btn("Clear filters", { onclick: () => setFilters({ ...EMPTY }) }) : null));
      return timeline.set([]);
    }
    return renderDays(box, found, { withGame: true });
  }
  timeline.set([]);
  // Games ordered by their newest clip.
  const byGame = new Map();
  for (const c of sortClips(clips)) {
    if (!byGame.has(c.game)) byGame.set(c.game, []);
    byGame.get(c.game).push(c);
  }
  const memories = onThisDay(clips);
  box.replaceChildren(
    ...(memories.length ? [memoryStrip(memories)] : []),
    ...[...byGame].map(([g, list]) =>
      el(
        "section",
        { class: "shelf" },
        el(
          "div",
          { class: "shelf-head" },
          logo(g),
          el("h2", { text: g }),
          el("span", { class: "muted", text: `${list.length} clip${list.length === 1 ? "" : "s"}` }),
          btn("See all", { cls: "small see-all", onclick: () => openGame(g) }),
        ),
        el("div", { class: "strip" }, ...list.slice(0, 20).map((c) => card(c, { withGame: false }))),
      ),
    ),
  );
}

function renderFilters() {
  const panel = $("filters");
  const n = activeFilters();
  $("filters-count").hidden = !n;
  $("filters-count").textContent = String(n);
  const toggleIn = (key, value) => {
    const cur = filters[key];
    setFilters({ [key]: cur.includes(value) ? cur.filter((v) => v !== value) : [...cur, value] });
  };
  const group = (label, ...kids) => el("div", { class: "fgroup" }, el("span", { class: "flabel", text: label }), el("div", { class: "fopts" }, ...kids));
  const opt = (on, label, onclick, count) => el("button", { class: `chip small ${on ? "on" : ""}`, onclick, "aria-pressed": String(!!on) }, label, count !== undefined ? el("small", { text: String(count) }) : null);
  const pcs = facet(clips, "pc");
  const sources = facet(clips, "source");
  const date = (key, label) => {
    const i = el("input", { type: "date", value: filters[key], "aria-label": label });
    i.addEventListener("change", () => setFilters({ [key]: i.value }));
    return i;
  };
  const damaged = clips.filter((c) => c.damaged).length;
  panel.replaceChildren(
    pcs.length > 1 || (pcs.length && pcs[0][0]) ? group("PC", ...pcs.map(([v, c]) => opt(filters.pcs.includes(v), v || "Unknown", () => toggleIn("pcs", v), c))) : null,
    sources.length > 1 ? group("Made with", ...sources.map(([v, c]) => opt(filters.sources.includes(v), v || "Hindsight", () => toggleIn("sources", v), c))) : null,
    group("Type", opt(!filters.kind, "Any", () => setFilters({ kind: "" })), opt(filters.kind === "clip", "Clips", () => setFilters({ kind: "clip" })), opt(filters.kind === "recording", "Recordings", () => setFilters({ kind: "recording" }))),
    group(
      "Length",
      opt(!filters.length, "Any", () => setFilters({ length: "" })),
      opt(filters.length === "short", "Under 30 s", () => setFilters({ length: "short" })),
      opt(filters.length === "medium", "30 s to 2 min", () => setFilters({ length: "medium" })),
      opt(filters.length === "long", "Over 2 min", () => setFilters({ length: "long" })),
    ),
    group("Date", date("from", "From"), el("span", { class: "muted", text: "to" }), date("to", "To")),
    group("Health", opt(filters.damaged, "Damaged only", () => setFilters({ damaged: !filters.damaged }), damaged), n ? btn("Clear filters", { cls: "small", onclick: () => setFilters({ ...EMPTY }) }) : null),
  );
}

/** A grid by day with the date scrubber (date orders), or one grid (size / length orders). */
function renderDays(box, found, { withGame, sortOrder = "new" }) {
  if (sortOrder === "big" || sortOrder === "long") {
    box.replaceChildren(el("div", { class: "grid" }, ...found.map((c) => card(c, { withGame }))));
    return timeline.set([]);
  }
  const days = groupByDay(found);
  if (sortOrder === "old") {
    days.reverse();
    days.forEach((d) => d.clips.reverse());
  }
  box.replaceChildren(
    ...days.map(({ day, clips: items }) =>
      el(
        "section",
        { class: "day", "data-day": day },
        el("div", { class: "day-head" }, el("h3", { text: dayLabel(day) }), el("span", { class: "muted", text: `${items.length} clip${items.length === 1 ? "" : "s"}` })),
        el("div", { class: "grid" }, ...items.map((c) => card(c, { withGame }))),
      ),
    ),
  );
  timeline.set(days.map((d) => ({ key: d.day, ts: dayTs(d.day) })));
}

// ---- a game's page ("See all") ----

function openGame(g) {
  game = g;
  pcPick = null;
  showView("game");
}

function renderGame() {
  const all = clips.filter((c) => c.game === game);
  if (!all.length) return showView("home");
  const bytes = all.reduce((n, c) => n + c.size, 0);
  $("game-title").textContent = game;
  $("game-logo").src = logoUrl(game);
  $("game-sub").textContent = `${all.length} clip${all.length === 1 ? "" : "s"} · ${size(bytes)}`;
  $("sort").value = order;
  const pcs = facet(all, "pc");
  if (pcPick !== null && !pcs.some(([p]) => p === pcPick)) pcPick = null;
  const chip = (p, label, n) => el("button", { class: `chip ${pcPick === p ? "on" : ""}`, "aria-pressed": String(pcPick === p), onclick: () => ((pcPick = p), renderGame()) }, icon("monitor", 14), label, el("small", { text: String(n) }));
  $("pc-chips").replaceChildren(...(pcs.length > 1 ? [chip(null, "All PCs", all.length), ...pcs.map(([p, n]) => chip(p, p || "Unknown PC", n))] : []));
  const found = sortBy(pcPick !== null ? all.filter((c) => (c.pc || "") === pcPick) : all, order);
  renderDays($("game-list"), found, { withGame: false, sortOrder: order });
}

// ---- On this day: clips from this date in earlier years ----

function onThisDay(all, now = new Date()) {
  const out = [];
  const byYear = new Map();
  const at = (c) => dayTs(c.day);
  for (const c of all) {
    const back = now.getFullYear() - new Date(at(c)).getFullYear();
    if (back >= 1 && back <= 20 && !c.damaged) byYear.set(back, [...(byYear.get(back) ?? []), c]);
  }
  for (const back of [...byYear.keys()].sort((a, b) => a - b)) {
    const list = byYear.get(back);
    const same = list.filter((c) => {
      const d = new Date(at(c));
      return d.getMonth() === now.getMonth() && d.getDate() === now.getDate();
    });
    const then = new Date(now);
    then.setFullYear(now.getFullYear() - back);
    const week = list.filter((c) => Math.abs(at(c) - then.getTime()) <= 3.5 * 86400000);
    const ago = `${back} year${back === 1 ? "" : "s"} ago`;
    if (same.length) out.push({ label: `${ago} today`, clips: same.sort((a, b) => a.mtime - b.mtime) });
    else if (week.length) out.push({ label: `This week, ${ago}`, clips: week.sort((a, b) => a.mtime - b.mtime) });
  }
  return out;
}

function memoryStrip(memories) {
  return el(
    "section",
    { class: "shelf" },
    el("div", { class: "shelf-head" }, el("h2", { text: "On this day" })),
    el(
      "div",
      { class: "memories-row" },
      ...memories.map((m) =>
        el(
          "button",
          { class: "memory-card", onclick: () => openClip(m.clips[0], m.clips) },
          thumbImg(m.clips[0]),
          el("span", { class: "memory-text" }, el("strong", { text: m.label }), el("small", { text: `${dayLabel(m.clips[0].day)} · ${m.clips.length} clip${m.clips.length === 1 ? "" : "s"}` })),
        ),
      ),
    ),
  );
}

// ---- To trim ----

function renderToTrim() {
  const list = sortClips(clips.filter((c) => c.toTrim && !c.damaged));
  const bytes = list.reduce((n, c) => n + c.size, 0);
  $("totrim-lead").textContent = list.length
    ? `${list.length} clip${list.length === 1 ? "" : "s"} nobody has trimmed or edited yet, ${size(bytes)} in all. Trim one to keep only the good part, or mark it Keep whole to take it off this list.`
    : "Every clip has been trimmed, edited or kept whole.";
  const keep = async (c, b) => {
    b.disabled = true;
    try {
      await postJson(api(c.id, "keep-whole"));
      await load();
    } catch (e) {
      b.disabled = false;
      alert(e.message);
    }
  };
  $("totrim-list").replaceChildren(
    list.length
      ? el(
          "div",
          { class: "grid" },
          ...list.map((c) => {
            const k = btn("Keep whole", { cls: "small", icon: "check" });
            k.addEventListener("click", () => keep(c, k));
            return card(c, { extra: el("div", { class: "card-actions" }, btn("Trim", { cls: "small primary", icon: "scissors", onclick: () => openEditor(c) }), k) });
          }),
        )
      : el("div", { class: "empty" }, el("strong", { text: "Nothing to trim" })),
  );
}

// ---- notifications ----

const NOTICE_ICON = { damaged: "alert", folders: "folder", dupes: "copy", new: "monitor" };

async function loadNotices() {
  try {
    const j = await (await fetch("api/notices", { cache: "no-cache" })).json();
    noticeList = j.notices ?? [];
    $("count-notices").hidden = !noticeList.length;
    $("count-notices").textContent = String(noticeList.length);
    if (view === "notices") renderNotices(j.canChange);
  } catch {
    /* tried again later */
  }
}

function renderNotices(change = canChange) {
  const box = $("notices-list");
  if (!noticeList.length) return box.replaceChildren(el("div", { class: "empty" }, el("strong", { text: "Nothing needs a look" }), el("span", { text: "Damaged clips, duplicates, folders spelled differently and new clips from your PCs show up here." })));
  box.replaceChildren(
    ...noticeList.map((n) => {
      const status = el("span", { class: "row-status muted" });
      const go = btn(n.action.label, { cls: "small primary" });
      if (n.action.needsChange && !change) {
        go.disabled = true;
        go.title = "Changes are off on this server";
      }
      go.addEventListener("click", async () => {
        if (n.action.do === "filter-damaged") return showHome({ ...EMPTY, damaged: true });
        if (n.action.do === "filter-pc") {
          await postJson("api/notices/hide", { id: n.id }).catch(() => {});
          loadNotices();
          return showHome({ ...EMPTY, pcs: [n.action.pc] });
        }
        if (n.action.do === "dedupe" && !confirm(`${n.action.label}?\n\nThey go to the trash for 30 days (Trash → Restore).`)) return;
        go.disabled = true;
        status.textContent = "Working…";
        try {
          await postJson("api/notices/act", { id: n.id });
          await Promise.all([load(), loadNotices(), loadTrashCount()]);
        } catch (e) {
          status.textContent = e.message;
          status.className = "row-status error";
          go.disabled = false;
        }
      });
      const hide = el("button", { class: "btn small icon-only", title: "Hide", "aria-label": `Hide: ${n.title}` }, icon("x", 16));
      hide.addEventListener("click", async () => {
        await postJson("api/notices/hide", { id: n.id }).catch(() => {});
        loadNotices();
      });
      const clip = n.clip && clips.find((c) => c.id === n.clip);
      return el(
        "div",
        { class: "row-item" },
        clip ? el("button", { class: "row-thumb", onclick: () => openClip(clip), title: "Play" }, thumbImg(clip)) : el("div", { class: `row-ic ${n.kind === "damaged" ? "bad" : ""}` }, icon(NOTICE_ICON[n.kind] || "bell")),
        el("div", { class: "row-main" }, el("div", { class: "row-title", text: n.title }), el("div", { class: "row-meta", text: n.detail }), status),
        el("div", { class: "row-actions" }, go, hide),
      );
    }),
  );
}

function showHome(f) {
  filters = f;
  search.value = "";
  $("filters").hidden = false;
  $("filters-btn").setAttribute("aria-expanded", "true");
  showView("home");
}

// ---- date scrubber (a port of the desktop app's Timeline.tsx) ----

const timeline = (() => {
  const rail = $("rail");
  let days = [];
  let marks = [];
  let dragging = false;
  const viewBox = el("div", { class: "tl-view" });
  const bubble = el("div", { class: "tl-bubble" });
  const listOf = () => (view === "game" ? $("game-list") : $("home-list"));
  const label = (ts) => new Date(ts).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric", year: new Date(ts).getFullYear() === new Date().getFullYear() ? undefined : "numeric" });

  function measure() {
    const total = Math.max(1, scroller.scrollHeight);
    const h = rail.clientHeight;
    let lastMonth = "";
    let lastYear = "";
    let lastLabelY = -99;
    marks = [];
    const ticks = [];
    for (const d of days) {
      const sec = listOf().querySelector(`[data-day="${CSS.escape(d.key)}"]`);
      if (!sec) continue;
      const y = ((sec.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop) / total) * h;
      const date = new Date(d.ts);
      const month = date.toLocaleDateString(undefined, { month: "short" });
      const year = String(date.getFullYear());
      const m = { key: d.key, ts: d.ts, y };
      if (year !== lastYear) {
        m.year = year;
        if (y - lastLabelY > 26) m.month = month;
        lastLabelY = y;
      } else if (month !== lastMonth && y - lastLabelY > 16) {
        m.month = month;
        lastLabelY = y;
      }
      lastMonth = month;
      lastYear = year;
      marks.push(m);
      const tick = el("span", { class: `tl-day ${m.month || m.year ? "month" : ""} ${m.year ? "year" : ""}` });
      tick.style.top = `${y}px`;
      if (m.month || m.year) tick.append(el("em", {}, m.year && el("b", { text: m.year }), m.month || ""));
      ticks.push(tick);
    }
    rail.replaceChildren(viewBox, ...ticks, bubble);
    position();
  }

  let queued = false;
  function position() {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      const total = Math.max(1, scroller.scrollHeight);
      const h = rail.clientHeight;
      viewBox.style.transform = `translateY(${(scroller.scrollTop / total) * h}px)`;
      viewBox.style.height = `${Math.max(3, (scroller.clientHeight / total) * h)}px`;
    });
  }

  const dayAt = (y) => {
    let best = marks[0];
    for (const m of marks) if (m.y <= y + 1) best = m;
    return best;
  };

  function jump(clientY, snap = true) {
    const rect = rail.getBoundingClientRect();
    const y = Math.max(0, Math.min(rect.height, clientY - rect.top));
    const d = dayAt(y);
    const sec = snap && d && listOf().querySelector(`[data-day="${CSS.escape(d.key)}"]`);
    scroller.scrollTop = sec ? sec.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop - 8 : (y / rect.height) * scroller.scrollHeight - scroller.clientHeight / 2;
  }

  function hover(clientY) {
    const rect = rail.getBoundingClientRect();
    const y = Math.max(0, Math.min(rect.height, clientY - rect.top));
    const d = dayAt(y);
    if (!d) return;
    bubble.textContent = label(d.ts);
    bubble.style.top = `${y}px`;
    rail.classList.add("active");
  }

  rail.addEventListener("pointermove", (e) => {
    if (rail.classList.contains("empty-rail")) return;
    hover(e.clientY);
    if (dragging) jump(e.clientY, false);
  });
  rail.addEventListener("pointerleave", () => !dragging && rail.classList.remove("active"));
  rail.addEventListener("pointerdown", (e) => {
    if (rail.classList.contains("empty-rail")) return;
    dragging = true;
    rail.setPointerCapture(e.pointerId);
    hover(e.clientY);
    jump(e.clientY);
  });
  const stop = () => {
    dragging = false;
    if (matchMedia("(hover: none)").matches) rail.classList.remove("active");
  };
  rail.addEventListener("pointerup", stop);
  rail.addEventListener("pointercancel", stop);
  scroller.addEventListener("scroll", position, { passive: true });
  new ResizeObserver(() => days.length > 1 && measure()).observe(scroller);
  addEventListener("resize", () => days.length > 1 && measure());

  return {
    set(next) {
      days = next;
      const empty = days.length < 2;
      rail.classList.toggle("empty-rail", empty);
      if (empty) rail.replaceChildren();
      else requestAnimationFrame(measure);
    },
  };
})();

// ---- player ----

const dialog = $("player");
const video = $("video");
let session = 0;

function setStatus(text, progress = null, isError = false) {
  $("status").hidden = !text;
  $("status-text").textContent = text || "";
  $("status-text").className = isError ? "error" : "";
  $("status-bar").parentElement.hidden = progress === null;
  if (progress !== null) $("status-bar").style.width = `${Math.round(progress * 100)}%`;
}

function playSrc(src) {
  setStatus("");
  video.src = src;
  video.play().catch(() => {});
}

let queue = null;

function step(c) {
  const at = queue ? queue.findIndex((x) => x.id === c.id) : -1;
  $("prev-clip").hidden = $("next-clip").hidden = !queue || queue.length < 2;
  $("prev-clip").disabled = at <= 0;
  $("next-clip").disabled = at < 0 || at >= queue.length - 1;
  $("prev-clip").onclick = () => openClip(queue[at - 1], queue);
  $("next-clip").onclick = () => openClip(queue[at + 1], queue);
  video.onended = () => queue && at >= 0 && at < queue.length - 1 && openClip(queue[at + 1], queue);
}

async function share(c) {
  const host = $("share-host").value;
  const out = $("share-out");
  const b = $("share");
  b.disabled = true;
  const label = (t) => b.replaceChildren(icon("share"), t);
  try {
    let st = await (await fetch(api(c.id, "web"))).json();
    if (st.state !== "ready") {
      if (st.state === "none" || st.state === "failed") st = await (await fetch(api(c.id, "web"), { method: "POST" })).json();
      while (st.state === "queued" || st.state === "working") {
        label(`Preparing… ${Math.round((st.progress || 0) * 100)}%`);
        await new Promise((r) => setTimeout(r, 1500));
        st = await (await fetch(api(c.id, "web"))).json();
      }
      if (st.state !== "ready") throw new Error(st.error || "Couldn't make the phone version.");
    }
    label("Uploading…");
    const res = await fetch(`${api(c.id, "share")}?host=${host}`, { method: "POST" });
    const j = await res.json();
    if (!res.ok) throw new Error(j.error || "Upload failed.");
    out.hidden = false;
    $("share-url").value = j.url;
    $("share-url").select();
  } catch (e) {
    out.hidden = false;
    $("share-url").value = `Couldn't share: ${e.message}`;
  } finally {
    b.disabled = false;
    label("Share link");
  }
}

$("share-copy").addEventListener("click", () => {
  const i = $("share-url");
  i.select();
  if (navigator.clipboard && window.isSecureContext) navigator.clipboard.writeText(i.value).catch(() => document.execCommand("copy"));
  else document.execCommand("copy");
  $("share-copy").textContent = "Copied";
  setTimeout(() => ($("share-copy").textContent = "Copy"), 1500);
});

async function openClip(c, inQueue = null) {
  queue = inQueue;
  step(c);
  $("share-out").hidden = true;
  $("share").onclick = () => share(c);
  $("edit-clip").onclick = () => openEditor(c);
  $("keep-whole").hidden = !c.toTrim;
  $("keep-whole").onclick = async () => {
    await postJson(api(c.id, "keep-whole")).catch((e) => alert(e.message));
    $("keep-whole").hidden = true;
    load();
  };
  fetch(api(c.id, "links"))
    .then((r) => r.json())
    .then((links) => {
      if (Array.isArray(links) && links.length) {
        $("share-out").hidden = false;
        $("share-url").value = links[0].url;
      }
    })
    .catch(() => {});
  const my = ++session;
  history.replaceState(null, "", `${location.pathname}${location.search}#clip=${c.id}`);
  $("player-logo").src = logoUrl(c.game);
  $("player-title").textContent = `${c.game} · ${clipTitle(c)}`;
  $("player-meta").textContent = [c.pc, c.source, size(c.size), c.ext.toUpperCase()].filter(Boolean).join(" · ");
  $("dl-original").href = `${api(c.id, "original")}?download`;
  $("dl-web").hidden = true;
  $("convert").hidden = true;
  $("try-original").hidden = true;
  $("note").hidden = true;
  video.removeAttribute("src");
  video.load();
  setStatus("Loading…");
  if (!dialog.open) dialog.showModal();

  let info;
  try {
    info = await (await fetch(api(c.id, "info"))).json();
  } catch {
    info = null;
  }
  if (my !== session) return;
  if (!info || info.error) return setStatus(`Can't read this clip${info?.error ? `: ${info.error}` : "."}`, null, true);

  $("player-meta").textContent = [c.pc, c.source, duration(info.duration), info.video && `${info.video.height}p ${codecLabel(info.video.codec)}`, size(c.size)].filter(Boolean).join(" · ");

  if (info.directPlay) return playSrc(api(c.id, "original"));
  if (info.web.state === "ready") return showWeb(c);
  if (info.web.state === "working" || info.web.state === "queued") return convert(c);
  $("convert").hidden = false;
  $("convert").onclick = () => convert(c);
  if (info.audio.length > 1) {
    $("note").hidden = false;
    $("note").textContent = "The original has separate desktop and mic tracks; the browser plays only the first. The phone version mixes them.";
  }
  tryOriginal(c, info, my);
}

function tryOriginal(c, info, my) {
  let settled = false;
  const fail = () => {
    if (settled || my !== session) return;
    settled = true;
    cleanup();
    convert(c);
  };
  const ok = () => {
    if (settled || my !== session) return;
    if (info.video && video.videoWidth === 0) return fail();
    settled = true;
    cleanup();
  };
  const timer = setTimeout(fail, 8000);
  const cleanup = () => {
    clearTimeout(timer);
    video.removeEventListener("error", fail);
    video.removeEventListener("loadeddata", ok);
  };
  video.addEventListener("error", fail);
  video.addEventListener("loadeddata", ok);
  playSrc(api(c.id, "original"));
}

async function convert(c) {
  const my = session;
  video.pause();
  video.removeAttribute("src");
  video.load();
  $("convert").hidden = true;
  setStatus("Making a version that plays everywhere…", 0);
  let st;
  try {
    st = await (await fetch(api(c.id, "web"), { method: "POST" })).json();
  } catch {
    return setStatus("Can't reach the server.", null, true);
  }
  while (my === session && (st.state === "queued" || st.state === "working")) {
    setStatus(st.state === "queued" ? "Waiting for another conversion to finish…" : `Making a version that plays everywhere… ${Math.round(st.progress * 100)}%`, st.progress);
    await new Promise((r) => setTimeout(r, 1500));
    if (my !== session) return;
    try {
      st = await (await fetch(api(c.id, "web"))).json();
    } catch {
      /* keep polling */
    }
  }
  if (my !== session) return;
  if (st.state === "ready") return showWeb(c);
  setStatus(`Conversion failed${st.error ? `: ${st.error}` : "."}`, null, true);
  $("convert").hidden = false;
  $("convert").textContent = "Try again";
}

function showWeb(c) {
  $("dl-web").hidden = false;
  $("dl-web").href = `${api(c.id, "web.mp4")}?download`;
  $("try-original").hidden = false;
  $("try-original").onclick = () => {
    $("try-original").hidden = true;
    playSrc(api(c.id, "original"));
  };
  playSrc(api(c.id, "web.mp4"));
}

function closePlayer() {
  session++;
  video.pause();
  video.removeAttribute("src");
  video.load();
  if (location.hash) history.replaceState(null, "", location.pathname + location.search);
}

$("close").addEventListener("click", () => dialog.close());
dialog.addEventListener("close", closePlayer);
dialog.addEventListener("click", (e) => e.target === dialog && dialog.close());

// ---- settings: colour and storage ----

const SCHEMES = [
  ["winblue", "Windows blue", "#4cc2ff"],
  ["cyan", "Cyan", "#38d6ff"],
  ["crimson", "Crimson", "#ff4d6d"],
  ["volt", "Volt", "#b6f23a"],
  ["ember", "Ember", "#ff9a1f"],
  ["violet", "Violet", "#a78bfa"],
  ["emerald", "Emerald", "#2ee59d"],
  ["gold", "Gold", "#ffd23f"],
  ["pink", "Pink", "#ff5fc8"],
  ["teal", "Teal", "#20c9c0"],
  ["mono", "Mono", "#f2f4f7"],
];
const currentScheme = () => document.documentElement.getAttribute("data-accent") || "winblue";

function applyScheme(id) {
  const s = SCHEMES.find((x) => x[0] === id) ?? SCHEMES[0];
  if (s[0] === "winblue") document.documentElement.removeAttribute("data-accent");
  else document.documentElement.setAttribute("data-accent", s[0]);
  remember("accent", s[0]);
  renderSwatches();
}

function renderSwatches() {
  const cur = currentScheme();
  $("swatches").replaceChildren(
    ...SCHEMES.map(([id, name, accent]) => {
      const dot = el("i");
      dot.style.background = accent;
      return el("button", { class: `swatch ${id === cur ? "on" : ""}`, "aria-pressed": String(id === cur), onclick: () => applyScheme(id) }, dot, name);
    }),
  );
}

let pairTimer = 0;
async function renderPairing() {
  clearTimeout(pairTimer);
  const body = $("pair-body");
  let p;
  try {
    p = await (await fetch("api/pair", { cache: "no-cache" })).json();
  } catch {
    return body.replaceChildren(el("p", { class: "error", text: "Can't read the paired PCs right now." }));
  }
  const rows = p.paired.map((x) =>
    el(
      "div",
      { class: "pair-row" },
      el("span", {}, el("b", { text: x.pc }), el("small", { class: "muted", text: ` paired ${new Date(x.at).toLocaleDateString()}` })),
      btn("Forget", {
        cls: "small",
        onclick: async () => {
          await postJson("api/pair/forget", { pc: x.pc });
          renderPairing();
        },
      }),
    ),
  );
  const code = p.code
    ? el(
        "div",
        { class: "pair-code" },
        el("b", { class: "mono", text: `${p.code.code.slice(0, 3)} ${p.code.code.slice(3)}` }),
        el("small", { class: "muted", text: `Type this in Hindsight on the PC. Works once, until ${new Date(p.code.expiresAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}.` }),
      )
    : null;
  body.replaceChildren(
    ...(rows.length ? rows : [el("p", { class: "muted small", text: "No PCs paired yet." })]),
    code ??
      btn("Pair a PC", {
        cls: "primary",
        onclick: async () => {
          await postJson("api/pair/code");
          renderPairing();
        },
      }),
  );
  // While a code is showing, notice when the PC has used it.
  if (p.code && view === "settings") pairTimer = setTimeout(renderPairing, 3000);
}

async function renderSettings() {
  renderSwatches();
  renderPairing();
  $("about").textContent = `Hindsight on the NAS ${about.release ?? ""} (${about.version ?? ""}). ${canChange ? "Cleanup and the editor can change files." : "Read-only: edits are kept in the cache to download, and cleanup only lists clips."}${about.scannedAt ? ` Library read ${new Date(about.scannedAt).toLocaleTimeString()}.` : ""}`;
  let s;
  try {
    s = await (await fetch("api/storage", { cache: "no-cache" })).json();
  } catch {
    return $("storage-body").replaceChildren(el("p", { class: "error", text: "Can't read the storage right now." }));
  }
  $("storage-sub").textContent = `${s.clips.count} clips in ${s.games.length} game${s.games.length === 1 ? "" : "s"}, ${size(s.clips.bytes)}`;
  const parts = [];
  if (s.disk) {
    const other = Math.max(0, s.disk.used - s.clips.bytes);
    const seg = (b, color) => {
      const i = el("i");
      i.style.width = `${((b / s.disk.total) * 100).toFixed(2)}%`;
      i.style.background = color;
      return i;
    };
    parts.push(
      el("div", { class: "meter-bar" }, seg(Math.min(s.clips.bytes, s.disk.used), "var(--accent)"), seg(other, "var(--text-2)")),
      el(
        "div",
        { class: "legend" },
        el("span", {}, el("i", { class: "dot", style: "background: var(--accent)" }), "Clips ", el("b", { text: size(s.clips.bytes) })),
        other > 0 && el("span", {}, el("i", { class: "dot", style: "background: var(--text-2)" }), "Other files ", el("b", { text: size(other) })),
        el("span", {}, el("i", { class: "dot", style: "background: var(--raised-2)" }), "Free ", el("b", { text: size(s.disk.free) })),
        el("span", {}, "of ", el("b", { text: size(s.disk.total) })),
      ),
    );
  }
  parts.push(el("p", { class: "muted small", text: `Thumbnails, phone versions and logos use ${size(s.cache.bytes)} more, in the app's own storage.` }));
  const top = s.games[0]?.bytes || 1;
  parts.push(
    el(
      "div",
      { class: "game-space" },
      ...s.games.flatMap((g) => {
        const bar = el("i");
        bar.style.width = `${Math.max(1, (g.bytes / top) * 100)}%`;
        return [logo(g.game), el("span", { text: g.game }), el("b", { text: size(g.bytes) }), el("span", {}), el("div", { class: "gbar" }, bar), el("span", {}), el("small", { text: `${g.count} clip${g.count === 1 ? "" : "s"} · ${Math.round((g.bytes / Math.max(1, s.clips.bytes)) * 100)}%` })];
      }),
    ),
    btn("Free up space", { icon: "drive", onclick: () => showView("cleanup") }),
  );
  $("storage-body").replaceChildren(...parts);
}

// ---- cleanup ----

async function loadCleanup() {
  const box = $("cleanup-list");
  let data;
  try {
    data = await (await fetch("api/cleanup", { cache: "no-cache" })).json();
  } catch {
    return box.replaceChildren(el("p", { class: "error", text: "Can't reach the server." }));
  }
  canChange = data.canChange;
  $("cleanup-readonly").hidden = data.canChange;
  const pct = Math.round((data.shownBytes / Math.max(1, data.totalBytes)) * 100);
  $("cleanup-lead").replaceChildren(
    el(
      "span",
      {},
      data.clips.length ? `The ${data.clips.length} biggest clips use ${size(data.shownBytes)}, ${pct}% of all your clips. ` : "Nothing left to look at: every big clip has been kept or cleaned up. ",
      data.canChange ? `Delete moves a clip to the trash for ${data.trashDays} days. Shrink swaps it for a smaller H.264 copy. ` : "",
      "Keep hides it until your next cleanup check.",
      data.kept ? el("span", { class: "muted", text: ` ${data.kept} kept since the last check.` }) : null,
    ),
  );
  box.replaceChildren(...data.clips.map((c) => cleanupRow(c, data)));
}

function cleanupRow(c, data) {
  const status = el("span", { class: "row-status muted" });
  const buttons = [];
  const act = async (label, fn) => {
    buttons.forEach((b) => (b.disabled = true));
    status.textContent = label;
    try {
      await fn();
      row.classList.add("gone");
      setTimeout(() => {
        row.remove();
        load();
        loadTrashCount();
      }, 250);
    } catch (e) {
      status.textContent = e.message;
      status.className = "row-status error";
      buttons.forEach((b) => (b.disabled = false));
    }
  };
  const shrink = async () => {
    for (;;) {
      const st = await postJson(api(c.id, "shrink"));
      if (st.state === "done") return (status.textContent = `Saved ${size(st.saved)}`);
      if (st.state === "failed") throw new Error(st.error || "The H.264 copy failed.");
      status.textContent = st.state === "queued" ? "Waiting for another conversion…" : `Making the smaller copy… ${Math.round((st.progress || 0) * 100)}%`;
      await new Promise((r) => setTimeout(r, 1500));
    }
  };
  const b = (label, cls, title, fn, needsChange) => {
    const x = btn(label, { cls: `small ${cls}`, title: needsChange && !data.canChange ? "Changes are off on this server" : title, onclick: (e) => (e.stopPropagation(), fn()) });
    if (needsChange && !data.canChange) x.disabled = true;
    buttons.push(x);
    return x;
  };
  const row = el(
    "div",
    { class: "row-item" },
    el("button", { class: "row-thumb", onclick: () => openClip(c), title: "Play" }, thumbImg(c), c.duration ? el("span", { class: "badge", text: duration(c.duration) }) : null),
    el(
      "div",
      { class: "row-main" },
      el("div", { class: "row-title", text: `${c.game} · ${clipTitle(c)}` }),
      el("div", { class: "row-meta" }, [c.pc, c.codec && `${c.height ? `${c.height}p ` : ""}${codecLabel(c.codec)}`].filter(Boolean).join(" · "), ...tags(c)),
      status,
    ),
    el("div", { class: "row-size big", text: size(c.size) }),
    el(
      "div",
      { class: "row-actions" },
      b("Keep", "", "Hide it until the next cleanup check", () => act("Keeping…", () => postJson(api(c.id, "keep")))),
      b("Shrink", "", "Swap it for a smaller H.264 copy (asks first; the original goes to the trash)", () => {
        if (!confirm(`Shrink "${c.name}"?\n\nIt's replaced by a smaller H.264 copy that looks a little worse. The full-quality original goes to the trash for ${data.trashDays} days (Trash → Restore), then it's gone for good.`)) return;
        act("Shrinking…", shrink);
      }, true),
      b("Delete", "danger", `Move it to the trash for ${data.trashDays} days (asks first)`, () => {
        if (!confirm(`Delete "${c.name}"?\n\nIt goes to the trash for ${data.trashDays} days (Trash → Restore), then it's gone for good.`)) return;
        act("Deleting…", () => postJson(api(c.id, "trash")));
      }, true),
    ),
  );
  return row;
}

$("check").addEventListener("click", async () => {
  $("check").disabled = true;
  try {
    await postJson("api/cleanup/check");
    await loadCleanup();
  } finally {
    $("check").disabled = false;
  }
});

// ---- trash ----

async function loadTrashCount() {
  try {
    const t = await (await fetch("api/trash", { cache: "no-cache" })).json();
    $("trash-count").hidden = !t.items.length;
    $("trash-count").textContent = String(t.items.length);
    return t;
  } catch {
    return null;
  }
}

function ago(ms) {
  const days = Math.floor((Date.now() - ms) / 86400000);
  if (days < 1) return "today";
  if (days === 1) return "yesterday";
  return `${days} days ago`;
}

async function loadTrash() {
  const t = await loadTrashCount();
  const box = $("trash-list");
  if (!t) return box.replaceChildren(el("p", { class: "error", text: "Can't reach the server." }));
  $("empty-trash").hidden = !t.items.length || !t.canChange;
  $("trash-lead").textContent = t.items.length
    ? `${t.items.length} clip${t.items.length === 1 ? "" : "s"}, ${size(t.bytes)}. Each one is removed for good ${t.trashDays} days after it was deleted. Restore puts it back where it was.`
    : `Deleted clips wait here for ${t.trashDays} days before they're removed for good.`;
  box.replaceChildren(
    ...(t.items.length ? [] : [el("div", { class: "empty" }, el("strong", { text: "The trash is empty" }))]),
    ...t.items.map((e) => {
      const status = el("span", { class: "row-status muted" });
      const restore = btn("Restore", { cls: "small", icon: "restore" });
      restore.disabled = !t.canChange;
      restore.addEventListener("click", async () => {
        restore.disabled = true;
        try {
          await postJson(`api/trash/${encodeURIComponent(e.id)}/restore`);
          loadTrash();
          load();
        } catch (err) {
          status.textContent = err.message;
          restore.disabled = false;
        }
      });
      const gone = new Date(e.removeAt).toLocaleDateString(undefined, { month: "short", day: "numeric" });
      return el(
        "div",
        { class: "row-item" },
        el("div", { class: "row-ic" }, icon("trash")),
        el("div", { class: "row-main" }, el("div", { class: "row-title", text: e.name }), el("div", { class: "row-meta", text: `${e.game} · ${e.why} ${ago(e.trashedAt)} · removed for good on ${gone}` }), status),
        el("div", { class: "row-size", text: size(e.size) }),
        el("div", { class: "row-actions" }, restore),
      );
    }),
  );
}

$("empty-trash").addEventListener("click", async () => {
  if (!confirm("Remove everything in the trash for good? This can't be undone.")) return;
  try {
    await postJson("api/trash/empty");
  } catch (e) {
    alert(e.message);
  }
  loadTrash();
});

// ---- views ----

const montageView = montage($("montage-root"), () => clips);
const editorView = editor($("editor-root"), {
  back: () => showView(editorFrom || "home"),
  changed: () => Promise.all([load(), loadNotices()]),
});
let editorFrom = "home";

function openEditor(c) {
  if (dialog.open) dialog.close();
  if (view !== "editor") editorFrom = view;
  editorView.open(c);
  showView("editor", { clip: c.id });
}

function showView(v, { clip = null } = {}) {
  if (v !== "editor" && view === "editor") editorView.close();
  view = v;
  document.querySelectorAll(".nav").forEach((t) => {
    const on = t.dataset.view === v || (v === "game" && t.dataset.view === "home") || (v === "editor" && t.dataset.view === editorFrom);
    t.classList.toggle("on", on);
    if (on) t.setAttribute("aria-current", "page");
    else t.removeAttribute("aria-current");
  });
  for (const name of VIEWS) $(`view-${name}`).hidden = name !== v;
  scroller.scrollTop = 0;
  timeline.set([]);
  if (v === "home") renderHome();
  if (v === "game") renderGame();
  if (v === "totrim") renderToTrim();
  if (v === "notices") (renderNotices(), loadNotices());
  if (v === "montage") montageView.draw();
  if (v === "cleanup") loadCleanup();
  if (v === "trash") loadTrash();
  if (v === "settings") renderSettings();
  syncUrl(clip);
}

document.querySelectorAll(".nav").forEach((t) => t.addEventListener("click", () => showView(t.dataset.view)));
$("game-back").addEventListener("click", () => showView("home"));
$("sort").addEventListener("change", () => {
  order = $("sort").value;
  remember("sort", order);
  renderGame();
});
$("filters-btn").addEventListener("click", () => {
  const open = $("filters").hidden;
  $("filters").hidden = !open;
  $("filters-btn").setAttribute("aria-expanded", String(open));
});

function syncUrl(clip = new URLSearchParams(location.search).get("clip")) {
  const p = new URLSearchParams();
  if (view !== "home") p.set("view", view);
  if (view === "game") p.set("game", game);
  if (view === "editor" && clip) p.set("clip", clip);
  if (view === "home") {
    if (search.value) p.set("q", search.value);
    for (const [k, v] of Object.entries(filters)) if (Array.isArray(v) ? v.length : v) p.set(k, Array.isArray(v) ? v.join("|") : String(v));
  }
  const qs = p.toString();
  history.replaceState(null, "", `${location.pathname}${qs ? `?${qs}` : ""}${location.hash}`);
}

search.addEventListener("input", () => {
  if (view !== "home") return showView("home");
  syncUrl();
  renderHome();
});

const params = new URLSearchParams(location.search);
search.value = params.get("q") || "";
for (const k of Object.keys(EMPTY)) {
  const v = params.get(k);
  if (v === null) continue;
  filters[k] = Array.isArray(EMPTY[k]) ? v.split("|") : typeof EMPTY[k] === "boolean" ? v === "true" : v;
}
if (activeFilters()) $("filters").hidden = false;

await load();
loadTrashCount();
loadNotices();
const start = params.get("view");
if (start === "game" && params.get("game")) openGame(params.get("game"));
else if (start === "editor" && params.get("clip") && clips.find((c) => c.id === params.get("clip"))) openEditor(clips.find((c) => c.id === params.get("clip")));
else if (VIEWS.includes(start) && start !== "editor" && start !== "game") showView(start);
else showView("home");
const deep = /#clip=([\w-]+)/.exec(location.hash);
if (deep) {
  const c = clips.find((x) => x.id === deep[1]);
  if (c) openClip(c);
}
setInterval(() => !document.hidden && load(), 15000);
setInterval(() => !document.hidden && loadNotices(), 60000);
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) {
    load();
    loadNotices();
  }
});
