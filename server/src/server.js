// HTTP API and static files. Writes to the clips folder only for cleanup and
// the editor, and only with ALLOW_CHANGES=1.

import { createReadStream } from "node:fs";
import { tracksToMix } from "./tracks.js";
import { makeClip } from "./library.js";
import { createWriteStream } from "node:fs";
import { mkdir, readFile, rename, rm, stat, statfs } from "node:fs/promises";
import { randomUUID } from "node:crypto";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { gamesOf } from "../public/group.js";
import { Media } from "./media.js";
import { probe as probeTracks, waveforms } from "./edit.js";
import { GAMEPAD_SVG } from "./logos.js";
import { hideNotice, mergeFolders, notices } from "./notices.js";

export const VERSION = (process.env.REWIND_SERVER_VERSION || "dev").slice(0, 7);
const PUBLIC = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "public");
/** The release number (package.json), shown next to the commit. */
export const RELEASE = JSON.parse(
  await readFile(path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "package.json"), "utf8"),
).version;
const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
  ".json": "application/json",
  ".webmanifest": "application/manifest+json",
  ".png": "image/png",
  ".woff2": "font/woff2",
  ".txt": "text/plain; charset=utf-8",
};

function send(res, code, body, headers = {}) {
  const json = typeof body === "object" && !Buffer.isBuffer(body);
  res.writeHead(code, {
    "content-type": json ? "application/json" : "text/plain; charset=utf-8",
    ...headers,
  });
  res.end(json ? JSON.stringify(body) : body);
}

/** Serve a file with HTTP Range support (needed for seeking in <video>). */
export async function sendFile(req, res, file, { type, download, cache } = {}) {
  let info;
  try {
    info = await stat(file);
  } catch {
    return send(res, 404, "not found");
  }
  const size = info.size;
  const headers = {
    "content-type": type || "application/octet-stream",
    "accept-ranges": "bytes",
    "last-modified": info.mtime.toUTCString(),
    "cache-control": cache || "no-cache",
  };
  if (download) headers["content-disposition"] = `attachment; filename*=UTF-8''${encodeURIComponent(download)}`;
  const range = /^bytes=(\d*)-(\d*)$/.exec(req.headers.range || "");
  let start = 0;
  let end = size - 1;
  if (range && (range[1] || range[2])) {
    if (range[1]) {
      start = Number(range[1]);
      if (range[2]) end = Math.min(Number(range[2]), size - 1);
    } else {
      start = Math.max(0, size - Number(range[2]));
    }
    if (start > end || start >= size) {
      res.writeHead(416, { "content-range": `bytes */${size}` });
      return res.end();
    }
    res.writeHead(206, { ...headers, "content-range": `bytes ${start}-${end}/${size}`, "content-length": end - start + 1 });
  } else {
    res.writeHead(200, { ...headers, "content-length": size });
  }
  if (req.method === "HEAD") return res.end();
  const stream = createReadStream(file, { start, end });
  stream.on("error", () => res.destroy());
  res.on("close", () => stream.destroy());
  stream.pipe(res);
}

/**
 * Space used: clips per game and in total, the cache, and the clips dataset's
 * size and free space as the NAS reports them (statfs of the mounted folder).
 */
export async function storage(library, media) {
  const games = new Map();
  let bytes = 0;
  for (const c of library.clips) {
    bytes += c.size;
    const g = games.get(c.game) ?? { game: c.game, bytes: 0, count: 0 };
    g.bytes += c.size;
    g.count++;
    games.set(c.game, g);
  }
  let disk = null;
  try {
    const s = await statfs(library.root);
    const total = s.blocks * s.bsize;
    const free = s.bavail * s.bsize;
    disk = { total, free, used: total - free };
  } catch {
    // not available on this filesystem
  }
  return {
    clips: { bytes, count: library.clips.length },
    games: [...games.values()].sort((a, b) => b.bytes - a.bytes),
    cache: { bytes: await media.cacheBytes() },
    disk,
  };
}

/** A request's JSON body (up to 1 MB). */
async function jsonBody(req) {
  let text = "";
  for await (const chunk of req) {
    text += chunk;
    if (text.length > 1 << 20) throw Object.assign(new Error("too big"), { status: 413 });
  }
  return text ? JSON.parse(text) : {};
}

export async function createServer({ library, media, cleanup = null, shares = null, montages = null, store = null, logos = null, edits = null, pairing = null, mixTracks = null }) {
  const withMeta = (c) => {
    const out = { ...c, ...(media.meta(c) ?? {}) };
    if (store) {
      out.toTrim = store.toTrim(c);
      out.nasOnly = store.nasOnly(c);
      out.keptWhole = !!store.data.keepWhole[c.rel.toLowerCase()];
    }
    return out;
  };
  const fileOf = (clip) => path.join(library.root, ...clip.rel.split("/"));
  const fail = (res, e) => send(res, e.status || 500, { error: String(e.message || e).slice(-500) });
  const post = (req) => req.method === "POST";

  const route = async (req, res) => {
    const url = new URL(req.url, "http://x");
    const parts = url.pathname.split("/").filter(Boolean);

    // ---- cleanup and trash ----
    if (cleanup && url.pathname === "/api/cleanup") {
      return send(res, 200, cleanup.list(library.clips.map(withMeta)), { "cache-control": "no-cache" });
    }
    if (cleanup && url.pathname === "/api/cleanup/check" && post(req)) {
      await cleanup.check();
      await library.rescan();
      return send(res, 200, cleanup.list(library.clips.map(withMeta)));
    }
    // Adding the All audio track to older clips (mixtracks.js): start it, or see how far it is.
    if (mixTracks && url.pathname === "/api/mix-tracks") {
      try {
        if (post(req)) {
          const body = await jsonBody(req).catch(() => ({}));
          return send(res, 200, mixTracks.start(Array.isArray(body.skip) ? body.skip.map(String) : []));
        }
        return send(res, 200, mixTracks.status(), { "cache-control": "no-cache" });
      } catch (e) {
        return fail(res, e);
      }
    }
    if (cleanup && url.pathname === "/api/trash") return send(res, 200, cleanup.trashList(), { "cache-control": "no-cache" });
    if (cleanup && url.pathname === "/api/trash/empty" && post(req)) {
      try {
        return send(res, 200, { removed: await cleanup.purge(true) });
      } catch (e) {
        return fail(res, e);
      }
    }
    // A clip's thumbnail by its place in the clips folder, for the desktop app: a
    // ready-made 17 KB picture is much quicker than ffmpeg reading the video over the network.
    if (url.pathname === "/api/thumb" && (req.method === "GET" || req.method === "HEAD")) {
      const clip = library.clips.find((c) => c.rel === url.searchParams.get("rel"));
      if (!clip) return send(res, 404, { error: "not in the library" });
      if (media.meta(clip)?.damaged) return send(res, 404, "no thumbnail: the clip can't be read");
      try {
        const out = await media.thumbnail(clip, path.join(library.root, ...clip.rel.split("/")));
        return sendFile(req, res, out, { type: "image/jpeg", cache: "public, max-age=86400" });
      } catch (e) {
        return fail(res, e);
      }
    }
    // The desktop app deletes (or replaces) a NAS clip by its place in the clips
    // folder, so it lands in this trash too; Undo there brings it back the same way.
    if (cleanup && (url.pathname === "/api/trash/add" || url.pathname === "/api/trash/restore-rel") && post(req)) {
      try {
        const body = await jsonBody(req);
        const rel = String(body.rel ?? "");
        const segs = rel.split("/");
        if (!rel || rel.includes("\\") || segs.some((p) => !p || p === "." || p === ".." || p.startsWith(".")) || path.isAbsolute(rel)) {
          return send(res, 400, { error: "Not a clip in the clips folder." });
        }
        if (url.pathname === "/api/trash/restore-rel") {
          const entry = cleanup.trash.filter((e) => e.rel === rel).sort((a, b) => b.trashedAt - a.trashedAt)[0];
          if (!entry) return send(res, 200, { restored: false });
          await cleanup.restore(entry.id);
          await library.rescan();
          return send(res, 200, { restored: true });
        }
        const file = path.join(library.root, ...segs);
        const st = await stat(file).catch(() => null);
        if (!st?.isFile()) return send(res, 200, { trashed: false });
        const clip = library.clips.find((c) => c.rel === rel) ?? makeClip(rel, { size: st.size, mtime: st.mtime });
        const why = String(body.why || "deleted").slice(0, 80);
        const entry = await cleanup.toTrash(clip, why);
        await library.rescan();
        return send(res, 200, { trashed: true, entry });
      } catch (e) {
        return fail(res, e);
      }
    }
    if (cleanup && parts[0] === "api" && parts[1] === "trash" && parts[3] === "restore" && post(req)) {
      try {
        await cleanup.restore(parts[2]);
        await library.rescan();
        return send(res, 200, { ok: true });
      } catch (e) {
        return fail(res, e);
      }
    }

    // ---- montages ----
    // A song for a montage, sent as the raw request body (x-filename names it). Up to 60 MB.
    if (montages && url.pathname === "/api/songs" && post(req)) {
      const ext = (/\.(mp3|m4a|aac|wav|flac|ogg|opus)$/i.exec(req.headers["x-filename"] || "")?.[1] || "").toLowerCase();
      if (!ext) return send(res, 400, { error: "Pick an MP3, M4A, WAV, FLAC, OGG or Opus file." });
      const id = `${randomUUID()}.${ext}`;
      const file = montages.songPath(id);
      await mkdir(path.dirname(file), { recursive: true });
      let bytes = 0;
      const out = createWriteStream(`${file}.part`);
      try {
        for await (const chunk of req) {
          bytes += chunk.length;
          if (bytes > 60 * 1024 * 1024) throw Object.assign(new Error("The song is too big (60 MB at most)."), { status: 413 });
          if (!out.write(chunk)) await new Promise((r) => out.once("drain", r));
        }
        await new Promise((r, j) => out.end((e) => (e ? j(e) : r())));
        await rename(`${file}.part`, file);
        const info = await media.probe({ id, size: bytes, mtime: Date.now() }, file).catch(() => null);
        return send(res, 200, { id, duration: info?.duration ?? 0 });
      } catch (e) {
        out.destroy();
        await rm(`${file}.part`, { force: true });
        return fail(res, e);
      }
    }
    if (montages && url.pathname === "/api/montage" && post(req)) {
      try {
        const body = await jsonBody(req);
        const pieces = [];
        for (const p of body.pieces ?? []) {
          const clip = library.byId.get(p.id);
          if (!clip) return send(res, 400, { error: "A clip in the montage isn't on the NAS any more." });
          const file = path.join(library.root, ...clip.rel.split("/"));
          const info = await media.probe(clip, file);
          const end = Math.min(Number(p.end), info.duration || Number(p.end));
          pieces.push({ file, start: Math.max(0, Number(p.start) || 0), end, audioTracks: info.audio.length, mix: tracksToMix(info.audioNames ?? info.audio.map(() => "")) });
        }
        const music = body.song ? { file: montages.songPath(body.song), offset: Number(body.songOffset) || 0, volumeDb: Number(body.songDb) || 0 } : null;
        const id = await montages.start({ pieces, music, clipVolumeDb: Number(body.clipDb) || 0, height: Number(body.height) || 1080, fps: Number(body.fps) || 30 });
        return send(res, 200, montages.status(id));
      } catch (e) {
        return fail(res, e);
      }
    }
    if (montages && parts[0] === "api" && parts[1] === "montage" && parts[2]) {
      try {
        if (parts[3] === "file") {
          const stamp = new Date().toISOString().slice(0, 16).replace("T", " ").replace(":", "-");
          return sendFile(req, res, montages.file(parts[2]), { type: "video/mp4", download: `Montage ${stamp}.mp4` });
        }
        return send(res, 200, montages.status(parts[2]), { "cache-control": "no-cache" });
      } catch (e) {
        return fail(res, e);
      }
    }

    // ---- game logos ----
    if (url.pathname === "/api/logo") {
      const file = logos ? await logos.get(url.searchParams.get("game") || "").catch(() => null) : null;
      if (file) return sendFile(req, res, file, { type: "image/jpeg", cache: "public, max-age=604800" });
      return send(res, 200, GAMEPAD_SVG, { "content-type": "image/svg+xml", "cache-control": "public, max-age=3600" });
    }

    // ---- notifications ----
    if (store && url.pathname === "/api/notices") {
      return send(res, 200, { notices: await notices(library.clips.map(withMeta), { store, cleanup }), canChange: !!cleanup?.canChange }, { "cache-control": "no-cache" });
    }
    if (store && url.pathname === "/api/notices/hide" && post(req)) {
      const body = await jsonBody(req);
      await hideNotice(String(body.id || ""), store);
      return send(res, 200, { ok: true });
    }
    if (store && url.pathname === "/api/notices/act" && post(req)) {
      try {
        const body = await jsonBody(req);
        const list = await notices(library.clips.map(withMeta), { store, cleanup });
        const n = list.find((x) => x.id === body.id);
        if (!n) return send(res, 404, { error: "That notice is gone already." });
        if (!cleanup) throw Object.assign(new Error("Changes are off on this server."), { status: 403 });
        cleanup.needChange();
        let done = 0;
        if (n.action.do === "merge") done = await mergeFolders(library.clips, n.action.game, library.root);
        else if (n.action.do === "dedupe") {
          for (const id of n.action.ids) {
            const c = library.byId.get(id);
            if (c) {
              await cleanup.toTrash(c, "a duplicate copy");
              done++;
            }
          }
        } else return send(res, 400, { error: "That notice has nothing to do on the server." });
        await library.rescan();
        return send(res, 200, { ok: true, done });
      } catch (e) {
        return fail(res, e);
      }
    }

    // ---- editor ----
    if (url.pathname === "/api/waveform") {
      const clip = library.byId.get(url.searchParams.get("id") || "");
      if (!clip) return send(res, 404, { error: "unknown clip" });
      try {
        return send(res, 200, { tracks: await waveforms(fileOf(clip), media.key(clip), media.cacheDir) }, { "cache-control": "private, max-age=3600" });
      } catch (e) {
        return fail(res, e);
      }
    }
    if (edits && url.pathname === "/api/edit" && post(req)) {
      try {
        const body = await jsonBody(req);
        const clip = library.byId.get(body.id);
        if (!clip) return send(res, 404, { error: "That clip isn't on the NAS any more." });
        // The editor needs the tracks' names too (for "Clip (Microphone).mp3").
        const info = await probeTracks(fileOf(clip));
        return send(res, 200, edits.start(clip, info, body));
      } catch (e) {
        return fail(res, e);
      }
    }
    if (edits && url.pathname === "/api/edit/jobs") return send(res, 200, { jobs: edits.list(), canChange: !!cleanup?.canChange }, { "cache-control": "no-cache" });
    if (edits && parts[0] === "api" && parts[1] === "edit" && parts[2] === "jobs" && parts[3]) {
      try {
        if (parts[4] === "file") {
          const file = edits.file(parts[3]);
          const ext = path.extname(file).toLowerCase();
          return sendFile(req, res, file, { type: ext === ".mp3" ? "audio/mpeg" : ext === ".mp4" ? "video/mp4" : "video/x-matroska", download: path.basename(file) });
        }
        return send(res, 200, edits.status(parts[3]), { "cache-control": "no-cache" });
      } catch (e) {
        return fail(res, e);
      }
    }

    // ---- pairing a PC (see pairing.js) ----
    if (pairing && url.pathname === "/api/pair" && !post(req)) {
      return send(res, 200, { code: pairing.current(), paired: pairing.paired() }, { "cache-control": "no-cache" });
    }
    if (pairing && url.pathname === "/api/pair/code" && post(req)) return send(res, 200, pairing.newCode());
    if (pairing && url.pathname === "/api/pair" && post(req)) {
      try {
        return send(res, 200, { ...(await pairing.pair(await jsonBody(req))), clips: library.clips.length, version: VERSION });
      } catch (e) {
        return fail(res, e);
      }
    }
    if (pairing && url.pathname === "/api/pair/forget" && post(req)) {
      await pairing.forget((await jsonBody(req)).pc);
      return send(res, 200, { ok: true });
    }

    if (url.pathname === "/healthz") return send(res, 200, { ok: true, version: VERSION, release: RELEASE, clips: library.clips.length });

    if (url.pathname === "/api/clips") {
      // Changes when clips come or go, and as durations / damage checks come in.
      const etag = `"v${library.version}-m${media.metaVersion}-s${store?.version ?? 0}"`;
      if (req.headers["if-none-match"] === etag) {
        res.writeHead(304, { etag });
        return res.end();
      }
      const clips = library.clips.map(withMeta);
      return send(res, 200, { clips, games: gamesOf(library.clips), scannedAt: library.scannedAt, version: VERSION, release: RELEASE, canChange: !!cleanup?.canChange, trashDays: cleanup?.trashDays ?? null }, {
        etag,
        "cache-control": "no-cache",
      });
    }

    if (url.pathname === "/api/storage") return send(res, 200, await storage(library, media), { "cache-control": "no-cache" });

    if (url.pathname === "/api/rescan" && req.method === "POST") {
      await library.rescan();
      return send(res, 200, { clips: library.clips.length });
    }

    // /api/clips/:id/<action>
    if (parts[0] === "api" && parts[1] === "clips" && parts.length === 4) {
      const clip = library.byId.get(parts[2]);
      if (!clip) return send(res, 404, { error: "unknown clip" });
      const file = path.join(library.root, ...clip.rel.split("/"));
      const action = parts[3];
      try {
        if (action === "original") {
          const type = { mp4: "video/mp4", mov: "video/quicktime", webm: "video/webm" }[clip.ext] || "video/x-matroska";
          return sendFile(req, res, file, { type, download: url.searchParams.has("download") ? clip.name : null });
        }
        if (action === "thumb.jpg") {
          // Known to be unreadable: don't start ffmpeg on it for every page view.
          if (media.meta(clip)?.damaged) return send(res, 404, "no thumbnail: the clip can't be read");
          const out = await media.thumbnail(clip, file);
          return sendFile(req, res, out, { type: "image/jpeg", cache: "public, max-age=86400" });
        }
        if (action === "info") {
          const info = await media.probe(clip, file);
          return send(res, 200, { ...info, directPlay: Media.directPlayable(clip, info), web: await media.webStatus(clip) });
        }
        if (action === "web" && req.method === "POST") return send(res, 200, await media.prepareWeb(clip, file));
        if (action === "web") return send(res, 200, await media.webStatus(clip));
        // Share: the phone version uploaded to Catbox / Litterbox. Make the phone version first.
        if (shares && action === "share" && post(req)) {
          const st = await media.webStatus(clip);
          if (st.state !== "ready") return send(res, 409, { error: "Make the phone version first.", web: st });
          return send(res, 200, await shares.share(clip.id, media.webPath(clip), clip.name, url.searchParams.get("host") || "catbox"));
        }
        if (shares && action === "links") return send(res, 200, await shares.list(clip.id), { "cache-control": "no-cache" });
        if (action === "web.mp4") {
          const out = media.webPath(clip);
          if (req.headers.range === "bytes=0-" || !req.headers.range) media.touch(out);
          const base = clip.name.replace(/\.[^.]+$/, "");
          return sendFile(req, res, out, {
            type: "video/mp4",
            download: url.searchParams.has("download") ? `${base} (phone).mp4` : null,
          });
        }
        if (store && action === "keep-whole" && post(req)) {
          await store.keepWhole(clip.rel, url.searchParams.get("on") !== "0");
          return send(res, 200, { ok: true });
        }
        if (cleanup && action === "keep" && post(req)) {
          await cleanup.keep(clip);
          return send(res, 200, { ok: true });
        }
        if (cleanup && action === "trash" && post(req)) {
          const entry = await cleanup.toTrash(clip);
          await library.rescan();
          return send(res, 200, entry);
        }
        if (cleanup && action === "shrink" && post(req)) {
          cleanup.needChange();
          // Make (or wait for) the H.264 copy first; the page polls this until it's done.
          const st = await media.webStatus(clip);
          if (st.state !== "ready") return send(res, 200, st.state === "none" || st.state === "failed" ? await media.prepareWeb(clip, file) : st);
          const out = await cleanup.shrink(clip, media.webPath(clip));
          await library.rescan();
          return send(res, 200, { state: "done", ...out });
        }
      } catch (e) {
        return fail(res, e);
      }
      return send(res, 404, { error: "unknown action" });
    }

    if (req.method !== "GET" && req.method !== "HEAD") return send(res, 405, "method not allowed");

    // Static UI
    const rel = url.pathname === "/" ? "index.html" : decodeURIComponent(url.pathname.slice(1));
    const file = path.join(PUBLIC, rel);
    if (!file.startsWith(PUBLIC + path.sep)) return send(res, 403, "forbidden");
    try {
      const body = await readFile(file);
      return send(res, 200, body, { "content-type": TYPES[path.extname(file)] || "application/octet-stream", "cache-control": "no-cache" });
    } catch {
      return send(res, 404, "not found");
    }
  };

  return http.createServer((req, res) => {
    route(req, res).catch((e) => {
      console.error(e);
      if (!res.headersSent) send(res, 500, "server error");
      else res.destroy();
    });
  });
}
