// Adding the All audio track to older clips on the NAS: only Hindsight's own clips,
// the original goes to the trash, the date is kept, and the sound isn't doubled.
// The test clips' audio is generated tones; nothing is ever played.

import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, rm, stat, utimes } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, test } from "node:test";
import { Cleanup } from "../src/cleanup.js";
import { probe } from "../src/edit.js";
import { MixTracks, needsMix } from "../src/mixtracks.js";
import { Library } from "../src/state.js";
import { tracksToMix } from "../src/tracks.js";

test("only Hindsight's own clips need the mix track", () => {
  assert.equal(needsMix(["Desktop audio", "Microphone", "Discord audio"]), true);
  assert.equal(needsMix(["Desktop audio", "Microphone"]), true);
  assert.equal(needsMix(["All audio", "Desktop audio", "Microphone"]), false, "already has it");
  assert.equal(needsMix(["", ""]), false, "OBS: unnamed tracks, often a mix first");
  assert.equal(needsMix(["All Audio", "Game", "Mic"]), false, "Medal");
  assert.equal(needsMix(["Desktop audio"]), false, "one track plays everywhere already");
});

const FFMPEG = process.env.FFMPEG || "ffmpeg";
const hasFfmpeg = spawnSync(FFMPEG, ["-version"]).status === 0;
const loudness = (file, track) => {
  const r = spawnSync(FFMPEG, ["-hide_banner", "-i", file, "-map", `0:a:${track}`, "-af", "volumedetect", "-f", "null", "-"], { encoding: "utf8" });
  return Number(/max_volume:\s*(-?[\d.]+)/.exec(r.stderr)?.[1]);
};

describe("adding the mix track on the NAS", { skip: !hasFfmpeg && "ffmpeg isn't installed" }, () => {
  test("a quiet-desktop clip gets a first track with the mic in it; others are left alone", async () => {
    const root = await mkdtemp(path.join(tmpdir(), "hs-mix-"));
    const cache = await mkdtemp(path.join(tmpdir(), "hs-mix-cache-"));
    try {
      await mkdir(path.join(root, "Desktop"));
      const make = (name, titles, sources) => {
        const args = ["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30"];
        for (const s of sources) args.push("-f", "lavfi", "-i", s);
        args.push("-t", "3", "-map", "0:v");
        sources.forEach((_, i) => args.push("-map", `${i + 1}:a`));
        args.push("-c:v", "mpeg4", "-c:a", "aac");
        titles.forEach((t, i) => args.push(`-metadata:s:a:${i}`, `title=${t}`));
        args.push(path.join(root, "Desktop", name));
        execFileSync(FFMPEG, args);
      };
      const silent = "anullsrc=r=48000:cl=stereo";
      make("Desktop 2026-11-12 02-34-09 - ADMIN.mkv", ["Desktop audio", "Microphone", "Discord audio"], [silent, "sine=frequency=440:sample_rate=48000", silent]);
      make("OBS 2026-09-07 03-14-17.mkv", ["", ""], ["sine=frequency=440:sample_rate=48000", "sine=frequency=880:sample_rate=48000"]);
      const ours = path.join(root, "Desktop", "Desktop 2026-11-12 02-34-09 - ADMIN.mkv");
      const taken = new Date(2026, 10, 12, 2, 34, 9);
      await utimes(ours, taken, taken);

      const library = new Library(root, { rescanSeconds: 0 });
      await library.start();
      const cleanup = new Cleanup({ root, cacheDir: cache, allow: true, trashDays: 30 });
      await cleanup.init();
      const job = new MixTracks({ library, cleanup });
      job.start();
      while (job.status().running) await new Promise((r) => setTimeout(r, 100));
      library.stop();

      const st = job.status();
      assert.equal(st.added, 1, JSON.stringify(st));
      assert.equal(st.failed, 0, st.lastError);
      const info = await probe(ours);
      assert.deepEqual(info.audio.map((a) => a.raw), ["All audio", "Desktop audio", "Microphone", "Discord audio"]);
      assert.ok(loudness(ours, 0) > -30, `other players hear the mic (${loudness(ours, 0)} dB)`);
      assert.deepEqual(tracksToMix(info.audio.map((a) => a.raw)), [1, 2, 3], "the NAS app's own mixes leave it out");
      assert.equal((await stat(ours)).mtime.getTime(), taken.getTime(), "keeps the date it was taken");
      assert.equal(cleanup.trash.length, 1, "the original is in the trash");
      const obs = await probe(path.join(root, "Desktop", "OBS 2026-09-07 03-14-17.mkv"));
      assert.equal(obs.audio.length, 2, "an OBS file is left alone");
    } finally {
      await rm(root, { recursive: true, force: true });
      await rm(cache, { recursive: true, force: true });
    }
  });
});
