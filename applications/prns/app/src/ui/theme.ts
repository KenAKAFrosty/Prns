import { useColorScheme } from "react-native";

export type AppPalette = {
  readonly background: string;
  readonly surface: string;
  readonly surfaceRaised: string;
  readonly text: string;
  readonly textMuted: string;
  readonly border: string;
  readonly accent: string;
  readonly accentText: string;
  readonly selected: string;
  readonly selectedText: string;
  readonly warning: string;
  readonly warningSurface: string;
  readonly destructive: string;
  readonly focus: string;
};

const light: AppPalette = {
  background: "#f3f6f2",
  surface: "#ffffff",
  surfaceRaised: "#e8eee9",
  text: "#142019",
  textMuted: "#4d5f54",
  border: "#bdc9c1",
  accent: "#13613e",
  accentText: "#ffffff",
  selected: "#d7eee0",
  selectedText: "#0d4b2f",
  warning: "#714a00",
  warningSurface: "#fff1c7",
  destructive: "#a4262c",
  focus: "#006caa",
};

const dark: AppPalette = {
  background: "#0f1712",
  surface: "#18231c",
  surfaceRaised: "#233129",
  text: "#f0f6f2",
  textMuted: "#b5c4ba",
  border: "#536359",
  accent: "#7bd4a5",
  accentText: "#072416",
  selected: "#254d38",
  selectedText: "#d7f7e4",
  warning: "#ffd682",
  warningSurface: "#4b380d",
  destructive: "#ffb3b5",
  focus: "#8bd5ff",
};

export const space = {
  xs: 4,
  sm: 8,
  md: 16,
  lg: 24,
  xl: 32,
  xxl: 48,
} as const;

export const radius = {
  sm: 8,
  md: 14,
  pill: 999,
} as const;

export type LayoutMode = "compact" | "medium" | "wide";

export function layoutModeForWidth(width: number): LayoutMode {
  if (width < 768) {
    return "compact";
  }
  if (width < 1200) {
    return "medium";
  }
  return "wide";
}

export function useAppPalette(): AppPalette {
  return useColorScheme() === "dark" ? dark : light;
}
