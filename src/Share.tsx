import { Select } from "./Select";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useState } from "react";
import { api, errText, fmtSize, Reach, Settings, Upload, UPLOAD_HOSTS, UploadHost, UploadProgress } from "./api";
import { ConfirmDialog } from "./ConfirmUpdate";
import { Icon } from "./icons";

/** Live upload progress for every clip, plus each clip's past links. */
export function useUploads() {
  const [progress, setProgress] = useState<Record<string, UploadProgress>>({});
  const [history, setHistory] = useState<Record<string, Upload[]>>({});
  const refresh = useCallback(() => {
    api.uploadHistory().then(setHistory).catch(() => {});
  }, []);
  useEffect(() => {
    refresh();
    const un = listen<UploadProgress>("upload-progress", (e) => {
      const p = e.payload;
      setProgress((all) => {
        const next = { ...all, [p.path]: p };
        // Finished ones fade out of "in progress" after a moment.
        if (p.stage === "done" || p.stage === "error") {
          setTimeout(() => setProgress((a) => (a[p.path] === p ? (({ [p.path]: _, ...rest }) => rest)(a) : a)), 6000);
        }
        return next;
      });
      if (p.stage === "done") refresh();
    });
    // Links run out over time: re-render once a minute so "time left" stays right.
    const tick = setInterval(() => setHistory((h) => ({ ...h })), 60_000);
    return () => {
      un.then((f) => f());
      clearInterval(tick);
    };
  }, [refresh]);
  return { progress, history, refresh };
}

export function hostLabel(h: UploadHost) {
  return UPLOAD_HOSTS.find((x) => x.id === h)?.label ?? h;
}

export function progressText(p: UploadProgress) {
  if (p.stage === "preparing") return "Making a shareable MP4…";
  if (p.stage === "uploading") return p.total ? `Uploading ${Math.round((p.sent / p.total) * 100)}%` : "Uploading…";
  if (p.stage === "done") return "Link copied";
  return p.error ?? "Upload failed";
}

/** "2d 4h", "35 min", or null once it has run out. Permanent links: "". */
export function timeLeft(u: Upload, now = Date.now()): string | null {
  if (u.expires == null) return "";
  const ms = u.expires - now;
  if (ms <= 0) return null;
  const min = Math.floor(ms / 60_000);
  if (min < 60) return `${Math.max(1, min)} min`;
  const h = Math.floor(min / 60);
  if (h < 24) return `${h}h ${min % 60 ? `${min % 60}m` : ""}`.trim();
  return `${Math.floor(h / 24)}d ${h % 24 ? `${h % 24}h` : ""}`.trim();
}

/** The newest link of a clip that still works, if any. */
export function liveLink(links: Upload[] | undefined): Upload | null {
  for (let i = (links?.length ?? 0) - 1; i >= 0; i--) if (timeLeft(links![i]) !== null) return links![i];
  return null;
}

export type ShareRequest = { path: string; host?: UploadHost };

/** Asks before uploading (unless turned off), then uploads. Lives once in App. */
export function ShareConfirm({
  req,
  settings,
  update,
  onClose,
  onOpenSettings,
  onError,
}: {
  req: ShareRequest;
  settings: Settings;
  update: (p: Partial<Settings>) => Promise<void>;
  onClose: () => void;
  onOpenSettings: () => void;
  onError: (m: string) => void;
}) {
  const [host, setHost] = useState<UploadHost>(req.host ?? settings.uploadHost);
  const [dontAsk, setDontAsk] = useState(false);
  const [have, setHave] = useState<{ discordWebhook: boolean; streamablePassword: boolean } | null>(null);
  const [reach, setReach] = useState<[UploadHost, Reach[]][] | null>(null);
  useEffect(() => {
    api.secretsSet().then(setHave).catch(() => {});
    api.checkSharing().then(setReach).catch(() => {});
  }, []);
  const name = req.path.split("\\").pop();
  const needsSetup = (host === "discord" && have && !have.discordWebhook) || (host === "streamable" && have && !have.streamablePassword);
  const problems = (reach?.find(([h]) => h === host)?.[1] ?? []).filter((r) => r.state !== "ok");
  const go = async () => {
    if (needsSetup) {
      onClose();
      onOpenSettings();
      return;
    }
    if (dontAsk) await update({ confirmUploads: false }).catch(() => {});
    onClose();
    api.uploadClip(req.path, host).catch((e) => onError(errText(e)));
  };
  return (
    <ConfirmDialog
      title={`Share “${name}”?`}
      confirmLabel={needsSetup ? "Set it up…" : `Upload to ${hostLabel(host)}`}
      onConfirm={go}
      onCancel={onClose}
    >
      <label className="setting" style={{ borderTop: 0, minHeight: 0 }}>
        <span>Upload to</span>
        <Select className="select" value={host} onChange={(e) => setHost(e.target.value as UploadHost)}>
          {UPLOAD_HOSTS.map((h) => (
            <option key={h.id} value={h.id}>
              {h.label}
            </option>
          ))}
        </Select>
      </label>
      <ul className="modal-list wrap">
        <li>{UPLOAD_HOSTS.find((h) => h.id === host)?.hint}</li>
        <li>Hindsight makes an MP4 that plays inside Discord, uploads it, and copies the link.</li>
        <li>Anyone who has the link can watch the clip.</li>
        {needsSetup && (
          <li className="err-text">
            {host === "discord" ? "Add a Discord webhook first (Settings → Sharing)." : "Add your Streamable account first (Settings → Sharing)."}
          </li>
        )}
        {problems.map((p) => (
          <li key={p.host} className="err-text">
            {p.state === "blocked"
              ? `Your network blocks ${p.host} (an ad-blocker like Pi-hole or AdGuard), so it won't work on this Wi-Fi.`
              : p.state === "webhook"
                ? p.host
                : `This PC can't reach ${p.host} right now.`}
          </li>
        ))}
      </ul>
      <label className="row" style={{ gap: 8, fontSize: 12 }}>
        <input type="checkbox" checked={dontAsk} onChange={(e) => setDontAsk(e.target.checked)} />
        Don't ask again (turn it back on in Settings → Sharing)
      </label>
    </ConfirmDialog>
  );
}

/** Studio's Share box: pick a service, upload, copy or open the link. */
export function SharePanel({
  path,
  settings,
  progress,
  links,
  onShare,
}: {
  path: string;
  settings: Settings | null;
  progress: UploadProgress | undefined;
  links: Upload[] | undefined;
  onShare: (r: ShareRequest) => void;
}) {
  const [host, setHost] = useState<UploadHost>(settings?.uploadHost ?? "catbox");
  const [copied, setCopied] = useState(false);
  useEffect(() => setHost(settings?.uploadHost ?? "catbox"), [settings?.uploadHost]);
  const busy = progress && (progress.stage === "preparing" || progress.stage === "uploading");
  const last = liveLink(links);
  const left = last ? timeLeft(last) : null;
  const copy = (url: string) =>
    api.copyText(url).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1800);
    });
  const pct = progress?.stage === "uploading" && progress.total ? (progress.sent / progress.total) * 100 : progress?.stage === "preparing" ? 4 : 0;
  return (
    <div className="share-box">
      <div className="row" style={{ gap: 8, flexWrap: "nowrap" }}>
        <Select className="select" value={host} onChange={(e) => setHost(e.target.value as UploadHost)} aria-label="Upload to" disabled={!!busy}>
          {UPLOAD_HOSTS.map((h) => (
            <option key={h.id} value={h.id}>
              {h.label}
            </option>
          ))}
        </Select>
        <button className="btn primary" style={{ flex: 1, minWidth: 0 }} disabled={!!busy} onClick={() => onShare({ path, host })}>
          {busy ? progressText(progress!) : "Upload & copy link"}
        </button>
      </div>
      {busy && (
        <div className="upload-bar" aria-hidden="true">
          <i style={{ width: `${pct}%` }} />
        </div>
      )}
      {progress?.stage === "error" && <small className="err-text">{progress.error}</small>}
      {progress?.stage === "done" && progress.warning && <small className="err-text">{progress.warning}</small>}
      {last && !busy && (
        <div className="share-link">
          <small className="muted">
            {hostLabel(last.host)} · {fmtSize(last.bytes)} · {left ? `${left} left` : "doesn't expire"}
          </small>
          <span className="link-text" title={last.url}>
            {last.url.replace(/^https?:\/\//, "").split("?")[0]}
          </span>
          <span className="row" style={{ gap: 6 }}>
            <button className="btn small" onClick={() => copy(last.url)}>
              {copied ? "Copied" : "Copy"}
            </button>
            <button className="btn small" onClick={() => openUrl(last.url).catch(() => {})}>
              Open
            </button>
          </span>
        </div>
      )}
      {!last && links?.length ? <small className="muted">The last link has expired. Upload again for a new one.</small> : null}
    </div>
  );
}

type Toggle = (p: { label: string; checked: boolean; onChange: (v: boolean) => void }) => React.ReactElement;

/** Settings → Sharing. Secrets (webhook, password) are sent once and never shown again. */
export function SharingSettings({
  s,
  set,
  Toggle,
  hidden,
  onError,
}: {
  s: Settings;
  set: (p: Partial<Settings>) => void;
  Toggle: Toggle;
  hidden?: boolean;
  onError: (m: string) => void;
}) {
  const [have, setHave] = useState({ discordWebhook: false, streamablePassword: false });
  const [hook, setHook] = useState("");
  const [pw, setPw] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [hookErr, setHookErr] = useState<string | null>(null);
  const [reach, setReach] = useState<[UploadHost, Reach[]][] | null>(null);
  const refresh = useCallback(() => {
    api.secretsSet().then(setHave).catch(() => {});
  }, []);
  useEffect(refresh, [refresh]);
  const saveHook = async (value: string) => {
    setBusy("hook");
    setHookErr(null);
    try {
      await api.setSecret("discord_webhook", value);
      setHook("");
      refresh();
    } catch (e) {
      setHookErr(errText(e));
    } finally {
      setBusy(null);
    }
  };
  const savePw = async (value: string) => {
    try {
      await api.setSecret("streamable_password", value);
      setPw("");
      refresh();
    } catch (e) {
      onError(errText(e));
    }
  };
  const check = async () => {
    setBusy("check");
    try {
      setReach(await api.checkSharing());
    } catch (e) {
      onError(errText(e));
    } finally {
      setBusy(null);
    }
  };
  const host = UPLOAD_HOSTS.find((h) => h.id === s.uploadHost);
  return (
    <section className="group" hidden={hidden}>
      <h2>Sharing</h2>
      <label className="setting">
        <span>
          Upload to
          <small>{host?.hint}</small>
        </span>
        <Select className="select" value={s.uploadHost} onChange={(e) => set({ uploadHost: e.target.value as UploadHost })}>
          {UPLOAD_HOSTS.map((h) => (
            <option key={h.id} value={h.id}>
              {h.label}
            </option>
          ))}
        </Select>
      </label>
      {s.uploadHost === "litterbox" && (
        <label className="setting sub">
          <span>Keep clips on Litterbox for</span>
          <Select className="select" value={s.litterboxTime} onChange={(e) => set({ litterboxTime: e.target.value as Settings["litterboxTime"] })}>
            <option value="1h">1 hour</option>
            <option value="12h">12 hours</option>
            <option value="24h">1 day</option>
            <option value="72h">3 days</option>
          </Select>
        </label>
      )}
      <label className="setting">
        <span>
          Ask before uploading
          <small>Shows what's about to be shared first. Links are public to anyone who has them.</small>
        </span>
        <Toggle label="Ask before uploading" checked={s.confirmUploads} onChange={(v) => set({ confirmUploads: v })} />
      </label>
      <label className="setting">
        <span>
          Upload every new clip automatically
          <small>Off by default. When on, each saved clip uploads in the background and its link is copied, ShareX-style.</small>
        </span>
        <Toggle label="Auto-upload" checked={s.autoUpload} onChange={(v) => set({ autoUpload: v })} />
      </label>
      <div className="setting">
        <span>
          Check connection
          <small>Makes sure this PC can reach each service and open its links. Uploads nothing.</small>
        </span>
        <button className="btn small" disabled={busy === "check"} onClick={check}>
          {busy === "check" ? "Checking…" : "Check"}
        </button>
      </div>
      {reach && (
        <ul className="reach-list">
          {reach.map(([h, rs]) => {
            const bad = rs.filter((r) => r.state !== "ok");
            return (
              <li key={h} className={bad.length ? "bad" : "ok"}>
                <strong>{hostLabel(h)}</strong>{" "}
                {bad.length === 0
                  ? "works"
                  : bad
                      .map((r) =>
                        r.state === "blocked"
                          ? `${r.host} is blocked by your network's DNS (Pi-hole, AdGuard or similar). Allow it there, or use another service.`
                          : r.state === "webhook"
                            ? r.host
                            : `can't reach ${r.host}`,
                      )
                      .join(" ")}
              </li>
            );
          })}
        </ul>
      )}

      <h3 className="group-sub">Discord</h3>
      <div className="setting stack-setting">
        <span>
          Discord webhook
          <small>
            A webhook is a private posting address for one channel in your Discord server: whoever has it can post there, nothing
            else. Hindsight uses it to drop your clips into that channel, no bot or login needed.
          </small>
          <small>
            Make one in Discord: right-click the channel → <b>Edit Channel</b> → <b>Integrations</b> → <b>Webhooks</b> →{" "}
            <b>New Webhook</b> → <b>Copy Webhook URL</b>, then paste it here. You need Manage Webhooks permission in that server.
          </small>
        </span>
        {have.discordWebhook ? (
          <div className="row" style={{ gap: 8 }}>
            <span className="src-tag ok with-icon"><Icon name="check" size={12} />Connected{s.discordWebhookName ? ` as “${s.discordWebhookName}”` : ""}</span>
            <span style={{ flex: 1 }} />
            <button className="btn small" disabled={busy === "hook"} onClick={() => saveHook("")}>
              Remove
            </button>
          </div>
        ) : (
          <div className="row" style={{ gap: 8, flexWrap: "nowrap" }}>
            <input
              className="text-input grow"
              type="password"
              placeholder="https://discord.com/api/webhooks/…"
              value={hook}
              onChange={(e) => setHook(e.target.value)}
              aria-label="Discord webhook address"
            />
            <button className="btn small" disabled={!hook.trim() || busy === "hook"} onClick={() => saveHook(hook)}>
              {busy === "hook" ? "Checking…" : "Check & save"}
            </button>
          </div>
        )}
        {hookErr && <small className="err-text">{hookErr}</small>}
      </div>
      {s.uploadHost !== "discord" && (
        <label className="setting sub">
          <span>
            Also post the link to that channel
            <small>A link plays inline in Discord whatever the clip's size, so no 10 MB squeeze.</small>
          </span>
          <Toggle label="Post links to Discord" checked={s.postLinkToDiscord && have.discordWebhook} onChange={(v) => set({ postLinkToDiscord: v })} />
        </label>
      )}

      <h3 className="group-sub">Streamable</h3>
      <div className="setting stack-setting">
        <span>
          Streamable account
          <small>
            {have.streamablePassword
              ? `Signed in as ${s.streamableEmail || "?"}. The password is stored encrypted on this PC.`
              : "Only needed to upload to Streamable. The password is stored encrypted on this PC."}
          </small>
        </span>
        {have.streamablePassword ? (
          <div className="row" style={{ gap: 8 }}>
            <span style={{ flex: 1 }} />
            <button className="btn small" onClick={() => savePw("")}>
              Sign out
            </button>
          </div>
        ) : (
          <div className="cred-row">
            <input className="text-input" placeholder="Email" value={s.streamableEmail} onChange={(e) => set({ streamableEmail: e.target.value })} aria-label="Streamable email" />
            <input className="text-input" type="password" placeholder="Password" value={pw} onChange={(e) => setPw(e.target.value)} aria-label="Streamable password" />
            <button className="btn small" disabled={!pw || !s.streamableEmail} onClick={() => savePw(pw)}>
              Save
            </button>
          </div>
        )}
      </div>
    </section>
  );
}
