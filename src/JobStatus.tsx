import { useJobs } from "./activity";
import { Icon } from "./icons";

/**
 * A clip being trimmed (compressed, exported…) in Studio, in the status bar, so you
 * see it from any page: "Trimming Team-up ult · 45%". Click to open the clip.
 */
export function JobStatusItem({ onOpen }: { onOpen?: (path: string) => void }) {
  const jobs = useJobs();
  if (!jobs.length) return null;
  const j = jobs[0];
  const name = j.path.split(/[\\/]/).pop()?.replace(/\.[^.]+$/, "") ?? "";
  const more = jobs.length - 1;
  return (
    <button
      className="sb-item sb-job"
      onClick={() => onOpen?.(j.path)}
      title={`${j.label} ${name} in Studio${more ? ` (and ${more} more)` : ""}: click to open it`}
    >
      <Icon name="totrim" size={15} />
      <span className="sb-text">
        {j.label} {name}
        {j.pct != null && <span className="sb-pct"> · {j.pct}%</span>}
        {more > 0 && <span className="muted"> +{more}</span>}
      </span>
    </button>
  );
}
