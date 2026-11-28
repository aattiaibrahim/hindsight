//! Finding you in the camera picture, for its blurred or removed background.
//!
//! Google's MediaPipe selfie segmentation (landscape, Apache-2.0, the ONNX build from
//! huggingface.co/onnx-community, 462 KB) run by the same ONNX Runtime the game finder
//! uses, on the CPU: about 256×144 pixels in, a 0-1 "this is you" value per pixel out,
//! a few milliseconds a frame. Nothing leaves the PC.
//!
//! The model (and ONNX Runtime, if the game finder hasn't fetched it already) are
//! downloaded the first time a background effect is turned on, each checked against
//! its SHA-256.

use ort::session::Session;
use ort::value::Tensor;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const W: usize = 256;
pub const H: usize = 144;
const MODEL: (&str, &str, u64) = ("selfie-seg-landscape.onnx", "7a0adcfdb1715d3b0ff0f61486d8a181c33f4a208343a5abfccbc6540e872d24", 462_338);
/// A fixed revision, so the file never changes under us.
const MODEL_URL: &str =
    "https://huggingface.co/onnx-community/mediapipe_selfie_segmentation_landscape/resolve/2497d5bec26c626c7b3c4edc6e1fefc21b64f6c3/onnx/model.onnx";

pub struct Segmenter {
    session: Session,
    /// The last mask, blended with each new one so edges don't flicker.
    last: Option<Vec<f32>>,
}

fn files() -> Option<(PathBuf, PathBuf)> {
    let d = crate::game_detect::dir()?;
    Some((d.join(crate::game_detect::PACK[0].0), d.join(MODEL.0)))
}

fn ok(p: &Path, size: u64) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.len() == size)
}

/// Get the files if they're missing (about 0.5 MB, or 17 MB more without ONNX Runtime).
pub fn install() -> Result<(), String> {
    let (runtime, model) = files().ok_or("not ready")?;
    let rt = crate::game_detect::PACK[0];
    if !ok(&runtime, rt.2) {
        fetch(&format!("{}{}", crate::game_detect::PACK_URL, rt.0), &runtime, rt.1)?;
    }
    if !ok(&model, MODEL.2) {
        fetch(MODEL_URL, &model, MODEL.1)?;
    }
    Ok(())
}

fn fetch(url: &str, out: &Path, hash: &str) -> Result<(), String> {
    use std::io::Write;
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(300)).build().map_err(|e| e.to_string())?;
    let res = client.get(url).send().map_err(|e| format!("Couldn't download the background model: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("Couldn't download the background model: {}", res.status()));
    }
    let bytes = res.bytes().map_err(|e| e.to_string())?;
    let tmp = out.with_extension("part");
    std::fs::File::create(&tmp).and_then(|mut f| f.write_all(&bytes)).map_err(|e| e.to_string())?;
    if crate::game_detect::sha256_file(&tmp).as_deref() != Some(hash) {
        let _ = std::fs::remove_file(&tmp);
        return Err("The background model didn't download correctly. Try again.".into());
    }
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())
}

impl Segmenter {
    /// Loads it, downloading it first if needed.
    pub fn new() -> Result<Segmenter, String> {
        install()?;
        let (runtime, model) = files().ok_or("not ready")?;
        Self::load(&runtime, &model)
    }

    pub fn load(runtime: &Path, model: &Path) -> Result<Segmenter, String> {
        // Already set up by the game finder: that's fine.
        let _ = ort::init_from(runtime.to_string_lossy().to_string()).commit();
        let session = Session::builder()
            .and_then(|b| b.with_intra_threads(2))
            .and_then(|b| b.commit_from_file(model))
            .map_err(|e| format!("The background model wouldn't start: {e}"))?;
        Ok(Segmenter { session, last: None })
    }

    /// "This is you" (0-1) for each of W × H points over the picture (`bgra`, `w` × `h`).
    pub fn mask(&mut self, bgra: &[u8], w: u32, h: u32) -> Result<Vec<f32>, String> {
        let mut data = vec![0f32; 3 * W * H];
        for y in 0..H {
            let sy = ((y as f64 + 0.5) * h as f64 / H as f64) as usize;
            for x in 0..W {
                let sx = ((x as f64 + 0.5) * w as f64 / W as f64) as usize;
                let i = (sy.min(h as usize - 1) * w as usize + sx.min(w as usize - 1)) * 4;
                for (c, from) in [2, 1, 0].into_iter().enumerate() {
                    data[c * W * H + y * W + x] = bgra[i + from] as f32 / 255.0;
                }
            }
        }
        let input = Tensor::from_array(([1usize, 3, H, W], data)).map_err(|e| e.to_string())?;
        let outputs = self.session.run(ort::inputs!["pixel_values" => input]).map_err(|e| e.to_string())?;
        let (_, values) = outputs["alphas"].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        let mut m: Vec<f32> = values.iter().map(|v| v.clamp(0.0, 1.0)).collect();
        if m.len() != W * H {
            return Err("The background model gave an odd answer.".into());
        }
        if let Some(last) = &self.last {
            for (v, l) in m.iter_mut().zip(last) {
                *v = *v * 0.6 + *l * 0.4;
            }
        }
        self.last = Some(m.clone());
        Ok(m)
    }
}

/// The mask's value at a point of the picture (fractions 0-1), smoothly between its points.
pub fn at(mask: &[f32], fx: f64, fy: f64) -> f32 {
    let x = (fx * W as f64 - 0.5).clamp(0.0, (W - 1) as f64);
    let y = (fy * H as f64 - 0.5).clamp(0.0, (H - 1) as f64);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(W - 1), (y0 + 1).min(H - 1));
    let (tx, ty) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let top = mask[y0 * W + x0] * (1.0 - tx) + mask[y0 * W + x1] * tx;
    let bottom = mask[y1 * W + x0] * (1.0 - tx) + mask[y1 * W + x1] * tx;
    top * (1.0 - ty) + bottom * ty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_lookup_is_smooth() {
        let mut m = vec![0f32; W * H];
        for y in 0..H {
            for x in W / 2..W {
                m[y * W + x] = 1.0;
            }
        }
        assert_eq!(at(&m, 0.1, 0.5), 0.0);
        assert_eq!(at(&m, 0.9, 0.5), 1.0);
        let mid = at(&m, 0.5, 0.5);
        assert!(mid > 0.2 && mid < 0.8, "{mid}");
    }

    /// The real model on a made-up picture (SEG_MODEL=<model.onnx>; uses the game finder's ONNX Runtime).
    #[test]
    #[ignore]
    fn real_model_runs() {
        let Ok(model) = std::env::var("SEG_MODEL") else { return };
        let rt = std::path::PathBuf::from(std::env::var("APPDATA").unwrap()).join("com.rewind.app/ai/onnxruntime.dll");
        let mut s = Segmenter::load(&rt, Path::new(&model)).unwrap();
        let pic = vec![90u8; 1280 * 720 * 4];
        let t = std::time::Instant::now();
        for _ in 0..10 {
            s.mask(&pic, 1280, 720).unwrap();
        }
        eprintln!("10 masks in {:?}", t.elapsed());
    }
}
