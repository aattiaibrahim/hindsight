// Pairing a PC: the code, its tries and expiry, and the folder check.

import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { Pairing } from "../src/pairing.js";
import { Store } from "../src/store.js";

let root, cache, store;
const token = "0123456789abcdef0123456789abcdef";

before(async () => {
  root = await mkdtemp(path.join(tmpdir(), "pair-root-"));
  cache = await mkdtemp(path.join(tmpdir(), "pair-cache-"));
  store = new Store(cache);
  await store.init();
});
after(async () => {
  await rm(root, { recursive: true, force: true });
  await rm(cache, { recursive: true, force: true });
});

test("a PC pairs with the right code and a token file in this server's folder", async () => {
  const p = new Pairing({ root, store });
  await assert.rejects(p.pair({ code: "000000", token, pc: "GAMING" }), /No pairing code/);
  const { code } = p.newCode();
  assert.match(code, /^\d{6}$/);
  // Right code, but the token file is in some other folder.
  await assert.rejects(p.pair({ code, token, pc: "GAMING" }), /isn't this server's clips folder/);
  await writeFile(path.join(root, `.hindsight-pair-${token}`), "");
  const r = await p.pair({ code: `${code.slice(0, 3)} ${code.slice(3)}`, token, pc: "GAMING" });
  assert.equal(r.pc, "GAMING");
  assert.deepEqual(p.paired().map((x) => x.pc), ["GAMING"]);
  // A code works once.
  await assert.rejects(p.pair({ code, token, pc: "OTHER" }), /No pairing code/);
  await p.forget("gaming");
  assert.equal(p.paired().length, 0);
});

test("five wrong codes close it, and codes expire", async () => {
  let now = 1_000_000;
  const p = new Pairing({ root, store, now: () => now });
  const { code } = p.newCode();
  const wrong = code === "123456" ? "654321" : "123456";
  for (let i = 0; i < 4; i++) await assert.rejects(p.pair({ code: wrong, token, pc: "X" }), /isn't right/);
  await assert.rejects(p.pair({ code: wrong, token, pc: "X" }), /Too many/);
  await assert.rejects(p.pair({ code, token, pc: "X" }), /No pairing code/);
  const fresh = p.newCode().code;
  now += 11 * 60 * 1000;
  assert.equal(p.current(), null);
  await assert.rejects(p.pair({ code: fresh, token, pc: "X" }), /No pairing code/);
});
