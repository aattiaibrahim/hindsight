import { useEffect, useState } from "react";
import { api, errText, fmtSize, NasFolderCheck } from "./api";

/**
 * Before a NAS folder is used: what's in it, what would be copied, warnings if
 * it doesn't look like a clips folder, and pairing with the NAS app. Nothing
 * syncs until Start syncing.
 */
export function NasSetup({
  dir,
  server,
  onStart,
  onClose,
}: {
  dir: string;
  server: string;
  onStart: (p: { nasDir: string; nasLibraryId: string; nasServer: string }) => Promise<void>;
  onClose: () => void;
}) {
  const [check, setCheck] = useState<NasFolderCheck | null>(null);
  const [address, setAddress] = useState(server);
  const [code, setCode] = useState("");
  const [paired, setPaired] = useState<string | null>(null);
  const [pairError, setPairError] = useState<string | null>(null);
  const [pairing, setPairing] = useState(false);
  const [sure, setSure] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .nasCheck(dir)
      .then((c) => {
        setCheck(c);
        if (!server && c.serverGuess) setAddress(c.serverGuess);
      })
      .catch((e) => setError(errText(e)));
  }, [dir, server]);

  const pair = async () => {
    setPairing(true);
    setPairError(null);
    try {
      setPaired(await api.nasPair(dir, address, code));
    } catch (e) {
      setPairError(errText(e));
    } finally {
      setPairing(false);
    }
  };

  const warned = !!check && check.warnings.length > 0;
  const needSure = !paired || warned;
  const canStart = !!check && check.reachable && check.writable && (!needSure || sure) && !busy;

  const start = async () => {
    setBusy(true);
    setError(null);
    try {
      const id = await api.nasConfirm(dir);
      await onStart({ nasDir: dir, nasLibraryId: id, nasServer: paired ? address : "" });
      onClose();
    } catch (e) {
      setError(errText(e));
      setBusy(false);
    }
  };

  return (
    <div className="modal-backdrop" onClick={() => !busy && onClose()}>
      <div className="modal nas-setup" role="dialog" aria-modal="true" aria-labelledby="nas-setup-title" onClick={(e) => e.stopPropagation()}>
        <h2 id="nas-setup-title">Check the NAS folder</h2>
        <p className="mono nas-dir">{dir}</p>
        {!check && !error && <p className="muted">Looking through the folder…</p>}
        {check && (
          <>
            <ul className="modal-list wrap">
              <li>
                In it now: <b>{check.clips}</b> clip{check.clips === 1 ? "" : "s"}
                {check.clips ? ` (${fmtSize(check.clipBytes)}) in ${check.games} game folder${check.games === 1 ? "" : "s"}` : ""}
                {check.otherFiles ? `, and ${check.otherFiles} other file${check.otherFiles === 1 ? "" : "s"}` : ""}.
              </li>
              <li>
                This PC would copy <b>{check.toCopy}</b> clip{check.toCopy === 1 ? "" : "s"} ({fmtSize(check.toCopyBytes)}) into it, in
                the background once your game is closed.
              </li>
              {check.knownLibrary && <li>It's already a Hindsight library (set up from this or another PC).</li>}
            </ul>
            {check.warnings.map((w) => (
              <p key={w} className="err-text">
                {w}
              </p>
            ))}

            <div className="nas-pair">
              <b>Pair with the Hindsight NAS app</b>
              <small className="muted">
                In the NAS app, open Settings and press Pair a PC, then type the 6-digit code here. It checks this folder really is
                that app's library.
              </small>
              {paired ? (
                <p className="ok-text">{paired}</p>
              ) : (
                <div className="row">
                  <input className="text-input" style={{ flex: 2 }} aria-label="NAS app address" placeholder="http://nas:8420" value={address} onChange={(e) => setAddress(e.target.value)} />
                  <input
                    className="text-input mono"
                    style={{ width: 110 }}
                    aria-label="Pairing code"
                    placeholder="123 456"
                    inputMode="numeric"
                    value={code}
                    onChange={(e) => setCode(e.target.value.replace(/[^\d ]/g, "").slice(0, 7))}
                  />
                  <button className="btn" disabled={pairing || code.replace(/\s/g, "").length !== 6 || !address} onClick={pair}>
                    {pairing ? "Pairing…" : "Pair"}
                  </button>
                </div>
              )}
              {pairError && <small className="err-text">{pairError}</small>}
            </div>

            {needSure && (
              <label className="nas-sure">
                <input type="checkbox" checked={sure} onChange={(e) => setSure(e.target.checked)} />
                <span>
                  {warned ? "I've read the warning and this is the right folder" : "Sync without pairing (I don't use the NAS app). This is the right folder"}
                </span>
              </label>
            )}
          </>
        )}
        {error && <p className="err-text">{error}</p>}
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button className="btn" disabled={busy} onClick={onClose}>
            Cancel
          </button>
          <button className="btn primary" disabled={!canStart} onClick={start}>
            {busy ? "Starting…" : "Start syncing"}
          </button>
        </div>
      </div>
    </div>
  );
}
