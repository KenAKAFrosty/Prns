import { ActivityIndicator, Pressable, StyleSheet, View } from "react-native";
import Svg, { Path } from "react-native-svg";
import { useAppPalette } from "../ui/theme";
import { Button, Text } from "../ui/primitives";

export function Paging({ label, active, page }: { label: string; active: boolean; page: {
  busy: boolean; loaded: boolean; error: string | undefined; page: number; hasNext: boolean; hasPrevious: boolean;
  next: () => void; previous: () => void; first: () => void; retry: () => void;
} }) {
  const palette = useAppPalette();
  const loading = page.busy && !page.loaded && page.error === undefined;
  const navigating = page.page > 1 || page.hasNext || page.hasPrevious;
  const blocked = !active || !page.loaded || page.error !== undefined;
  if (!loading && !navigating && page.error === undefined) return null;
  return <View style={styles.section}>
    {page.error !== undefined && <View style={styles.section}>
      <Text accessibilityRole="alert">{page.error}</Text>
      <Button title={`Retry ${label.toLowerCase()} page`} disabled={!active} onPress={page.retry} />
    </View>}
    {loading && <ActivityIndicator accessibilityLabel={`Loading ${label.toLowerCase()}`} />}
    {navigating && <View style={styles.row}>
      <PageArrow label={`First ${label.toLowerCase()} page`} direction="first"
        disabled={!active || page.page === 1 || loading} onPress={page.first} />
      <PageArrow label={`Previous ${label.toLowerCase()}`} direction="previous"
        disabled={blocked || !page.hasPrevious} onPress={page.previous} />
      <Text accessibilityLabel={`${label} page ${page.page}`} style={[styles.counter, { color: palette.textMuted }]}>Page {page.page}</Text>
      <PageArrow label={`Next ${label.toLowerCase()}`} direction="next"
        disabled={blocked || !page.hasNext} onPress={page.next} />
    </View>}
  </View>;
}

const arrows = {
  first: "M5 5v14 M18 6l-6 6 6 6",
  previous: "M15 6l-6 6 6 6",
  next: "M9 6l6 6-6 6",
};
function PageArrow({ label, direction, disabled, onPress }: {
  label: string; direction: keyof typeof arrows; disabled: boolean; onPress: () => void;
}) {
  const palette = useAppPalette();
  return <Pressable accessibilityRole="button" accessibilityLabel={label} accessibilityState={{ disabled }}
    disabled={disabled} onPress={onPress}
    style={({ pressed }) => [styles.arrow, { borderColor: palette.border, backgroundColor: pressed ? palette.surfaceRaised : "transparent" }]}>
    <Svg width={20} height={20} viewBox="0 0 24 24" accessibilityElementsHidden importantForAccessibility="no">
      <Path d={arrows[direction]} fill="none" stroke={disabled ? palette.border : palette.accent} strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" />
    </Svg>
  </Pressable>;
}
const styles = StyleSheet.create({
  section: { gap: 8 },
  row: { flexDirection: "row", alignItems: "center", justifyContent: "center", gap: 8 },
  arrow: { width: 44, height: 44, borderWidth: 1, borderRadius: 8, alignItems: "center", justifyContent: "center" },
  counter: { minWidth: 64, textAlign: "center", fontSize: 14, lineHeight: 20, flexShrink: 1 },
});
