// Hindsight Server: serves a Hindsight clip library over the local network.
//
// Environment:
//   CLIPS_DIR      clips folder                        default /clips
//   ALLOW_CHANGES  1 = cleanup may trash/shrink files  default off (read-only)
//   TRASH_DAYS     days deleted clips stay in trash    default 30
//   CACHE_DIR      thumbnails and phone copies         default /cache
//   PORT           HTTP port                           default 8420
//   RESCAN_SECONDS full rescan interval, 0 = off       default 60
//   MAX_CACHE_GB   cap for converted copies, 0 = none  default 20
//   TRANSCODE_JOBS conversions at once                 default 1
//   H264_ENCODER   ffmpeg encoder                      default libx264
//   HINDSIGHT_ONLINE_LOGOS  0 = no game logos from Steam   default on

import path from "node:path";
import { Library } from "./state.js";
import { Cleanup } from "./cleanup.js";
import { Media } from "./media.js";
import { createServer, VERSION } from "./server.js";
import { Montages } from "./montage.js";
import { Shares } from "./share.js";
import { Edits } from "./edit.js";
import { Logos } from "./logos.js";
import { Store } from "./store.js";
import { Pairing } from "./pairing.js";
import { MixTracks } from "./mixtracks.js";

const env = process.env;
const clipsDir = env.CLIPS_DIR || "/clips";
const cacheDir = env.CACHE_DIR || "/cache";
const port = Number(env.PORT || 8420);

const library = new Library(clipsDir, { rescanSeconds: Number(env.RESCAN_SECONDS ?? 60) });
const media = new Media({
  cacheDir,
  transcodeJobs: Number(env.TRANSCODE_JOBS || 1),
  maxCacheBytes: Number(env.MAX_CACHE_GB ?? 20) * 1024 ** 3,
});

await media.init();
const cleanup = new Cleanup({
  root: clipsDir,
  cacheDir,
  allow: env.ALLOW_CHANGES === "1" || env.ALLOW_CHANGES === "true",
  trashDays: Number(env.TRASH_DAYS || 30),
});
await cleanup.init();
console.log(cleanup.canChange ? `Cleanup can change files (trash kept ${cleanup.trashDays} days)` : "Read-only: cleanup only lists the biggest clips");
// Trash older than TRASH_DAYS goes for good, checked every 6 hours.
setInterval(() => cleanup.canChange && cleanup.purge().catch((e) => console.error("trash purge:", e.message)), 6 * 3600 * 1000).unref();
const fileOf = (clip) => path.join(clipsDir, ...clip.rel.split("/"));
library.onChange = (clips) => media.scanMeta(clips, fileOf);
await library.start();
const store = new Store(cacheDir);
await store.init();
const logos = new Logos({ dir: path.join(cacheDir, "logos"), online: env.HINDSIGHT_ONLINE_LOGOS !== "0" && env.HINDSIGHT_ONLINE_LOGOS !== "false" });
const edits = new Edits({
  root: clipsDir,
  cacheDir,
  store,
  canChange: () => cleanup.canChange,
  encoder: env.H264_ENCODER || "libx264",
  onSaved: () => library.rescan(),
});
await edits.init();
const shares = new Shares(cacheDir);
const montages = new Montages(cacheDir, { encoder: env.H264_ENCODER || "libx264" });
const pairing = new Pairing({ root: clipsDir, store });
const mixTracks = new MixTracks({ library, cleanup });
const server = await createServer({ library, media, cleanup, shares, montages, store, logos, edits, pairing, mixTracks });
server.listen(port, () => {
  console.log(`Hindsight Server ${VERSION}: ${library.clips.length} clips in ${clipsDir}, listening on :${port}`);
});

const stop = () => {
  library.stop();
  server.close(() => process.exit(0));
  setTimeout(() => process.exit(0), 2000).unref();
};
process.on("SIGTERM", stop);
process.on("SIGINT", stop);
