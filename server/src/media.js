// ffmpeg work: probe, thumbnails and the phone-friendly H.264 MP4.
// Everything is written to the cache folder, never next to the clips.

import { spawn } from "node:child_process";
import { trackName, tracksToMix } from "./tracks.js";
import { mkdir, readdir, readFile, rename, rm, stat, utimes, writeFile } from "node:fs/promises";
import path from "node:path";

const FFMPEG = process.env.FFMPEG || "ffmpeg";
const FFPROBE = process.env.FFPROBE || "ffprobe";
const H264 = process.env.H264_ENCODER || "libx264";

/** Run a command; resolves with stdout, rejects with the tail of stderr. */
function run(cmd, args, { onStdout } = {}) {
  return new Promise((resolve, reject) => {
    const p = spawn(cmd, args, { stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    let err = "";
    p.stdout.on("data", (d) => {
      out += d;
      onStdout?.(String(d));
    });
    p.stderr.on("data", (d) => (err = (err + d).slice(-4000)));
    p.on("error", reject);
    p.on("close", (code) => (code === 0 ? resolve(out) : reject(new Error(`${cmd} exited ${code}: ${err.trim()}`))));
  });
}

/** A tiny concurrency limiter, so a page of thumbnails doesn't start 50 ffmpegs. */
function limiter(max) {
  let active = 0;
  const queue = [];
  const next = () => {
    if (active >= max || !queue.length) return;
    active++;
    const { fn, resolve, reject } = queue.shift();
    fn().then(resolve, reject).finally(() => {
      active--;
      next();
    });
  };
  return (fn) => new Promise((resolve, reject) => (queue.push({ fn, resolve, reject }), next()));
}

async function exists(p) {
  try {
    await stat(p);
    return true;
  } catch {
    return false;
  }
}

export class Media {
  constructor({ cacheDir, thumbJobs = 2, transcodeJobs = 1, maxCacheBytes = 0 }) {
    this.cacheDir = cacheDir;
    this.thumbDir = path.join(cacheDir, "thumbs");
    this.webDir = path.join(cacheDir, "web");
    this.maxCacheBytes = maxCacheBytes;
    this.thumbQueue = limiter(thumbJobs);
    this.transcodeQueue = limiter(transcodeJobs);
    this.probes = new Map(); // key -> Promise<info>
    // Per-clip facts for the list (duration, codec, damaged), kept in the cache across restarts.
    this.metaFile = path.join(cacheDir, "meta.json");
    this.metaByKey = {};
    this.metaVersion = 0;
    this.metaQueue = limiter(2);
    this.metaPending = new Set();
    this.thumbs = new Map(); // key -> Promise<path>
    this.jobs = new Map(); // key -> { state, progress, error, promise }
  }

  async init() {
    await mkdir(this.thumbDir, { recursive: true });
    await mkdir(this.webDir, { recursive: true });
    // Leftovers from a run that stopped mid-transcode.
    for (const dir of [this.thumbDir, this.webDir]) {
      for (const f of await readdir(dir)) if (f.includes(".tmp")) await rm(path.join(dir, f), { force: true });
    }
    await this.loadMeta();
  }

  async loadMeta() {
    try {
      this.metaByKey = JSON.parse(await readFile(this.metaFile, "utf8"));
    } catch {
      this.metaByKey = {};
    }
  }

  saveMetaSoon() {
    clearTimeout(this.metaTimer);
    this.metaTimer = setTimeout(async () => {
      const tmp = `${this.metaFile}.tmp`;
      try {
        await writeFile(tmp, JSON.stringify(this.metaByKey));
        await rename(tmp, this.metaFile);
      } catch (e) {
        console.error("saving clip info failed:", e.message);
      }
    }, 2000);
  }

  /** What's known about a clip so far: { duration, codec, height, damaged } or null. */
  meta(clip) {
    return this.metaByKey[this.key(clip)] ?? null;
  }

  /**
   * Read every clip once in the background (two at a time), so the list can show
   * durations and flag clips that can't be read. Results stay in the cache.
   */
  scanMeta(clips, fileOf) {
    const keep = new Set(clips.map((c) => this.key(c)));
    let dropped = false;
    for (const k of Object.keys(this.metaByKey)) {
      if (!keep.has(k)) {
        delete this.metaByKey[k];
        dropped = true;
      }
    }
    if (dropped) this.saveMetaSoon();
    for (const clip of clips) {
      const k = this.key(clip);
      if (this.metaByKey[k] || this.metaPending.has(k)) continue;
      this.metaPending.add(k);
      this.metaQueue(async () => {
        let m;
        try {
          const info = await this.probe(clip, fileOf(clip));
          m = info.video
            ? { duration: info.duration || null, codec: info.video.codec, height: info.video.height, damaged: false }
            : { duration: info.duration || null, codec: null, height: null, damaged: true, why: "no video in the file" };
        } catch (e) {
          m = { duration: null, codec: null, height: null, damaged: true, why: String(e.message || e).split("\n").pop().slice(-200) };
        }
        this.metaByKey[k] = m;
        this.metaVersion++;
        this.saveMetaSoon();
      }).finally(() => this.metaPending.delete(k));
    }
  }

  /** Bytes used by thumbnails and phone versions. */
  async cacheBytes() {
    let total = 0;
    for (const dir of [this.thumbDir, this.webDir]) {
      for (const f of await readdir(dir).catch(() => [])) {
        const s = await stat(path.join(dir, f)).catch(() => null);
        if (s?.isFile()) total += s.size;
      }
    }
    return total;
  }

  /** Cache key: changes when the clip is replaced. */
  key(clip) {
    return `${clip.id}-${clip.size.toString(36)}-${Math.floor(clip.mtime / 1000).toString(36)}`;
  }

  probe(clip, file) {
    const k = this.key(clip);
    if (!this.probes.has(k)) {
      const p = run(FFPROBE, ["-v", "error", "-show_streams", "-show_format", "-of", "json", file]).then((out) => {
        const j = JSON.parse(out);
        const v = j.streams.find((s) => s.codec_type === "video");
        const audio = j.streams.filter((s) => s.codec_type === "audio");
        return {
          duration: Number(j.format?.duration) || 0,
          video: v ? { codec: v.codec_name, width: v.width, height: v.height, pixFmt: v.pix_fmt } : null,
          audio: audio.map((a) => a.codec_name),
          audioNames: audio.map(trackName),
        };
      });
      p.catch(() => this.probes.delete(k));
      this.probes.set(k, p);
    }
    return this.probes.get(k);
  }

  /** Plays in every browser as-is: H.264 8-bit 4:2:0 in MP4, with at most one AAC track. */
  static directPlayable(clip, info) {
    return (
      clip.ext === "mp4" &&
      info.video?.codec === "h264" &&
      (!info.video.pixFmt || info.video.pixFmt === "yuv420p" || info.video.pixFmt === "yuvj420p") &&
      info.audio.length <= 1 &&
      info.audio.every((a) => a === "aac")
    );
  }

  thumbPath(clip) {
    return path.join(this.thumbDir, `${this.key(clip)}.jpg`);
  }

  thumbnail(clip, file) {
    const k = this.key(clip);
    if (!this.thumbs.has(k)) {
      const out = this.thumbPath(clip);
      const p = (async () => {
        if (await exists(out)) return out;
        return this.thumbQueue(async () => {
          const tmp = `${out}.tmp.jpg`;
          const grab = (at) =>
            run(FFMPEG, [
              "-hide_banner", "-v", "error", "-y",
              "-ss", String(at), "-i", file,
              "-frames:v", "1", "-vf", "scale=480:-2", "-q:v", "5",
              tmp,
            ]);
          // 2 s in skips black first frames; very short clips fall back to the start.
          await grab(2).catch(() => grab(0));
          if (!(await exists(tmp))) await grab(0);
          await rename(tmp, out);
          return out;
        });
      })();
      p.catch(() => this.thumbs.delete(k));
      this.thumbs.set(k, p);
    }
    return this.thumbs.get(k);
  }

  webPath(clip) {
    return path.join(this.webDir, `${this.key(clip)}.mp4`);
  }

  /** State of the phone-friendly copy: none | queued | working | ready | failed. */
  async webStatus(clip) {
    const job = this.jobs.get(this.key(clip));
    if (job) return { state: job.state, progress: job.progress, error: job.error };
    if (await exists(this.webPath(clip))) return { state: "ready", progress: 1 };
    return { state: "none", progress: 0 };
  }

  /** Start (or join) the H.264 conversion. Returns the status right away. */
  async prepareWeb(clip, file) {
    const k = this.key(clip);
    const current = await this.webStatus(clip);
    if (current.state !== "none" && current.state !== "failed") return current;
    const job = { state: "queued", progress: 0, error: null };
    this.jobs.set(k, job);
    job.promise = this.transcodeQueue(async () => {
      job.state = "working";
      const info = await this.probe(clip, file);
      const out = this.webPath(clip);
      const tmp = `${out}.tmp.mp4`;
      const args = ["-hide_banner", "-v", "error", "-y", "-i", file, "-map", "0:v:0"];
      // Desktop and mic are separate tracks in Rewind clips: mix them so phones hear both
      // (not a track that already mixes the others, like Medal's "All Audio": see tracks.js).
      const mix = tracksToMix(info.audioNames ?? info.audio.map(() => ""));
      if (mix.length === 1) args.push("-map", `0:a:${mix[0]}`);
      if (mix.length > 1) {
        const inputs = mix.map((i) => `[0:a:${i}]`).join("");
        args.push("-filter_complex", `${inputs}amix=inputs=${mix.length}:duration=longest:normalize=0[mix]`, "-map", "[mix]");
      }
      args.push(
        "-vf", "scale='min(1920,iw)':-2,format=yuv420p",
        "-c:v", H264, ...encoderArgs(H264),
        "-c:a", "aac", "-b:a", "160k", "-ac", "2",
        "-movflags", "+faststart",
        "-progress", "pipe:1", "-nostats",
        tmp,
      );
      await run(FFMPEG, args, {
        onStdout: (chunk) => {
          const m = /out_time_us=(\d+)/.exec(chunk);
          if (m && info.duration) job.progress = Math.min(0.99, Number(m[1]) / 1e6 / info.duration);
        },
      });
      await rename(tmp, out);
      await rm(tmp, { force: true });
      job.state = "ready";
      job.progress = 1;
      this.jobs.delete(k);
      await this.pruneWeb(out);
    }).catch(async (e) => {
      job.state = "failed";
      job.error = String(e.message || e).slice(-500);
      await rm(`${this.webPath(clip)}.tmp.mp4`, { force: true });
    });
    return { state: job.state, progress: 0 };
  }

  /** Mark a cached copy as used, so pruning keeps recently watched clips. */
  async touch(file) {
    const now = new Date();
    await utimes(file, now, now).catch(() => {});
  }

  /** Keep the converted copies under MAX_CACHE_GB, removing the least recently used. */
  async pruneWeb(keep) {
    if (!this.maxCacheBytes) return;
    const files = [];
    for (const f of await readdir(this.webDir)) {
      if (!f.endsWith(".mp4") || f.includes(".tmp")) continue;
      const p = path.join(this.webDir, f);
      const s = await stat(p).catch(() => null);
      if (s) files.push({ p, size: s.size, used: s.mtimeMs });
    }
    let total = files.reduce((n, f) => n + f.size, 0);
    files.sort((a, b) => a.used - b.used);
    for (const f of files) {
      if (total <= this.maxCacheBytes) break;
      if (f.p === keep) continue;
      await rm(f.p, { force: true });
      total -= f.size;
    }
  }
}

function encoderArgs(enc) {
  if (enc === "libx264") return ["-preset", process.env.X264_PRESET || "veryfast", "-crf", "23", "-profile:v", "high"];
  if (enc === "libopenh264") return ["-b:v", "6M"];
  if (enc.endsWith("_qsv") || enc.endsWith("_nvenc")) return ["-b:v", "8M"];
  return [];
}
