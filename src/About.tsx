import { openUrl } from "@tauri-apps/plugin-opener";
import { useState } from "react";
import { api, errText } from "./api";
import logo from "./logo.svg";

export const REPO = "https://github.com/aattiaibrahim/hindsight";

/**
 * Early users and beta testers, thanked on the About page. Add a name here
 * (as the person likes to be called) and it shows in the next release.
 */
export const BETA_TESTERS: string[] = ["KronorK"];

/** "Run diagnostics": a report to send when something looks wrong. */
export function DiagnosticsButton({ clip, label = "Run diagnostics" }: { clip?: string; label?: string }) {
  const [busy, setBusy] = useState(false);
  const [out, setOut] = useState<{ path?: string; err?: string } | null>(null);
  const run = async () => {
    setBusy(true);
    setOut(null);
    try {
      setOut({ path: await api.runDiagnostics(clip) });
    } catch (e) {
      setOut({ err: errText(e) });
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="diag">
      <button className="btn" disabled={busy} onClick={run}>
        {busy ? "Checking…" : label}
      </button>
      {out?.path && (
        <div className="result">
          Saved <span className="mono">{out.path.split("\\").pop()}</span> in Downloads. Send it with your bug report.
          <div className="row" style={{ marginTop: 8 }}>
            <button className="btn small" onClick={() => api.reveal(out.path!)}>
              Show
            </button>
            <button className="btn small" onClick={() => openUrl(`${REPO}/issues/new?template=bug_report.yml`).catch(() => {})}>
              Report a bug
            </button>
          </div>
        </div>
      )}
      {out?.err && <div className="result err">{out.err}</div>}
    </div>
  );
}

/** Settings → About. */
export function About({ version, hidden, logSetting }: { version: string | null; hidden: boolean; logSetting?: React.ReactNode }) {
  return (
    <section className="group about" hidden={hidden}>
      <div className="about-head">
        <img src={logo} alt="" width={44} height={44} />
        <div>
          <h2 style={{ margin: 0 }}>
            Hindsight <span className="beta-tag">BETA</span>
          </h2>
          <small className="muted">
            Version {version ?? "…"} · a free, open-source game clipper for Windows
          </small>
        </div>
      </div>
      <p className="about-text">
        You're using a beta: thanks for testing it. If something looks wrong, run diagnostics below and attach the file to a
        bug report. Ideas are welcome as feature requests.
      </p>
      <div className="diag-block">
        <span>Diagnostics</span>
        <small>
          Saves a text file in Downloads with your PC's details (Windows, GPU, driver, screens, audio devices), Hindsight's
          settings and status, and its recent messages. Your Windows user name, NAS name, email and webhook are left out.
          Nothing is sent anywhere: you choose who gets it. For a problem with one clip, use "Report a problem with this
          clip" in the editor instead: it adds that clip's details.
        </small>
        <DiagnosticsButton />
      </div>
      {logSetting}
      <div className="setting">
        <span>
          Feedback
          <small>Bugs and feature requests go on GitHub, where you can follow them.</small>
        </span>
        <div className="row" style={{ flexWrap: "nowrap", flexShrink: 0 }}>
          <button className="btn small" onClick={() => openUrl(`${REPO}/issues/new?template=bug_report.yml`).catch(() => {})}>
            Report a bug
          </button>
          <button className="btn small" onClick={() => openUrl(`${REPO}/issues/new?template=feature_request.yml`).catch(() => {})}>
            Request a feature
          </button>
          <button className="btn small" onClick={() => openUrl(REPO).catch(() => {})}>
            GitHub
          </button>
        </div>
      </div>
      <div className="thanks">
        <h3>Thank you</h3>
        <p className="muted" style={{ margin: 0 }}>To the early users and beta testers who tried Hindsight first and helped shape it:</p>
        {BETA_TESTERS.length ? (
          <ul className="thanks-list">
            {BETA_TESTERS.map((n) => (
              <li key={n}>{n}</li>
            ))}
          </ul>
        ) : (
          <p className="muted" style={{ margin: 0 }}>
            <em>Names coming soon.</em>
          </p>
        )}
        <p className="muted" style={{ margin: 0, fontSize: 12 }}>
          Built on FFmpeg, Tauri, NVIDIA NVENC, AMD AMF and Intel Quick Sync. Game logos from Steam. Third-party licences are in THIRD_PARTY_NOTICES.md.
        </p>
      </div>
    </section>
  );
}
