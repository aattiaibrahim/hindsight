// The NAS editor: audio filters, the To trim and replace rules, and (when ffmpeg
// is installed) real trims, mutes and saves on a generated test clip.
// The test clip's audio is a generated tone; nothing is ever played.

import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readdir, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import { audioEmpty, audioFilter, audioChain, compressPlan, Edits, levels, probe, pruneReplaced, REPLACED, trim, uniqueName } from "../src/edit.js";
import { makeClip } from "../src/library.js";
import { notAGame, sameGame, tidyProgramName } from "../src/logos.js";
import { MADE_BY_EDIT, Store } from "../src/store.js";

const FFMPEG = process.env.FFMPEG || "ffmpeg";
const FFPROBE = process.env.FFPROBE || "ffprobe";
const has = (cmd) => {
  try {
    execFileSync(cmd, ["-version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
};
const hasFfmpeg = has(FFMPEG) && has(FFPROBE);
const encoders = hasFfmpeg ? execFileSync(FFMPEG, ["-hide_banner", "-encoders"], { encoding: "utf8" }) : "";
const ENCODER = process.env.H264_ENCODER || (/\slibx264\s/.test(encoders) ? "libx264" : "libopenh264");

const clipAt = (rel, extra = {}) => ({ ...makeClip(rel, { size: 1000, mtime: new Date(2026, 10, 3) }), ...extra });

describe("audio edits", () => {
  test("filters match the desktop's", () => {
    const e = [{ gainDb: 0, mutes: [] }, { gainDb: 6, mutes: [[2, 3.5], [10, 11]] }];
    assert.equal(audioFilter(e, 0, 0), null);
    assert.equal(audioFilter(e, 1, 1), "volume=6.00dB,volume=0.00001:enable='between(t,1.000,2.500)+between(t,9.000,10.000)'");
    assert.equal(audioChain(e, 0), "[0:a:0]");
    assert.equal(audioChain(e, 1).startsWith("[0:a:1]volume=6.00dB,"), true);
    assert.equal(audioEmpty(e), false);
    assert.equal(audioEmpty([{ gainDb: 0.01, mutes: [[3, 3]] }]), true);
    assert.equal(audioFilter([{ gainDb: 99 }], 0), "volume=30.00dB", "clamped like the desktop");
  });

  test("waveform levels follow loudness", () => {
    const s = new Int16Array(4000);
    s.fill(16384, 2000);
    const w = levels(s, 2000);
    assert.equal(w.levels.length, 40);
    assert.equal(w.levels[0], 0);
    assert.ok(w.levels[39] > 220);
    assert.equal(w.step, 0.05);
  });

  test("compress plan fits the size and drops to 720p when the bitrate is low", () => {
    assert.deepEqual(compressPlan(10, 60, true, 1080), { videoKbps: 1142, scale720: true });
    assert.equal(compressPlan(500, 60, true, 1080).scale720, false);
  });
});

describe("To trim and replace rules", () => {
  let dir, store;
  before(async () => {
    dir = await mkdtemp(path.join(tmpdir(), "hs-store-"));
    store = new Store(dir);
    await store.init();
  });
  after(() => rm(dir, { recursive: true, force: true }));

  test("edit outputs and edit folders aren't To trim", () => {
    for (const n of ["a (trim).mkv", "a (trim) (2).mkv", "a (remux).mp4", "a (H.264).mp4", "a (repaired).mkv", "a (audio).mp4", "a (25 MB).mp4", "a (trimmed).mp4", "a (good part).mkv"]) {
      assert.ok(MADE_BY_EDIT.test(n), n);
      assert.equal(store.toTrim(clipAt(`Game/${n}`)), false, n);
    }
    assert.equal(MADE_BY_EDIT.test("Game 2026-11-03 10-00-00 - PC (2).mkv"), false);
    for (const f of ["Edits", "exports", "Montages"]) assert.equal(store.toTrim(clipAt(`${f}/x 2026-11-03 10-00-00.mkv`)), false, f);
    assert.equal(store.toTrim(clipAt("Game/Game 2026-11-03 10-00-00 - PC.mkv")), true);
  });

  test("clips the editor made or edited, and Keep whole, leave To trim", async () => {
    const a = clipAt("Game/Game 2026-11-03 10-00-00 - PC.mkv");
    const b = clipAt("Game/Game 2026-11-03 11-00-00.mkv");
    const c = clipAt("Game/Game 2026-11-03 12-00-00 - PC.mkv");
    await store.recordEdit(a.rel, b.rel);
    assert.equal(store.toTrim(a), false, "edited");
    assert.equal(store.toTrim(b), false, "made by the editor");
    assert.equal(store.toTrim(c), true);
    await store.keepWhole(c.rel);
    assert.equal(store.toTrim(c), false);
    await store.keepWhole(c.rel, false);
    assert.equal(store.toTrim(c), true);
    const again = new Store(dir);
    await again.init();
    assert.equal(again.toTrim(b), false, "kept across restarts");
  });

  test("only NAS-only clips may be replaced", async () => {
    const fromPc = clipAt("Game/Game 2026-11-03 10-00-00 - GAMING-PC.mkv");
    const nasOnly = clipAt("Game/Game 2026-11-03 10-00-00.mkv");
    assert.equal(fromPc.pc, "GAMING-PC");
    assert.equal(store.nasOnly(fromPc), false);
    assert.equal(store.nasOnly(nasOnly), true);
    const info = { duration: 10, audio: [{ name: "Desktop" }] };
    let change = true;
    const edits = new Edits({ root: dir, cacheDir: dir, store, canChange: () => change });
    // A PC's clip can be saved over too: the PC picks the edit up from the edit log.
    assert.equal(edits.plan(fromPc, info, { op: "save", start: 1, end: 5, replace: true }).replace, true);
    assert.equal(edits.plan(fromPc, info, { op: "save", start: 1, end: 5 }).suffix, " (trim)", "a new file is fine");
    assert.equal(edits.plan(nasOnly, info, { op: "save", start: 1, end: 5, replace: true }).replace, true);
    // A file the NAS editor made counts as NAS-only, even with a PC name in it.
    const made = clipAt("Game/Game 2026-11-03 10-00-00 - GAMING-PC (trim).mkv");
    await store.recordEdit(null, made.rel);
    assert.equal(edits.plan(made, info, { op: "save", start: 1, end: 5, replace: true }).replace, true);
    change = false;
    assert.throws(() => edits.plan(nasOnly, info, { op: "save", start: 1, end: 5, replace: true }), (e) => e.status === 403);
    assert.throws(() => edits.plan(nasOnly, info, { op: "save", start: 0, end: 10 }), /Nothing to save/);
    assert.throws(() => edits.plan(nasOnly, info, { op: "mp3", replace: true }), /MP3/);
    assert.equal(edits.plan(nasOnly, info, { op: "compress", targetMb: 25 }).suffix, " (25 MB)");
    assert.equal(edits.plan(nasOnly, info, { op: "mp3", track: 0 }).suffix, " (Desktop)");
  });

  test("unique names count up like the desktop", async () => {
    await mkdir(path.join(dir, "u"), { recursive: true });
    assert.equal(await uniqueName(path.join(dir, "u"), "a", " (trim)", "mkv"), "a (trim).mkv");
    execFileSync(process.execPath, ["-e", `require("fs").writeFileSync(${JSON.stringify(path.join(dir, "u", "a (trim).mkv"))}, "x")`]);
    assert.equal(await uniqueName(path.join(dir, "u"), "a", " (trim)", "mkv"), "a (trim) (2).mkv");
  });

  test("old replaced versions go after 30 days", async () => {
    await mkdir(path.join(dir, REPLACED, "2026-01-01", "Game"), { recursive: true });
    await mkdir(path.join(dir, REPLACED, "2026-11-01"), { recursive: true });
    await pruneReplaced(dir, new Date(2026, 10, 7));
    assert.deepEqual(await readdir(path.join(dir, REPLACED)), ["2026-11-01"]);
  });
});

describe("logos", () => {
  test("Steam names match only the same game", () => {
    assert.ok(sameGame("huntshowdown", "Hunt: Showdown 1896"));
    assert.ok(sameGame("overwatch2", "Overwatch®"));
    assert.ok(sameGame("tomclancysrainbowsixsiege", "Tom Clancy's Rainbow Six® Siege X"));
    assert.ok(!sameGame("minecraft", "Minecraft Dungeons"));
    assert.ok(!sameGame("render", "Render Voice VR"));
    assert.ok(sameGame("residentevil3remake", "Resident Evil 3"));
    assert.ok(sameGame("counterstrike2", "Counter-Strike 2"));
    assert.ok(!sameGame("edit", "Edith Finch"));
    assert.ok(!sameGame("funnynotcool", "Funny"));
    assert.ok(!sameGame("residentevilrequiem", "Resident Evil"));
    assert.ok(notAGame("desktop") && notAGame("edits") && !notAGame("valorant"));
    assert.equal(tidyProgramName("SpeciesUnknown-Win64-Shipping"), "Species Unknown");
    assert.equal(tidyProgramName("VALORANT"), null);
  });
});

describe("editing real files", { skip: !hasFfmpeg && "ffmpeg isn't installed" }, () => {
  let root, cache, src, clip, store;
  const frames = (f) => Number(execFileSync(FFPROBE, ["-v", "error", "-count_packets", "-select_streams", "v:0", "-show_entries", "stream=nb_read_packets", "-of", "csv=p=0", f], { encoding: "utf8" }).trim());
  /** Loudest sample (dB) of one track between two times. */
  const loudest = (f, track, from, to) => {
    const res = spawnSync(FFMPEG, ["-hide_banner", "-nostats", "-i", f, "-map", `0:a:${track}`, "-af", `atrim=${from}:${to},volumedetect`, "-f", "null", "-"], { encoding: "utf8" }).stderr;
    const m = /max_volume: (-?[\d.]+|-inf) dB/.exec(res);
    return m ? (m[1] === "-inf" ? -Infinity : Number(m[1])) : null;
  };
  const videoHash = (f) => execFileSync(FFMPEG, ["-v", "error", "-i", f, "-map", "0:v:0", "-c", "copy", "-f", "md5", "-"], { encoding: "utf8" }).trim();

  before(async () => {
    root = await mkdtemp(path.join(tmpdir(), "hs-edit-clips-"));
    cache = await mkdtemp(path.join(tmpdir(), "hs-edit-cache-"));
    await mkdir(path.join(root, "Game"));
    src = path.join(root, "Game", "Game 2026-11-03 10-00-00.mkv");
    // 6 s at 30 fps, a keyframe every second, two tone tracks (never played).
    execFileSync(FFMPEG, [
      "-hide_banner", "-v", "error", "-y",
      "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30",
      "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
      "-f", "lavfi", "-i", "sine=frequency=660:sample_rate=48000",
      "-map", "0:v", "-map", "1:a", "-map", "2:a", "-t", "6",
      "-pix_fmt", "yuv420p", "-c:v", ENCODER, ...(ENCODER === "libx264" ? ["-preset", "ultrafast"] : []), "-g", "30", "-bf", "0",
      "-c:a", "aac", src,
    ]);
    const s = await stat(src);
    clip = makeClip("Game/Game 2026-11-03 10-00-00.mkv", s);
    store = new Store(cache);
    await store.init();
  });
  after(async () => {
    await rm(root, { recursive: true, force: true });
    await rm(cache, { recursive: true, force: true });
  });

  test("a cut on a keyframe is a plain copy", async () => {
    const out = path.join(cache, "copy.mkv");
    const r = await trim(src, { start: 1, end: 4, out, format: "matroska", encoder: ENCODER, tmpDir: cache });
    assert.equal(r.method, "copy");
    assert.equal(frames(out), 90);
  });

  test("a cut between keyframes is exact", async () => {
    const out = path.join(cache, "exact.mkv");
    const r = await trim(src, { start: 1.5, end: 4.2, out, format: "matroska", encoder: ENCODER, tmpDir: cache });
    assert.ok(["smart", "encode"].includes(r.method), r.method);
    assert.equal(frames(out), 81, "frames 45 to 126");
    const info = await probe(out);
    assert.ok(Math.abs(info.duration - 2.7) < 0.1, String(info.duration));
    assert.equal(info.audio.length, 2, "both tracks kept");
  });

  test("a trim whose muted stretch starts at the cut finishes and is silent there", async () => {
    const out = path.join(cache, "mute-at-cut.mkv");
    await trim(src, { start: 1.5, end: 4.5, out, format: "matroska", encoder: ENCODER, tmpDir: cache, audio: [{ gainDb: 0, mutes: [] }, { gainDb: 0, mutes: [[0, 2.5]] }] });
    assert.equal(frames(out), 90);
    assert.ok(loudest(out, 1, 0.1, 0.9) < -80, "muted from the cut on");
    assert.ok(loudest(out, 1, 1.2, 2.8) > -20, "sound after the mute");
  });

  test("muting a stretch silences it and copies the video", async () => {
    let change = true;
    const edits = new Edits({ root, cacheDir: cache, store, canChange: () => change, encoder: ENCODER });
    await edits.init();
    const info = await probe(src);
    const job = edits.start(clip, info, { op: "save", audio: [{ gainDb: 0, mutes: [] }, { gainDb: 0, mutes: [[2, 4]] }] });
    await edits.chain;
    const done = edits.status(job.id);
    assert.equal(done.state, "done", done.error);
    assert.equal(done.method, "audio");
    assert.equal(done.name, "Game 2026-11-03 10-00-00 (trim).mkv");
    const out = path.join(root, "Game", done.name);
    assert.equal(videoHash(out), videoHash(src), "video copied untouched");
    assert.ok(loudest(out, 1, 2.2, 3.8) < -80, "muted");
    assert.ok(loudest(out, 1, 0.2, 1.8) > -20, "not muted before");
    assert.ok(loudest(out, 0, 2.2, 3.8) > -20, "other track untouched");
    assert.equal(store.toTrim(clip), false, "the original left To trim");
    assert.equal(store.madeHere(done.rel), true);
    change = false;
  });

  test("saving over a NAS-only clip sets the original aside", async () => {
    const edits = new Edits({ root, cacheDir: cache, store, canChange: () => true, encoder: ENCODER });
    const before = videoHash(src);
    const info = await probe(src);
    const job = edits.start(clip, info, { op: "save", start: 1, end: 4, replace: true });
    await edits.chain;
    const done = edits.status(job.id);
    assert.equal(done.state, "done", done.error);
    assert.equal(done.rel, clip.rel, "the edit took the original's name");
    assert.equal(frames(src), 90);
    const d = new Date();
    const dayDir = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
    const aside = path.join(root, REPLACED, dayDir, "Game", "Game 2026-11-03 10-00-00.mkv");
    assert.equal(videoHash(aside), before, "the original is kept in .hindsight-replaced");
  });

  test("exports: H.264, MP3 and compress", async () => {
    const edits = new Edits({ root, cacheDir: cache, store, canChange: () => true, encoder: ENCODER });
    const s = await stat(src);
    const c = makeClip(clip.rel, s);
    const info = await probe(src);
    const jobs = [
      edits.start(c, info, { op: "h264" }),
      edits.start(c, info, { op: "mp3" }),
      edits.start(c, info, { op: "mp3", track: 1 }),
      edits.start(c, info, { op: "compress", targetMb: 1 }),
    ];
    await edits.chain;
    const got = jobs.map((j) => edits.status(j.id));
    for (const g of got) assert.equal(g.state, "done", `${g.op}: ${g.error}`);
    assert.deepEqual(got.map((g) => g.name), ["Game 2026-11-03 10-00-00 (H.264).mp4", "Game 2026-11-03 10-00-00 (audio).mp3", "Game 2026-11-03 10-00-00 (Microphone).mp3", "Game 2026-11-03 10-00-00 (1 MB).mp4"]);
    const h = await probe(path.join(root, "Game", got[0].name));
    assert.equal(h.video.codec, "h264");
    assert.equal(h.audio.length, 1, "tracks mixed into one");
    const m = await probe(path.join(root, "Game", got[1].name));
    assert.equal(m.video, null);
    assert.equal(m.audio[0].codec, "mp3");
    assert.ok((await stat(path.join(root, "Game", got[3].name))).size < 1024 * 1024);
  });
});

