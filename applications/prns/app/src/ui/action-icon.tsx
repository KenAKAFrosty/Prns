import Svg, { Path } from "react-native-svg";
import type { ColorValue } from "react-native";

const paths = {
  more: "M5 11v2 M12 11v2 M19 11v2",
  back: "M14 6l-6 6 6 6 M8 12h13",
  close: "M6 6l12 12 M18 6 6 18",
  edit: "m15 4 5 5 M4 20l5-1L21 7l-5-5L4 14Z",
  pin: "m8 3 8 0 -1 7 4 4H5l4-4Z M12 14v7",
  share: "M12 15V3 M7 8l5-5 5 5 M5 13v7h14v-7",
  add: "M12 5v14 M5 12h14",
  card: "M3 5h18v14H3Z M7 9h3v3H7Z M6 15h5 M14 9h4 M14 13h4",
  qr: "M3 3h6v6H3Z M15 3h6v6h-6Z M3 15h6v6H3Z M15 15h3v3h3v3h-6Z",
} as const;
export type ActionIconName = keyof typeof paths;
export function ActionIcon({ name, color }: { name: ActionIconName; color: ColorValue }) {
  return <Svg width={20} height={20} viewBox="0 0 24 24" accessibilityElementsHidden importantForAccessibility="no">
    <Path d={paths[name]} fill="none" stroke={color} strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" />
  </Svg>;
}
