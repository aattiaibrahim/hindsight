import { useEffect, useState } from "react";
import { api, Settings } from "./api";
import { Select } from "./Select";

/** The same presets as naming.rs (the template, and what it's called). */
const PRESETS: [string, string][] = [
  ["{game} {date} {time}", "Game, date, time"],
  ["{date} {time} {game}", "Date, time, game"],
  ["{game} {date-us} {time12}", "Game, US date, 12-hour time"],
  ["{game} {date-eu} {time}", "Game, day-month-year, time"],
  ["{game} {month}-{day} {time12}", "Game, month-day, 12-hour time"],
];
const CUSTOM = "custom";
const TOKENS = "{game} {date} {time} {date-us} {date-eu} {time12} {year} {month} {day} {hour} {minute} {second} {pc}";

/**
 * Settings → Recording: what new clips are called. A preset, or your own template,
 * with an example of the name a clip saved now would get. This PC's name is left
 * out unless you tick it.
 */
export function FileNameSetting({ s, set }: { s: Settings; set: (p: Partial<Settings>) => void }) {
  const isPreset = PRESETS.some(([t]) => t === s.fileNameTemplate);
  const [custom, setCustom] = useState(!isPreset);
  const [draft, setDraft] = useState(s.fileNameTemplate);
  const [example, setExample] = useState("");
  const template = custom ? draft : s.fileNameTemplate;

  useEffect(() => setDraft(s.fileNameTemplate), [s.fileNameTemplate]);
  // The example, a moment after typing stops.
  useEffect(() => {
    const t = setTimeout(() => {
      api
        .previewFileName(template, s.fileNamePc)
        .then(setExample)
        .catch(() => {});
    }, 150);
    return () => clearTimeout(t);
  }, [template, s.fileNamePc]);
  // Save a custom template a moment after typing stops (not on every key).
  useEffect(() => {
    if (!custom || draft === s.fileNameTemplate || !draft.trim()) return;
    const t = setTimeout(() => set({ fileNameTemplate: draft }), 500);
    return () => clearTimeout(t);
  }, [custom, draft, s.fileNameTemplate, set]);

  return (
    <>
      <div className="setting">
        <span>
          File names
          <small>
            What new clips and recordings are called. Clips already saved keep their names (click a clip's name in Studio to
            rename it).
          </small>
          {example && (
            <small className="file-example" title={example}>
              Example: <b>{example}</b>
            </small>
          )}
        </span>
        <Select
          className="select"
          aria-label="File name format"
          value={custom ? CUSTOM : s.fileNameTemplate}
          onChange={(e) => {
            const v = e.target.value;
            if (v === CUSTOM) {
              setCustom(true);
              setDraft(s.fileNameTemplate);
            } else {
              setCustom(false);
              set({ fileNameTemplate: v });
            }
          }}
        >
          {PRESETS.map(([t, label]) => (
            <option key={t} value={t}>
              {label}
            </option>
          ))}
          <option value={CUSTOM}>Custom…</option>
        </Select>
      </div>
      {custom && (
        <div className="setting sub stack-setting">
          <input
            className="text-input"
            style={{ width: "100%", maxWidth: "none" }}
            value={draft}
            maxLength={120}
            spellCheck={false}
            aria-label="Custom file name"
            placeholder="{game} {date} {time}"
            onChange={(e) => setDraft(e.target.value)}
          />
          <small className="muted">You can use {TOKENS}. Anything else is kept as you type it.</small>
        </div>
      )}
      <label className="setting">
        <span>
          Add this PC's name
          <small>
            Adds this PC's name to the end (like " - GAMING-PC"). Only useful if several PCs save to the same NAS folder; without
            it, a name that's already taken there gets " (2)".
          </small>
        </span>
        <input
          type="checkbox"
          className="switch"
          aria-label="Add this PC's name"
          checked={s.fileNamePc}
          onChange={(e) => set({ fileNamePc: e.target.checked })}
        />
      </label>
    </>
  );
}
