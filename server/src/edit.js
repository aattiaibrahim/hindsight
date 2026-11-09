// The NAS editor: the same edits as the desktop's Studio, made by ffmpeg on the
// NAS with CPU encoders (libx264 by default, H264_ENCODER to change it).
//
// - Trim is exact: only the frames before the first keyframe in the range are
//   re-encoded, everything after it is copied untouched (src-tauri library.rs
//   `smart_trim`). A cut that starts on a keyframe is a plain copy. Files that
//   don't suit that are re-encoded in full.
// - Audio: per-track volume (dB) and muted stretches; the video is copied.
// - Exports: H.264 MP4, MP3 (all tracks mixed or one), compress to a size in MB
//   (two-pass with libx264), and repair (decode past errors, then re-encode).
// - Outputs go next to the original with the desktop's suffixes, written to a
//   hidden ".…rewind-partial" file first so the library never lists half a file.
// - Saving over the original works for every clip. The replaced file moves to
//   `.hindsight-replaced/<day>/`, like the desktop's NAS sync, and goes after
//   30 days. Each replace is added to `.hindsight-edits.jsonl` in the clips
//   folder, so the PC the clip came from swaps in the edit on its next sync
//   (its old copy goes to its Recycle Bin): an edit shows up everywhere.
// - Without ALLOW_CHANGES the outputs are kept in the cache, to download.

import { spawn } from "node:child_process";
import { trackName, tracksToMix } from "./tracks.js";
import { randomUUID } from "node:crypto";
import { appendFile, mkdir, readdir, readFile, rename, rm, stat, utimes, writeFile } from "node:fs/promises";
import path from "node:path";
import { freePath } from "./cleanup.js";

const FFMPEG = () => process.env.FFMPEG || "ffmpeg";
const FFPROBE = () => process.env.FFPROBE || "ffprobe";
export const REPLACED = ".hindsight-replaced";
/** Clips saved over here, for PCs to pick up: one JSON line each, { rel, old, at, size }. */
export const EDIT_LOG = ".hindsight-edits.jsonl";

/** Note a saved-over clip in the edit log that PCs read. */
export async function logEdit(root, entry) {
  await appendFile(path.join(root, EDIT_LOG), `${JSON.stringify(entry)}\n`);
}
const KEEP_REPLACED_DAYS = 30;

function fail(message, status = 400) {
  return Object.assign(new Error(message), { status });
}

/** Run a command; resolves with stdout, rejects with the tail of stderr. */
export function run(cmd, args, { onStdout } = {}) {
  return new Promise((resolve, reject) => {
    const p = spawn(cmd, args, { stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    let err = "";
    // A run that reports progress and then goes quiet for 2 minutes is stuck: stop it.
    let stalled = false;
    let watchdog = null;
    const pet = () => {
      if (!args.includes("-progress")) return;
      clearTimeout(watchdog);
      watchdog = setTimeout(() => {
        stalled = true;
        p.kill("SIGKILL");
      }, 120000);
    };
    pet();
    p.stdout.on("data", (d) => {
      out += d;
      pet();
      onStdout?.(String(d));
    });
    p.stderr.on("data", (d) => (err = (err + d).slice(-4000)));
    p.on("error", reject);
    p.on("close", (code) => {
      clearTimeout(watchdog);
      if (code === 0) resolve(out);
      else reject(new Error(stalled ? `${path.basename(cmd)} stopped responding` : `${path.basename(cmd)} exited ${code}: ${err.trim().split("\n").slice(-3).join(" ")}`));
    });
  });
}

const fx = (n) => Number(n).toFixed(3);

// ---- audio edits (src-tauri audio_edit.rs) ----

/** Nothing changed on any track. `tracks`: [{ gainDb, mutes: [[from, to]] }] */
export function audioEmpty(tracks = []) {
  return tracks.every((t) => !t || (Math.abs(t.gainDb || 0) < 0.05 && !(t.mutes ?? []).some(([a, b]) => b > a)));
}

/** ffmpeg filters for track `i`, or null if unchanged. `offset` shifts mute times (input cut at `offset`). */
export function audioFilter(tracks = [], i, offset = 0) {
  const t = tracks[i];
  if (!t) return null;
  const parts = [];
  if (Math.abs(t.gainDb || 0) >= 0.05) parts.push(`volume=${Math.max(-30, Math.min(30, t.gainDb)).toFixed(2)}dB`);
  const spans = (t.mutes ?? []).filter(([a, b]) => b > a).map(([a, b]) => `between(t,${fx(Math.max(0, a - offset))},${fx(Math.max(0, b - offset))})`);
  // -100 dB rather than exact zero: some ffmpeg builds' AAC encoder hangs on a
  // track that starts in pure digital silence. Nobody can hear the difference.
  if (spans.length) parts.push(`volume=0.00001:enable='${spans.join("+")}'`);
  return parts.length ? parts.join(",") : null;
}

/** "[0:a:1]volume=3.00dB," or "[0:a:1]" when unchanged. */
export function audioChain(tracks, i, offset = 0) {
  const f = audioFilter(tracks, i, offset);
  return f ? `[0:a:${i}]${f},` : `[0:a:${i}]`;
}

/** `idx` mixed into one track `[out]` (each through its edits). */
function mixGraph(idx, tracks, offset, out) {
  if (idx.length === 1) return `${audioChain(tracks, idx[0], offset)}anull[${out}]`;
  const parts = idx.map((i) => `${audioChain(tracks, i, offset)}anull[t${i}]`);
  parts.push(`${idx.map((i) => `[t${i}]`).join("")}amix=inputs=${idx.length}:duration=longest:normalize=0[${out}]`);
  return parts.join(";");
}

/** Map video plus one audio track: the tracks mixed, so phones and Discord hear them all
 *  (not a track that already mixes the others, like Medal's "All Audio": see tracks.js). */
function mixedAudio(info, tracks, offset) {
  const idx = tracksToMix(info.audio.map((a) => a.raw ?? ""));
  if (!idx.length) return [];
  return ["-filter_complex", mixGraph(idx, tracks, offset, "mix"), "-map", "[mix]"];
}

export function encoderArgs(enc) {
  if (enc === "libx264") return ["-preset", process.env.X264_PRESET || "veryfast", "-crf", "20"];
  return ["-b:v", "8M"];
}

// ---- names ----

/** "<stem><suffix>.<ext>" in `dir`, adding (2), (3)… if it's taken (desktop `unique`). */
export async function uniqueName(dir, stem, suffix, ext) {
  const taken = async (n) => stat(path.join(dir, n)).then(() => true, () => false);
  let name = `${stem}${suffix}.${ext}`;
  for (let n = 2; await taken(name); n++) name = `${stem}${suffix} (${n}).${ext}`;
  return name;
}

const stemOf = (name) => name.replace(/\.[^.]+$/, "");
const extOf = (name) => (/\.([^.]+)$/.exec(name)?.[1] || "").toLowerCase();
const videoExt = (name) => (["mkv", "mp4"].includes(extOf(name)) ? extOf(name) : "mp4");
const FORMAT = { mkv: "matroska", mp4: "mp4", mp3: "mp3", webm: "webm", mov: "mov" };

// ---- probing ----

export async function probe(file) {
  const j = JSON.parse(await run(FFPROBE(), ["-v", "error", "-show_streams", "-show_format", "-of", "json", file]));
  const v = j.streams.find((s) => s.codec_type === "video");
  const audio = j.streams.filter((s) => s.codec_type === "audio");
  const rate = (r) => {
    const [n, d] = String(r || "0/1").split("/").map(Number);
    return d ? n / d : 0;
  };
  return {
    duration: Number(j.format?.duration) || 0,
    size: Number(j.format?.size) || 0,
    video: v ? { codec: v.codec_name, width: v.width, height: v.height, fps: rate(v.avg_frame_rate) || rate(v.r_frame_rate) } : null,
    audio: audio.map((a, i) => ({ codec: a.codec_name, raw: trackName(a), name: trackName(a) || (audio.length === 1 ? "Audio" : i === 0 ? "Desktop" : i === 1 ? "Microphone" : `Track ${i + 1}`) })),
  };
}

/** The stream properties a decoder sets itself up from (desktop `stream_sig`). */
function streamSig(v) {
  return ["profile", "level", "pix_fmt", "color_range", "color_space", "color_primaries", "color_transfer", "width", "height"]
    .map((k) => {
      let val = v[k] === undefined || v[k] === null ? "" : String(v[k]);
      if (["unknown", "unspecified", "reserved"].includes(val)) val = "";
      if (k === "color_range" && !val) val = "tv";
      return `${k}=${val}`;
    })
    .join(" ");
}

/** What a smart cut needs to know about the video stream; throws when the file doesn't suit one. */
export async function cutInfo(file) {
  const j = JSON.parse(await run(FFPROBE(), ["-v", "error", "-print_format", "json", "-show_streams", "-i", file]));
  const v = j.streams.find((s) => s.codec_type === "video");
  if (!v) throw new Error("no video");
  const raw = { av1: "obu", hevc: "hevc", h264: "h264" }[v.codec_name];
  if (!raw) throw new Error(`codec ${v.codec_name}`);
  if (!["yuv420p", "nv12"].includes(v.pix_fmt)) throw new Error(`pixel format ${v.pix_fmt}`);
  const rate = (r) => {
    const [n, d] = String(r || "0/1").split("/").map(Number);
    return d ? n / d : 0;
  };
  const fps = rate(v.r_frame_rate);
  const avg = rate(v.avg_frame_rate);
  // Raw streams get their timing back from a fixed frame rate: constant frame rate only.
  if (fps <= 0 || Math.abs(avg - fps) > fps * 0.002) throw new Error(`variable frame rate (${fps.toFixed(3)} vs ${avg.toFixed(3)})`);
  return {
    codec: v.codec_name,
    raw,
    fps,
    first: Number(v.start_time) || 0,
    audio: j.streams.filter((s) => s.codec_type === "audio").length,
    level: Number.isFinite(v.level) ? v.level : -99,
    profile: v.profile || "",
    sig: streamSig(v),
  };
}

/** Frame numbers (from the first frame) of the keyframes between `from` and `to` seconds. */
export async function keyframes(file, ci, from, to) {
  const out = await run(FFPROBE(), ["-v", "error", "-select_streams", "v:0", "-read_intervals", `${fx(Math.max(0, from - 1))}%${fx(to + 1)}`, "-show_entries", "packet=pts_time,flags", "-of", "csv=p=0", "-i", file]);
  return out
    .split(/\r?\n/)
    .filter((l) => l.includes(",K"))
    .map((l) => Number(l.split(",")[0]))
    .filter(Number.isFinite)
    .map((t) => Math.round((t - ci.first) * ci.fps));
}

async function countFrames(file) {
  return Number((await run(FFPROBE(), ["-v", "error", "-count_packets", "-select_streams", "v:0", "-show_entries", "stream=nb_read_packets", "-of", "csv=p=0", "-i", file])).trim());
}

// ---- trim ----

/**
 * Trim `file` to [start, end] into `out` (format from `format`), applying audio edits.
 * -> { method: "copy" | "smart" | "encode" }
 */
export async function trim(file, { start, end, audio = [], encoder = "libx264", out, format, onProgress, tmpDir }) {
  if (!(end > start)) throw fail("The end point has to be after the start point.");
  let ci = null;
  try {
    ci = await cutInfo(file);
  } catch {
    // not suited to a smart cut: re-encode below
  }
  if (ci) {
    try {
      return { method: await smartTrim(file, ci, { start, end, audio, encoder, out, format, tmpDir }) };
    } catch (e) {
      if (e.status) throw e;
      console.error(`smart trim not used (${e.message}); re-encoding the whole range`);
      await rm(out, { force: true });
    }
  }
  const info = await probe(file);
  const gop = String(Math.max(1, Math.round(info.video?.fps || 30)));
  const n = info.audio.length;
  const args = ["-hide_banner", "-loglevel", "error", "-y", "-ss", fx(start), "-to", fx(end), "-i", file, "-map", "0:v:0"];
  if (audioEmpty(audio)) args.push("-map", "0:a?");
  else if (n) {
    // The input starts at the cut, so mute times shift by `start`.
    args.push("-filter_complex", Array.from({ length: n }, (_, i) => `${audioChain(audio, i, start)}anull[a${i}]`).join(";"));
    for (let i = 0; i < n; i++) args.push("-map", `[a${i}]`, `-map_metadata:s:a:${i}`, `0:s:a:${i}`);
  }
  args.push("-c:v", encoder, ...encoderArgs(encoder), "-g", gop, "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k");
  if (format === "mp4") args.push("-movflags", "+faststart");
  args.push("-progress", "pipe:1", "-nostats", "-f", format, out);
  await run(FFMPEG(), args, { onStdout: progressOf(end - start, onProgress) });
  return { method: "encode" };
}

async function smartTrim(file, ci, { start, end, audio, encoder, out, format, tmpDir }) {
  const fps = ci.fps;
  // Work in frame numbers so rounding can't add or drop a frame.
  const iStart = Math.max(0, Math.floor((start - ci.first) * fps + 1e-3));
  const iEnd = Math.ceil((end - ci.first) * fps - 1e-3);
  if (iEnd <= iStart) throw fail("The range is shorter than a frame.");
  const keys = (await keyframes(file, ci, start, end)).filter((k) => k >= iStart && k < iEnd);
  const iKey = keys.length ? Math.min(...keys) : iEnd; // no keyframe inside: the whole range is "head"
  const at = (i) => (ci.first + i / fps).toFixed(6);
  const before = (i) => (ci.first + (i - 0.5) / fps).toFixed(6);
  const pre = (i) => fx(Math.max(0, ci.first + i / fps - 2));
  // Only H.264 can have its start re-encoded on the CPU and joined to the copied rest.
  if (iKey > iStart && ci.codec !== "h264") throw new Error(`a ${ci.codec} start would need a ${ci.codec} encoder`);

  // On a keyframe with the sound unchanged: a plain copy.
  if (iKey === iStart && audioEmpty(audio)) {
    const args = ["-hide_banner", "-loglevel", "error", "-y", "-ss", (ci.first + iStart / fps + 0.001).toFixed(6), "-i", file, "-t", ((iEnd - iStart) / fps).toFixed(6), "-frames:v", String(iEnd - iStart), "-map", "0:v:0", "-map", "0:a?", "-c", "copy", "-avoid_negative_ts", "make_zero"];
    if (format === "mp4") args.push("-movflags", "+faststart");
    args.push("-f", format, out);
    await run(FFMPEG(), args);
    return "copy";
  }

  const dir = path.join(tmpDir, `trim-${randomUUID()}`);
  await mkdir(dir, { recursive: true });
  let audioJob = null;
  try {
    // Audio, all tracks, sample-exact, edits applied on the original's timeline.
    const audioPath = path.join(dir, "audio.mka");
    audioJob = ci.audio
      ? (() => {
          const graph = Array.from({ length: ci.audio }, (_, i) => `${audioChain(audio, i, 0)}atrim=start=${at(iStart)}:end=${at(iEnd)},asetpts=PTS-STARTPTS[a${i}]`);
          const a = ["-hide_banner", "-loglevel", "error", "-y", "-i", file, "-filter_complex", graph.join(";")];
          for (let i = 0; i < ci.audio; i++) a.push("-map", `[a${i}]`, `-map_metadata:s:a:${i}`, `0:s:a:${i}`);
          a.push("-c:a", "aac", "-b:a", "192k", "-f", "matroska", audioPath);
          return run(FFMPEG(), a);
        })()
      : null;
    audioJob?.catch(() => {});

    // 1. Frames before the first keyframe: re-encoded, near-lossless, matching profile and level.
    const head = path.join(dir, "head.raw");
    if (iKey > iStart) {
      const gop = String(Math.max(1, Math.round(fps)));
      const profile = /main/i.test(ci.profile) ? "main" : /baseline/i.test(ci.profile) ? "baseline" : "high";
      const a = ["-hide_banner", "-loglevel", "error", "-y", "-copyts", "-ss", pre(iStart), "-i", file, "-map", "0:v:0", "-an", "-vf", `trim=start=${before(iStart)}:end=${before(iKey)}`];
      a.push("-c:v", encoder);
      if (encoder === "libx264") {
        a.push("-preset", "medium", "-crf", "16", "-bf", "0", "-g", gop, "-pix_fmt", "yuv420p", "-profile:v", profile);
        if (ci.level > 0) a.push("-level", (ci.level / 10).toFixed(1));
      } else a.push("-b:v", "20M", "-g", gop, "-pix_fmt", "yuv420p");
      a.push("-f", ci.raw, head);
      await run(FFMPEG(), a);
      // Joining two different-looking streams makes a file some players refuse: re-encode instead.
      if (iKey < iEnd) {
        const j = JSON.parse(await run(FFPROBE(), ["-v", "error", "-f", ci.raw, "-print_format", "json", "-show_streams", "-i", head]));
        const got = j.streams[0] ? streamSig(j.streams[0]) : "";
        if (got !== ci.sig) throw new Error(`the re-encoded start wouldn't match the rest (${got} vs ${ci.sig})`);
      }
    }
    // 2. From the keyframe on: copied untouched.
    const tail = path.join(dir, "tail.raw");
    if (iKey < iEnd) {
      await run(FFMPEG(), ["-hide_banner", "-loglevel", "error", "-y", "-copyts", "-ss", pre(iKey), "-i", file, "-ss", before(iKey), "-to", before(iEnd), "-map", "0:v:0", "-an", "-c", "copy", "-f", ci.raw, tail]);
    }
    // 3. One video stream (raw bitstreams join end to end), then mux with the audio.
    const video = path.join(dir, "video.raw");
    const parts = await Promise.all([head, tail].map((p) => readFile(p).catch(() => Buffer.alloc(0))));
    await writeFile(video, Buffer.concat(parts));
    if (audioJob) await audioJob;
    const a = ["-hide_banner", "-loglevel", "error", "-y", "-r", String(fps), "-f", ci.raw, "-i", video];
    if (ci.audio) a.push("-i", audioPath);
    a.push("-map", "0:v");
    if (ci.audio) a.push("-map", "1:a");
    a.push("-c", "copy");
    if (format === "mp4") a.push("-movflags", "+faststart");
    a.push("-f", format, out);
    await run(FFMPEG(), a);

    // 4. Checks: the right number of frames, and clean decoding across the join.
    const frames = await countFrames(out);
    if (frames !== iEnd - iStart) throw new Error(`${frames} frames instead of ${iEnd - iStart}`);
    const join = (iKey - iStart) / fps;
    await run(FFMPEG(), ["-hide_banner", "-v", "error", "-xerror", "-ss", fx(Math.max(0, join - 1)), "-t", "2.5", "-i", out, "-f", "null", "-"]).catch((e) => {
      throw new Error(`decode check failed: ${e.message}`);
    });
    return "smart";
  } finally {
    // Windows can't remove a file ffmpeg still has open.
    await audioJob?.catch(() => {});
    await rm(dir, { recursive: true, force: true });
  }
}

/** Audio edits only, over the whole clip: the video is copied. */
export async function audioOnly(file, { audio, out, format }) {
  const info = await probe(file);
  const n = info.audio.length;
  if (!n) throw fail("This clip has no audio.");
  const args = ["-hide_banner", "-loglevel", "error", "-y", "-i", file, "-filter_complex", Array.from({ length: n }, (_, i) => `${audioChain(audio, i, 0)}anull[a${i}]`).join(";"), "-map", "0:v:0"];
  for (let i = 0; i < n; i++) args.push("-map", `[a${i}]`, `-map_metadata:s:a:${i}`, `0:s:a:${i}`);
  args.push("-c:v", "copy", "-c:a", "aac", "-b:a", "192k");
  if (format === "mp4") args.push("-movflags", "+faststart");
  args.push("-f", format, out);
  await run(FFMPEG(), args);
}

function progressOf(total, onProgress) {
  return (chunk) => {
    const m = /out_time_us=(\d+)/.exec(chunk);
    if (m && total > 0) onProgress?.(Math.min(0.99, Number(m[1]) / 1e6 / total));
  };
}

const rangeArgs = (start, end) => [...(start > 0 ? ["-ss", fx(start)] : []), ...(end > start ? ["-to", fx(end)] : [])];

/** An H.264 + AAC MP4 that plays everywhere (tracks mixed into one). */
export async function exportH264(file, { start = 0, end = 0, audio = [], encoder = "libx264", out, onProgress }) {
  const info = await probe(file);
  if (!info.video) throw fail("This file has no picture.");
  const len = (end > start ? end : info.duration) - start;
  const args = ["-hide_banner", "-loglevel", "error", "-y", ...rangeArgs(start, end), "-i", file, "-map", "0:v:0", ...mixedAudio(info, audio, start)];
  args.push("-c:v", encoder, ...encoderArgs(encoder), "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart", "-progress", "pipe:1", "-nostats", "-f", "mp4", out);
  await run(FFMPEG(), args, { onStdout: progressOf(len, onProgress) });
}

/** The sound as an MP3: every track mixed, or one track (`track`). */
export async function exportMp3(file, { start = 0, end = 0, track = null, audio = [], out, onProgress }) {
  const info = await probe(file);
  const n = info.audio.length;
  if (!n) throw fail("This clip has no audio.");
  if (track !== null && !(track >= 0 && track < n)) throw fail("That audio track isn't in this clip.");
  const graph = track !== null ? `${audioChain(audio, track, start)}anull[out]` : mixGraph(tracksToMix(info.audio.map((a) => a.raw ?? "")), audio, start, "out");
  const len = (end > start ? end : info.duration) - start;
  await run(FFMPEG(), ["-hide_banner", "-loglevel", "error", "-y", ...rangeArgs(start, end), "-i", file, "-filter_complex", graph, "-map", "[out]", "-vn", "-c:a", "libmp3lame", "-b:a", "256k", "-progress", "pipe:1", "-nostats", "-f", "mp3", out], { onStdout: progressOf(len, onProgress) });
}

/** The video bitrate that fits `targetMb` (desktop `compress_into`). */
export function compressPlan(targetMb, duration, hasAudio, height) {
  const audioKbps = hasAudio ? 128 : 0;
  const totalKbps = (targetMb * 8 * 1024 * 0.93) / duration;
  const videoKbps = Math.round(Math.max(300, totalKbps - audioKbps));
  return { videoKbps, scale720: videoKbps < 2500 && height > 720 };
}

/** Fit under `targetMb`: two passes with libx264, one pass with other encoders. */
export async function compress(file, { start = 0, end = 0, targetMb, audio = [], encoder = "libx264", out, onProgress, tmpDir }) {
  const info = await probe(file);
  if (!info.video) throw fail("This file has no picture.");
  const len = (end > start ? end : info.duration) - start;
  if (!(len > 0)) throw fail("Couldn't read the video length.");
  if (!(targetMb >= 1 && targetMb <= 100000)) throw fail("Pick a size in MB.");
  const { videoKbps, scale720 } = compressPlan(targetMb, len, info.audio.length > 0, info.video.height);
  const b = `${videoKbps}k`;
  const vf = scale720 ? ["-vf", "scale=-2:720"] : [];
  const input = ["-hide_banner", "-loglevel", "error", "-y", ...rangeArgs(start, end), "-i", file];
  const tail = ["-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "128k", "-movflags", "+faststart", "-progress", "pipe:1", "-nostats", "-f", "mp4", out];
  if (encoder === "libx264") {
    const log = path.join(tmpDir, `pass-${randomUUID()}`);
    try {
      await run(FFMPEG(), [...input, "-map", "0:v:0", ...vf, "-c:v", "libx264", "-preset", "medium", "-b:v", b, "-pass", "1", "-passlogfile", log, "-an", "-progress", "pipe:1", "-nostats", "-f", "null", "-"], { onStdout: progressOf(len * 2, (p) => onProgress?.(p, "pass 1 of 2")) });
      await run(FFMPEG(), [...input, "-map", "0:v:0", ...mixedAudio(info, audio, start), ...vf, "-c:v", "libx264", "-preset", "medium", "-b:v", b, "-pass", "2", "-passlogfile", log, ...tail], {
        onStdout: (c) => progressOf(len * 2, (p) => onProgress?.(Math.min(0.99, 0.5 + p), "pass 2 of 2"))(c),
      });
    } finally {
      for (const f of await readdir(tmpDir).catch(() => [])) if (f.startsWith(path.basename(log))) await rm(path.join(tmpDir, f), { force: true });
    }
  } else {
    await run(FFMPEG(), [...input, "-map", "0:v:0", ...mixedAudio(info, audio, start), ...vf, "-c:v", encoder, "-b:v", b, "-maxrate", b, "-bufsize", `${videoKbps * 2}k`, ...tail], { onStdout: progressOf(len, onProgress) });
  }
}

/** Decode past the damage, then re-encode (src-tauri health.rs). */
export async function repair(file, { encoder = "libx264", out, format, onProgress }) {
  let info;
  try {
    info = await probe(file);
  } catch {
    throw fail("This file can't be read at all, so there's nothing to repair.", 422);
  }
  if (!info.video) throw fail("This file has no picture to repair.", 422);
  const gop = String(Math.max(1, Math.round(info.video.fps || 30)));
  const args = ["-hide_banner", "-loglevel", "error", "-y", "-err_detect", "ignore_err", "-ec", "guess_mvs+deblock", "-fflags", "+genpts", "-i", file, "-map", "0:v:0", "-map", "0:a?", "-c:v", encoder, ...encoderArgs(encoder), "-g", gop, "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k"];
  if (format === "mp4") args.push("-movflags", "+faststart");
  args.push("-progress", "pipe:1", "-nostats", "-f", format, out);
  // ffmpeg reports every broken frame as an error but still writes the file: judge by the result.
  await run(FFMPEG(), args, { onStdout: progressOf(info.duration, onProgress) }).catch(() => {});
  const got = await probe(out).catch(() => null);
  if (!got?.video || got.duration < 0.5 || got.duration < info.duration * 0.8) {
    await rm(out, { force: true });
    throw fail("The repair didn't produce a usable video. Trimming to the good part is the way to go for this one.", 422);
  }
}

// ---- waveforms ----

const RATE = 2000;

/** Peak per 1/20 s, mapped from -54..0 dBFS to 0..255 (desktop `levels`). */
export function levels(samples, rate = RATE) {
  const per = Math.max(1, Math.floor(rate / 20));
  const out = new Uint8Array(Math.ceil(samples.length / per));
  for (let b = 0; b < out.length; b++) {
    let peak = 0;
    for (let i = b * per; i < Math.min(samples.length, (b + 1) * per); i++) peak = Math.max(peak, Math.abs(samples[i]));
    if (peak <= 0) continue;
    const db = 20 * Math.log10(peak / 32768);
    out[b] = Math.round(Math.max(0, Math.min(1, (db + 54) / 54)) * 255);
  }
  return { step: per / rate, levels: out };
}

function decodeTrack(file, track) {
  return new Promise((resolve, reject) => {
    const p = spawn(FFMPEG(), ["-hide_banner", "-loglevel", "error", "-i", file, "-map", `0:a:${track}`, "-vn", "-ac", "1", "-ar", String(RATE), "-f", "s16le", "-"], { stdio: ["ignore", "pipe", "ignore"] });
    const chunks = [];
    p.stdout.on("data", (d) => chunks.push(d));
    p.on("error", reject);
    p.on("close", () => {
      const raw = Buffer.concat(chunks);
      resolve(new Int16Array(raw.buffer, raw.byteOffset, Math.floor(raw.length / 2)));
    });
  });
}

/** Every track's waveform, cached by `key` (the clip's version). -> [{ step, levels: base64, name }] */
export async function waveforms(file, key, cacheDir) {
  const dir = path.join(cacheDir, "waves");
  const cached = path.join(dir, `${key}.json`);
  try {
    return JSON.parse(await readFile(cached, "utf8"));
  } catch {
    // not made yet
  }
  const info = await probe(file);
  const out = [];
  for (let i = 0; i < info.audio.length; i++) {
    const w = levels(await decodeTrack(file, i));
    out.push({ name: info.audio[i].name, step: w.step, levels: Buffer.from(w.levels).toString("base64") });
  }
  await mkdir(dir, { recursive: true });
  await writeFile(cached, JSON.stringify(out));
  return out;
}

// ---- jobs ----

const day = (d = new Date()) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

/** Move a clip into today's `.hindsight-replaced` folder (never overwriting). -> its new path */
export async function setAside(root, rel, now = new Date()) {
  const from = path.join(root, ...rel.split("/"));
  const dir = path.join(root, REPLACED, day(now), ...path.posix.dirname(rel).split("/").filter((p) => p !== "."));
  await mkdir(dir, { recursive: true });
  const to = await freePath(dir, path.posix.basename(rel));
  await rename(from, to);
  return to;
}

/** Remove replaced versions older than 30 days (by their day folder). */
export async function pruneReplaced(root, now = new Date()) {
  for (const d of await readdir(path.join(root, REPLACED)).catch(() => [])) {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(d)) continue;
    if ((now - new Date(`${d}T00:00:00`)) / 86400000 > KEEP_REPLACED_DAYS) await rm(path.join(root, REPLACED, d), { recursive: true, force: true });
  }
}

export const OPS = ["save", "h264", "mp3", "compress", "repair"];

export class Edits {
  /** `canChange`: () => bool (ALLOW_CHANGES and a writable clips folder). */
  constructor({ root, cacheDir, store, canChange = () => false, encoder = "libx264", onSaved = () => {} }) {
    this.root = root;
    this.cacheDir = cacheDir;
    this.outDir = path.join(cacheDir, "edits");
    this.tmpDir = path.join(cacheDir, "tmp");
    this.store = store;
    this.canChange = canChange;
    this.encoder = encoder;
    this.onSaved = onSaved;
    this.jobs = new Map();
    this.chain = Promise.resolve(); // one edit at a time: CPU encodes are heavy
  }

  async init() {
    await mkdir(this.tmpDir, { recursive: true });
    await rm(this.outDir, { recursive: true, force: true }); // downloads from a previous run
    if (this.canChange()) await pruneReplaced(this.root).catch(() => {});
  }

  /**
   * Plan an edit: what it makes, its name and where it goes. Throws for edits
   * that aren't allowed. `body`: { op, start, end, audio, track, targetMb, replace }
   */
  plan(clip, info, body) {
    const op = body.op;
    if (!OPS.includes(op)) throw fail("Unknown edit.");
    const duration = info.duration || 0;
    const start = Math.max(0, Number(body.start) || 0);
    let end = Number(body.end) || duration;
    if (duration) end = Math.min(end, duration);
    const audio = Array.isArray(body.audio) ? body.audio.map((t) => ({ gainDb: Number(t?.gainDb) || 0, mutes: (t?.mutes ?? []).map(([a, b]) => [Number(a), Number(b)]).filter(([a, b]) => b > a) })) : [];
    const trimmed = start > 0.02 || (duration && end < duration - 0.02);
    const replace = !!body.replace;
    const stem = stemOf(clip.name);
    let suffix;
    let ext;
    if (op === "save") {
      if (!trimmed && audioEmpty(audio)) throw fail("Nothing to save: move the start or end, or change the sound.");
      suffix = " (trim)";
      ext = videoExt(clip.name);
    } else if (op === "h264") {
      suffix = " (H.264)";
      ext = "mp4";
    } else if (op === "mp3") {
      const track = body.track === null || body.track === undefined || body.track === "" ? null : Number(body.track);
      const label = track === null ? "" : String(info.audio[track]?.name || "").replace(/[^\p{L}\p{N} _&-]/gu, "").trim();
      suffix = label ? ` (${label})` : " (audio)";
      ext = "mp3";
      body.track = track;
    } else if (op === "compress") {
      const mb = Math.round(Number(body.targetMb));
      if (!(mb >= 1)) throw fail("Pick a size in MB.");
      suffix = ` (${mb} MB)`;
      ext = "mp4";
      body.targetMb = mb;
    } else {
      suffix = " (repaired)";
      ext = videoExt(clip.name);
    }
    if (replace) {
      if (op === "mp3") throw fail("An MP3 can't replace the clip.");
      if (!this.canChange()) throw fail("Changes are off on this server (set ALLOW_CHANGES=1 and mount the clips folder read-write).", 403);
    }
    return { op, start, end, audio, trimmed, replace, stem, suffix, ext, track: body.track ?? null, targetMb: body.targetMb };
  }

  /** Start an edit; returns the job's status right away. */
  start(clip, info, body) {
    const p = this.plan(clip, info, body);
    const id = randomUUID();
    const job = { id, clipId: clip.id, clip: clip.name, game: clip.game, op: p.op, replace: p.replace, state: "queued", progress: 0, error: null, name: `${p.stem}${p.suffix}.${p.ext}`, rel: null, file: null, method: null, at: Date.now() };
    this.jobs.set(id, job);
    for (const old of [...this.jobs.values()].filter((j) => j.state === "done" || j.state === "failed").slice(0, -20)) this.jobs.delete(old.id);
    this.chain = this.chain.then(() => this.runJob(job, clip, p)).catch(() => {});
    return this.status(id);
  }

  async runJob(job, clip, p) {
    job.state = "working";
    const src = path.join(this.root, ...clip.rel.split("/"));
    const dir = path.dirname(src);
    const change = this.canChange();
    const outDir = change ? dir : path.join(this.outDir, job.id);
    await mkdir(outDir, { recursive: true });
    const tmp = path.join(outDir, `.${job.name}.rewind-partial`);
    const format = FORMAT[p.ext];
    // `step`: which pass, for edits that take two (compressing with libx264).
    const onProgress = (x, step = null) => {
      job.progress = x;
      job.step = step;
    };
    const common = { encoder: this.encoder, out: tmp, format, onProgress, tmpDir: this.tmpDir, audio: p.audio };
    try {
      if (p.op === "save") {
        if (p.trimmed) job.method = (await trim(src, { ...common, start: p.start, end: p.end })).method;
        else {
          await audioOnly(src, common);
          job.method = "audio";
        }
      } else if (p.op === "h264") await exportH264(src, { ...common, start: p.start, end: p.trimmed ? p.end : 0 });
      else if (p.op === "mp3") await exportMp3(src, { ...common, start: p.start, end: p.trimmed ? p.end : 0, track: p.track });
      else if (p.op === "compress") await compress(src, { ...common, start: p.start, end: p.trimmed ? p.end : 0, targetMb: p.targetMb });
      else await repair(src, common);

      // The output has to read back with the expected length.
      const got = await probe(tmp);
      const want = p.op === "repair" ? 0 : (p.trimmed ? p.end - p.start : 0) || got.duration;
      if (!(got.duration > 0) || (want && Math.abs(got.duration - want) > Math.max(1, want * 0.05))) throw new Error(`the result is ${got.duration.toFixed(2)} s long instead of ${want.toFixed(2)} s`);
      if (p.ext !== "mp3" && !got.video) throw new Error("the result has no picture");

      let final;
      if (p.replace) {
        // The original goes to .hindsight-replaced/<day>/, the edit takes its name.
        const mtime = (await stat(src)).mtime;
        const target = path.join(dir, `${stemOf(clip.name)}.${p.ext}`);
        await setAside(this.root, clip.rel);
        if (target !== src && (await stat(target).then(() => true, () => false))) await setAside(this.root, path.relative(this.root, target).split(path.sep).join("/"));
        await rename(tmp, target);
        await utimes(target, mtime, mtime).catch(() => {});
        final = target;
        const rel = path.relative(this.root, target).split(path.sep).join("/");
        await logEdit(this.root, { rel, old: clip.rel, at: Date.now(), size: (await stat(target)).size }).catch((e) => console.error("edit log:", e.message));
      } else {
        final = path.join(outDir, await uniqueName(outDir, p.stem, p.suffix, p.ext));
        await rename(tmp, final);
      }
      job.name = path.basename(final);
      job.file = final;
      if (change) {
        job.rel = path.relative(this.root, final).split(path.sep).join("/");
        await this.store.recordEdit(clip.rel, p.ext === "mp3" ? null : job.rel);
      } else await this.store.recordEdit(clip.rel, null);
      job.state = "done";
      job.progress = 1;
      await this.onSaved(job);
    } catch (e) {
      await rm(tmp, { force: true });
      job.state = "failed";
      job.error = String(e.message || e).slice(-400);
    }
  }

  status(id) {
    const j = this.jobs.get(id);
    if (!j) throw fail("unknown edit", 404);
    const { file, ...rest } = j;
    return { ...rest, download: j.state === "done" };
  }

  list() {
    return [...this.jobs.keys()].map((id) => this.status(id)).reverse();
  }

  file(id) {
    const j = this.jobs.get(id);
    if (!j || j.state !== "done" || !j.file) throw fail("not ready", 404);
    return j.file;
  }
}
