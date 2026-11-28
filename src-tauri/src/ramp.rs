//! Speed ramps at the joins: instead of a hard cut, the end of one part speeds up
//! into the join and the start of the next slows back down from it, along a smooth
//! (bell-shaped) curve. Used by Studio's cuts and by montages.
//!
//! The curve is written as how long each source second takes to play ("slowness",
//! 1/speed), blended with a cosine from 1 to 1/peak, so where each frame lands has a
//! closed form: one FFmpeg `setpts` expression per piece, then `fps` for an even
//! frame rate. The sound follows in small steps (each its own `atempo`, pitch kept)
//! with a few milliseconds' fade at the join so it doesn't click.

use std::f64::consts::PI;

/// How strong a ramp is: its top speed, and how much of each side (source seconds) it takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub peak: f64,
    pub side: f64,
}

/// "subtle" | "medium" | "strong"; anything else is no ramp (a hard cut).
pub fn shape(strength: &str) -> Option<Shape> {
    match strength {
        "subtle" => Some(Shape { peak: 3.0, side: 0.5 }),
        "medium" => Some(Shape { peak: 5.0, side: 0.75 }),
        "strong" => Some(Shape { peak: 8.0, side: 1.0 }),
        _ => None,
    }
}

/// One piece's ramps: `a` seconds slowing down at its start (after a join), `b`
/// speeding up at its end (before one). A short piece gets shorter ramps, and a
/// side too short to show is left out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sides {
    pub len: f64,
    pub a: f64,
    pub b: f64,
    pub peak: f64,
}

impl Sides {
    pub fn new(len: f64, ramp_in: bool, ramp_out: bool, s: Shape) -> Sides {
        // Ramps take at most 90% of the piece between them, so some of it always plays normally.
        let limit = len * 0.9 / if ramp_in && ramp_out { 2.0 } else { 1.0 };
        let each = |on: bool| {
            let r = if on { s.side.min(limit) } else { 0.0 };
            if r < 0.15 { 0.0 } else { r }
        };
        Sides { len, a: each(ramp_in), b: each(ramp_out), peak: s.peak }
    }

    fn k(&self) -> f64 {
        1.0 - 1.0 / self.peak
    }

    /// Where source time `t` (0..len) plays in the ramped piece.
    pub fn out_time(&self, t: f64) -> f64 {
        let (a, b, k, len) = (self.a, self.b, self.k(), self.len);
        let after_a = a * (1.0 - k / 2.0);
        if t < a {
            let w = t / a;
            return a * (w - k * (w / 2.0 + (PI * w).sin() / (2.0 * PI)));
        }
        if t < len - b || b <= 0.0 {
            return after_a + (t - a);
        }
        let u = (t - (len - b)) / b;
        after_a + (len - b - a) + b * (u - k * (u / 2.0 - (PI * u).sin() / (2.0 * PI)))
    }

    /// The ramped piece's length.
    pub fn out_len(&self) -> f64 {
        self.out_time(self.len)
    }

    /// The same as `out_time`, as an FFmpeg expression of the frame's time `T`.
    pub fn setpts_expr(&self) -> String {
        let (a, b, k, len) = (self.a, self.b, self.k(), self.len);
        let after_a = a * (1.0 - k / 2.0);
        let down = if a > 0.0 { format!("{a:.6}*(T/{a:.6}-{k:.6}*(T/{a:.6}/2+sin(PI*T/{a:.6})/(2*PI)))") } else { "0".into() };
        let mid = format!("({after_a:.6}+T-{a:.6})");
        let s = len - b;
        let up = if b > 0.0 {
            format!("({after_a:.6}+{:.6}+{b:.6}*((T-{s:.6})/{b:.6}-{k:.6}*((T-{s:.6})/{b:.6}/2-sin(PI*(T-{s:.6})/{b:.6})/(2*PI))))", s - a)
        } else {
            mid.clone()
        };
        // Plain commas: it goes inside single quotes in the filter graph.
        format!("if(lt(T,{a:.6}),{down},if(lt(T,{s:.6}),{mid},{up}))")
    }

    /// The picture: `[from]` (a piece starting at 0) ramped, at `fps`, as `[to]`.
    pub fn video(&self, from: &str, to: &str, fps: f64) -> String {
        if self.a <= 0.0 && self.b <= 0.0 {
            return format!("[{from}]null[{to}]");
        }
        format!("[{from}]setpts='({})/TB',fps={fps:.5},trim=duration={:.4},setpts=PTS-STARTPTS[{to}]", self.setpts_expr(), self.out_len())
    }

    /// The sound: `[from]` (a piece starting at 0) ramped as `[to]`, in steps; `tag` keeps
    /// this piece's labels apart from the others'.
    pub fn audio(&self, from: &str, to: &str, tag: &str) -> Vec<String> {
        if self.a <= 0.0 && self.b <= 0.0 {
            return vec![format!("[{from}]anull[{to}]")];
        }
        const STEPS: usize = 8;
        // The pieces of source time, each played at its own steady speed.
        let mut cuts = vec![0.0];
        for i in 1..=STEPS {
            if self.a > 0.0 {
                cuts.push(self.a * i as f64 / STEPS as f64);
            }
        }
        if self.b > 0.0 {
            let s = self.len - self.b;
            if s > *cuts.last().unwrap() + 1e-6 {
                cuts.push(s);
            }
            for i in 1..=STEPS {
                cuts.push(s + self.b * i as f64 / STEPS as f64);
            }
        } else if self.len > *cuts.last().unwrap() + 1e-6 {
            cuts.push(self.len);
        }
        cuts.dedup_by(|x, y| (*x - *y).abs() < 1e-6);
        let n = cuts.len() - 1;
        let mut g = Vec::new();
        let outs: String = (0..n).map(|i| format!("[{tag}s{i}]")).collect();
        g.push(format!("[{from}]asplit={n}{outs}"));
        for i in 0..n {
            let (t0, t1) = (cuts[i], cuts[i + 1]);
            let speed = (t1 - t0) / (self.out_time(t1) - self.out_time(t0)).max(1e-6);
            g.push(format!("[{tag}s{i}]atrim=start={t0:.5}:end={t1:.5},asetpts=PTS-STARTPTS{}[{tag}c{i}]", atempo(speed)));
        }
        let ins: String = (0..n).map(|i| format!("[{tag}c{i}]")).collect();
        // A few milliseconds' fade at each join, so the sound doesn't click there.
        let mut edge = String::new();
        if self.a > 0.0 {
            edge += ",afade=t=in:st=0:d=0.015";
        }
        let out = self.out_len();
        if self.b > 0.0 {
            edge += &format!(",afade=t=out:st={:.4}:d=0.015", (out - 0.015).max(0.0));
        }
        g.push(format!("{ins}concat=n={n}:v=0:a=1,apad,atrim=duration={out:.4}{edge}[{to}]"));
        g
    }
}

/// How long pieces of these lengths come out, joined with ramps.
pub fn joined_len(lens: &[f64], s: Shape) -> f64 {
    let n = lens.len();
    lens.iter().enumerate().map(|(i, l)| Sides::new(*l, i > 0, i + 1 < n, s).out_len()).sum()
}

/// `atempo` for any speed (one filter only goes from 0.5 to 2 on older FFmpegs).
fn atempo(mut f: f64) -> String {
    if (f - 1.0).abs() < 0.001 {
        return String::new();
    }
    let mut s = String::new();
    while f > 2.0 {
        s += ",atempo=2";
        f /= 2.0;
    }
    s + &format!(",atempo={f:.5}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn medium() -> Shape {
        shape("medium").unwrap()
    }

    #[test]
    fn plain_middle_and_smooth_ends() {
        let s = Sides::new(10.0, true, true, medium());
        assert_eq!((s.a, s.b), (0.75, 0.75));
        // Starts at 0, runs at normal speed in the middle.
        assert!(s.out_time(0.0).abs() < 1e-9);
        let (m1, m2) = (s.out_time(3.0), s.out_time(5.0));
        assert!((m2 - m1 - 2.0).abs() < 1e-9);
        // Each ramp side plays in a bit over half its time at 5×: 0.75 × (1 − 0.8/2) = 0.45 s.
        assert!((s.out_len() - (10.0 - 2.0 * 0.75 * 0.4)).abs() < 1e-9, "{}", s.out_len());
        // Never goes backwards, and the speed is highest right at the join.
        let mut last = -1.0;
        for i in 0..=1000 {
            let t = s.out_time(10.0 * i as f64 / 1000.0);
            assert!(t >= last);
            last = t;
        }
        let speed = |t: f64| 0.001 / (s.out_time(t + 0.0005) - s.out_time(t - 0.0005));
        assert!((speed(9.9995) - 5.0).abs() < 0.1, "{}", speed(9.9995));
        assert!((speed(0.0005) - 5.0).abs() < 0.1);
        assert!((speed(5.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn only_the_sides_next_to_a_join() {
        let first = Sides::new(6.0, false, true, medium());
        assert_eq!((first.a, first.b), (0.0, 0.75));
        let last = Sides::new(6.0, true, false, medium());
        assert_eq!((last.a, last.b), (0.75, 0.0));
        assert!((last.out_len() - (6.0 - 0.3)).abs() < 1e-9);
    }

    #[test]
    fn short_pieces_get_short_ramps() {
        let s = Sides::new(1.0, true, true, shape("strong").unwrap());
        assert!(s.a <= 0.5 && s.b <= 0.5 && s.a + s.b <= 1.0, "{s:?}");
        let tiny = Sides::new(0.2, true, true, medium());
        assert_eq!((tiny.a, tiny.b), (0.0, 0.0), "too short to ramp");
    }

    #[test]
    fn expression_matches_the_curve() {
        // Work the FFmpeg expression out by hand for a few times and compare.
        let s = Sides::new(4.0, true, true, shape("strong").unwrap());
        let e = s.setpts_expr();
        assert!(e.starts_with("if(lt(T"), "{e}");
        assert!(!e.contains(",,"));
        for t in [0.1, 0.5, 2.0, 3.5, 3.9] {
            assert!(s.out_time(t).is_finite());
        }
    }

    #[test]
    fn sound_steps_add_up() {
        let s = Sides::new(5.0, true, true, medium());
        let g = s.audio("in", "out", "p0");
        assert!(g[0].starts_with("[in]asplit="));
        assert!(g.last().unwrap().ends_with("[out]"));
        assert!(g.iter().any(|f| f.contains("atempo=2,atempo=")), "5× takes two atempos");
        assert_eq!(atempo(1.0), "");
        assert_eq!(atempo(4.0), ",atempo=2,atempo=2.00000");
    }
}

#[cfg(test)]
mod real {
    use super::*;

    /// A made-up clip whose brightness is its frame number (20 + frame mod 200), two
    /// named sound tracks; cut into two pieces joined with a medium ramp. The frame
    /// numbers read back from the result give the real speed around the join.
    #[test]
    fn ramped_cut_speeds_up_into_the_join() {
        crate::ffmpeg::init(None);
        if crate::ffmpeg::skip_media_tests() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("hindsight-ramp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("Test 2026-01-01 00-00-00.mkv");
        let made = crate::ffmpeg::cmd("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=black:s=320x180:r=60:d=12,format=yuv420p,geq=lum='20+mod(N,200)':cb=128:cr=128"])
            .args(["-f", "lavfi", "-i", "sine=f=440:d=12", "-f", "lavfi", "-i", "sine=f=660:d=12"])
            .args(["-map", "0", "-map", "1", "-map", "2", "-c:v", "h264_nvenc", "-cq", "18", "-g", "30", "-c:a", "aac"])
            .args(["-metadata:s:a:0", "title=Game", "-metadata:s:a:1", "title=Mic"])
            .arg(&src)
            .status()
            .unwrap();
        assert!(made.success());
        let shape = shape("medium").unwrap();
        let pieces = [(0.0, 4.0), (6.0, 12.0)];
        let out = crate::library::cut_ramped(&src.to_string_lossy(), &pieces, &Default::default(), None, shape).unwrap();
        let p = crate::library::probe(&out).unwrap();
        let want = joined_len(&[4.0, 6.0], shape);
        eprintln!("ramped: {:.3}s (expected {want:.3}), {} tracks {:?}", p.duration, p.audio_tracks, crate::library::track_titles(&out));
        assert!((p.duration - want).abs() < 0.1);
        assert_eq!(p.audio_tracks, 2);
        assert_eq!(crate::library::track_titles(&out), vec!["Game", "Mic"]);
        // Each output frame's source frame, from the brightness of its Y plane.
        let raw = crate::ffmpeg::cmd("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-i", &out, "-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
            .output()
            .unwrap()
            .stdout;
        let fsz = 320 * 180 * 3 / 2;
        let ys: Vec<f64> = raw.chunks_exact(fsz).map(|f| f[..320 * 180].iter().map(|v| *v as f64).sum::<f64>() / (320.0 * 180.0)).collect();
        // Source frames moved per output frame (mod 200), smoothed over 3 frames.
        let step: Vec<f64> = ys.windows(2).map(|w| (w[1] - w[0]).rem_euclid(200.0)).collect();
        let at = |sec: f64| {
            let i = (sec * 60.0) as usize;
            step[i.saturating_sub(1)..(i + 2).min(step.len())].iter().sum::<f64>() / 3.0
        };
        let join = Sides::new(4.0, false, true, shape).out_len();
        let j = (join * 60.0) as usize;
        eprintln!("steps around the join: {:?}", &step[j.saturating_sub(12)..(j + 30).min(step.len())]);
        eprintln!("speed at 1 s {:.2}, just before the join {:.2}, after {:.2}, at the end {:.2}", at(1.0), at(join - 0.03), at(join + 0.07), at(join + 3.0));
        assert!((at(1.0) - 1.0).abs() < 0.3, "normal speed away from the join");
        // The fastest step on each side of the join (the jump itself left out).
        let before = step[j.saturating_sub(8)..j.saturating_sub(1)].iter().cloned().fold(0.0, f64::max);
        let after = step[j + 1..j + 8].iter().cloned().fold(0.0, f64::max);
        eprintln!("fastest before the join {before}, after {after}");
        assert!(before >= 3.5, "fast into the join");
        assert!(after >= 3.5, "fast out of the join");
        assert!((at(join + 3.0) - 1.0).abs() < 0.3, "back to normal after");
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&out);
        let _ = std::fs::remove_dir(&dir);
    }
}
