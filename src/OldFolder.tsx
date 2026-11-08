import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { api, errText, fmtSize, OldFolder } from "./api";

/**
 * Remove a recorder's old folder once all its clips are in Hindsight. Hindsight
 * checks it again right before (not one video may be in it), and it goes to the
 * Recycle Bin, never straight to deleted.
 */
export function OldFolderDialog({ folder, onClose, onDone }: { folder: OldFolder; onClose: () => void; onDone: () => void }) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && !busy && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onClose]);
  const remove = async () => {
    setBusy(true);
    setErr(null);
    try {
      await api.removeOldFolder(folder.path);
      onDone();
    } catch (e) {
      setErr(errText(e));
      setBusy(false);
    }
  };
  return createPortal(
    <div className="modal-backdrop" onClick={() => !busy && onClose()}>
      <div className="modal" role="dialog" aria-modal="true" aria-labelledby="old-title" onClick={(e) => e.stopPropagation()}>
        <h2 id="old-title">Delete the old {folder.label} folder?</h2>
        <p className="muted" style={{ margin: 0 }}>
          {folder.moved
            ? `All ${folder.moved} of its clips are in Hindsight now, and no videos are left in it.`
            : "No videos are in it."}
          {folder.files
            ? ` What's left is ${folder.files} other file${folder.files === 1 ? "" : "s"} (${fmtSize(folder.bytes)}), like the recorder's thumbnails or settings.`
            : " It's empty."}
        </p>
        <code className="old-path">{folder.path}</code>
        <p className="muted" style={{ margin: 0, fontSize: 12.5 }}>
          It goes to the Recycle Bin, and Hindsight stops showing it in your library. Undo for the move still works afterwards.
        </p>
        {err && <small className="err-text">{err}</small>}
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button className="btn" disabled={busy} onClick={onClose}>
            Keep it
          </button>
          <button className="btn danger" disabled={busy} onClick={remove}>
            {busy ? "Deleting…" : "Move to the Recycle Bin"}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
