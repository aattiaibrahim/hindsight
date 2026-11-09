import { Select } from "./Select";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useState } from "react";
import { api, BenchApp, BenchProgress, BenchReport, Comparison, RecorderSettings, Stat, Verdict, errText } from "./api";
import { useUpdate } from "./hooks";

/** The repo's "Performance test result" issue form, with the version and game filled in. */
function shareUrl(r: BenchReport, version: string | undefined) {
  const q = new URLSearchParams({ template: "benchmark.yml", title: `[Benchmark] ${r.game || "Desktop"}` });
  if (version) q.set("version", version);
  if (r.game) q.set("game", r.game);
  return `https://github.com/aattiaibrahim/hindsight/issues/new?${q}`;
}

const OTHERS: { app: BenchApp; label: string; how: string }[] = [
  { app: "obs", label: "OBS replay buffer", how: "Hindsight opens and closes OBS for you." },
  { app: "nvidia", label: "NVIDIA Instant Replay", how: "Hindsight presses your Instant Replay shortcut for you." },
  { app: "medal", label: "Medal", how: "Hindsight opens and closes Medal for you. Clipping must be on in Medal's settings." },
];

const NAME: Record<BenchApp, string> = {
  rewind: "Hindsight",
  obs: "OBS replay buffer",
  nvidia: "NVIDIA Instant Replay",
  medal: "Medal",
};

const f1 = (n: number) => n.toFixed(1);
const f2 = (n: number) => n.toFixed(2);
const signed = (n: number, d = 1) => `${n < 0 ? "−" : n > 0 ? "+" : "±"}${Math.abs(n).toFixed(d)}`;
const mmss = (s: number) => `${Math.floor(s / 60)}:${String(Math.round(s % 60)).padStart(2, "0")}`;

/** "1.2% ±0.8" — mean with its 95% range. */
function pm(s: Stat | null, unit: string, d = 1) {
  if (!s) return "–";
  return `${s.mean.toFixed(d)}${unit}${s.ci95 !== null ? ` ±${s.ci95.toFixed(d)}` : ""}`;
}

/** FPS cost shown as a slowdown: positive = slower. */
function cost(s: Stat | null) {
  if (!s) return "–";
  const ci = s.ci95 !== null ? ` ±${f1(s.ci95)}` : "";
  return `${signed(-s.mean)}%${ci}`;
}

function verdictText(c: Comparison): { text: string; tone: "good" | "bad" | "dim" } {
  const v: Verdict = c.verdict;
  const s = c.fpsCost;
  switch (v) {
    case "noMeasurableCost":
      return { text: `No measurable FPS cost (within ±${f1(s?.ci95 ?? 0)}%)`, tone: "good" };
    case "costs":
      return { text: `Costs ${f1(s!.mean)}% FPS (${f1(s!.mean - s!.ci95!)}–${f1(s!.mean + s!.ci95!)}%)`, tone: "bad" };
    case "faster":
      return { text: "Faster than nothing recording: noise or a changing scene", tone: "dim" };
    case "oneRun":
      return { text: "Only one good run: no confidence range", tone: "dim" };
    default:
      return { text: "No good runs", tone: "dim" };
  }
}

function res(s: RecorderSettings) {
  return s.width && s.height ? `${s.width}×${s.height}` : "?";
}

function asText(r: BenchReport) {
  const b = r.baseline;
  const lines = [
    `Hindsight performance test: ${r.game}, ${r.started}`,
    `${r.rounds} rounds × ${r.seconds} s runs, interleaved with nothing-recording runs, shuffled order, ${r.warmupSecs} s warm-up, ${f1(r.minutes)} min total`,
    `Nothing recording: ${b.fps ? `${f1(b.fps.mean)} FPS` : "–"} over ${b.runs} runs, run-to-run noise ±${f1(b.noisePercent ?? 0)}%, drift ${signed(b.driftPercent ?? 0)}%` +
      (b.gpuBusyShare !== null ? `, GPU busy ${Math.round(b.gpuBusyShare * 100)}% of each frame` : ""),
    "",
    "Recorder | FPS change (95% range) | 1% low change | GPU time per frame | CPU (whole PC) | GPU 3D | Encoder | RAM | Good runs | Verdict",
  ];
  for (const c of r.results) {
    lines.push(
      [
        NAME[c.app],
        cost(c.fpsCost),
        cost(c.low1Cost),
        c.gpuMsAdded ? `${signed(c.gpuMsAdded.mean, 2)} ms` : "–",
        pm(c.cpuPercent, "%", 2),
        pm(c.gpu3dPercent, "%"),
        pm(c.encoderPercent, "%"),
        c.ramMb ? `${Math.round(c.ramMb.mean)} MB` : "–",
        c.runs,
        verdictText(c).text,
      ].join(" | "),
    );
  }
  lines.push("", "Settings: recorder | resolution | fps | codec | quality | buffer");
  for (const s of r.settings) {
    lines.push([NAME[s.app], res(s), s.fps ?? "?", s.codec ?? "?", s.quality ?? "?", s.bufferSecs ? `${s.bufferSecs} s` : "?"].join(" | "));
  }
  if (r.warnings.length) lines.push("", "Warnings:", ...r.warnings.map((w) => `- ${w}`));
  lines.push("", "All runs: # | round | recording | at | avg FPS | 1% low | GPU ms/frame | note");
  r.runs.forEach((run, i) =>
    lines.push(
      [
        i + 1,
        run.round,
        run.app ? NAME[run.app] : "nothing",
        mmss(run.atSecs),
        run.frames ? f1(run.frames.avgFps) : "–",
        run.frames ? f1(run.frames.low1Fps) : "–",
        run.frames?.gpuBusyMs != null ? f2(run.frames.gpuBusyMs) : "–",
        run.excluded ?? "",
      ].join(" | "),
    ),
  );
  return lines.join("\n");
}

export function Performance() {
  const [compare, setCompare] = useState<BenchApp[]>(["obs", "nvidia", "medal"]);
  const [seconds, setSeconds] = useState(20);
  const [rounds, setRounds] = useState(3);
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<BenchProgress | null>(null);
  const [report, setReport] = useState<BenchReport | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [shared, setShared] = useState(false);
  const upd = useUpdate();

  useEffect(() => {
    api.benchState().then(([r, last]) => {
      setRunning(r);
      setReport(last);
    });
    const a = listen<BenchProgress>("bench-progress", (e) => setProgress(e.payload));
    const b = listen<BenchReport>("bench-done", (e) => {
      setReport(e.payload);
      setRunning(false);
      setProgress(null);
    });
    return () => {
      a.then((f) => f());
      b.then((f) => f());
    };
  }, []);

  const start = async () => {
    setErr(null);
    try {
      await api.startBench(compare, seconds, rounds);
      setRunning(true);
      setReport(null);
    } catch (e) {
      setErr(errText(e));
    }
  };

  // Each recorder run and the baseline after it, plus switching time (opening OBS or Medal takes longer).
  const recorders = 1 + compare.length;
  const slow = compare.filter((a) => a !== "nvidia").length;
  const minutes = Math.ceil(
    (30 + 20 + seconds + rounds * (recorders * (2 * seconds + 2 * 4 + 8 + 5 + 6) + slow * 15)) / 60,
  );

  return (
    <div className="page">
      <h1>Performance test</h1>
      <p className="muted" style={{ margin: 0, maxWidth: 760, lineHeight: 1.6 }}>
        Shows how much each recorder slows your game. Hindsight measures your frame rate with nothing recording, then with each
        recorder on, back and forth several times, and switches every recorder on and off by itself. You just play.
      </p>

      {!running && (
        <section className="group" style={{ maxWidth: 760 }}>
          <h2>Compare Hindsight with</h2>
          <p className="muted" style={{ margin: "0 0 4px", fontSize: 13 }}>
            Hindsight is always measured. Switch on the recorders you have installed to measure them too; each one adds a few minutes.
            Leave them all off to measure Hindsight alone.
          </p>
          {OTHERS.map((o) => (
            <label key={o.app} className="setting">
              <span>
                {o.label}
                <small>{o.how}</small>
              </span>
              <input
                type="checkbox"
                className="switch"
                checked={compare.includes(o.app)}
                onChange={(e) => setCompare((c) => (e.target.checked ? [...c, o.app] : c.filter((x) => x !== o.app)))}
              />
            </label>
          ))}
          <label className="setting">
            <span>
              Rounds
              <small>How many times each recorder is measured. 3 or more gives a confidence range worth trusting.</small>
            </span>
            <Select className="select" value={rounds} onChange={(e) => setRounds(Number(e.target.value))}>
              <option value={2}>2</option>
              <option value={3}>3</option>
              <option value={5}>5</option>
            </Select>
          </label>
          <label className="setting">
            <span>
              Length of each run
              <small>The whole test takes about {minutes} min</small>
            </span>
            <Select className="select" value={seconds} onChange={(e) => setSeconds(Number(e.target.value))}>
              <option value={15}>15 seconds</option>
              <option value={20}>20 seconds</option>
              <option value={30}>30 seconds</option>
            </Select>
          </label>
          <h2 style={{ marginTop: 16 }}>How to run it</h2>
          <ol className="muted bench-tips">
            <li>
              Open your game and stand somewhere quiet that looks the same every time: a training range, an empty map, or the
              game's built-in benchmark.
            </li>
            <li>Turn off V-Sync and any FPS cap in the game, or the cost gets hidden.</li>
            <li>Press <b>Start test</b>, then switch to the game within 10 seconds.</li>
            <li>
              Keep playing in that spot until a voice says the test is finished (about {minutes} min). Don't switch windows. If
              something needs you, the voice tells you.
            </li>
            <li>Come back here for the results.</li>
          </ol>
          <div className="row" style={{ paddingTop: 12, gap: 12, flexWrap: "wrap" }}>
            <button className="btn primary" onClick={start}>
              Start test
            </button>
            <span className="muted" style={{ fontSize: 12 }}>
              Recorders are put back the way they were when the test ends.
            </span>
          </div>
          {err && <div className="err-text">{err}</div>}
        </section>
      )}

      {running && (
        <section className="group" style={{ maxWidth: 760 }}>
          <h2>{progress?.step ?? "Starting…"}</h2>
          <p style={{ margin: "4px 0 12px", fontSize: 14, color: progress?.waitingForUser ? "var(--accent-ink)" : "var(--text-2)" }}>
            {progress?.detail ?? "Switch to your game."}
          </p>
          <div className="meter" aria-hidden="true">
            <div style={{ width: `${progress ? (progress.done / Math.max(1, progress.total)) * 100 : 0}%` }} />
          </div>
          <div className="row" style={{ paddingTop: 12, gap: 12 }}>
            <button className="btn" onClick={() => api.cancelBench()}>
              Cancel test
            </button>
            {progress && (
              <span className="muted" style={{ fontSize: 12 }}>
                Run {progress.done} of {progress.total}
              </span>
            )}
          </div>
        </section>
      )}

      {report && (
        <Results
          report={report}
          copied={copied}
          onCopy={() => {
            navigator.clipboard.writeText(asText(report)).then(() => {
              setCopied(true);
              setTimeout(() => setCopied(false), 2000);
            });
          }}
          onShare={async () => {
            // The results are too long for a link: copy them, then open the form to paste into.
            await navigator.clipboard.writeText(asText(report)).catch(() => {});
            setShared(true);
            setTimeout(() => setShared(false), 6000);
            openUrl(shareUrl(report, upd?.current)).catch(() => {});
          }}
          shared={shared}
        />
      )}
    </div>
  );
}

function Results({
  report: r,
  copied,
  onCopy,
  onShare,
  shared,
}: {
  report: BenchReport;
  copied: boolean;
  onCopy: () => void;
  onShare: () => void;
  shared: boolean;
}) {
  const b = r.baseline;
  return (
    <section className="group" style={{ maxWidth: 1060 }}>
      <div className="row" style={{ justifyContent: "space-between", flexWrap: "wrap" }}>
        <h2 style={{ margin: 0 }}>
          Results{r.game ? `: ${r.game}` : ""}{" "}
          <span className="muted" style={{ fontSize: 12, fontWeight: 400 }}>{r.started}</span>
        </h2>
        <span className="row" style={{ gap: 8 }}>
          <button className="btn small" onClick={onCopy}>
            {copied ? "Copied" : "Copy results"}
          </button>
          <button
            className="btn small primary"
            onClick={onShare}
            title="Copies the results and opens the benchmark form on GitHub, where you paste them and add screenshots"
          >
            Share on GitHub
          </button>
        </span>
      </div>
      {shared && (
        <p className="muted" style={{ fontSize: 12, margin: "6px 0 0" }}>
          Results copied. Paste them into the Results box on GitHub, then drag in your screenshots.
        </p>
      )}
      {r.error && <div className="err-text" style={{ padding: "8px 0" }}>{r.error} Results below use the runs finished before that.</div>}

      <div className="bench-summary">
        <div>
          <span>Nothing recording</span>
          <strong>{b.fps ? `${f1(b.fps.mean)} FPS` : "–"}</strong>
          <small>{b.runs} runs · 1% low {b.low1 ? f1(b.low1.mean) : "–"}</small>
        </div>
        <div>
          <span>Run-to-run noise</span>
          <strong>{b.noisePercent !== null ? `±${f1(b.noisePercent)}%` : "–"}</strong>
          <small>Spread between identical runs</small>
        </div>
        <div>
          <span>Drift over the test</span>
          <strong>{b.driftPercent !== null ? `${signed(b.driftPercent)}%` : "–"}</strong>
          <small>Cancelled out by pairing</small>
        </div>
        <div>
          <span>GPU busy per frame</span>
          <strong>{b.gpuBusyShare !== null ? `${Math.round(b.gpuBusyShare * 100)}%` : "–"}</strong>
          <small>{b.vsync ? "V-Sync on" : b.gpuBusyShare !== null && b.gpuBusyShare > 0.9 ? "GPU-bound: good for this test" : "Near 100% is best"}</small>
        </div>
        <div>
          <span>Test</span>
          <strong>{r.rounds} × {r.seconds} s</strong>
          <small>{f1(r.minutes)} min · {r.runs.length} runs</small>
        </div>
      </div>

      {r.warnings.length > 0 && (
        <ul className="bench-warnings">
          {r.warnings.map((w) => (
            <li key={w}>{w}</li>
          ))}
        </ul>
      )}

      <div style={{ overflowX: "auto", marginTop: 12 }}>
        <table className="bench">
          <thead>
            <tr>
              <th>Recorder</th>
              <th title="Change in average FPS vs the nothing-recording runs either side, with its 95% range">FPS change</th>
              <th title="Change in 1% low FPS (the slowest 1% of frames)">1% low change</th>
              <th title="Extra time the GPU spends on each frame. Still shows a cost when FPS is capped.">GPU time / frame</th>
              <th title={`Share of your whole CPU (${r.cores} threads), on top of what it uses idle`}>CPU</th>
              <th title="Graphics (3D) engine, on top of idle">GPU</th>
              <th title="Video encoder chip">Encoder</th>
              <th title="Private memory, including its replay buffer">RAM</th>
              <th>Verdict</th>
            </tr>
          </thead>
          <tbody>
            {r.results.map((c) => {
              const v = verdictText(c);
              const skip = r.skipped.find((s) => s.app === c.app);
              return (
                <tr key={c.app} className={c.app === "rewind" ? "ours" : ""}>
                  <td>
                    {NAME[c.app]}
                    <div className="muted" style={{ fontSize: 11 }}>
                      {skip ? skip.reason : `${c.runs} good run${c.runs === 1 ? "" : "s"}`}
                    </div>
                  </td>
                  <td className="mono">{cost(c.fpsCost)}</td>
                  <td className="mono">{cost(c.low1Cost)}</td>
                  <td className="mono">{c.gpuMsAdded ? `${signed(c.gpuMsAdded.mean, 2)} ms` : "–"}</td>
                  <td className="mono">{pm(c.cpuPercent, "%", 2)}</td>
                  <td className="mono">{pm(c.gpu3dPercent, "%")}</td>
                  <td className="mono">{pm(c.encoderPercent, "%")}</td>
                  <td className="mono">{c.ramMb ? `${Math.round(c.ramMb.mean)} MB` : "–"}</td>
                  <td className={`verdict ${v.tone}`}>{v.text}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>

      <h3 className="bench-h3">Recorder settings</h3>
      <div style={{ overflowX: "auto" }}>
        <table className="bench">
          <thead>
            <tr>
              <th>Recorder</th>
              <th>Resolution</th>
              <th>FPS</th>
              <th>Codec</th>
              <th>Quality</th>
              <th>Buffer</th>
              <th>Read from</th>
            </tr>
          </thead>
          <tbody>
            {r.settings.map((s) => (
              <tr key={s.app} className={s.app === "rewind" ? "ours" : ""}>
                <td>{NAME[s.app]}</td>
                <td className="mono">{res(s)}</td>
                <td className="mono">{s.fps !== null ? Math.round(s.fps) : "?"}</td>
                <td>{s.codec ?? "?"}</td>
                <td>{s.quality ?? "?"}</td>
                <td className="mono">{s.bufferSecs ? `${s.bufferSecs} s` : "?"}</td>
                <td className="muted wrap">{s.source}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <details className="bench-more">
        <summary>All runs ({r.runs.length})</summary>
        <div style={{ overflowX: "auto" }}>
          <table className="bench">
            <thead>
              <tr>
                <th>#</th>
                <th>Round</th>
                <th>Recording</th>
                <th>At</th>
                <th>Avg FPS</th>
                <th>1% low</th>
                <th>GPU ms / frame</th>
                <th>Check</th>
              </tr>
            </thead>
            <tbody>
              {r.runs.map((run, i) => (
                <tr key={i} className={run.excluded ? "left-out" : ""}>
                  <td className="mono">{i + 1}</td>
                  <td className="mono">{run.round}</td>
                  <td>{run.app ? NAME[run.app] : "Nothing"}</td>
                  <td className="mono">{mmss(run.atSecs)}</td>
                  <td className="mono">{run.frames ? f1(run.frames.avgFps) : "–"}</td>
                  <td className="mono">{run.frames ? f1(run.frames.low1Fps) : "–"}</td>
                  <td className="mono">{run.frames?.gpuBusyMs != null ? f2(run.frames.gpuBusyMs) : "–"}</td>
                  <td className="wrap">{run.excluded ? `Left out: ${run.excluded}` : "OK"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </details>

      <details className="bench-more">
        <summary>How this test works</summary>
        <ul className="muted bench-tips">
          <li>
            <b>Frames</b> come from Intel PresentMon, which reads the game's frame presents from Windows' event tracing: the same
            source as CapFrameX and NVIDIA FrameView. 1% low is the average of the slowest 1% of frames.
          </li>
          <li>
            <b>Order</b>: {r.warmupSecs} s warm-up, then nothing → recorder → nothing → recorder … for {r.rounds} rounds, with the
            recorder order shuffled each round. Each recorder run waits {8} s after its recorder starts encoding, and each
            nothing-recording run waits {5} s after it stops.
          </li>
          <li>
            <b>Pairing</b>: each recorder run is compared with the nothing-recording runs right before and after it, so slow
            changes (heat, background tasks) cancel out instead of being blamed on whichever recorder ran then.
          </li>
          <li>
            <b>Confidence</b>: the ± numbers are 95% ranges (Student's t) across rounds. If the range includes zero, the test
            can't tell the recorder from noise, and says so.
          </li>
          <li>
            <b>Checks</b>: a nothing-recording run is left out if any recorder was using the video encoder; a recorder run is left
            out if that recorder wasn't encoding; any run is left out if the game wasn't in front for 90% of it.
          </li>
          <li>
            <b>Use</b>: CPU, GPU and encoder come from Windows' per-process counters for each recorder and its helper processes,
            minus what it used while idle. RAM is private memory, which includes the replay buffer.
          </li>
          <li>
            <b>Limits</b>: the scene isn't perfectly repeatable unless you use a built-in benchmark; settings for NVIDIA and Medal
            are only partly readable; and a CPU-bound or capped game hides GPU-side costs (GPU time per frame still shows them).
          </li>
        </ul>
      </details>
    </section>
  );
}
