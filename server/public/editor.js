// The NAS editor, matching the desktop's Studio: a player, a timeline with start
// and end handles, one lane per audio track (volume, mute, muted stretches drawn
// with a drag), and a side panel with the clip's details, Save and the exports.
// The work runs on the NAS (see src/edit.js); progress shows from polling.

import { icon } from "./icons.js";
import { api, btn, clipTitle, codecLabel, dayLabel, duration, el, loadVideo, logo, postJson, preciseTime, size, sleep, timeOf } from "./ui.js";

const OP_LABEL = { save: "Save", h264: "H.264 MP4", mp3: "MP3", compress: "Compress", repair: "Repair" };

export function editor(root, { back, changed }) {
  let c = null;
  let info = null;
  let dur = 0;
  let start = 0;
  let end = 0;
  let tracks = []; // [{ name, gainDb, mutes: [[a, b]] }]
  let waves = []; // [{ step, levels: Uint8Array }]
  let jobs = [];
  let canChange = false;
  let replace = false;
  let session = 0;
  let polling = false;
  let stopAt = null;

  const video = el("video", { playsinline: true, preload: "auto" });
  const statusText = el("p");
  const statusBar = el("div");
  const status = el("div", { class: "status", hidden: true }, statusText, el("div", { class: "bar" }, statusBar));
  const setStatus = (text, progress = null, isError = false) => {
    status.hidden = !text;
    statusText.textContent = text || "";
    statusText.className = isError ? "error" : "";
    statusBar.parentElement.hidden = progress === null;
    if (progress !== null) statusBar.style.width = `${Math.round(progress * 100)}%`;
  };

  // ---- timeline ----
  const outL = el("div", { class: "tl-out" });
  const outR = el("div", { class: "tl-out" });
  const range = el("div", { class: "tl-range" });
  const hStart = el("div", { class: "tl-handle", role: "slider", tabindex: "0", "aria-label": "Start" }, el("i"));
  const hEnd = el("div", { class: "tl-handle", role: "slider", tabindex: "0", "aria-label": "End" }, el("i"));
  const playhead = el("div", { class: "tl-playhead" });
  const thumbs = el("div", { class: "tl-thumbs" });
  const bar = el("div", { class: "timeline" }, thumbs, outL, outR, range, playhead, hStart, hEnd);
  const lanes = el("div", { class: "lanes" });
  const timeLabel = el("span", { class: "ed-time" });
  const playBtn = el("button", { class: "btn icon-only", "aria-label": "Play" }, icon("play"));
  const side = el("div", { class: "ed-side" });

  const pct = (t) => `${(dur ? (t / dur) * 100 : 0).toFixed(3)}%`;
  const frame = () => 1 / (info?.video?.fps || 30);
  const timeAt = (clientX, box) => {
    const r = box.getBoundingClientRect();
    return Math.max(0, Math.min(dur, ((clientX - r.left) / r.width) * dur));
  };

  function drawRange() {
    outL.style.left = "0";
    outL.style.width = pct(start);
    outR.style.left = pct(end);
    outR.style.right = "0";
    range.style.left = pct(start);
    range.style.width = `calc(${pct(end)} - ${pct(start)})`;
    hStart.style.left = pct(start);
    hEnd.style.left = pct(end);
    for (const [h, v] of [[hStart, start], [hEnd, end]]) {
      h.setAttribute("aria-valuemin", "0");
      h.setAttribute("aria-valuemax", dur.toFixed(2));
      h.setAttribute("aria-valuenow", v.toFixed(2));
      h.setAttribute("aria-valuetext", preciseTime(v));
    }
    drawTime();
    drawSide();
  }

  function drawTime() {
    const t = video.currentTime || 0;
    playhead.style.left = pct(t);
    for (const p of lanes.querySelectorAll(".tl-playhead")) p.style.left = pct(t);
    timeLabel.textContent = `${preciseTime(t)} / ${preciseTime(dur)}`;
  }

  const setStart = (t) => {
    start = Math.max(0, Math.min(t, end - 0.1));
    drawRange();
  };
  const setEnd = (t) => {
    end = Math.min(dur, Math.max(t, start + 0.1));
    drawRange();
  };

  // Drag a handle (big hit area, works with touch); tap the bar to seek.
  function dragHandle(h, set) {
    h.addEventListener("pointerdown", (e) => {
      e.stopPropagation();
      e.preventDefault();
      h.setPointerCapture(e.pointerId);
      video.pause();
      const move = (ev) => {
        const t = timeAt(ev.clientX, bar);
        set(t);
        video.currentTime = h === hStart ? start : end;
      };
      move(e);
      h.onpointermove = move;
      h.onpointerup = h.onpointercancel = () => (h.onpointermove = null);
    });
    h.addEventListener("keydown", (e) => {
      const stepBy = e.shiftKey ? 1 : frame();
      const v = h === hStart ? start : end;
      if (e.key === "ArrowLeft" || e.key === "ArrowDown") set(v - stepBy);
      else if (e.key === "ArrowRight" || e.key === "ArrowUp") set(v + stepBy);
      else if (e.key === "Home") set(0);
      else if (e.key === "End") set(dur);
      else return;
      e.preventDefault();
      video.currentTime = h === hStart ? start : end;
    });
  }
  dragHandle(hStart, setStart);
  dragHandle(hEnd, setEnd);
  bar.addEventListener("pointerdown", (e) => {
    bar.setPointerCapture(e.pointerId);
    const seek = (ev) => (video.currentTime = timeAt(ev.clientX, bar));
    seek(e);
    bar.onpointermove = seek;
    bar.onpointerup = bar.onpointercancel = () => (bar.onpointermove = null);
  });

  let raf = 0;
  const tick = () => {
    drawTime();
    if (stopAt !== null && video.currentTime >= stopAt) {
      video.pause();
      stopAt = null;
    }
    if (!video.paused) raf = requestAnimationFrame(tick);
  };
  video.addEventListener("play", () => {
    playBtn.replaceChildren(icon("pause"));
    playBtn.setAttribute("aria-label", "Pause");
    cancelAnimationFrame(raf);
    raf = requestAnimationFrame(tick);
  });
  video.addEventListener("pause", () => {
    playBtn.replaceChildren(icon("play"));
    playBtn.setAttribute("aria-label", "Play");
    stopAt = null;
    drawTime();
  });
  video.addEventListener("seeked", drawTime);
  video.addEventListener("timeupdate", drawTime);
  playBtn.addEventListener("click", () => (video.paused ? video.play().catch(() => {}) : video.pause()));

  // ---- audio lanes ----

  function drawLanes() {
    lanes.replaceChildren(
      ...tracks.map((t, i) => {
        const canvas = el("canvas");
        const wave = el("div", { class: "wave", title: "Drag to mute a stretch" }, canvas);
        const spans = () => {
          for (const s of wave.querySelectorAll(".mute-span")) s.remove();
          t.mutes.forEach(([a, b], k) => {
            const s = el("div", { class: "mute-span", title: `Muted ${preciseTime(a)} to ${preciseTime(b)}: tap to unmute` });
            s.style.left = pct(a);
            s.style.width = `calc(${pct(b)} - ${pct(a)})`;
            s.addEventListener("pointerdown", (e) => e.stopPropagation());
            s.addEventListener("click", () => {
              t.mutes.splice(k, 1);
              drawLanes();
              drawSide();
            });
            wave.append(s);
          });
        };
        const ph = el("div", { class: "tl-playhead" });
        wave.append(ph);
        wave.addEventListener("pointerdown", (e) => {
          wave.setPointerCapture(e.pointerId);
          const from = timeAt(e.clientX, wave);
          const live = el("div", { class: "mute-span" });
          wave.append(live);
          const move = (ev) => {
            const to = timeAt(ev.clientX, wave);
            live.style.left = pct(Math.min(from, to));
            live.style.width = `calc(${pct(Math.max(from, to))} - ${pct(Math.min(from, to))})`;
            live.dataset.to = String(to);
          };
          move(e);
          wave.onpointermove = move;
          wave.onpointerup = wave.onpointercancel = (ev) => {
            wave.onpointermove = null;
            if (ev.type === "pointerup") move(ev);
            const to = Number(live.dataset.to);
            live.remove();
            const [a, b] = [Math.min(from, to), Math.max(from, to)];
            if (b - a >= 0.05) {
              t.mutes.push([a, b]);
              t.mutes = merge(t.mutes);
              drawLanes();
              drawSide();
            } else video.currentTime = a;
          };
        });
        const whole = t.mutes.length === 1 && t.mutes[0][0] <= 0.01 && t.mutes[0][1] >= dur - 0.01;
        const muteBtn = el("button", { class: `round ${whole ? "on" : ""}`, title: whole ? "Unmute this track" : "Mute this track", "aria-pressed": String(whole) }, icon(whole ? "volumeOff" : "volume", 16));
        muteBtn.addEventListener("click", () => {
          t.mutes = whole ? [] : [[0, dur]];
          drawLanes();
          drawSide();
        });
        const gainOut = el("span", { class: "mono", text: gainText(t.gainDb) });
        const gain = el("input", { type: "range", min: -30, max: 30, step: 1, value: t.gainDb, "aria-label": `${t.name} volume` });
        gain.addEventListener("input", () => {
          t.gainDb = Number(gain.value);
          gainOut.textContent = gainText(t.gainDb);
          drawSide();
        });
        gain.addEventListener("dblclick", () => {
          t.gainDb = 0;
          gain.value = "0";
          gainOut.textContent = gainText(0);
          drawSide();
        });
        requestAnimationFrame(() => {
          drawWave(canvas, waves[i]);
          spans();
          ph.style.left = pct(video.currentTime || 0);
        });
        return el(
          "div",
          { class: "lane" },
          el("div", { class: "lane-head" }, el("div", { class: "lane-name" }, muteBtn, el("span", { text: t.name })), el("label", { class: "lane-gain" }, gain, gainOut)),
          wave,
        );
      }),
    );
  }

  function drawWave(canvas, w) {
    const r = canvas.getBoundingClientRect();
    const dpr = devicePixelRatio || 1;
    canvas.width = Math.max(1, Math.round(r.width * dpr));
    canvas.height = Math.max(1, Math.round(r.height * dpr));
    const g = canvas.getContext("2d");
    g.clearRect(0, 0, canvas.width, canvas.height);
    if (!w || !w.levels.length || !dur) return;
    g.fillStyle = getComputedStyle(root).getPropertyValue("--text-2").trim() || "#b3b3b3";
    const cols = canvas.width;
    const mid = canvas.height / 2;
    for (let x = 0; x < cols; x += Math.max(1, Math.round(2 * dpr))) {
      const t0 = (x / cols) * dur;
      const t1 = ((x + 2 * dpr) / cols) * dur;
      let peak = 0;
      for (let i = Math.floor(t0 / w.step); i <= Math.min(w.levels.length - 1, Math.floor(t1 / w.step)); i++) peak = Math.max(peak, w.levels[i]);
      const h = Math.max(1, (peak / 255) * (canvas.height - 6));
      g.fillRect(x, mid - h / 2, Math.max(1, dpr), h);
    }
  }
  addEventListener("resize", () => c && root.isConnected && !root.closest("[hidden]") && drawLanes());

  // ---- side panel ----

  const trimmed = () => start > 0.02 || end < dur - 0.02;
  const audioChanged = () => tracks.some((t) => Math.abs(t.gainDb) >= 0.05 || t.mutes.length);
  const audioBody = () => tracks.map((t) => ({ gainDb: t.gainDb, mutes: t.mutes }));

  async function run(op, extra = {}) {
    try {
      const j = await postJson("api/edit", { id: c.id, op, start, end, audio: audioBody(), replace: op !== "mp3" && op !== "repair" && replace, ...extra });
      jobs = [j, ...jobs.filter((x) => x.id !== j.id)];
      drawSide();
      poll();
    } catch (e) {
      alert(e.message);
    }
  }

  async function poll() {
    if (polling) return;
    polling = true;
    const my = session;
    try {
      for (;;) {
        const j = await fetch("api/edit/jobs", { cache: "no-cache" }).then((r) => r.json()).catch(() => null);
        if (my !== session) return;
        if (j) {
          const before = jobs.filter((x) => x.state === "done").length;
          jobs = j.jobs.filter((x) => x.clipId === c.id);
          canChange = j.canChange;
          if (jobs.filter((x) => x.state === "done").length > before) changed?.();
          drawSide();
        }
        if (!jobs.some((x) => x.state === "queued" || x.state === "working")) return;
        await sleep(1000);
      }
    } finally {
      polling = false;
    }
  }

  function drawSide() {
    if (!c || !info) return side.replaceChildren();
    const len = end - start;
    const nothing = !trimmed() && !audioChanged();
    const mayReplace = canChange;
    const rep = el("input", { type: "checkbox", checked: replace && mayReplace ? true : null, disabled: mayReplace ? null : true });
    rep.addEventListener("change", () => ((replace = rep.checked), drawSide()));
    const repHint = !canChange
      ? "This server is read-only, so edits are kept to download."
      : c.nasOnly
        ? "The original moves to the hidden .hindsight-replaced folder for 30 days."
        : `The original moves to the hidden .hindsight-replaced folder for 30 days. ${c.pc} gets the edit on its next sync (its old copy goes to its Recycle Bin).`;
    const saveBtn = btn(replace && mayReplace ? "Save over the original" : "Save trim", { cls: "primary", icon: "save", disabled: nothing ? true : null, onclick: () => run("save") });
    const busy = jobs.some((x) => x.state === "queued" || x.state === "working");

    const mp3 = el("select", { "aria-label": "Which audio" }, el("option", { value: "" }, "All tracks"), ...tracks.map((t, i) => el("option", { value: String(i) }, t.name)));
    const mb = el("input", { type: "number", min: 1, max: 4000, value: "25", "aria-label": "Target size in MB", inputmode: "numeric" });
    const where = trimmed() ? ` (${duration(len)} selected)` : "";

    side.replaceChildren(
      el(
        "section",
        { class: "card-box" },
        el("h3", { text: "Details" }),
        el(
          "dl",
          { class: "kv" },
          ...[
            ["Game", c.game],
            ["From", c.pc || (c.source ? c.source : "This NAS")],
            ["Taken", `${dayLabel(c.day)}, ${timeOf(c)}`],
            ["Length", duration(dur)],
            ["Size", size(c.size)],
            ["Video", info.video ? `${info.video.height}p ${codecLabel(info.video.codec)}` : "None"],
            ["Audio", info.audio.length ? `${info.audio.length} track${info.audio.length === 1 ? "" : "s"}` : "None"],
            ["File", c.name],
          ].flatMap(([k, v]) => [el("dt", { text: k }), el("dd", { text: v, title: v })]),
        ),
      ),
      el(
        "section",
        { class: "card-box" },
        el("h3", { text: "Save" }),
        el("p", { class: "hint", text: nothing ? "Move the start or end, or change the sound, then save." : `Keeps ${preciseTime(start)} to ${preciseTime(end)} (${duration(len)}). Only the start is re-encoded; the rest is copied untouched.` }),
        el("label", { class: `opt ${mayReplace ? "" : "off"}` }, rep, "Replace the original"),
        el("p", { class: "hint", text: repHint }),
        saveBtn,
      ),
      el(
        "section",
        { class: "card-box" },
        el("h3", { text: `Export${where}` }),
        btn("H.264 MP4 that plays everywhere", { icon: "film", onclick: () => run("h264") }),
        el("div", { class: "export-row" }, mp3, btn("MP3", { icon: "music", onclick: () => run("mp3", { track: mp3.value === "" ? null : Number(mp3.value), replace: false }) })),
        el("div", { class: "export-row" }, mb, el("span", { class: "muted small", style: "align-self:center", text: "MB" }), btn("Compress", { icon: "shrink", onclick: () => run("compress", { targetMb: Number(mb.value) }) })),
        btn(c.damaged ? "Repair this damaged clip" : "Repair", { icon: "wrench", onclick: () => run("repair", { start: 0, end: 0 }) }),
        el("p", { class: "hint", text: "Made on the NAS with the CPU, one at a time. Outputs are saved next to the original." }),
      ),
      jobs.length
        ? el(
            "section",
            { class: "card-box" },
            el("h3", { text: busy ? "Working" : "Done" }),
            el(
              "div",
              { class: "jobs" },
              ...jobs.map((j) =>
                el(
                  "div",
                  { class: "job" },
                  el(
                    "div",
                    { class: "job-line" },
                    el("span", { text: j.name, title: j.name }),
                    j.state === "done" ? el("a", { class: "btn small", href: `api/edit/jobs/${j.id}/file`, download: j.name, title: "Download" }, icon("download", 16)) : null,
                  ),
                  j.state === "queued" || j.state === "working"
                    ? el("div", { class: "bar" }, el("div", { style: `width:${Math.round((j.progress || 0) * 100)}%` }))
                    : null,
                  el("span", { class: j.state === "failed" ? "error" : "muted", text: jobText(j) }),
                ),
              ),
            ),
          )
        : null,
    );
  }

  // ---- open / close ----

  async function open(clip) {
    const my = ++session;
    c = clip;
    info = null;
    jobs = [];
    replace = false;
    start = end = dur = 0;
    tracks = [];
    waves = [];
    video.pause();
    video.removeAttribute("src");
    video.load();
    thumbs.replaceChildren(el("img", { src: api(c.id, "thumb.jpg"), alt: "" }));
    root.replaceChildren(
      el(
        "div",
        { class: "toolbar" },
        el("button", { class: "btn icon-only", "aria-label": "Back", title: "Back", onclick: () => back() }, icon("arrowLeft")),
        logo(c.game, "game-logo big"),
        el("div", { class: "title-block" }, el("h1", { text: `Edit · ${c.game}` }), el("span", { class: "muted", text: clipTitle(c) })),
      ),
      el(
        "div",
        { class: "ed" },
        el(
          "div",
          { class: "ed-main" },
          el("div", { class: "ed-stage" }, video, status),
          el(
            "div",
            { class: "ed-controls" },
            playBtn,
            timeLabel,
            el("span", { class: "grow" }),
            btn("Start here", { cls: "small", icon: "bracketStart", title: "Set the start to the playhead", onclick: () => setStart(video.currentTime) }),
            btn("End here", { cls: "small", icon: "bracketEnd", title: "Set the end to the playhead", onclick: () => setEnd(video.currentTime) }),
            btn("Play selection", { cls: "small", icon: "play", onclick: () => ((video.currentTime = start), (stopAt = end), video.play().catch(() => {})) }),
            btn("Reset", { cls: "small", icon: "restore", onclick: () => ((start = 0), (end = dur), tracks.forEach((t) => ((t.gainDb = 0), (t.mutes = []))), drawRange(), drawLanes()) }),
          ),
          bar,
          lanes,
          el("p", { class: "hint", text: "Drag the handles to trim. Drag across a track to mute that stretch; tap a red stretch to unmute it. Double-tap a volume slider to reset it." }),
        ),
        side,
      ),
    );
    setStatus("Loading…");
    try {
      info = await (await fetch(api(c.id, "info"))).json();
      if (info.error) throw new Error(info.error);
    } catch (e) {
      if (my === session) setStatus(`Can't read this clip: ${e.message}`, null, true);
      return;
    }
    if (my !== session) return;
    dur = info.duration || c.duration || 0;
    end = dur;
    tracks = info.audio.map((_, i) => ({ name: info.audio.length === 1 ? "Audio" : i === 0 ? "Desktop" : i === 1 ? "Microphone" : `Track ${i + 1}`, gainDb: 0, mutes: [] }));
    drawRange();
    drawLanes();
    setStatus("");
    loadVideo(video, c, info, { onStatus: setStatus, alive: () => my === session }).then(() => drawTime());
    fetch(`api/waveform?id=${encodeURIComponent(c.id)}`)
      .then((r) => r.json())
      .then((j) => {
        if (my !== session || !j.tracks) return;
        waves = j.tracks.map((t) => ({ step: t.step, levels: Uint8Array.from(atob(t.levels), (ch) => ch.charCodeAt(0)) }));
        j.tracks.forEach((t, i) => tracks[i] && t.name && (tracks[i].name = t.name));
        drawLanes();
      })
      .catch(() => {});
    poll();
  }

  function close() {
    session++;
    video.pause();
    video.removeAttribute("src");
    video.load();
  }

  return { open, close };
}

function gainText(db) {
  return db === 0 ? "0 dB" : `${db > 0 ? "+" : ""}${db} dB`;
}

/** Overlapping muted stretches become one. */
function merge(spans) {
  const s = [...spans].sort((a, b) => a[0] - b[0]);
  const out = [];
  for (const [a, b] of s) {
    if (out.length && a <= out.at(-1)[1]) out.at(-1)[1] = Math.max(out.at(-1)[1], b);
    else out.push([a, b]);
  }
  return out;
}

function jobText(j) {
  const what = OP_LABEL[j.op] || j.op;
  if (j.state === "queued") return `${what}: waiting for the edit before it`;
  if (j.state === "working") return `${what}${j.step ? `, ${j.step}` : ""}: ${Math.round((j.progress || 0) * 100)}%`;
  if (j.state === "failed") return `${what} failed: ${j.error}`;
  const how = j.method === "copy" ? " (copied, no re-encode)" : j.method === "smart" ? " (only the start re-encoded)" : j.method === "audio" ? " (video copied)" : "";
  return j.rel ? `Saved${j.replace ? " over the original" : ""}${how}` : `Ready to download${how}`;
}
