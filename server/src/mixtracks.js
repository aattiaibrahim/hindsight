// Add an "All audio" track to Hindsight's older clips, on the NAS itself.
//
// Since desktop 0.1.48 a new clip's first audio track is desktop, mic and Discord
// mixed, because other players (browsers, phones, VLC) only play the first track:
// a clip with a quiet desktop track seemed to have no sound at all. This does the
// same for clips saved before that, without sending them over the network.
//
// Safe by design:
// - Only Hindsight's own clips: every audio track is one Hindsight names (Desktop
//   audio, Microphone, Discord audio), there are 2 or more, and none is a mix yet.
//   Medal and OBS files are never touched (OBS's first track is often a mix already,
//   unnamed, and mixing it in again would double the sound).
// - The video and the separate tracks are copied untouched; only the mix is new.
// - The new file is checked (same length, one more track) before it takes the
//   clip's place, and the original goes to the trash (restorable for TRASH_DAYS).
// - The clip keeps its date.

import { spawn } from "node:child_process";
import { rename, rm, stat, utimes } from "node:fs/promises";
import path from "node:path";
import { probe } from "./edit.js";

export const MIX_TITLE = "All audio";
const OWN = new Set(["desktop audio", "microphone", "discord audio"]);

/** Does a clip with these track names need the mix track? */
export function needsMix(names) {
  const l = names.map((n) => String(n || "").trim().toLowerCase());
  return l.length >= 2 && l.every((n) => OWN.has(n));
}

const FFMPEG = () => process.env.FFMPEG || "ffmpeg";

function ffmpeg(args) {
  return new Promise((resolve, reject) => {
    const p = spawn(FFMPEG(), args, { stdio: ["ignore", "ignore", "pipe"], windowsHide: true });
    let err = "";
    p.stderr.on("data", (d) => (err = (err + d).slice(-2000)));
    p.on("error", reject);
    p.on("close", (code) => (code === 0 ? resolve() : reject(new Error(err.trim().split("\n").slice(-3).join(" ") || `ffmpeg exited ${code}`))));
  });
}

/** Write `out`: `file` with a first audio track that mixes all its tracks. */
export async function addMixTrack(file, out, names) {
  const n = names.length;
  const ins = names.map((_, i) => `[0:a:${i}]aresample=48000,aformat=channel_layouts=stereo[m${i}];`).join("");
  const refs = names.map((_, i) => `[m${i}]`).join("");
  const mp4 = /\.(mp4|m4v|mov)$/i.test(file);
  const args = ["-hide_banner", "-loglevel", "error", "-y", "-i", file, "-filter_complex", `${ins}${refs}amix=inputs=${n}:duration=longest:normalize=0[mix]`, "-map", "0:v", "-map", "[mix]"];
  for (let i = 0; i < n; i++) args.push("-map", `0:a:${i}`);
  args.push("-c", "copy", "-c:a:0", "aac", "-b:a:0", "192k", "-metadata:s:a:0", `title=${MIX_TITLE}`);
  names.forEach((t, i) => args.push(`-metadata:s:a:${i + 1}`, `title=${t}`));
  if (mp4) args.push("-movflags", "+faststart");
  args.push("-f", mp4 ? "mp4" : "matroska", out);
  await ffmpeg(args);
}

export class MixTracks {
  constructor({ library, cleanup }) {
    this.library = library;
    this.cleanup = cleanup;
    this.state = { running: false, checked: 0, total: 0, added: 0, failed: 0, lastError: null, finishedAt: null };
  }

  status() {
    return { ...this.state };
  }

  /** Start in the background. `skip`: clips (by place in the clips folder) a PC does itself. */
  start(skip = []) {
    if (this.state.running) return this.status();
    this.cleanup.needChange();
    const skipSet = new Set(skip.map((r) => String(r).toLowerCase()));
    const clips = this.library.clips.filter((c) => !skipSet.has(c.rel.toLowerCase()));
    this.state = { running: true, checked: 0, total: clips.length, added: 0, failed: 0, lastError: null, finishedAt: null };
    this.run(clips).finally(() => {
      this.state.running = false;
      this.state.finishedAt = Date.now();
      this.library.rescan().catch(() => {});
    });
    return this.status();
  }

  async run(clips) {
    for (const clip of clips) {
      try {
        await this.one(clip);
      } catch (e) {
        this.state.failed++;
        this.state.lastError = `${clip.name}: ${e.message}`;
      }
      this.state.checked++;
    }
  }

  async one(clip) {
    const file = path.join(this.library.root, ...clip.rel.split("/"));
    const info = await probe(file);
    const names = info.audio.map((a) => a.raw);
    if (!info.video || !needsMix(names)) return;
    const before = await stat(file);
    const tmp = path.join(path.dirname(file), `.${path.basename(file)}.mix.part`);
    try {
      await addMixTrack(file, tmp, names);
      const out = await probe(tmp);
      if (out.audio.length !== names.length + 1 || out.audio[0].raw !== MIX_TITLE || Math.abs(out.duration - info.duration) > Math.max(1, info.duration * 0.01)) {
        throw new Error("the new file didn't check out, so the clip was left as it was");
      }
      // Keep the date the clip was taken.
      await utimes(tmp, before.atime, before.mtime);
      const entry = await this.cleanup.toTrash(clip, "replaced when the All audio track was added");
      try {
        await rename(tmp, file);
      } catch (e) {
        // Put the original straight back rather than leave the clip missing.
        await this.cleanup.restore(entry.id).catch(() => {});
        throw e;
      }
      this.state.added++;
    } finally {
      await rm(tmp, { force: true });
    }
  }
}
