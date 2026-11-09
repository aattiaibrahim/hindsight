import { useEffect, useRef } from "react";

/** "Move 3 clips to the Recycle Bin?" — Enter confirms, Esc cancels. */
export function ConfirmDelete({
  paths,
  busy,
  onConfirm,
  onCancel,
  nas = false,
}: {
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
  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div
        className="modal"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="del-title"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 id="del-title">{n === 1 ? "Delete this clip?" : `Delete ${n} clips?`}</h2>
        <p className="muted">
          {nas
            ? `${n === 1 ? "It's" : "They're"} deleted from this PC and the NAS. Copies on this PC go to the Recycle Bin, and the NAS keeps its copies for 30 days in its hidden .hindsight-deleted folder, so you can still undo this.`
            : `${n === 1 ? "It goes" : "They go"} to the Recycle Bin, so you can still undo this.`}
        </p>
        <ul className="modal-list">
          {names.map((name) => (
            <li key={name}>{name}</li>
          ))}
          {n > 4 && <li className="muted">and {n - 4} more</li>}
        </ul>
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button className="btn" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button ref={confirmBtn} className="btn danger" onClick={onConfirm} disabled={busy}>
            {busy ? "Deleting…" : n === 1 ? "Delete" : `Delete ${n}`}
          </button>
        </div>
      </div>
    </div>
  );
}
