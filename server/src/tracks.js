// Audio tracks that are the other tracks already mixed together.
//
// Medal saves "All Audio" next to "Game" and "Mic"; other recorders call it
// "Mix" or "Master". Mixing that track in again with the separate ones plays the
// game and mic twice, so whenever a clip's sound becomes one track (phone
// version, exports, montages), it's left out. Same names as the desktop's
// library::MIX_NAMES.

const MIX_NAMES = new Set(["all audio", "all", "mix", "mixed", "master", "combined", "everything", "full mix", "main mix", "stereo mix"]);

/** A stream's own name: its title tag, or the name tag MP4s from Medal use. */
export const trackName = (s) => String(s?.tags?.title ?? s?.tags?.TITLE ?? s?.tags?.name ?? "").trim();

/** Which track is a mix of the others, if exactly one is (and there are others). */
export function mixCopy(names) {
  if (names.length < 2) return null;
  const hits = names.map((n, i) => (MIX_NAMES.has(String(n).toLowerCase()) ? i : -1)).filter((i) => i >= 0);
  return hits.length === 1 ? hits[0] : null;
}

/** The tracks to mix into one: all of them, except a track that already mixes the others. */
export function tracksToMix(names) {
  const skip = mixCopy(names);
  return names.map((_, i) => i).filter((i) => i !== skip);
}
