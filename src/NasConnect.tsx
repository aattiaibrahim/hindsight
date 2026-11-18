import { useEffect, useState } from "react";
import { api, errText, NasShare } from "./api";

/**
 * "Sign in again" from anywhere (the notice when the NAS turns the saved password
 * down): opens this dialog in Settings, even if Settings wasn't open yet.
 */
export function requestNasSignIn() {
  signInPending = true;
  window.dispatchEvent(new Event("hindsight:nas-sign-in"));
}
let signInPending = false;
/** Settings → Library calls this when it opens and on the event: was a sign-in asked for? */
export function takeNasSignInRequest() {
  const was = signInPending;
  signInPending = false;
  return was;
}

/**
 * Connect to the NAS like any NAS app does: its address, the user made on the
 * NAS and its password, then pick a shared folder from the list. Windows does
 * the signing in (the same connection File Explorer makes), and "Remember"
 * keeps the password in Windows Credential Manager so it reconnects by itself.
 * The folder picked then goes through the usual check (NasSetup).
 */
export function NasConnect({
  current,
  rejected,
  onPicked,
  onBrowse,
  onClose,
}: {
  /** The NAS folder set now, if any (to fill in the address). */
  current: string;
  /** The NAS turned down the saved password: ask for the new one. */
  rejected?: boolean;
  onPicked: (dir: string) => void;
  /** Pick a folder the old way instead (a mapped drive, a folder on this PC). */
  onBrowse: () => void;
  onClose: () => void;
}) {
  const hostNow = current.startsWith("\\\\") ? current.slice(2).split("\\")[0] : "";
  const [host, setHost] = useState(hostNow);
  const [user, setUser] = useState("");
  const [password, setPassword] = useState("");
  const [remember, setRemember] = useState(true);
  const [show, setShow] = useState(false);
  const [saved, setSaved] = useState<string | null>(null);
  const [shares, setShares] = useState<NasShare[] | null>(null);
  const [share, setShare] = useState("");
  const [sub, setSub] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // A login saved for this NAS: fill in the user, and allow connecting without retyping the password.
  useEffect(() => {
    if (!host.trim()) return setSaved(null);
    const t = setTimeout(() => {
      api
        .nasSavedLogin(host)
        .then((l) => {
          setSaved(l?.user ?? null);
          if (l?.user) setUser((u) => u || l.user);
        })
        .catch(() => setSaved(null));
    }, 300);
    return () => clearTimeout(t);
  }, [host]);

  const connect = async () => {
    setBusy(true);
    setError(null);
    try {
      const list = await api.nasSignIn(host, user, password, remember);
      setShares(list);
      const now = list.find((s) => current.toLowerCase().startsWith(s.path.toLowerCase()));
      const clipsLike = list.find((s) => /clip|hindsight|rewind|video|record/i.test(s.name));
      setShare((now ?? clipsLike ?? list[0])?.path ?? "");
      if (now) setSub(current.slice(now.path.length).replace(/^\\+/, ""));
      if (remember && password) setSaved(user);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const useShare = async () => {
    setBusy(true);
    setError(null);
    try {
      const folder = sub.trim().replace(/[\\/]+/g, "\\").replace(/^\\|\\$/g, "");
      const dir = await api.nasOpenShare(folder ? `${share}\\${folder}` : share, user, password);
      onPicked(dir);
    } catch (e) {
      setError(errText(e));
      setBusy(false);
    }
  };

  const forget = async () => {
    await api.nasForgetLogin(host).catch(() => {});
    setSaved(null);
    setPassword("");
  };

  // A saved password the NAS turned down can't be used again: a new one is needed.
  const savedUsable = !!saved && !rejected;
  const canConnect = !!host.trim() && (!!password || savedUsable) && (!!user.trim() || savedUsable) && !busy;

  return (
    <div className="modal-backdrop" onClick={() => !busy && onClose()}>
      <div className="modal nas-setup nas-connect" role="dialog" aria-modal="true" aria-labelledby="nas-connect-title" onClick={(e) => e.stopPropagation()}>
        <h2 id="nas-connect-title">Connect to your NAS</h2>
        {!shares ? (
          <>
            {rejected && (
              <p className="err-text">
                The NAS turned down the saved password, so it was probably changed on the NAS. Type the new one. (Forgot it? Whoever
                runs the NAS can set a new one: on TrueNAS, Credentials → Users → this user → Edit.)
              </p>
            )}
            <p className="muted">
              Use a user made on the NAS (on TrueNAS: Credentials → Users, with <b>SMB User</b> ticked). Windows signs in with it,
              the same way File Explorer does.
            </p>
            <form
              className="nas-form"
              onSubmit={(e) => {
                e.preventDefault();
                if (canConnect) connect();
              }}
            >
              <label>
                NAS address
                <input className="text-input" autoFocus={!host} placeholder="192.168.0.10 or nas-name" value={host} onChange={(e) => setHost(e.target.value)} />
              </label>
              <label>
                User name
                <input className="text-input" autoComplete="username" placeholder="hindsight" value={user} onChange={(e) => setUser(e.target.value)} />
              </label>
              <label>
                Password
                <span className="row nas-pass">
                  <input
                    className="text-input"
                    type={show ? "text" : "password"}
                    autoComplete="current-password"
                    autoFocus={!!host}
                    placeholder={savedUsable ? "Saved in Windows (leave empty)" : ""}
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                  />
                  <button
                    type="button"
                    className="btn small"
                    aria-pressed={show}
                    title={show ? "Hide the password" : "Show the password"}
                    onClick={() => setShow((v) => !v)}
                  >
                    {show ? "Hide" : "Show"}
                  </button>
                </span>
              </label>
              <label className="nas-sure">
                <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
                <span>Remember the password (kept by Windows in Credential Manager, so it reconnects after a restart)</span>
              </label>
              {saved && (
                <small className="muted">
                  Windows has a saved login for this NAS ({saved}).{" "}
                  <button type="button" className="btn small" onClick={forget}>
                    Forget it
                  </button>
                </small>
              )}
              <button type="submit" hidden />
            </form>
          </>
        ) : (
          <>
            <p className="muted">
              Signed in to <b>{host.trim()}</b>. Pick the shared folder for your clips.
            </p>
            {shares.length === 0 ? (
              <p className="err-text">This user can't see any shared folders. On the NAS, share a folder for clips and give this user access.</p>
            ) : (
              <div className="nas-shares" role="radiogroup" aria-label="Shared folders">
                {shares.map((s) => (
                  <label key={s.path} className={`nas-share${share === s.path ? " on" : ""}`}>
                    <input type="radio" name="share" checked={share === s.path} onChange={() => setShare(s.path)} />
                    <span>
                      <b>{s.name}</b>
                      {s.comment && <small className="muted"> {s.comment}</small>}
                    </span>
                  </label>
                ))}
              </div>
            )}
            <label className="nas-form">
              Folder in it (optional)
              <input className="text-input" placeholder="Leave empty to use the whole share" value={sub} onChange={(e) => setSub(e.target.value)} />
            </label>
            <p className="mono nas-dir">{share ? (sub.trim() ? `${share}\\${sub.trim().replace(/[\\/]+/g, "\\").replace(/^\\|\\$/g, "")}` : share) : ""}</p>
          </>
        )}
        {error && <p className="err-text">{error}</p>}
        <div className="row" style={{ justifyContent: "space-between" }}>
          <button className="btn small" disabled={busy} onClick={onBrowse} title="A mapped drive, or a folder Windows can already open">
            Pick a folder instead…
          </button>
          <div className="row">
            {shares && (
              <button className="btn" disabled={busy} onClick={() => setShares(null)}>
                Back
              </button>
            )}
            <button className="btn" disabled={busy} onClick={onClose}>
              Cancel
            </button>
            {!shares ? (
              <button className="btn primary" disabled={!canConnect} onClick={connect}>
                {busy ? "Connecting…" : "Connect"}
              </button>
            ) : (
              <button className="btn primary" disabled={!share || busy} onClick={useShare}>
                {busy ? "Opening…" : "Next"}
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
