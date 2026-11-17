//! The maths behind the performance test.
//!
//! Design: runs alternate "nothing recording" (baseline) and one recorder,
//! B R B R B …, over several rounds with the recorder order shuffled each
//! round. Each recorder run is compared with the baselines right before and
//! after it, so slow changes during the test (heat, background tasks, the
//! scene drifting) cancel out instead of being blamed on a recorder. The
//! repeats give a 95% confidence range, so a difference smaller than the
//! game's own run-to-run noise is reported as "no measurable difference".

use std::collections::HashMap;

use serde::Serialize;

use super::measure::{App, AppUse, Frames};

/// One measured stretch of the test.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub round: u32,
    /// None = nothing recording (baseline).
    pub app: Option<App>,
    /// Seconds since the first run started.
    pub at_secs: f64,
    pub frames: Option<Frames>,
    pub apps: HashMap<App, AppUse>,
    /// Why this run is left out of the results, if it is.
    pub excluded: Option<String>,
}

impl Run {
    fn ok(&self) -> Option<&Frames> {
        if self.excluded.is_some() {
            None
        } else {
            self.frames.as_ref()
        }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stat {
    pub mean: f64,
    pub sd: f64,
    pub n: usize,
    /// Half-width of the 95% confidence range (Student's t). None with one value.
    pub ci95: Option<f64>,
}

/// Two-sided 95% Student's t critical values, df = 1..30.
const T95: [f64; 30] = [
    12.706, 4.303, 3.182, 2.776, 2.571, 2.447, 2.365, 2.306, 2.262, 2.228, 2.201, 2.179, 2.160, 2.145, 2.131, 2.120, 2.110,
    2.101, 2.093, 2.086, 2.080, 2.074, 2.069, 2.064, 2.060, 2.056, 2.052, 2.048, 2.045, 2.042,
];

pub fn t95(df: usize) -> f64 {
    if df == 0 {
        f64::INFINITY
    } else {
        T95.get(df - 1).copied().unwrap_or(1.96)
    }
}

pub fn stat(v: &[f64]) -> Option<Stat> {
    let n = v.len();
    if n == 0 {
        return None;
    }
    let mean = v.iter().sum::<f64>() / n as f64;
    if n == 1 {
        return Some(Stat { mean, sd: 0.0, n, ci95: None });
    }
    let sd = (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt();
    Some(Stat { mean, sd, n, ci95: Some(t95(n - 1) * sd / (n as f64).sqrt()) })
}

/// What the numbers say about one recorder's FPS cost.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    /// Only one good run: there's a number but no confidence range.
    OneRun,
    /// The 95% range includes zero: no difference the test can detect.
    NoMeasurableCost,
    /// The whole 95% range is a slowdown.
    Costs,
    /// The whole 95% range is a speed-up: noise or a changing scene.
    Faster,
    /// No good runs at all.
    NoData,
}

pub fn verdict(s: Option<&Stat>) -> Verdict {
    match s {
        None => Verdict::NoData,
        Some(Stat { ci95: None, .. }) => Verdict::OneRun,
        Some(Stat { mean, ci95: Some(ci), .. }) => {
            if mean - ci > 0.0 {
                Verdict::Costs
            } else if mean + ci < 0.0 {
                Verdict::Faster
            } else {
                Verdict::NoMeasurableCost
            }
        }
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub app: App,
    /// Good runs used.
    pub runs: usize,
    /// % of FPS lost vs the baselines either side (positive = slower).
    pub fps_cost: Option<Stat>,
    /// % of 1%-low FPS lost.
    pub low1_cost: Option<Stat>,
    /// Extra GPU time per frame, ms. Shows a cost even when FPS is capped.
    pub gpu_ms_added: Option<Stat>,
    /// The recorder's own use while recording, minus its use while idle.
    pub cpu_percent: Option<Stat>,
    pub gpu_3d_percent: Option<Stat>,
    pub encoder_percent: Option<Stat>,
    pub ram_mb: Option<Stat>,
    pub verdict: Verdict,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Baseline {
    pub runs: usize,
    pub fps: Option<Stat>,
    pub low1: Option<Stat>,
    /// Run-to-run spread of baseline FPS after removing slow drift, % (1 SD).
    pub noise_percent: Option<f64>,
    /// How much baseline FPS changed from the first to the last run, %.
    pub drift_percent: Option<f64>,
    /// Share of each frame the GPU was busy (1.0 = GPU-bound).
    pub gpu_busy_share: Option<f64>,
    pub cpu_busy_share: Option<f64>,
    pub vsync: bool,
    pub present_mode: Option<String>,
}

fn mean(v: &[f64]) -> Option<f64> {
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

/// Least-squares line through (x, y): (slope, intercept).
fn line(x: &[f64], y: &[f64]) -> Option<(f64, f64)> {
    let (mx, my) = (mean(x)?, mean(y)?);
    let sxx: f64 = x.iter().map(|a| (a - mx).powi(2)).sum();
    if sxx <= 0.0 {
        return None;
    }
    let sxy: f64 = x.iter().zip(y).map(|(a, b)| (a - mx) * (b - my)).sum();
    let slope = sxy / sxx;
    Some((slope, my - slope * mx))
}

pub fn baseline(runs: &[Run]) -> Baseline {
    let good: Vec<(&Run, &Frames)> = runs.iter().filter(|r| r.app.is_none()).filter_map(|r| r.ok().map(|f| (r, f))).collect();
    let fps: Vec<f64> = good.iter().map(|(_, f)| f.avg_fps).collect();
    let t: Vec<f64> = good.iter().map(|(r, _)| r.at_secs).collect();
    let m = mean(&fps);
    let (mut noise, mut drift) = (None, None);
    if let (Some(m), Some((slope, icept))) = (m, line(&t, &fps)) {
        drift = Some(slope * (t[t.len() - 1] - t[0]) / m * 100.0);
        if fps.len() >= 3 {
            let ss: f64 = t.iter().zip(&fps).map(|(x, y)| (y - (icept + slope * x)).powi(2)).sum();
            noise = Some((ss / (fps.len() - 2) as f64).sqrt() / m * 100.0);
        }
    }
    if noise.is_none() {
        noise = stat(&fps).filter(|s| s.n >= 2).map(|s| s.sd / s.mean * 100.0);
    }
    let share = |pick: fn(&Frames) -> Option<f64>| {
        mean(&good.iter().filter_map(|(_, f)| pick(f).map(|b| b / f.avg_frame_ms)).collect::<Vec<_>>())
    };
    let mut modes: HashMap<&str, usize> = HashMap::new();
    for (_, f) in &good {
        if let Some(p) = &f.present_mode {
            *modes.entry(p.as_str()).or_default() += 1;
        }
    }
    Baseline {
        runs: good.len(),
        fps: stat(&fps),
        low1: stat(&good.iter().map(|(_, f)| f.low1_fps).collect::<Vec<_>>()),
        noise_percent: noise,
        drift_percent: drift,
        gpu_busy_share: share(|f| f.gpu_busy_ms),
        cpu_busy_share: share(|f| f.cpu_busy_ms),
        vsync: good.iter().filter(|(_, f)| f.sync_interval.unwrap_or(0) > 0).count() * 2 > good.len().max(1),
        present_mode: modes.into_iter().max_by_key(|(_, n)| *n).map(|(m, _)| m.to_string()),
    }
}

/// The good baselines right before and after run `i`.
fn neighbours(runs: &[Run], i: usize) -> Vec<&Run> {
    let before = runs[..i].iter().rev().take_while(|r| r.app.is_none() || r.excluded.is_some()).find(|r| r.app.is_none() && r.ok().is_some());
    let after = runs[i + 1..].iter().take_while(|r| r.app.is_none() || r.excluded.is_some()).find(|r| r.app.is_none() && r.ok().is_some());
    before.into_iter().chain(after).collect()
}

pub fn compare(runs: &[Run], app: App) -> Comparison {
    let (mut fps, mut low, mut gpu_ms) = (Vec::new(), Vec::new(), Vec::new());
    let (mut cpu, mut g3d, mut enc, mut ram) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (i, r) in runs.iter().enumerate() {
        if r.app != Some(app) {
            continue;
        }
        let Some(f) = r.ok() else { continue };
        let base = neighbours(runs, i);
        if base.is_empty() {
            continue;
        }
        let bf: Vec<&Frames> = base.iter().filter_map(|b| b.ok()).collect();
        let avg = |pick: fn(&Frames) -> f64| bf.iter().map(|b| pick(b)).sum::<f64>() / bf.len() as f64;
        fps.push((1.0 - f.avg_fps / avg(|b| b.avg_fps)) * 100.0);
        low.push((1.0 - f.low1_fps / avg(|b| b.low1_fps)) * 100.0);
        if let (Some(g), true) = (f.gpu_busy_ms, bf.iter().all(|b| b.gpu_busy_ms.is_some())) {
            gpu_ms.push(g - avg(|b| b.gpu_busy_ms.unwrap()));
        }
        let now = r.apps.get(&app).cloned().unwrap_or_default();
        let idle = |pick: fn(&AppUse) -> f64| base.iter().map(|b| b.apps.get(&app).map(pick).unwrap_or(0.0)).sum::<f64>() / base.len() as f64;
        cpu.push((now.cpu_percent - idle(|u| u.cpu_percent)).max(0.0));
        g3d.push((now.gpu_3d_percent - idle(|u| u.gpu_3d_percent)).max(0.0));
        enc.push((now.encoder_percent - idle(|u| u.encoder_percent)).max(0.0));
        ram.push(now.ram_mb);
    }
    let fps_cost = stat(&fps);
    Comparison {
        app,
        runs: fps.len(),
        verdict: verdict(fps_cost.as_ref()),
        fps_cost,
        low1_cost: stat(&low),
        gpu_ms_added: stat(&gpu_ms),
        cpu_percent: stat(&cpu),
        gpu_3d_percent: stat(&g3d),
        encoder_percent: stat(&enc),
        ram_mb: stat(&ram),
    }
}

/// The common FPS cap or refresh rate `fps` sits on (within 1.5%), if any.
pub fn near_cap(fps: f64) -> Option<f64> {
    const CAPS: [f64; 30] = [
        30.0, 48.0, 50.0, 60.0, 72.0, 75.0, 90.0, 100.0, 110.0, 120.0, 138.0, 141.0, 144.0, 155.0, 160.0, 165.0, 170.0, 175.0,
        180.0, 200.0, 225.0, 237.0, 240.0, 250.0, 280.0, 300.0, 360.0, 400.0, 480.0, 500.0,
    ];
    CAPS.iter().copied().find(|c| (fps / c - 1.0).abs() <= 0.015)
}

/// Things that make the results less trustworthy, in plain words.
pub fn warnings(b: &Baseline, comps: &[Comparison]) -> Vec<String> {
    let mut w = Vec::new();
    if b.runs < 3 {
        w.push("Fewer than 3 good runs with nothing recording, so the noise and drift checks are weak.".into());
    }
    if let (Some(d), Some(n)) = (b.drift_percent, b.noise_percent) {
        if d.abs() > 2.0_f64.max(2.0 * n) {
            w.push(format!(
                "FPS with nothing recording changed by {:+.1}% from the start to the end of the test. Something other than the recorders changed (heat, a background task, the scene). Each recorder is compared with the runs right next to it, which cancels steady drift, but stand still in a quiet spot and run again if you can.",
                d
            ));
        }
    }
    if let Some(n) = b.noise_percent {
        if n > 3.0 {
            w.push(format!(
                "FPS varied a lot between identical runs (±{n:.1}%). Small differences can't be told apart. Pick a calmer spot, or use the game's built-in benchmark."
            ));
        }
    }
    if b.vsync {
        w.push("V-Sync is on in this game, so FPS is held at your refresh rate and hides a recorder's cost. Turn V-Sync off for the test, or look at \"GPU time per frame\", which still shows the cost.".into());
    } else if let (Some(g), Some(c), Some(fps)) = (b.gpu_busy_share, b.cpu_busy_share, b.fps.as_ref().map(|s| s.mean)) {
        // An in-game limiter waits inside the game's own code, which
        // PresentMon counts as CPU time, so a cap can look CPU-bound. FPS
        // sitting on a common cap or refresh rate gives it away.
        let cap = near_cap(fps);
        if g < 0.8 && (c < 0.8 || cap.is_some()) {
            let at = cap.map(|c| format!(" at about {c:.0} FPS")).unwrap_or_default();
            w.push(format!(
                "FPS looks capped{at}: the GPU was busy only {:.0}% of each frame. An FPS cap hides a recorder's cost. Turn the cap off for the test, or look at \"GPU time per frame\", which still shows it.",
                g * 100.0
            ));
        } else if c > 0.9 && g < 0.8 {
            w.push("This scene is limited by the CPU, not the GPU. Recorders mostly load the GPU, so their FPS cost may look smaller here than in a GPU-heavy game.".into());
        }
    }
    for c in comps {
        if c.runs == 1 {
            w.push(format!("{} has only one good run, so it has no confidence range.", super::run::name(c.app)));
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(fps: f64) -> Frames {
        Frames {
            frames: 1000,
            avg_fps: fps,
            low1_fps: fps * 0.7,
            avg_frame_ms: 1000.0 / fps,
            gpu_busy_ms: Some(1000.0 / fps * 0.97),
            cpu_busy_ms: Some(1000.0 / fps * 0.5),
            sync_interval: Some(0),
            present_mode: Some("Hardware: Independent Flip".into()),
        }
    }

    fn run(app: Option<App>, at: f64, fps: f64) -> Run {
        Run { round: 1, app, at_secs: at, frames: Some(frames(fps)), apps: HashMap::new(), excluded: None }
    }

    #[test]
    fn stat_matches_hand_calculation() {
        let s = stat(&[1.0, 2.0, 3.0]).unwrap();
        assert!((s.mean - 2.0).abs() < 1e-9);
        assert!((s.sd - 1.0).abs() < 1e-9);
        // t(2) = 4.303, sd/sqrt(3) = 0.57735
        assert!((s.ci95.unwrap() - 4.303 * 0.577_350_269).abs() < 1e-6);
        assert_eq!(stat(&[5.0]).unwrap().ci95, None);
        assert_eq!(stat(&[]), None);
    }

    #[test]
    fn paired_baselines_cancel_steady_drift() {
        // FPS falls 1 fps per minute on its own; the recorder costs exactly 2%.
        let fps_at = |t: f64| 300.0 - t / 60.0;
        let mut runs = Vec::new();
        let mut t = 0.0;
        for _ in 0..3 {
            runs.push(run(None, t, fps_at(t)));
            t += 30.0;
            let between = (fps_at(t - 30.0) + fps_at(t + 30.0)) / 2.0;
            runs.push(run(Some(App::Rewind), t, between * 0.98));
            t += 30.0;
        }
        runs.push(run(None, t, fps_at(t)));
        let c = compare(&runs, App::Rewind);
        let s = c.fps_cost.unwrap();
        assert_eq!(c.runs, 3);
        assert!((s.mean - 2.0).abs() < 1e-6, "{}", s.mean);
        assert_eq!(c.verdict, Verdict::Costs);
        let b = baseline(&runs);
        assert!(b.drift_percent.unwrap() < -1.0, "{:?}", b.drift_percent);
        assert!(b.noise_percent.unwrap() < 0.01, "{:?}", b.noise_percent);
    }

    #[test]
    fn noise_bigger_than_the_difference_is_not_a_cost() {
        let base = [300.0, 291.0, 306.0, 296.0];
        let rec = [297.0, 301.0, 294.0];
        let mut runs = vec![run(None, 0.0, base[0])];
        for i in 0..3 {
            runs.push(run(Some(App::Obs), (2 * i + 1) as f64 * 30.0, rec[i]));
            runs.push(run(None, (2 * i + 2) as f64 * 30.0, base[i + 1]));
        }
        assert_eq!(compare(&runs, App::Obs).verdict, Verdict::NoMeasurableCost);
    }

    #[test]
    fn excluded_runs_are_ignored() {
        let mut runs = vec![run(None, 0.0, 300.0), run(Some(App::Medal), 30.0, 100.0), run(None, 60.0, 300.0)];
        runs[1].excluded = Some("switched away".into());
        let c = compare(&runs, App::Medal);
        assert_eq!(c.runs, 0);
        assert_eq!(c.verdict, Verdict::NoData);
        // A bad baseline isn't used for pairing either.
        let mut runs = vec![run(None, 0.0, 100.0), run(Some(App::Obs), 30.0, 297.0), run(None, 60.0, 300.0)];
        runs[0].excluded = Some("recorder still on".into());
        assert!((compare(&runs, App::Obs).fps_cost.unwrap().mean - 1.0).abs() < 1e-9);
    }

    #[test]
    fn flags_vsync_and_caps() {
        let mut runs: Vec<Run> = (0..3).map(|i| run(None, i as f64 * 60.0, 144.0)).collect();
        for r in &mut runs {
            let f = r.frames.as_mut().unwrap();
            f.gpu_busy_ms = Some(2.0);
            f.cpu_busy_ms = Some(2.0);
        }
        let w = warnings(&baseline(&runs), &[]);
        assert!(w.iter().any(|s| s.contains("capped at about 144")), "{w:?}");
        // An in-game limiter at 60: looks CPU-busy, but sits on a cap.
        let mut capped: Vec<Run> = (0..3).map(|i| run(None, i as f64 * 60.0, 59.4)).collect();
        for r in &mut capped {
            let f = r.frames.as_mut().unwrap();
            f.gpu_busy_ms = Some(0.3);
            f.cpu_busy_ms = Some(16.5);
        }
        let w = warnings(&baseline(&capped), &[]);
        assert!(w.iter().any(|s| s.contains("capped at about 60")), "{w:?}");
        // CPU-bound and not on a cap.
        for r in &mut capped {
            let f = r.frames.as_mut().unwrap();
            f.avg_fps = 211.0;
            f.avg_frame_ms = 1000.0 / 211.0;
            f.cpu_busy_ms = Some(4.6);
            f.gpu_busy_ms = Some(2.0);
        }
        let w = warnings(&baseline(&capped), &[]);
        assert!(w.iter().any(|s| s.contains("limited by the CPU")), "{w:?}");
        for r in &mut runs {
            r.frames.as_mut().unwrap().sync_interval = Some(1);
        }
        let w = warnings(&baseline(&runs), &[]);
        assert!(w.iter().any(|s| s.contains("V-Sync")), "{w:?}");
    }

    /// Real PresentMon runs on a stand-in game (ffplay at 60 fps), fed
    /// through the same statistics: shows the noise floor and that a
    /// recorder that's encoding is noticed.
    /// cargo test live_stats_on_stand_in_game -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_stats_on_stand_in_game() {
        use std::process::Stdio;
        use std::time::{Duration, Instant};
        crate::ffmpeg::init(None);
        if crate::ffmpeg::skip_media_tests() {
            return;
        }
        let pm = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../vendor/presentmon/PresentMon.exe"));
        let mut game = std::process::Command::new(crate::ffmpeg::exe("ffplay"))
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=60"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_secs(3));
        println!("encoding now: {:?}", super::super::measure::encoder_use_all());
        let t0 = Instant::now();
        let mut runs = Vec::new();
        for i in 0..5 {
            let at = t0.elapsed().as_secs_f64();
            let w = super::super::measure::measure(5, Some(game.id()), &pm, &std::env::temp_dir());
            let f = w.frames.clone().expect("frames");
            println!("run {}: {:.2} fps, 1% low {:.1}, GPU busy {:?} ms, sync {:?}, mode {:?}", i + 1, f.avg_fps, f.low1_fps, f.gpu_busy_ms, f.sync_interval, f.present_mode);
            runs.push(Run { round: 1, app: None, at_secs: at, frames: w.frames, apps: w.apps, excluded: None });
        }
        let _ = game.kill();
        let b = baseline(&runs);
        println!("baseline: {:?}", b);
        println!("warnings: {:?}", warnings(&b, &[]));
        // ffplay draws on the CPU, so its own FPS can swing; the test just has to measure and say so.
        assert_eq!(b.runs, 5);
    }

    #[test]
    fn recorder_use_subtracts_idle() {
        let mut runs = vec![run(None, 0.0, 300.0), run(Some(App::Rewind), 30.0, 300.0), run(None, 60.0, 300.0)];
        let u = |cpu| AppUse { cpu_percent: cpu, ram_mb: 500.0, gpu_3d_percent: 1.0, encoder_percent: 0.0 };
        runs[0].apps.insert(App::Rewind, u(0.1));
        runs[2].apps.insert(App::Rewind, u(0.3));
        runs[1].apps.insert(App::Rewind, AppUse { cpu_percent: 0.5, ram_mb: 700.0, gpu_3d_percent: 2.5, encoder_percent: 9.0 });
        let c = compare(&runs, App::Rewind);
        assert!((c.cpu_percent.unwrap().mean - 0.3).abs() < 1e-9);
        assert!((c.gpu_3d_percent.unwrap().mean - 1.5).abs() < 1e-9);
        assert_eq!(c.ram_mb.unwrap().mean, 700.0);
    }
}
