import Svg, { Path } from "react-native-svg";

const paths: Record<string, string> = {
  inbox: "M5 4h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H9l-5 3v-3H3V6a2 2 0 0 1 2-2Z M7 9h10 M7 13h6",
  contacts:
    "M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8Z M2 21v-2a7 7 0 0 1 14 0v2 M17 4a4 4 0 0 1 0 8 M18 15a6 6 0 0 1 4 5",
  nodes: "M9 8h6v13H9Z M10 16h4 M12 8V3 M5 4a10 10 0 0 0 0 10 M19 4a10 10 0 0 1 0 10",
  more: "M4 6h16 M4 12h16 M4 18h16",
};

export function NavigationIcon({
  section,
  color,
}: {
  readonly section: string;
  readonly color: string;
}) {
  return (
    <Svg
      width={22}
      height={22}
      viewBox="0 0 24 24"
      accessibilityElementsHidden
      importantForAccessibility="no"
    >
      <Path
        d={paths[section] ?? "M4 6h16 M4 12h16 M4 18h16"}
        fill="none"
        stroke={color}
        strokeWidth={1.7}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </Svg>
  );
}
