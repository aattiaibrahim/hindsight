// A track that's the other tracks already mixed (Medal's "All Audio") is left out
// when a clip's sound becomes one track, or the game and mic play twice.
// The test clip's audio is a generated tone; nothing is ever played.

import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, test } from "node:test";
import { exportMp3 } from "../src/edit.js";
import { mixCopy, trackName, tracksToMix } from "../src/tracks.js";

test("a track that mixes the others is found by name", () => {
  assert.equal(mixCopy(["All Audio", "Game", "Mic"]), 0);
  assert.equal(mixCopy(["Game", "Mic", "Mix"]), 2);
  assert.equal(mixCopy(["Desktop audio", "Microphone", "Discord"]), null);
  assert.equal(mixCopy(["All Audio"]), null, "a lone track is just the sound");
  assert.equal(mixCopy(["Mix", "Master", "Mic"]), null, "unsure: two candidates");
  assert.deepEqual(tracksToMix(["All Audio", "Game", "Mic"]), [1, 2]);
  assert.deepEqual(tracksToMix(["", ""]), [0, 1]);
  assert.equal(trackName({ tags: { name: "All Audio" } }), "All Audio", "Medal's MP4s use the name tag");
  assert.equal(trackName({ tags: { title: "Game" } }), "Game");
});

const FFMPEG = process.env.FFMPEG || "ffmpeg";
const hasFfmpeg = spawnSync(FFMPEG, ["-version"]).status === 0;

describe("mixing a Medal-style clip", { skip: !hasFfmpeg && "ffmpeg isn't installed" }, () => {
  test("the All Audio copy isn't mixed in again", async () => {
    const dir = await mkdtemp(path.join(tmpdir(), "hindsight-mix-"));
    try {
      const clip = path.join(dir, "medal.mp4");
      // All Audio is a loud tone; Game and Mic are silent. Mixed right, the result is silent.
      execFileSync(FFMPEG, [
        "-hide_banner", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30",
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
        "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo",
        "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo",
        "-t", "3", "-map", "0:v", "-map", "1:a", "-map", "2:a", "-map", "3:a", "-c:v", "mpeg4", "-c:a", "aac",
        "-metadata:s:a:0", "title=All Audio", "-metadata:s:a:1", "title=Game", "-metadata:s:a:2", "title=Mic", clip,
      ]);
      const out = path.join(dir, "all.mp3");
      await exportMp3(clip, { out });
      const r = spawnSync(FFMPEG, ["-hide_banner", "-i", out, "-af", "volumedetect", "-f", "null", "-"], { encoding: "utf8" });
      const max = Number(/max_volume:\s*(-?[\d.]+)/.exec(r.stderr)?.[1]);
      assert.ok(max < -50, `the All Audio copy was mixed in again (max ${max} dB)`);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
});
