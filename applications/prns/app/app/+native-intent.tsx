/** Contact URLs are reviewed by the retained incoming-link queue, never routed as commands. */
export function redirectSystemPath({ path }: { path: string; initial: boolean }): string {
  if (path.length > 16_384) return "/invalid-link";
  // Validate encoding once with the native linear decoder before the router's
  // query parser sees it. Malformed escapes must never enter its fallback decoder.
  try { decodeURIComponent(path); } catch { return "/invalid-link"; }
  return /^rns-contact:\/\//i.test(path) ? "/contacts" : path;
}
