import { useEffect, useState } from "react";
import { api, errText, MixTracksStatus } from "./api";
import { ProgressBar } from "./Progress";

/**
 * Settings → Library: give clips saved before 0.1.48 the All audio track, so they
 * play with full sound in other players too. Clips on this PC are done here (their
 * NAS copies follow), clips only on the NAS by the NAS app (nothing crosses the network).
 */
export function MixTracksSetting({ nasPaired }: { nasPaired: boolean }) {
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [nas, setNas] = useState<MixTracksStatus | null>(null);
  const [watch, setWatch] = useState(false);

  // While the NAS app works through its clips, show how far it is.
  useEffect(() => {
    if (!nasPaired) return;
    let alive = true;
    let t = 0;
    const load = () =>
      api
        .mixTracksStatus()
        .then((s) => {
          if (!alive) return;
          setNas(s);
          if (s?.running || watch) t = window.setTimeout(load, s?.running ? 2000 : 4000);
          if (s && !s.running && watch) setWatch(false);
        })
        .catch(() => {});
    load();
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [nasPaired, watch]);

  const start = async () => {
    setBusy(true);
    setNote(null);
    try {
      const r = await api.addMixTracks();
      const parts = [
        r.addedHere ? `${r.addedHere} clip${r.addedHere === 1 ? "" : "s"} on this PC done` : "Nothing to do on this PC",
        r.failedHere.length ? `${r.failedHere.length} couldn't be done (${r.failedHere[0][1]})` : "",
        r.nasStarted ? "the NAS app is doing the clips on the NAS" : r.nasError ? `couldn't start it on the NAS (${r.nasError})` : "",
      ].filter(Boolean);
      setNote(parts.join(", ") + ".");
      if (r.nasStarted) setWatch(true);
    } catch (e) {
      setNote(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const running = !!nas?.running;
  const pct = nas && nas.total ? Math.round((nas.checked / nas.total) * 100) : null;
  return (
    <div className="setting stack-setting">
      <span>
        All audio track for older clips
        <small>
          Clips saved from 0.1.48 on start with desktop, mic and Discord mixed, so other players (and phones and the NAS app)
          play all the sound. This gives older Hindsight clips the same. The video and the separate tracks are copied untouched
          and each clip keeps its date. Hindsight still uses the separate tracks, so nothing plays twice. Only Hindsight's own
          clips: Medal and OBS files are left alone. Old versions are kept for 30 days in the NAS app's Trash.
        </small>
      </span>
      <span className="row" style={{ gap: 10, flexWrap: "wrap" }}>
        <button className="btn small" disabled={busy || running} onClick={start}>
          {busy ? "Working on this PC…" : running ? "Adding on the NAS…" : "Add to older clips"}
        </button>
        {note && <small className="muted">{note}</small>}
      </span>
      {running && nas && (
        <div className="job-progress muted">
          On the NAS: {nas.checked} of {nas.total} checked, {nas.added} given the track{nas.failed ? `, ${nas.failed} couldn't be done` : ""} ({pct}%)
          <ProgressBar pct={pct} />
        </div>
      )}
      {!running && nas?.finishedAt && (
        <small className="muted">
          Last run on the NAS: {nas.added} clip{nas.added === 1 ? "" : "s"} given the track
          {nas.failed ? `, ${nas.failed} couldn't be done${nas.lastError ? ` (${nas.lastError})` : ""}` : ""}.
        </small>
      )}
    </div>
  );
}
