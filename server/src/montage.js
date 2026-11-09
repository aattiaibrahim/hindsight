// Montages on the NAS: clip pieces one after another, an optional song,
// rendered by ffmpeg into the cache (the clips folder stays read-only) and
// downloaded from the page. Same rules as the desktop app's montages:
// every piece scaled to one size and frame rate (letterboxed, never
// stretched), each clip's tracks mixed, the song looped/trimmed to length
// with a 2 s fade out.

import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { mkdir, readdir, rm, stat } from "node:fs/promises";
import path from "node:path";

const FFMPEG = process.env.FFMPEG || "ffmpeg";

/**
 * ffmpeg arguments for a montage. `pieces`: [{ file, start, end, audioTracks }],
 * `music`: { file, offset, volumeDb } or null. Returns { args, total }.
 */
export function montageArgs({ pieces, music = null, clipVolumeDb = 0, height = 1080, fps = 30, encoder = "libx264" }, out) {
  if (!pieces?.length) throw Object.assign(new Error("Add some clips to the montage first."), { status: 400 });
  const h = Math.floor(Math.min(2160, Math.max(360, height)) / 2) * 2;
  const w = Math.floor((h * 16) / 9 / 2) * 2;
  fps = Math.min(120, Math.max(24, fps));
  const args = ["-hide_banner", "-loglevel", "error", "-y", "-progress", "pipe:1", "-nostats"];
  let total = 0;
  for (const p of pieces) {
    const len = Math.max(0.1, p.end - p.start);
    total += len;
    args.push("-ss", Math.max(0, p.start).toFixed(3), "-t", len.toFixed(3), "-i", p.file);
  }
  if (music) args.push("-stream_loop", "-1", "-ss", Math.max(0, music.offset || 0).toFixed(3), "-i", music.file);
  const g = [];
  const gain = clipVolumeDb <= -60 ? "volume=0" : `volume=${clipVolumeDb.toFixed(2)}dB`;
  pieces.forEach((p, i) => {
    const len = Math.max(0.1, p.end - p.start).toFixed(3);
    g.push(
      `[${i}:v:0]scale=${w}:${h}:force_original_aspect_ratio=decrease,pad=${w}:${h}:(ow-iw)/2:(oh-ih)/2:black,fps=${fps},format=yuv420p,setsar=1,trim=duration=${len},setpts=PTS-STARTPTS[v${i}]`,
    );
    const tail = `aresample=48000,aformat=channel_layouts=stereo,${gain},apad,atrim=duration=${len},asetpts=PTS-STARTPTS[a${i}]`;
    // `mix`: the tracks to mix (not one that already mixes the others, see tracks.js).
    const mix = p.mix ?? Array.from({ length: p.audioTracks ?? 1 }, (_, t) => t);
    if (mix.length === 0) g.push(`anullsrc=r=48000:cl=stereo,atrim=duration=${len},asetpts=PTS-STARTPTS[a${i}]`);
    else if (mix.length === 1) g.push(`[${i}:a:${mix[0]}]${tail}`);
    else g.push(`${mix.map((t) => `[${i}:a:${t}]`).join("")}amix=inputs=${mix.length}:duration=longest:normalize=0,${tail}`);
  });
  g.push(`${pieces.map((_, i) => `[v${i}][a${i}]`).join("")}concat=n=${pieces.length}:v=1:a=1[vout][aclips]`);
  if (music) {
    const fadeAt = Math.max(0, total - 2).toFixed(3);
    g.push(
      `[${pieces.length}:a:0]aresample=48000,aformat=channel_layouts=stereo,volume=${(music.volumeDb || 0).toFixed(2)}dB,atrim=duration=${total.toFixed(3)},afade=t=out:st=${fadeAt}:d=2,asetpts=PTS-STARTPTS[song]`,
    );
    g.push("[aclips][song]amix=inputs=2:duration=first:normalize=0[aout]");
  } else g.push("[aclips]anull[aout]");
  args.push("-filter_complex", g.join(";"), "-map", "[vout]", "-map", "[aout]");
  const video = encoder === "libx264" ? ["-c:v", "libx264", "-preset", "veryfast", "-crf", "21"] : ["-c:v", encoder];
  args.push(...video, "-pix_fmt", "yuv420p", "-g", String(fps * 2), "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart", "-t", total.toFixed(3), out);
  return { args, total };
}

export class Montages {
  constructor(cacheDir, { encoder = "libx264", keepHours = 24 } = {}) {
    this.dir = path.join(cacheDir, "montages");
    this.songs = path.join(cacheDir, "songs");
    this.encoder = encoder;
    this.keepMs = keepHours * 3600 * 1000;
    this.jobs = new Map();
  }

  songPath(id) {
    if (!/^[\w-]+\.(mp3|m4a|aac|wav|flac|ogg|opus)$/i.test(id)) throw Object.assign(new Error("bad song"), { status: 400 });
    return path.join(this.songs, id);
  }

  /** Start a render; returns the job id. Only one runs at a time. */
  async start(plan) {
    if ([...this.jobs.values()].some((j) => j.state === "working")) throw Object.assign(new Error("A montage is already being made. Try again when it's done."), { status: 409 });
    await mkdir(this.dir, { recursive: true });
    await this.prune();
    const id = randomUUID();
    const out = path.join(this.dir, `${id}.mp4`);
    const { args, total } = montageArgs({ ...plan, encoder: this.encoder }, out);
    const job = { id, state: "working", progress: 0, error: null, out, total, name: plan.name || "Montage" };
    this.jobs.set(id, job);
    const p = spawn(FFMPEG, args, { stdio: ["ignore", "pipe", "pipe"] });
    let err = "";
    p.stderr.on("data", (d) => (err = (err + d).slice(-2000)));
    p.stdout.on("data", (d) => {
      const m = /out_time_us=(\d+)/.exec(String(d));
      if (m) job.progress = Math.min(1, Number(m[1]) / 1e6 / total);
    });
    p.on("close", (code) => {
      if (code === 0) Object.assign(job, { state: "ready", progress: 1 });
      else Object.assign(job, { state: "failed", error: err.trim().split("\n").pop() || `ffmpeg exited ${code}` });
    });
    return id;
  }

  status(id) {
    const j = this.jobs.get(id);
    if (!j) throw Object.assign(new Error("unknown montage"), { status: 404 });
    return { id: j.id, state: j.state, progress: j.progress, error: j.error, total: j.total };
  }

  file(id) {
    const j = this.jobs.get(id);
    if (!j || j.state !== "ready") throw Object.assign(new Error("not ready"), { status: 404 });
    return j.out;
  }

  /** Finished montages and uploaded songs go after a day. */
  async prune() {
    for (const dir of [this.dir, this.songs]) {
      for (const f of await readdir(dir).catch(() => [])) {
        const p = path.join(dir, f);
        const s = await stat(p).catch(() => null);
        if (s && Date.now() - s.mtimeMs > this.keepMs) await rm(p, { force: true });
      }
    }
  }
}
