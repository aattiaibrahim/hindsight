import { useEffect, useRef } from "react";
import { ProgressBar, useCountProgress } from "./Progress";

/** "Move 3 clips to the Recycle Bin?" — Enter confirms, Esc cancels. */
export function ConfirmDelete({
  paths,
  busy,
  onConfirm,
  onCancel,
  nas = false,
  wholeFolders = [],
}: {
  /** Folders every clip of which is being deleted: they're left empty and removed */
  wholeFolders?: string[];
  /** Some of them are on the NAS (those copies are deleted too) */
  nas?: boolean;
  paths: string[];
  busy: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const confirmBtn = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    confirmBtn.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  const n = paths.length;
  const names = paths.slice(0, 4).map((p) => p.split("\\").pop());
  // How far a big delete has got (it goes in batches).
  const progress = useCountProgress("delete-progress", busy);
  const pct = progress && progress[1] ? Math.round((progress[0] / progress[1]) * 100) : null;
  const title =
    wholeFolders.length === 1
      ? `Delete the whole “${wholeFolders[0]}” folder?`
      : wholeFolders.length > 1
        ? `Delete ${n} clips and ${wholeFolders.length} whole folders?`
        : n === 1
          ? "Delete this clip?"
          : `Delete ${n} clips?`;
  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div
        className="modal"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="del-title"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 id="del-title">{title}</h2>
        {wholeFolders.length > 0 && (
          <p className="err-text">
            {wholeFolders.length === 1
              ? `Every clip in “${wholeFolders[0]}” is selected (${n}), so the folder will be empty and is removed too.`
              : `Every clip in ${wholeFolders.map((f) => `“${f}”`).join(", ")} is selected, so those folders will be empty and are removed too.`}
          </p>
        )}
        <p className="muted">
          {nas
            ? `${n === 1 ? "It's" : "They're"} deleted from this PC and the NAS. Copies on this PC go to the Recycle Bin, and the NAS copies go to the NAS app's Trash for 30 days, so you can still undo this.`
            : `${n === 1 ? "It goes" : "They go"} to the Recycle Bin, so you can still undo this.`}
        </p>
        <ul className="modal-list">
          {names.map((name) => (
            <li key={name}>{name}</li>
          ))}
          {n > 4 && <li className="muted">and {n - 4} more</li>}
        </ul>
        {busy && progress && progress[1] > 1 && (
          <div className="job-progress muted">
            Deleting {Math.min(progress[0], progress[1])} of {progress[1]}… ({pct}%)
            <ProgressBar pct={pct} />
          </div>
        )}
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button className="btn" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button ref={confirmBtn} className="btn danger" onClick={onConfirm} disabled={busy}>
            {busy ? (pct != null ? `Deleting… ${pct}%` : "Deleting…") : wholeFolders.length ? "Delete folder" + (wholeFolders.length > 1 ? "s" : "") : n === 1 ? "Delete" : `Delete ${n}`}
          </button>
        </div>
      </div>
    </div>
  );
}
