// The desktop app's deletes and replaces land in the NAS trash by the clip's
// place in the clips folder, and its Undo brings them back.

import assert from "node:assert/strict";
import { access, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import { Cleanup } from "../src/cleanup.js";
import { Media } from "../src/media.js";
import { createServer } from "../src/server.js";
import { Library } from "../src/state.js";

describe("trash by path (from the desktop app)", () => {
  let root, cache, server, base, cleanup, library;
  const exists = (p) => access(p).then(() => true, () => false);
  const post = (url, body) => fetch(base + url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }).then(async (r) => ({ status: r.status, json: await r.json() }));

  before(async () => {
    root = await mkdtemp(path.join(tmpdir(), "hs-trash-api-"));
    cache = await mkdtemp(path.join(tmpdir(), "hs-trash-api-cache-"));
    await mkdir(path.join(root, "VALORANT"));
    await writeFile(path.join(root, "VALORANT", "VALORANT 2026-11-03 10-00-00 - PC1.mkv"), Buffer.from("clip"));
    library = new Library(root, { rescanSeconds: 0 });
    const media = new Media({ cacheDir: cache });
    await media.init();
    await library.start();
    cleanup = new Cleanup({ root, cacheDir: cache, allow: true, trashDays: 30 });
    await cleanup.init();
    server = await createServer({ library, media, cleanup });
    await new Promise((r) => server.listen(0, "127.0.0.1", r));
    base = `http://127.0.0.1:${server.address().port}`;
  });
  after(async () => {
    library.stop();
    server.closeAllConnections?.();
    server.close();
    await rm(root, { recursive: true, force: true });
    await rm(cache, { recursive: true, force: true });
  });

  test("delete, then undo", async () => {
    const rel = "VALORANT/VALORANT 2026-11-03 10-00-00 - PC1.mkv";
    const file = path.join(root, ...rel.split("/"));
    const a = await post("/api/trash/add", { rel, why: "deleted on PC1" });
    assert.equal(a.status, 200);
    assert.equal(a.json.trashed, true);
    assert.equal(await exists(file), false);
    assert.equal(cleanup.trash.length, 1);
    assert.equal(cleanup.trash[0].why, "deleted on PC1");
    assert.equal((await post("/api/trash/add", { rel })).json.trashed, false, "nothing left to trash");
    const u = await post("/api/trash/restore-rel", { rel });
    assert.equal(u.json.restored, true);
    assert.equal(await exists(file), true);
    assert.equal(cleanup.trash.length, 0);
    assert.equal((await post("/api/trash/restore-rel", { rel })).json.restored, false);
  });

  test("a thumbnail by path is only for clips in the library", async () => {
    assert.equal((await fetch(`${base}/api/thumb?rel=${encodeURIComponent("../secret.mkv")}`)).status, 404);
    assert.equal((await fetch(`${base}/api/thumb?rel=nope.mkv`)).status, 404);
  });

  test("paths outside the clips folder are refused", async () => {
    for (const rel of ["../x.mkv", "/etc/passwd", ".hindsight-trash/a.mkv", "a\\b.mkv", "", "VALORANT/../x.mkv"]) {
      assert.equal((await post("/api/trash/add", { rel })).status, 400, rel);
    }
  });
});
