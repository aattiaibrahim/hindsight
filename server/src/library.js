// Reads the clip library the Rewind desktop app writes to a NAS:
//   <root>/<Game>/<Game> [Recording ]<YYYY-MM-DD HH-MM-SS>[ - PCNAME][ (n)].mkv|.mp4
// Never writes to it.

import { createHash } from "node:crypto";
import { readdir, stat } from "node:fs/promises";
import path from "node:path";

export const VIDEO_EXTS = new Set([".mkv", ".mp4", ".mov", ".webm"]);
export const PARTIAL = ".rewind-partial";

const NAME_RE =
  /^(?<game>.+?) (?<rec>Recording )?(?<date>\d{4}-\d{2}-\d{2}) (?<h>\d{2})-(?<m>\d{2})-(?<s>\d{2})(?: - (?<pc>[A-Za-z0-9_-]+))?(?: \((?<n>\d+)\))?\.(?<ext>mkv|mp4)$/i;

/** Hidden files/folders and copies the desktop app is still writing. */
export function isSkipped(name) {
  return name.startsWith(".") || name.toLowerCase().endsWith(PARTIAL);
}

export function isVideo(name) {
  return VIDEO_EXTS.has(path.extname(name).toLowerCase());
}

/**
 * Parse a Rewind clip name. Returns null for names that don't follow the pattern.
 * -> { game, recording, day, time, takenAt, pc, n, ext }
 */
export function parseClipName(name) {
  const m = NAME_RE.exec(name);
  if (!m) return null;
  const g = m.groups;
  const time = `${g.h}:${g.m}:${g.s}`;
  return {
    game: g.game,
    recording: Boolean(g.rec),
    day: g.date,
    time,
    takenAt: `${g.date}T${time}`,
    pc: g.pc || null,
    n: g.n ? Number(g.n) : 1,
    ext: g.ext.toLowerCase(),
  };
}

/**
 * A date and time found anywhere in a name from another recorder, as "YYYY-MM-DDTHH:MM:SS".
 * Medal: "MedalTVRust20230605210532.mp4"; NVIDIA: "Sea of Thieves 2024.07.27 - 21.19.42.02.DVR.mp4";
 * others: "Replay 2024-10-24 01-25-00.mp4".
 */
export function stampFromName(name) {
  const m = /(20\d{2})[._-]?(\d{2})[._-]?(\d{2})[ _T.-]*(\d{2})[._:-]?(\d{2})[._:-]?(\d{2})/.exec(name);
  if (!m) return null;
  const [, y, mo, d, h, mi, s] = m;
  if (mo < "01" || mo > "12" || d < "01" || d > "31" || h > "23" || mi > "59" || s > "59") return null;
  return `${y}-${mo}-${d}T${h}:${mi}:${s}`;
}

/** Local wall-clock time as "YYYY-MM-DDTHH:MM:SS" (used when a name has no timestamp). */
export function localStamp(date) {
  const p = (v) => String(v).padStart(2, "0");
  return `${date.getFullYear()}-${p(date.getMonth() + 1)}-${p(date.getDate())}T${p(date.getHours())}:${p(date.getMinutes())}:${p(date.getSeconds())}`;
}

/** Which recorder made a clip, from its name: "" for Hindsight's own. */
export function sourceOf(name, parsed = parseClipName(name)) {
  if (parsed) return "";
  if (/^medaltv/i.test(name)) return "Medal";
  if (/\.dvr\.[^.]+$/i.test(name)) return "NVIDIA";
  return "";
}

export function clipId(rel) {
  return createHash("sha1").update(rel).digest("base64url").slice(0, 16);
}

/**
 * Build a clip record from a file name, its folder (the game) and stat info.
 * The folder wins over the name for the game, so clips always sit with their folder.
 */
export function makeClip(rel, info) {
  const name = path.posix.basename(rel);
  const folder = path.posix.dirname(rel);
  const parsed = parseClipName(name);
  const takenAt = parsed?.takenAt ?? stampFromName(name) ?? localStamp(info.mtime);
  const game = folder !== "." ? folder.split("/")[0] : parsed?.game || "Other";
  return {
    id: clipId(rel),
    rel,
    name,
    game,
    pc: parsed?.pc ?? null,
    recording: parsed?.recording ?? false,
    n: parsed?.n ?? 1,
    source: sourceOf(name, parsed),
    takenAt,
    day: takenAt.slice(0, 10),
    ext: path.extname(name).slice(1).toLowerCase(),
    size: info.size,
    mtime: info.mtime.getTime(),
  };
}

/**
 * Scan the library: video files in the root and in game folders, up to three
 * folders deep ("Game/nate moaning/clip.mp4" counts as Game's, like the desktop app).
 * Hidden entries and partial copies are skipped.
 */
export async function scanLibrary(root) {
  const clips = [];
  const add = async (rel) => {
    try {
      const info = await stat(path.join(root, rel));
      if (info.isFile() && info.size > 0) clips.push(makeClip(rel, info));
    } catch {
      // vanished between readdir and stat (moved/renamed): skip
    }
  };
  let top;
  try {
    top = await readdir(root, { withFileTypes: true });
  } catch (e) {
    if (e.code === "ENOENT") return [];
    throw e;
  }
  for (const entry of top) {
    if (isSkipped(entry.name)) continue;
    if (entry.isDirectory()) {
      // Folders of clips inside a game's folder count as that game's (up to three deep).
      const walk = async (rel, depth) => {
        let files;
        try {
          files = await readdir(path.join(root, rel), { withFileTypes: true });
        } catch {
          return;
        }
        for (const f of files) {
          if (isSkipped(f.name)) continue;
          if (f.isDirectory()) {
            if (depth < 3) await walk(`${rel}/${f.name}`, depth + 1);
          } else if (f.isFile() && isVideo(f.name)) await add(`${rel}/${f.name}`);
        }
      };
      await walk(entry.name, 1);
    } else if (entry.isFile() && isVideo(entry.name)) {
      await add(entry.name);
    }
  }
  return mergeGameNames(clips);
}

/** Same rule as the desktop app: case, spaces and punctuation don't make a different game. */
export function gameKey(name) {
  return name.toLowerCase().replace(/[^a-z0-9]+/g, "");
}

/**
 * Folders that differ only by case, spacing or punctuation ("VALORANT" and "Valorant",
 * "Tom Clancys Rainbow Six Siege" and "Tom Clancy's Rainbow Six  Siege") show as one game,
 * named after the folder with the most clips. The clip's own folder is kept in `folder`.
 */
export function mergeGameNames(clips) {
  const counts = new Map(); // key -> Map(name -> count)
  for (const c of clips) {
    const k = gameKey(c.game) || c.game;
    if (!counts.has(k)) counts.set(k, new Map());
    const m = counts.get(k);
    m.set(c.game, (m.get(c.game) || 0) + 1);
  }
  const names = new Map();
  for (const [k, m] of counts) {
    const best = [...m].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))[0][0];
    names.set(k, best);
  }
  for (const c of clips) {
    c.folder = c.game;
    c.game = names.get(gameKey(c.game) || c.game);
  }
  return clips;
}
