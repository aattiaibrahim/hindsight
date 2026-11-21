# Hindsight on Linux, macOS and ARM: the plan

Hindsight is a Tauri 2 app, and Tauri itself runs on Windows, macOS and Linux, on x64
and ARM64. The interface (React) already works everywhere. What doesn't is the Rust
side: 25 of its 40 files call Windows directly. This plan lists every one of those
pieces, what replaces it on each system, and the order to do it in, so each phase ends
in something people can use.

## Where things stand (0.1.77)

| | Windows x64 | Windows ARM64 | Linux x64 / ARM64 | macOS (Apple Silicon and Intel) |
| --- | --- | --- | --- | --- |
| Builds and passes its tests | Yes | Phase 0 | Phase 0 | Phase 0 |
| Library, Studio, Cleanup, NAS, sharing | Yes | Phase 1 | Phase 2 | Phase 2 |
| Recording | Yes (NVIDIA, AMD, Intel, any) | Phase 1 | Phase 3 | Phase 3 |
| Discord on its own track, game detection | Yes | Phase 1 | Phase 4 | Phase 4 |
| Installer and auto-update | Yes | Phase 1 | Phase 5 | Phase 5 |

## Every Windows-only piece, and what replaces it

| What | File(s) | Windows | Linux | macOS |
| --- | --- | --- | --- | --- |
| Screen capture (native) | `native/capture.rs`, `gpu.rs` | Windows Graphics Capture + D3D11 | PipeWire screencast through the xdg-desktop-portal (Wayland and X11) | ScreenCaptureKit (macOS 12.3+) |
| Screen capture (FFmpeg) | `engine.rs` | `ddagrab` | `x11grab` on X11; Wayland needs the portal | `avfoundation` (video only) |
| Video encoder | `encoders.rs`, `native/nvenc.rs` | NVENC, AMF, Quick Sync, Media Foundation | NVENC (NVIDIA driver), VAAPI (AMD, Intel) | VideoToolbox (every Mac) |
| Desktop audio | `audio.rs` (cpal loopback) | WASAPI loopback | PipeWire / PulseAudio monitor source | ScreenCaptureKit audio (macOS 13+) |
| Discord on its own track | `app_audio.rs` | Process loopback by PID | PipeWire: link Discord's stream to its own capture | ScreenCaptureKit filter: Discord's app only / everything but Discord |
| Microphone | `audio.rs` (cpal) | WASAPI | Works as is (ALSA / PipeWire) | Works as is (Core Audio), needs microphone permission |
| Sound outputs list (diagnostics) | `audio_sessions.rs` | MMDevice sessions | PipeWire nodes (`pw-dump`) | Core Audio devices |
| Which monitor, previews | `monitors.rs` | DXGI outputs, device IDs | Portal picks the screen; X11 RandR | `CGDisplay` |
| Game detection | `games.rs` | Foreground window, full screen, process path | `/proc` + Steam's `SteamLaunch AppId=`; X11 active window (Wayland doesn't say which window is in front) | `NSWorkspace.frontmostApplication` |
| Program icons for logos | `game_icons.rs` | Shell image factory | `.desktop` file icon from the icon theme | `NSWorkspace.icon(forFile:)` |
| Hotkeys | `tauri-plugin-global-shortcut` | Works | X11 works; Wayland needs the portal's GlobalShortcuts (KDE, GNOME 48+) | Works |
| Recycle Bin | `trash` crate | Works | Works (freedesktop trash) | Works (Finder trash) |
| NAS connection and login | `nas_connect.rs`, `nas.rs` | `\\server\share`, WNet, Credential Manager | A mounted share (`gvfs` or `cifs`), password in the Secret Service (`keyring` crate) | `/Volumes/share` via `mount_smbfs`, password in the Keychain |
| Saved secrets | `secrets.rs` | DPAPI | Secret Service (`keyring`) | Keychain (`keyring`) |
| Copy a file to the clipboard | `upload.rs` | `CF_HDROP` | `text/uri-list` | `NSPasteboard` file URL |
| Clip-saved sound | `sound.rs` | `PlaySound` | Play the WAV through cpal | Play the WAV through cpal |
| FFmpeg dies with the app | `ffmpeg.rs` | Job object | `prctl(PR_SET_PDEATHSIG)` | A watcher on the parent (kqueue) |
| Free disk space | `ffmpeg.rs` | `GetDiskFreeSpaceExW` | `statvfs` | `statvfs` |
| Keep a file's date | `library.rs` | `FileTimesExt` (created + modified) | Modified only (Linux has no settable creation date) | Created + modified |
| Shutting down / signing out | `session_end.rs` | `WM_QUERYENDSESSION` | logind `PrepareForShutdown` over D-Bus | `NSWorkspaceWillPowerOffNotification` |
| Where other recorders keep clips | `bench/recorders.rs`, `drives.rs` | Registry, `%APPDATA%` | OBS's `~/.config/obs-studio`, `~/Videos` | OBS's `~/Library/Application Support/obs-studio`, `~/Movies` |
| Performance test | `bench/` | PresentMon | Hidden (MangoHud logs later) | Hidden |
| Game finder model | `game_detect.rs` | onnxruntime x64 DLL | onnxruntime `.so` (x64, ARM64) | onnxruntime `.dylib` (universal) |
| Bundled tools | `vendor/` | FFmpeg, ffprobe, PresentMon (x64) | FFmpeg static builds (linux64, linuxarm64) | FFmpeg built with VideoToolbox (arm64, x86_64) |

## Phases

### Phase 0: the code builds and tests pass everywhere (this change)

- The `windows` crates are only pulled in on Windows. Each Windows-only module gets a
  stand-in on other systems with the same functions, which say the feature isn't
  available. The Windows code itself doesn't change.
- The test workflow builds and tests on Ubuntu (x64), macOS (Apple Silicon) and
  Windows ARM64 (built, not run: GitHub's ARM64 Windows machines are a paid add-on).
- Nothing new is released. Windows x64 behaves exactly as before.

### Phase 1: Windows on ARM (Snapdragon laptops)

The smallest step to real users. Almost everything already works: the only gaps are
the bundled tools and the GPU encoder.

- Build `aarch64-pc-windows-msvc`, ship FFmpeg's `winarm64` build and onnxruntime ARM64.
- Recording: Qualcomm's encoder through Media Foundation, which `encoders.rs` already
  supports (the native engine is NVIDIA-only). PresentMon runs under x64 emulation.
- The updater needs a `windows-aarch64` entry in `latest.json`.
- Risk: no ARM64 Windows PC to test on. A tester, or a Windows Dev Kit, is needed before release.

### Phase 2: the library on Linux and macOS (no recording)

Useful for people who record elsewhere, or keep their clips on a NAS.

- Library, Studio, Cleanup, the game finder, Montage, sharing, the NAS as a mounted folder.
- Port the logic that assumes Windows paths (drive letters, `\`), which 18 tests marked
  `#[cfg(windows)]` cover today: NAS sync and its relative paths (`nas.rs`, 9 tests), sorting
  clips into game folders (`game_detect.rs`), game names from program paths (`games.rs`),
  recorder folder labels (`drives.rs`), hiding the user name in diagnostics (`diag.rs`) and
  old recorder folders (`old_folders.rs`). Each test runs on all three systems once ported.
- `encoders.rs` learns VAAPI and VideoToolbox for edits.
- Bundle FFmpeg per system and architecture.
- Paths: `~/.local/share/hindsight` (Linux), `~/Library/Application Support/Hindsight` (macOS).

### Phase 3: recording

- macOS first: ScreenCaptureKit gives the screen and the system sound together, on the
  GPU, with Screen Recording permission. Encode with VideoToolbox.
- Linux X11: `x11grab` + the PulseAudio/PipeWire monitor source, through the FFmpeg engine.
- Linux Wayland (most new installs): the screencast portal asks once which screen,
  then PipeWire hands over frames. This is the hardest part: no FFmpeg input does it
  today, so it's a Rust capture (`ashpd` + `pipewire` crates) feeding FFmpeg.

### Phase 4: the extras

Discord on its own track, game detection and per-game folders, hotkeys on Wayland,
saved NAS logins, safe shutdown, other recorders' folders.

### Phase 5: installers and auto-update

- Linux: AppImage (auto-updates) and `.deb`; ARM64 builds of both.
- macOS: a universal `.dmg`. **It must be signed and notarized with an Apple
  Developer ID (99 USD a year)**, or macOS refuses to open it without a trip to
  System Settings. This is the owner's decision.
- `latest.json` gets `linux-x86_64`, `linux-aarch64`, `darwin-aarch64`, `darwin-x86_64`.

## Risks

- **No Linux, Mac or ARM PC to test on here.** GitHub's machines can build and run the
  tests, but not capture a screen or play a game. Each phase that records needs a person
  with that system before it's released.
- **Wayland** limits on purpose what apps can see: which window is in front (game
  detection), global hotkeys and screen capture all need the user's permission through
  portals, and support differs between GNOME, KDE and others.
- **macOS permissions**: Screen Recording and Microphone are asked for once, and
  without notarization even that first launch is awkward.
- **Anti-cheat**: like on Windows, capture never touches the game's process, so this is
  no different on other systems.

## Rough size

| Phase | Work |
| --- | --- |
| 0 | A day: stand-ins and CI |
| 1 | A few days, plus a tester |
| 2 | One to two weeks |
| 3 | Two to four weeks (Wayland is most of it) |
| 4 | One to two weeks |
| 5 | A few days, plus the Apple account |
