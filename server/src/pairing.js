// Pairing a PC with this server, so a PC only syncs into a folder after
// someone who can see this app agreed to it.
//
// 1. Here, "Pair a PC" shows a 6-digit code (10 minutes, 5 tries).
// 2. The PC writes a one-time file `.hindsight-pair-<token>` into the folder it
//    wants to sync to, then sends the code and token here.
// 3. This server checks the code, and that the token file is in *its own*
//    clips folder: so the PC's folder really is this server's library.
// The server never writes into the clips folder for this.

import { randomInt } from "node:crypto";
import { stat } from "node:fs/promises";
import path from "node:path";

const CODE_MS = 10 * 60 * 1000;
const TRIES = 5;
const TOKEN = /^[a-f0-9]{32}$/;

export class Pairing {
  constructor({ root, store, now = () => Date.now() }) {
    this.root = root;
    this.store = store;
    this.now = now;
    this.code = null; // { code, expires, tries }
  }

  /** A fresh code (replaces any earlier one). */
  newCode() {
    this.code = { code: String(randomInt(0, 1_000_000)).padStart(6, "0"), expires: this.now() + CODE_MS, tries: 0 };
    return this.current();
  }

  current() {
    const c = this.code;
    return c && c.expires > this.now() ? { code: c.code, expiresAt: c.expires } : null;
  }

  paired() {
    return Object.values(this.store.data.paired ?? {}).sort((a, b) => b.at - a.at);
  }

  /** A PC asks to pair. Throws with a plain message when it can't. */
  async pair({ code, token, pc }) {
    const c = this.code;
    const name = String(pc ?? "").trim().slice(0, 64);
    if (!name) throw err(400, "The PC didn't say its name.");
    if (!c || c.expires <= this.now()) throw err(403, "No pairing code is open. Press Pair a PC in the NAS app for a new one.");
    if (String(code ?? "").replace(/\s/g, "") !== c.code) {
      c.tries++;
      if (c.tries >= TRIES) this.code = null;
      throw err(403, c.tries >= TRIES ? "Too many wrong codes. Make a new code in the NAS app." : "That code isn't right.");
    }
    if (!TOKEN.test(String(token ?? ""))) throw err(400, "Bad pairing token.");
    try {
      await stat(path.join(this.root, `.hindsight-pair-${token}`));
    } catch {
      throw err(409, "The folder this PC picked isn't this server's clips folder. Pick the folder the NAS app shows its clips from.");
    }
    this.code = null; // one PC per code
    this.store.data.paired = this.store.data.paired ?? {};
    this.store.data.paired[name.toLowerCase()] = { pc: name, at: this.now() };
    await this.store.save();
    return { ok: true, pc: name };
  }

  async forget(pc) {
    if (this.store.data.paired) delete this.store.data.paired[String(pc ?? "").toLowerCase()];
    await this.store.save();
  }
}

const err = (status, message) => Object.assign(new Error(message), { status });
