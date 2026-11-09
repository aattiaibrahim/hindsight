// Game logos for the library rows: the game's official icon from Steam, the
// same lookup as the desktop app (src-tauri/src/game_icons.rs).
//
// Only a folder's name is sent (to Steam's store search), and only for folders
// that look like a game. The icon is downloaded once and kept in the data
// folder; a miss is asked again after a week. HINDSIGHT_ONLINE_LOGOS=0 turns
// the lookups off, and every game then gets the neutral gamepad icon.

import { mkdir, rename, stat, writeFile, utimes } from "node:fs/promises";
import path from "node:path";
import { gameKey } from "./library.js";

/** Folder names that are never a game (and never looked up online). */
export const UNSORTED = [
  "desktop", "obs", "other", "imports", "imported", "recordings", "screenrecording", "captures", "xboxgamebar", "ascent", "clips", "videos", "temp", "untitled",
];

export function notAGame(key) {
  return !key || UNSORTED.includes(key) || ["edits", "edit", "exports", "montages", "trimmed", "highlights", "funny"].includes(key);
}

const EDITIONS = ["x", "remake", "remastered", "hd", "goty", "definitiveedition", "enhancededition", "gameoftheyearedition", "classic", "reloaded"];
const edition = (tail) => /^\d+$/.test(tail) || EDITIONS.includes(tail);

/**
 * Is a Steam title the same game as a folder? Same name, or one that only adds
 * a number or an edition at the end ("Hunt: Showdown 1896" for "Hunt Showdown"),
 * but not "Minecraft Dungeons" for "Minecraft".
 */
export function sameGame(folderKey, steamName) {
  const k = gameKey(steamName);
  return (
    k === folderKey ||
    (folderKey.length >= 6 && k.startsWith(folderKey) && edition(k.slice(folderKey.length))) ||
    (k.length >= 8 && folderKey.startsWith(k) && edition(folderKey.slice(k.length)))
  );
}

/** Games whose Steam name changed, or that the store search no longer lists. */
export const STEAM_ALIASES = {
  hitman3: 1659040,
  hitman: 1659040,
  rocketleague: 252950,
  counterstrikeglobaloffensive: 730,
  csgo: 730,
  cs2: 730,
  pubg: 578080,
  gtav: 271590,
  grandtheftautov: 271590,
};

/** "SpeciesUnknown-Win64-Shipping" → "Species Unknown"; null when it isn't a program name. */
export function tidyProgramName(folder) {
  const lower = folder.toLowerCase();
  if (folder.includes(" ") || !["-win64", "-shipping", "_x64", "_dx12", "_dx11", "-win32"].some((s) => lower.includes(s))) return null;
  let name = lower;
  for (const s of ["-win64-shipping", "-win64", "_x64", "-shipping", "_dx12", "_dx11"]) if (name.endsWith(s)) name = name.slice(0, -s.length);
  // Recover the capitals and spaces from the original spelling.
  const orig = [...folder].slice(0, [...name].length);
  let out = "";
  let prevLower = false;
  for (const ch of orig) {
    if (ch === "_" || ch === "-") {
      out += " ";
      prevLower = false;
      continue;
    }
    const upper = ch !== ch.toLowerCase();
    if (upper && prevLower) out += " ";
    prevLower = (ch !== ch.toUpperCase() && !upper) || /\d/.test(ch);
    out += ch;
  }
  out = out.split(/\s+/).filter(Boolean).join(" ") || name;
  return out && out !== folder ? out : null;
}

/** Percent-encode everything but letters and digits, like the desktop app. */
const term = (s) => [...Buffer.from(s, "utf8")].map((b) => (/[A-Za-z0-9]/.test(String.fromCharCode(b)) ? String.fromCharCode(b) : `%${b.toString(16).toUpperCase().padStart(2, "0")}`)).join("");

/** The Steam app id for a game: a known alias, else the store search. */
export async function steamApp(game, fetchJson) {
  const key = gameKey(game);
  if (STEAM_ALIASES[key]) return STEAM_ALIASES[key];
  // "Resident Evil 3 Remake" finds nothing, "Resident Evil 3" does: drop up to two words from the end.
  // A word that's only punctuation ("Empyrion - Galactic Survival") makes Steam find nothing.
  const words = game.split(/\s+/).filter((w) => /[\p{L}\p{N}]/u.test(w));
  for (let keep = words.length; keep >= Math.max(1, words.length - 2); keep--) {
    const j = await fetchJson(`https://store.steampowered.com/api/storesearch/?term=${term(words.slice(0, keep).join(" "))}&l=english&cc=US`);
    if (!j) return null;
    // The closest name wins: an exact match over one with an edition added.
    const best = (j.items ?? [])
      .filter((i) => i.type === "app" && i.id && typeof i.name === "string" && sameGame(key, i.name))
      .sort((a, b) => Math.abs(gameKey(a.name).length - key.length) - Math.abs(gameKey(b.name).length - key.length))[0];
    if (best) return best.id;
  }
  return null;
}

const DAY = 86400 * 1000;

async function mtime(p) {
  try {
    return (await stat(p)).mtimeMs;
  } catch {
    return null;
  }
}

/** The neutral icon for a game without a logo (Lucide "gamepad-2", ISC licence). */
export const GAMEPAD_SVG = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="-6 -6 36 36" fill="none" stroke="#b3b3b3" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="-6" y="-6" width="36" height="36" fill="#2a2a2a" stroke="none"/><line x1="6" x2="10" y1="11" y2="11"/><line x1="8" x2="8" y1="9" y2="13"/><line x1="15" x2="15.01" y1="12" y2="12"/><line x1="18" x2="18.01" y1="10" y2="10"/><path d="M17.32 5H6.68a4 4 0 0 0-3.978 3.59c-.006.052-.01.101-.017.152C2.604 9.416 2 14.456 2 16a3 3 0 0 0 3 3c1 0 1.5-.5 2-1l1.414-1.414A2 2 0 0 1 9.828 16h4.344a2 2 0 0 1 1.414.586L17 18c.5.5 1 1 2 1a3 3 0 0 0 3-3c0-1.545-.604-6.584-.685-7.258-.007-.05-.011-.1-.017-.151A4 4 0 0 0 17.32 5z"/></svg>`;

export class Logos {
  /** `dir`: where icons are kept. `online`: false = no lookups. `fetchImpl`: for tests. */
  constructor({ dir, online = true, fetchImpl = globalThis.fetch, log = console.error } = {}) {
    this.dir = dir;
    this.online = online;
    this.fetch = fetchImpl;
    this.log = log;
    this.pending = new Map(); // key -> Promise<path|null>
    this.chain = Promise.resolve(); // one lookup at a time, so Steam isn't flooded
  }

  async get(game) {
    const key = gameKey(game);
    if (!key || !this.online || notAGame(key)) return null;
    const file = path.join(this.dir, `${key}.steam2.jpg`);
    if ((await mtime(file)) !== null) return file;
    // "steam3": asked again after the search learned to skip punctuation words.
    const miss = path.join(this.dir, `${key}.steam3-none`);
    const missAt = await mtime(miss);
    if (missAt !== null && Date.now() - missAt < 7 * DAY) return null;
    if (!this.pending.has(key)) {
      const p = (this.chain = this.chain.then(async () => {
        try {
          const title = tidyProgramName(game) ?? game;
          await this.download(title, file);
          return file;
        } catch (e) {
          this.log(`game logo for ${game}: ${e.message}`);
          await mkdir(this.dir, { recursive: true });
          await writeFile(miss, "");
          const now = new Date();
          await utimes(miss, now, now).catch(() => {});
          return null;
        }
      })).finally(() => this.pending.delete(key));
      this.pending.set(key, p);
    }
    return this.pending.get(key);
  }

  async fetchJson(url) {
    try {
      const r = await this.fetch(url, { headers: { "user-agent": "Hindsight" }, signal: AbortSignal.timeout(15000) });
      return r.ok ? await r.json() : null;
    } catch {
      return null;
    }
  }

  async download(game, out) {
    const id = await steamApp(game, (u) => this.fetchJson(u));
    if (!id) throw new Error("not on Steam");
    const j = await this.fetchJson(`https://api.steampowered.com/ICommunityService/GetApps/v1/?appids%5B0%5D=${id}`);
    const hash = j?.response?.apps?.[0]?.icon;
    if (typeof hash !== "string" || !/^[0-9a-f]+$/i.test(hash)) throw new Error("no icon");
    // Newer icons are only at the first address, older ones at either.
    let bytes = null;
    for (const host of ["https://shared.akamai.steamstatic.com/community_assets/images/apps", "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps"]) {
      try {
        const r = await this.fetch(`${host}/${id}/${hash}.jpg`, { headers: { "user-agent": "Hindsight" }, signal: AbortSignal.timeout(15000) });
        if (r.ok) {
          bytes = Buffer.from(await r.arrayBuffer());
          break;
        }
      } catch {
        // try the other address
      }
    }
    if (!bytes) throw new Error("the icon wouldn't download");
    // A real JPEG, not an error page.
    if (bytes.length < 200 || bytes[0] !== 0xff || bytes[1] !== 0xd8) throw new Error("not an image");
    await mkdir(this.dir, { recursive: true });
    await writeFile(`${out}.part`, bytes);
    await rename(`${out}.part`, out);
  }
}
