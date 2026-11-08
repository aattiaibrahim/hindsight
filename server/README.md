# Hindsight Server (Hindsight on the NAS)

Renamed from Rewind Server. The image is `ghcr.io/aattiaibrahim/hindsight-server`. The old
`rewind-server` image is no longer published: if your install still uses it, change the image to
`ghcr.io/aattiaibrahim/hindsight-server:latest` (your clips and settings stay where they are).

A small web app that runs on your NAS and lets you browse, watch, edit and tidy your Hindsight clips
from any browser on the local network, including on your phone.

Capture still happens on the gaming PC. The desktop app saves each clip locally, then copies it
to the NAS folder. Hindsight Server reads that folder:

```
<clips dataset>/
  VALORANT/
    VALORANT 2026-11-03 21-14-03 - GAMING-PC.mkv
    VALORANT 2026-11-03 21-14-03 - GAMING-PC (2).mkv
    .VALORANT 2026-11-03 22-00-00 - GAMING-PC.mkv.rewind-partial   <- still copying, ignored
  Apex Legends/
    Apex Legends Recording 2026-11-03 20-01-10 - GAMING-PC.mkv
  .hindsight-trash/                                                 <- cleanup's trash, hidden
  .hindsight-replaced/2026-11-07/VALORANT/…                         <- originals an edit replaced, hidden
```

**The look**
- The same as the desktop app: a black frame with soft dark panels, a slim icon rail on the left
  (labels on hover), and a bottom tab bar on phones. The main action is a white pill. Your colour
  (Settings) is only used for selection, progress and focus.

**Home**
- One row per game, newest game first: its logo, name, clip count and **See all**, then a strip of
  clip cards you scroll sideways. Each card shows when the clip was taken and which PC it came from
  (from the " - GAMING-PC" part of the file name).
- **See all** opens the game's page: every clip grouped by day, with the date timeline down the
  right side, a sort (newest, oldest, biggest, longest) and a PC filter.
- Search or a filter shows the matching clips by day instead.
- **Game logos** come from Steam, looked up by the folder's name the same way the desktop app does
  (a strict name match, so "Edits" or "Minecraft Dungeons" never get the wrong logo). Each logo is
  downloaded once and kept in the cache; a game Steam doesn't know is asked again after a week, and
  shows a gamepad meanwhile. Only folder names are sent. `HINDSIGHT_ONLINE_LOGOS=0` turns this off.

**To trim**
- Clips nobody has trimmed or edited yet, like the desktop's list. Left out: the Edits, exports and
  Montages folders, files named like an edit (" (trim)", " (H.264)", " (25 MB)" and so on), files
  the NAS editor made, and clips you marked **Keep whole**.

**Notifications**
- One row each, with an action and a hide button: damaged clips, a game whose clips sit in folders
  spelled differently (**Merge** moves them into one folder), duplicate copies of a clip (the extra
  copies go to the trash), and new clips from your PCs since you last looked. A hidden row comes
  back when what it's about changes.

**Editor** (the same edits as the desktop's Studio, made on the NAS with the CPU)
- A player, a timeline with start and end handles (drag them, or use the arrow keys), and one lane
  per audio track with its waveform, a volume slider and a mute button. Drag across a lane to mute
  that stretch; tap it to unmute.
- **Save** trims exactly: only the frames before the first keyframe are re-encoded (libx264), the
  rest is copied untouched. A cut that starts on a keyframe is a plain copy. Sound-only changes copy
  the video. Files that don't suit this (HEVC or AV1 with a start between keyframes, variable frame
  rate) are re-encoded in full.
- **Export:** an H.264 MP4 that plays everywhere, an MP3 (all tracks mixed, or one), compress to a
  size in MB (two passes with libx264), and **Repair** for damaged clips (decodes past the damage,
  then re-encodes).
- Outputs are saved next to the original with the desktop's names, for example
  `VALORANT 2026-11-03 21-14-03 - GAMING-PC (trim).mkv`. Edits run one at a time, with progress.
- **Replace the original** works for every clip. The replaced file moves to
  `.hindsight-replaced/<date>/` for 30 days, like the desktop's NAS sync. Each replace is noted in
  `.hindsight-edits.jsonl` in the clips folder, and the PC the clip came from swaps in the edit on
  its next sync: it checks the copy byte for byte and sends its old copy to the Recycle Bin. Edits
  made on a PC replace the NAS copy the same way, so an edit shows up everywhere.
- **Pairing:** Settings > PCs that sync here > Pair a PC shows a 6-digit code. A PC only syncs into
  the library once it's paired (or once someone confirms the folder on the PC). The PC proves its
  folder is this server's clips folder with a one-time file it writes there; the server never writes
  into the clips folder for this.
- Without `ALLOW_CHANGES=1` the editor still works, but its outputs stay in the cache to download.

**Clips**
- **Filters:** PC, the recorder it was made with (Hindsight, Medal, NVIDIA), clips or recordings,
  length, date range, and damaged only. Filters live in the address, so a filtered view can be
  bookmarked.
- A clip that ffprobe can't read is marked **Damaged**. Lengths and codecs are read in the
  background and kept in the cache.
- Folders that differ only by case or spacing ("VALORANT" and "Valorant") show as one game.
- Also shows clips from other recorders (Medal, NVIDIA, `.mov`/`.webm`), using the date in the
  file name when there is one.
- **Playback:** H.264 MP4 clips play directly. For MKV, HEVC or AV1 clips, the browser tries the
  original first. If that fails (for example on an iPhone), the server makes an H.264 MP4 copy
  once, stores it in the cache and plays that. The copy mixes the desktop and mic tracks.
- **Downloads:** the original file, and the phone version once it exists.
- **Share:** in the player, **Share link** uploads the phone version to Catbox (kept, up to 200 MB)
  or Litterbox (72 hours, up to 1 GB) and shows a link that plays inside Discord and on phones.
  Links you've made are shown again the next time you open the clip. Nothing is uploaded unless
  you press Share.
- **On this day:** clips from this date in earlier years ("1 year ago today") at the top of the
  library; tap one to watch them through with Previous / Next.

**Montage**
- Pick a length (30 s to 10 min), which clips (random, a date range, edited or unedited ones), a
  game, and how much of each clip (the end, where a replay's action usually is, the middle or the
  start). Then reorder, trim (in / out) or remove clips, add a song (uploaded from your phone or
  PC) and set the clip and song volumes.
- The montage is made on the NAS with ffmpeg (every clip scaled to one size, the song looped and
  faded out at the end) and kept in the cache for a day to download. The clips folder isn't touched.

**Settings**
- Your colour (the desktop app's schemes, saved per browser), the space your clips use per game,
  the dataset's used and free space, and the cache size.

**Cleanup** (needs `ALLOW_CHANGES=1`, see below)
- Lists the biggest clips. For each one:
  - **Delete** moves it to a hidden trash on the NAS for 30 days (`TRASH_DAYS`).
  - **Shrink** swaps it for the smaller H.264 copy, with the same name and date. The original goes
    to the trash.
  - **Keep** hides it until you press **Run cleanup check** again.
- **Trash** lists what's waiting to be removed, with **Restore**, and **Empty trash** to remove
  everything now.
- Hindsight's NAS sync remembers what it already copied, so a clip deleted here isn't copied back.

Without `ALLOW_CHANGES=1`, the server never writes to the clips folder. Cleanup then only lists the
biggest clips, Keep still works, and editor outputs are kept in the cache to download.

Port: **8420**. No database and no accounts.

## Install on TrueNAS SCALE

These steps are for TrueNAS SCALE 24.10 (Electric Eel) or later, where apps run on Docker. They
were tested on 25.10 (Goldeye).

The image is published at **`ghcr.io/aattiaibrahim/hindsight-server`** (amd64 and arm64), with ffmpeg
included. TrueNAS downloads it once. After that, the app starts and runs with no internet.

### 1. Clips dataset and user

Use the dataset your SMB share points at, the one Hindsight copies to, for example
`/mnt/tank/clips`. The container runs as your SMB user and the `builtin_users` group (gid 545),
which can read and change SMB-share datasets by default, so no permission changes are needed.
Find your uid under Credentials → Users (the first user you create is usually 3000).

### 2. Install with YAML

**Apps → Discover Apps → ⋮ (next to Custom App) → Install via YAML**. Name it `hindsight-server` and
paste this, with your own path, uid and time zone:

```yaml
services:
  hindsight-server:
    image: ghcr.io/aattiaibrahim/hindsight-server:0.4.0
    restart: unless-stopped
    user: "3000:545"
    ports:
      - "8420:8420"
    environment:
      TZ: America/Los_Angeles
      MAX_CACHE_GB: "20"
      ALLOW_CHANGES: "1"      # cleanup and the editor may change clips (with a way back)
    volumes:
      - /mnt/tank/clips:/clips    # add :ro and drop ALLOW_CHANGES for a strictly read-only server
      - hindsight-cache:/cache
volumes:
  hindsight-cache: {}
```

If the app shows **Stopped** after saving, press Start (▶) in the Controls column. Then open
`http://<nas-ip>:8420`. On a phone, use "Add to Home Screen" to open it like an app.

### Or with the Custom App form

**Apps → Discover Apps → Custom App**:

- Image repository `ghcr.io/aattiaibrahim/hindsight-server`, tag `0.4.0`
- User `3000`, group `545`
- Port: container **8420** → host **8420**, TCP
- Storage: host path `/mnt/tank/clips` → `/clips` (tick **read-only** for a read-only server); an
  ixVolume → `/cache`
- Environment: `TZ`, `ALLOW_CHANGES=1` for cleanup, plus any settings from the table below

### Cache

Thumbnails, phone versions, waveforms, game logos and the background clip info go in the `/cache`
volume, and so does `hindsight-state.json` (Keep whole, which files the editor made, hidden
notifications). Phone versions are capped at `MAX_CACHE_GB` (default 20 GB), and the copies watched
least recently are removed first.

### Updating

The page shows the running version under Settings → About and when you hover over the logo, and so does
`http://<nas-ip>:8420/healthz` (`release` and the Git commit).

Each release is tagged with its number, for example `0.2.0`, on the
[package page](https://github.com/aattiaibrahim/hindsight/pkgs/container/hindsight-server). Every
change in between also gets `latest` and `sha-<commit>`.

TrueNAS only downloads an image it doesn't already have, so it won't fetch a newer `latest` on its
own. To update, open the app's **Edit**, change the image tag to the new release and save. TrueNAS
downloads that image and restarts the app; the cache is kept. Going back is the same: put the old
tag back.

## Settings

All settings are environment variables.

| Variable | Default | |
|----------|---------|--|
| `CLIPS_DIR` | `/clips` | Clips folder inside the container |
| `CACHE_DIR` | `/cache` | Thumbnails, phone versions, clip info |
| `PORT` | `8420` | HTTP port |
| `TZ` | UTC | Time zone. Only used for files whose names have no timestamp |
| `ALLOW_CHANGES` | off | `1` lets cleanup delete (to the trash), shrink and restore, notifications merge folders, and the editor save next to clips. Needs a writable clips folder |
| `TRASH_DAYS` | `30` | How long deleted clips stay in the trash |
| `RESCAN_SECONDS` | `60` | Full rescan interval, on top of watching the folder. `0` turns it off |
| `MAX_CACHE_GB` | `20` | Cap for phone versions. `0` means no cap |
| `TRANSCODE_JOBS` | `1` | How many conversions run at once |
| `X264_PRESET` | `veryfast` | Use `ultrafast` on slow CPUs, or `medium` for smaller files |
| `H264_ENCODER` | `libx264` | ffmpeg encoder for phone versions and the editor. Two-pass compress needs libx264 |
| `HINDSIGHT_ONLINE_LOGOS` | on | `0` stops looking game logos up on Steam |

A 1-minute 1080p60 clip takes roughly 20 to 60 seconds to convert on a typical NAS CPU. The player,
Shrink and the editor show progress while it works. Each clip is only converted once. A trim is much
quicker, since only the start is re-encoded.

## Security

There's no login. Anyone who can reach port 8420 can watch and download your clips, and with
`ALLOW_CHANGES=1` also delete them (to the trash). Keep it on your home network, and don't forward
the port on your router. Share links are the exception you choose: a shared clip is public to
anyone with its link. To watch clips away from home, use a VPN such as Tailscale or WireGuard
(both are available as TrueNAS apps).

## Run it locally

You need Docker, or Node 22+ and ffmpeg.

```bash
cd server
node scripts/make-samples.js sample-clips   # a few short test clips with silent audio
docker compose up --build                   # http://localhost:8420
```

Or without Docker:

```bash
CLIPS_DIR=sample-clips CACHE_DIR=.cache ALLOW_CHANGES=1 node src/main.js
```

## Tests

```bash
cd server
npm test
```

The tests cover file-name parsing, grouping, filters, sorting, folders, search, skipping hidden and
partial files, cleanup (Keep, the trash, restore, expiry, Shrink, read-only mode), storage, the
HTTP API (byte ranges, downloads), the To trim and replace rules, the Steam name match and the
editor's audio filters. If ffmpeg is on the PATH, they also make short test clips (one with tone
tracks, never played) and check the thumbnail, the H.264 phone version, the Damaged check, keyframe
and exact trims, mutes, saving over a NAS-only clip, and the H.264, MP3 and compress exports.
Without ffmpeg, those tests are skipped. With an ffmpeg that has no libx264, set
`H264_ENCODER=libopenh264`.

## How it's built

- `src/library.js`: scans the folder, parses file names, skips hidden and `*.rewind-partial` files.
- `src/state.js`: keeps the list in memory, watches the folder and rescans.
- `src/media.js`: ffprobe, thumbnails, the H.264 conversion and the background clip info.
- `src/cleanup.js`: the biggest-clips list, Keep, the trash, Shrink and the duplicate scan.
- `src/edit.js`: the editor (trim, audio, exports, repair, waveforms) and its job queue.
- `src/logos.js`: game logos from Steam, like the desktop's `game_icons.rs`.
- `src/notices.js`: the notifications and their actions.
- `src/store.js`: Keep whole, files the editor made, hidden notifications (`hindsight-state.json`).
- `src/server.js`: the JSON API and file serving, with byte ranges so seeking works.
- `public/`: the web page (plain JavaScript, no build step). `app.js` is the library, `editor.js`
  the editor, `ui.js` shared helpers. `group.js` (grouping, filters, sorting) is shared with the
  tests. Icons: Lucide (`icons.js`, ISC licence). Fonts: Geist, under the SIL Open Font License
  (`public/fonts/OFL.txt`).

There are no npm dependencies.
