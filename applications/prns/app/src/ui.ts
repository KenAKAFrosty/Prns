import { useMemo } from "react";
import { StyleSheet } from "react-native";
import { useAppPalette } from "./ui/theme";

export function useStyles() {
  const palette = useAppPalette();
  return useMemo(() => StyleSheet.create({
  root: { flex: 1, backgroundColor: palette.background },
  page: { padding: 24, gap: 24, maxWidth: 1040, width: "100%", alignSelf: "center" },
  brand: { fontSize: 34, fontWeight: "700", color: palette.text },
  heading: { fontSize: 24, fontWeight: "600", color: palette.text },
  title: { fontSize: 18, fontWeight: "600", color: palette.text },
  section: { gap: 14 },
  row: { flexDirection: "row", flexWrap: "wrap", gap: 8, alignItems: "center" },
  card: { gap: 12, padding: 16, backgroundColor: palette.surface, borderColor: palette.border, borderWidth: 1, borderRadius: 14 },
  input: { borderWidth: 1, borderColor: palette.border, padding: 14, borderRadius: 10, color: palette.text, backgroundColor: palette.surface },
  error: { color: palette.destructive, backgroundColor: palette.warningSurface, padding: 12, borderRadius: 8 },
  fingerprint: { fontFamily: "monospace", color: palette.textMuted, fontSize: 12 },
  muted: { color: palette.textMuted },
  hidden: { display: "none" },
  }), [palette]);
}
