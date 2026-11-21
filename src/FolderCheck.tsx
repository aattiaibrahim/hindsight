import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { api, errText, FolderCheck, GameSuggestion } from "./api";
import { GameIcon } from "./GameIcon";
import { useThumb } from "./hooks";
import { ProgressBar, useCountProgress } from "./Progress";

type Stage = "choose" | "scanning" | "result";

function Found({ s }: { s: GameSuggestion }) {
  const thumb = useThumb(s.path);
  return (
    <div className="fc-row">
      <span className={`fc-thumb ${thumb ? "" : "loading"}`}>{thumb && <img src={thumb} alt="" />}</span>
      <span className="fc-name" title={s.path}>
        {s.name}
      </span>
      <span className="fc-game" title={s.sure ? `Looks like ${s.game}` : `Might be ${s.game}`}>
        <GameIcon name={s.game} size={16} />
        <span>{s.game}</span>
        {!s.sure && <small className="muted">?</small>}
      </span>
    </div>
  );
}

/**
 * Check a game's folder for clips of other games (a clip saved while the wrong game
 * was in front, or moved there by mistake). Two ways, both temporary: nothing about
 * the folder is saved, and nothing moves until you review and press Move.
 *  - Quick look: from what the game finder has already looked at. Instant.
 *  - Full scan: looks at every clip in the folder now, then the same.
 */
export function FolderCheckDialog({
  folder,
  onClose,
  onReview,
  onAlways,
}: {
  folder: string;
  onClose: () => void;
  /** Open the move review with these clips */
  onReview: (list: GameSuggestion[], heading: string) => void;
  /** "Not a game's folder": look through it for good (the old Find the game) */
  onAlways?: () => void;
}) {
  const [stage, setStage] = useState<Stage>("choose");
  const [check, setCheck] = useState<FolderCheck | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [scanned, setScanned] = useState(false);
  const progress = useCountProgress("folder-scan-progress", stage === "scanning");
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    api
      .checkGameFolder(folder)
      .then((c) => alive.current && setCheck(c))
      .catch((e) => alive.current && setErr(errText(e)));
    return () => {
      alive.current = false;
    };
  }, [folder]);

  // Esc closes; during a scan it stops it first.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (stage === "scanning") api.cancelFolderScan().catch(() => {});
      else onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [stage, onClose]);

  const scan = () => {
    setErr(null);
    setStage("scanning");
    api
      .scanGameFolder(folder)
      .then((c) => {
        if (!alive.current) return;
        setCheck(c);
        setScanned(true);
        setStage("result");
      })
      .catch((e) => {
        if (!alive.current) return;
        setErr(errText(e));
        setStage("choose");
      });
  };

  const left = check ? check.clips - check.lookedAt : 0;
  const found = check?.suggestions ?? [];
  const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;
  const heading = `In “${folder}”: ${plural(found.length, "clip")} that look${found.length === 1 ? "s" : ""} like another game`;
  const pct = progress && progress[1] > 0 ? Math.round((progress[0] / progress[1]) * 100) : null;

  return createPortal(
    <div className="modal-backdrop" onClick={() => stage !== "scanning" && onClose()}>
      <div className="modal fc-modal" role="dialog" aria-modal="true" aria-labelledby="fc-title" onClick={(e) => e.stopPropagation()}>
        <h2 id="fc-title">
          <GameIcon name={folder} size={22} />
          <span>Check “{folder}”</span>
        </h2>

        {stage === "choose" && (
          <>
            <p className="muted" style={{ margin: 0 }}>
              Find clips in this folder that look like a different game, like one saved while another game was in front, or moved here by
              mistake. Nothing is changed: you see what it finds, and nothing moves until you say so.
            </p>
            <div className="fc-options">
              <button className="fc-option" disabled={!check} onClick={() => setStage("result")}>
                <b>Quick look</b>
                <span>
                  Instant. Uses what the game finder has already looked at
                  {check ? `: ${check.lookedAt} of ${plural(check.clips, "clip")}.` : "…"}
                </span>
              </button>
              <button className="fc-option" disabled={!check || check.clips === 0} onClick={scan}>
                <b>Full scan</b>
                <span>
                  {check && left === 0
                    ? "Every clip here has been looked at already, so this finds the same as a quick look."
                    : `Looks at every clip in this folder now${check ? ` (${plural(left, "clip")} still to look at)` : ""}. Takes a while; you can stop it.`}
                </span>
              </button>
            </div>
            {err && <small className="err-text">{err}</small>}
            <div className="row" style={{ justifyContent: "space-between" }}>
              {onAlways ? (
                <button
                  className="btn small ghost"
                  onClick={onAlways}
                  title="Add this folder to the ones the game finder looks through on its own, and sort its clips into their games"
                >
                  Not a game's folder? Always look through it
                </button>
              ) : (
                <span />
              )}
              <button className="btn" onClick={onClose}>
                Cancel
              </button>
            </div>
          </>
        )}

        {stage === "scanning" && (
          <>
            <div className="job-progress">
              {progress && progress[1] > 0 ? `Looking at clip ${Math.min(progress[0] + 1, progress[1])} of ${progress[1]}…` : "Getting ready…"}
              <ProgressBar pct={pct} />
            </div>
            <p className="muted small" style={{ margin: 0 }}>
              A few frames of each clip are read; clips on the NAS take longer. What's looked at is kept, so the next quick look is up to
              date too.
            </p>
            <div className="row" style={{ justifyContent: "flex-end" }}>
              <button className="btn" onClick={() => api.cancelFolderScan().catch(() => {})}>
                Stop and show what it found
              </button>
            </div>
          </>
        )}

        {stage === "result" && check && (
          <>
            <div className="fc-summary">
              <b>{found.length ? heading.replace(`In “${folder}”: `, "") : "Nothing here looks like another game"}</b>
              <span className="muted">
                {scanned ? "Full scan" : "Quick look"} · {check.lookedAt} of {plural(check.clips, "clip")} looked at
              </span>
            </div>
            {found.length > 0 && (
              <div className="fc-list">
                {found.map((s) => (
                  <Found key={s.path} s={s} />
                ))}
              </div>
            )}
            {left > 0 && (
              <p className="muted small" style={{ margin: 0 }}>
                {plural(left, "clip")} here {left === 1 ? "hasn't" : "haven't"} been looked at yet. A full scan checks them too.
              </p>
            )}
            <div className="row" style={{ justifyContent: "flex-end" }}>
              {left > 0 && (
                <button className="btn" onClick={scan}>
                  Full scan
                </button>
              )}
              <button className="btn" onClick={onClose}>
                Close
              </button>
              {found.length > 0 && (
                <button className="btn primary" onClick={() => onReview(found, heading)}>
                  Review and move…
                </button>
              )}
            </div>
          </>
        )}
      </div>
    </div>,
    document.body,
  );
}
