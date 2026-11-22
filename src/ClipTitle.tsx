import { useEffect, useRef, useState } from "react";
import { Pencil } from "lucide-react";
import { api, errText } from "./api";

/**
 * The clip's name at the top of Studio. Click it (or the pencil that shows on
 * hover) to type a new name; Enter or clicking away saves, Esc cancels. The file
 * is renamed on this PC and its NAS copy together; the extension stays.
 */
export function ClipTitle({
  path,
  onRenamed,
  onError,
  disabled,
}: {
  path: string;
  onRenamed: (newPath: string) => void;
  onError: (message: string) => void;
  /** While an edit is saving (the file mustn't change name under it) */
  disabled?: boolean;
}) {
  const name = path.split(/[\\/]/).pop() ?? path;
  const dot = name.lastIndexOf(".");
  const stem = dot > 0 ? name.slice(0, dot) : name;
  const ext = dot > 0 ? name.slice(dot) : "";
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(stem);
  const [busy, setBusy] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  // Saving once: Enter, then the blur that follows, mustn't rename twice.
  const done = useRef(false);

  useEffect(() => {
    if (!editing) setText(stem);
  }, [stem, editing]);
  useEffect(() => {
    if (editing) {
      done.current = false;
      input.current?.focus();
      input.current?.select();
    }
  }, [editing]);

  const save = async () => {
    if (done.current) return;
    done.current = true;
    const next = text.trim();
    if (!next || next === stem) {
      setEditing(false);
      return;
    }
    setBusy(true);
    try {
      const to = await api.renameClip(path, next);
      setEditing(false);
      onRenamed(to);
    } catch (e) {
      onError(errText(e));
      done.current = false;
      input.current?.focus();
    } finally {
      setBusy(false);
    }
  };

  if (editing)
    return (
      <span className="clip-title editing">
        <input
          ref={input}
          className="clip-title-input"
          value={text}
          disabled={busy}
          maxLength={150}
          spellCheck={false}
          aria-label="Clip name"
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            // Studio's shortcuts (Space, arrows, Esc…) don't apply while typing.
            e.stopPropagation();
            if (e.key === "Enter") {
              e.preventDefault();
              save();
            } else if (e.key === "Escape") {
              e.preventDefault();
              done.current = true;
              setEditing(false);
            }
          }}
          onBlur={() => save()}
        />
        <span className="clip-title-ext">{busy ? "Renaming…" : ext}</span>
      </span>
    );

  return (
    <button
      type="button"
      className="clip-title"
      disabled={disabled}
      title={`${path}\nClick to rename`}
      onClick={() => setEditing(true)}
    >
      <span className="clip-title-text">{name}</span>
      <Pencil className="clip-title-pen" size={15} strokeWidth={2} aria-hidden="true" />
    </button>
  );
}
