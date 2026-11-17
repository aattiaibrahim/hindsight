import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import { filterClips, gamesOf, groupByDay, groupClips } from "../public/group.js";
import { isSkipped, makeClip, parseClipName, scanLibrary, sourceOf, stampFromName } from "../src/library.js";

describe("parseClipName", () => {
  test("plain clip", () => {
    assert.deepEqual(parseClipName("VALORANT 2026-11-03 10-00-00.mkv"), {
      game: "VALORANT",
      recording: false,
      day: "2026-11-03",
      time: "10:00:00",
      takenAt: "2026-11-03T10:00:00",
      pc: null,
      n: 1,
      ext: "mkv",
    });
  });

  test("PC name from a shared NAS", () => {
    const p = parseClipName("VALORANT 2026-11-03 10-00-00 - GAMING-PC.mkv");
    assert.equal(p.pc, "GAMING-PC");
    assert.equal(p.game, "VALORANT");
  });

  test("duplicate counter, with and without PC name", () => {
    assert.equal(parseClipName("Desktop 2026-11-03 10-01-00 (2).mkv").n, 2);
    const p = parseClipName("Desktop 2026-11-03 10-01-00 - PC_2 (13).mkv");
    assert.equal(p.pc, "PC_2");
    assert.equal(p.n, 13);
  });

  test("recordings", () => {
    const p = parseClipName("Apex Legends Recording 2026-01-02 23-59-58 - PC1.mkv");
    assert.equal(p.recording, true);
    assert.equal(p.game, "Apex Legends");
    assert.equal(p.takenAt, "2026-01-02T23:59:58");
  });

  test("mp4 from 'Move into Rewind', case-insensitive extension", () => {
    const p = parseClipName("Tom Clancys Rainbow Six Siege 2025-12-31 08-05-09.MP4");
    assert.equal(p.ext, "mp4");
    assert.equal(p.game, "Tom Clancys Rainbow Six Siege");
  });

  test("game names with dashes and digits", () => {
    const p = parseClipName("Call of Duty - Warzone 2 2026-11-03 10-00-00 - PC1.mkv");
    assert.equal(p.game, "Call of Duty - Warzone 2");
    assert.equal(p.pc, "PC1");
  });

  test("names that are not Rewind clips", () => {
    for (const n of ["clip one.mp4", "VALORANT 2026-11-03.mkv", "VALORANT 2026-11-03 10-00-00.avi", "VALORANT unknown date.mkv", "2026-11-03 10-00-00.mkv"]) {
      assert.equal(parseClipName(n), null, n);
    }
  });
});

describe("isSkipped", () => {
  test("hidden files and folders", () => {
    assert.ok(isSkipped(".thumbs"));
    assert.ok(isSkipped(".DS_Store"));
    assert.ok(isSkipped(".VALORANT 2026-11-03 10-00-00.mkv"));
  });
  test("copies still in progress", () => {
    assert.ok(isSkipped(".VALORANT 2026-11-03 10-00-00 - PC1.mkv.rewind-partial"));
    assert.ok(isSkipped("VALORANT 2026-11-03 10-00-00.mkv.REWIND-PARTIAL"));
  });
  test("normal clips", () => {
    assert.ok(!isSkipped("VALORANT 2026-11-03 10-00-00.mkv"));
  });
});

describe("stampFromName (other recorders)", () => {
  test("Medal, NVIDIA and other names", () => {
    assert.equal(stampFromName("MedalTVRust20230605210532.mp4"), "2023-06-05T21:05:32");
    assert.equal(stampFromName("MedalTVMinecraftWindows10Edition20230331211056-tr-edit.mp4"), "2023-03-31T21:10:56");
    assert.equal(stampFromName("Sea of Thieves 2024.07.27 - 21.19.42.02.DVR.mp4"), "2024-07-27T21:19:42");
    assert.equal(stampFromName("Replay 2024-10-24 01-25-00.mp4"), "2024-10-24T01:25:00");
  });
  test("no date", () => {
    for (const n of ["1T0ER7rahNjPIg.mov", "Sequence 01.mp4", "SIDEWAYS_6.mp4", "clip 20231399999999.mp4"]) assert.equal(stampFromName(n), null, n);
  });
  test("other recorders' clips get the date from their name", () => {
    const c = makeClip("Rust/MedalTVRust20230605210532.mp4", { size: 1, mtime: new Date(2026, 0, 1) });
    assert.equal(c.day, "2023-06-05");
    assert.equal(c.game, "Rust");
  });
});

describe("makeClip", () => {
  test("folder is the game; unparseable names fall back to the file time", () => {
    const mtime = new Date(2026, 10, 3, 9, 8, 7);
    const c = makeClip("Valorant/clip one.mp4", { size: 10, mtime });
    assert.equal(c.game, "Valorant");
    assert.equal(c.takenAt, "2026-11-03T09:08:07");
    assert.equal(c.day, "2026-11-03");
    assert.equal(c.pc, null);
  });
  test("ids are stable and distinct", () => {
    const info = { size: 1, mtime: new Date() };
    assert.equal(makeClip("A/a.mkv", info).id, makeClip("A/a.mkv", info).id);
    assert.notEqual(makeClip("A/a.mkv", info).id, makeClip("A/b.mkv", info).id);
  });
});

describe("scanLibrary", () => {
  let root;
  const put = async (rel, bytes = 100) => {
    await mkdir(path.dirname(path.join(root, rel)), { recursive: true });
    await writeFile(path.join(root, rel), Buffer.alloc(bytes));
  };

  before(async () => {
    root = await mkdtemp(path.join(tmpdir(), "rewind-scan-"));
    await put("VALORANT/VALORANT 2026-11-03 10-00-00 - PC1.mkv");
    await put("VALORANT/VALORANT 2026-11-03 10-00-00 - PC1 (2).mkv");
    await put("VALORANT/.VALORANT 2026-11-03 11-00-00 - PC1.mkv.rewind-partial");
    await put("VALORANT/VALORANT 2026-11-03 12-00-00.mkv.rewind-partial");
    await put("VALORANT/.hidden.mkv");
    await put("VALORANT/notes.txt");
    await put("VALORANT/empty 2026-11-03 10-00-00.mkv", 0);
    await put("Desktop/Desktop 2026-11-02 22-00-00.mp4");
    await put(".thumbs/Thumb 2026-11-03 10-00-00.mkv");
    await put(".Trash-1000/files/VALORANT 2026-11-03 10-00-00.mkv");
    await put("VALORANT/.sub/VALORANT 2026-11-03 10-00-00.mkv");
    await put("Loose 2026-11-01 01-02-03.mkv");
    await put("Tom Clancy's Rainbow Six  Siege/1T0ER7rahNjPIg.mov");
  });
  after(() => rm(root, { recursive: true, force: true }));

  test("finds clips and skips hidden, partial, empty and non-video files", async () => {
    const clips = await scanLibrary(root);
    assert.deepEqual(clips.map((c) => c.rel).sort(), [
      "Desktop/Desktop 2026-11-02 22-00-00.mp4",
      "Loose 2026-11-01 01-02-03.mkv",
      "Tom Clancy's Rainbow Six  Siege/1T0ER7rahNjPIg.mov",
      "VALORANT/VALORANT 2026-11-03 10-00-00 - PC1 (2).mkv",
      "VALORANT/VALORANT 2026-11-03 10-00-00 - PC1.mkv",
    ]);
    const loose = clips.find((c) => c.rel.startsWith("Loose"));
    assert.equal(loose.game, "Loose");
  });

  test("folders that differ only by case, spaces or punctuation are one game", async () => {
    const r = await mkdtemp(path.join(tmpdir(), "rewind-merge-"));
    const add = async (rel) => {
      await mkdir(path.dirname(path.join(r, rel)), { recursive: true });
      await writeFile(path.join(r, rel), Buffer.alloc(10));
    };
    await add("Tom Clancys Rainbow Six Siege/Tom Clancys Rainbow Six Siege 2026-11-03 10-00-00 - PC1.mkv");
    await add("Tom Clancy's Rainbow Six  Siege/a.mov");
    await add("Tom Clancy's Rainbow Six  Siege/b.mov");
    await add("VALORANT/VALORANT 2026-11-03 10-00-00 - PC1.mkv");
    await add("Rust/MedalTVRust20230605210532.mp4");
    const clips = await scanLibrary(r);
    await rm(r, { recursive: true, force: true });
    assert.deepEqual(gamesOf(clips), ["Rust", "Tom Clancy's Rainbow Six  Siege", "VALORANT"]);
    const r6 = clips.find((c) => c.name.endsWith(".mkv") && c.folder.startsWith("Tom"));
    assert.equal(r6.game, "Tom Clancy's Rainbow Six  Siege");
    assert.equal(r6.folder, "Tom Clancys Rainbow Six Siege");
  });

  test("missing root is an empty library", async () => {
    assert.deepEqual(await scanLibrary(path.join(root, "nope")), []);
  });
});

describe("grouping and filtering", () => {
  const info = (size = 1) => ({ size, mtime: new Date(0) });
  const clips = [
    makeClip("VALORANT/VALORANT 2026-11-03 10-00-00 - PC1.mkv", info()),
    makeClip("Desktop/Desktop 2026-11-03 09-00-00.mkv", info()),
    makeClip("VALORANT/VALORANT 2026-11-03 08-00-00 - LAPTOP.mkv", info()),
    makeClip("Apex Legends/Apex Legends Recording 2026-11-02 23-00-00 - PC1.mkv", info()),
    makeClip("VALORANT/VALORANT 2026-11-03 10-00-00 - PC1 (2).mkv", info()),
  ];

  test("days newest first, games by newest clip, clips newest first", () => {
    const g = groupClips(clips);
    assert.deepEqual(g.map((d) => d.day), ["2026-11-03", "2026-11-02"]);
    assert.deepEqual(g[0].games.map((x) => x.game), ["VALORANT", "Desktop"]);
    assert.deepEqual(
      g[0].games[0].clips.map((c) => c.name),
      [
        "VALORANT 2026-11-03 10-00-00 - PC1 (2).mkv",
        "VALORANT 2026-11-03 10-00-00 - PC1.mkv",
        "VALORANT 2026-11-03 08-00-00 - LAPTOP.mkv",
      ],
    );
    assert.equal(g[1].games[0].game, "Apex Legends");
  });

  test("search matches game, PC name and words in any order", () => {
    assert.equal(filterClips(clips, { search: "laptop" }).length, 1);
    assert.equal(filterClips(clips, { search: "pc1 valorant" }).length, 2);
    assert.equal(filterClips(clips, { search: "apex" }).length, 1);
    assert.equal(filterClips(clips, { search: "nothing" }).length, 0);
    assert.equal(filterClips(clips, { search: "  " }).length, clips.length);
  });

  test("game filter", () => {
    assert.equal(filterClips(clips, { game: "Desktop" }).length, 1);
    assert.equal(filterClips(clips, { game: "VALORANT", search: "laptop" }).length, 1);
    assert.deepEqual(gamesOf(clips), ["Apex Legends", "Desktop", "VALORANT"]);
  });

  test("by day, like the desktop library: newest day first, every game together", () => {
    const days = groupByDay(clips);
    assert.deepEqual(days.map((d) => d.day), ["2026-11-03", "2026-11-02"]);
    assert.deepEqual(days[0].clips.map((c) => c.takenAt.slice(11)), ["10:00:00", "10:00:00", "09:00:00", "08:00:00"]);
    assert.equal(days[1].clips[0].game, "Apex Legends");
  });
});

describe("sourceOf", () => {
  test("names other recorders, nothing for Hindsight's own", () => {
    assert.equal(sourceOf("MedalTVRust20230605210532.mp4"), "Medal");
    assert.equal(sourceOf("Sea of Thieves 2024.07.27 - 21.19.42.02.DVR.mp4"), "NVIDIA");
    assert.equal(sourceOf("VALORANT 2026-11-03 10-00-00 - PC1.mkv"), "");
    assert.equal(sourceOf("Sequence 01.mp4"), "");
  });
});

describe("filters, sorting and folders", async () => {
  const { facet, filterClips: f, folders, sortBy } = await import("../public/group.js");
  const mk = (rel, size, extra = {}) => ({ ...makeClip(rel, { size, mtime: new Date(0) }), ...extra });
  const clips = [
    mk("VALORANT/VALORANT 2026-11-03 10-00-00 - PC1.mkv", 300, { duration: 20 }),
    mk("VALORANT/VALORANT Recording 2026-10-01 10-00-00 - LAPTOP.mkv", 900, { duration: 600 }),
    mk("Rust/MedalTVRust20230605210532.mp4", 100, { duration: 60, damaged: true }),
    mk("Rust/Rust 2026-11-02 10-00-00 - PC1.mkv", 500),
  ];

  test("PC, source, kind, dates, length, damaged", () => {
    assert.equal(f(clips, { pcs: ["LAPTOP"] }).length, 1);
    assert.equal(f(clips, { sources: ["Medal"] }).length, 1);
    assert.equal(f(clips, { sources: [""] }).length, 3);
    assert.equal(f(clips, { kind: "recording" }).length, 1);
    assert.equal(f(clips, { kind: "clip" }).length, 3);
    assert.equal(f(clips, { from: "2026-11-01" }).length, 2);
    assert.equal(f(clips, { from: "2026-10-01", to: "2026-10-01" }).length, 1);
    // Not measured yet (no duration) stays in.
    assert.deepEqual(f(clips, { length: "short" }).map((c) => c.size).sort(), [300, 500]);
    assert.equal(f(clips, { length: "long" })[0].size, 900);
    assert.equal(f(clips, { damaged: true })[0].game, "Rust");
  });

  test("sorting", () => {
    assert.deepEqual(sortBy(clips, "big").map((c) => c.size), [900, 500, 300, 100]);
    assert.equal(sortBy(clips, "old")[0].source, "Medal");
    assert.equal(sortBy(clips, "long")[0].duration, 600);
    assert.equal(sortBy(clips)[0].size, 300);
  });

  test("folders and facets", () => {
    const fs = folders(clips);
    assert.deepEqual(fs.map((x) => [x.game, x.clips.length, x.bytes]), [["Rust", 2, 600], ["VALORANT", 2, 1200]]);
    assert.equal(fs[1].newest.size, 300);
    assert.deepEqual(facet(clips, "pc"), [["PC1", 2], ["", 1], ["LAPTOP", 1]]);
  });
});

describe("scanLibrary with folders inside game folders", () => {
  let root;
  before(async () => {
    root = await mkdtemp(path.join(tmpdir(), "hs-nested-"));
    await mkdir(path.join(root, "Tom Clancy's Rainbow Six  Siege", "nate moaning"), { recursive: true });
    await writeFile(path.join(root, "Tom Clancy's Rainbow Six  Siege", "nate moaning", "MedalTVTomClancysRainbowSixSiege20230412002943.mp4"), "x");
    await mkdir(path.join(root, "Tom Clancys Rainbow Six Siege"), { recursive: true });
    await writeFile(path.join(root, "Tom Clancys Rainbow Six Siege", "a.mp4"), "x");
  });
  after(() => rm(root, { recursive: true, force: true }));

  test("a clip in a subfolder is found and counts as that game's", async () => {
    const clips = await scanLibrary(root);
    const nested = clips.find((c) => c.rel.includes("nate moaning"));
    assert.ok(nested, "the nested clip is listed");
    // Both spellings show as one game.
    assert.equal(new Set(clips.map((c) => c.game)).size, 1);
  });
});
