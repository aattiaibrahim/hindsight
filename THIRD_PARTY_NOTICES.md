# Third-party notices

## FFmpeg

Rewind's installer includes FFmpeg (`ffmpeg.exe`, `ffprobe.exe` and its shared libraries), which Rewind runs as a separate program for capture, encoding and editing.

- FFmpeg is licensed under the **GNU Lesser General Public License v2.1 or later** (LGPL). The build used is an LGPL-only build, with no GPL or non-free components.
- Build: `ffmpeg-master-latest-win64-lgpl-shared` from https://github.com/BtbN/FFmpeg-Builds (`scripts/get-ffmpeg.ps1` fetches the exact package).
- Source code: https://ffmpeg.org/download.html and https://github.com/BtbN/FFmpeg-Builds
- The full license text ships with the app as `ffmpeg/LICENSE.txt` in the install folder.

FFmpeg is a trademark of Fabrice Bellard, originator of the FFmpeg project. Rewind is not affiliated with or endorsed by the FFmpeg project.

## NVIDIA Video Codec SDK header (nvEncodeAPI.h)

`src-tauri/nvenc/nvEncodeAPI.h` (from FFmpeg's nv-codec-headers) and the Rust bindings generated from it (`src-tauri/src/native/nvenc_sys.rs`) are © NVIDIA Corporation under the MIT license, as stated at the top of the header. Rewind loads NVIDIA's encoder from the installed graphics driver (`nvEncodeAPI64.dll`) and does not redistribute it.

## PresentMon

Rewind's installer includes Intel's PresentMon 2.6.0 (`presentmon/PresentMon.exe`), which the Performance test runs as a separate program to measure a game's frame rate.

- PresentMon is © Intel Corporation under the **MIT license**. The license text ships with the app as `presentmon/LICENSE.txt`.
- Source and releases: https://github.com/GameTechDev/PresentMon (`scripts/get-ffmpeg.ps1` fetches the exact release).

## Archivo (website headings)

The website's headings use Archivo by Omnibus-Type, under the SIL Open Font License 1.1:
https://fonts.google.com/specimen/Archivo/license

## Rust and JavaScript dependencies

Rewind is built with Tauri, React, cpal, the `windows` crate and other open-source libraries under MIT, Apache-2.0 and similar licenses. See `src-tauri/Cargo.lock` and `pnpm-lock.yaml` for the full list.

## Game logos from Steam

With "Online game logos" on, game icons are fetched from Steam's public store and community APIs. They are the
property of their respective owners and Valve Corporation, shown only to label your own clips. Hindsight is not
affiliated with Valve or Steam.

## ONNX Runtime (optional download)

Used by "Find the game in unsorted clips" and the camera's background blur. `onnxruntime.dll` 1.31.1, Copyright (c) Microsoft Corporation,
MIT License: https://github.com/microsoft/onnxruntime/blob/main/LICENSE

## CLIP ViT-B/32 image model (optional download)

Used by "Find the game in unsorted clips". The vision half of OpenAI's CLIP ViT-B/32 (MIT License,
https://github.com/openai/CLIP/blob/main/LICENSE), as the quantized ONNX export published by Xenova
(https://huggingface.co/Xenova/clip-vit-base-patch32).

## MediaPipe Selfie Segmentation model (optional download)

Used by the camera's blurred or removed background. Google's MediaPipe selfie segmentation (landscape)
model, Copyright Google LLC, Apache License 2.0 (https://www.apache.org/licenses/LICENSE-2.0), as the ONNX
export published by onnx-community (https://huggingface.co/onnx-community/mediapipe_selfie_segmentation_landscape).
