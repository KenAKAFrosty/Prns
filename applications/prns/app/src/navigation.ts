import { useCallback, useRef, useState } from "react";
import { useFocusEffect } from "expo-router";
import { useForeground } from "./use-foreground";

export function useRouteActive(): boolean {
  return useRouteLease().active;
}

/** Async completions can update their captured data but navigate only while
 * their original route still owns focus. Native accepted work is unaffected. */
export function useRouteLease() {
  const foreground = useForeground();
  const foregroundNow = useRef(foreground);
  foregroundNow.current = foreground;
  const focusedNow = useRef(false);
  const [focused, setFocused] = useState(false);
  useFocusEffect(useCallback(() => {
    focusedNow.current = true; setFocused(true);
    return () => { focusedNow.current = false; setFocused(false); };
  }, []));
  const isCurrent = useCallback(() => focusedNow.current && foregroundNow.current, []);
  return { active: focused && foreground, isCurrent };
}

/** Router parameters must remain scalar; repeated parameters are rejected. */
export function scalar(value: string | string[] | undefined): string | undefined {
  return typeof value === "string" ? value : undefined;
}
