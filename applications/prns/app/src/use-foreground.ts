import { useEffect, useState } from "react";
import { AppState } from "react-native";

/** Unknown startup state is not evidence that a person can see the screen. */
export function useForeground(): boolean {
  const [active, setActive] = useState(AppState.currentState === "active");
  useEffect(() => {
    const subscription = AppState.addEventListener("change", (state) => setActive(state === "active"));
    setActive(AppState.currentState === "active");
    return () => subscription.remove();
  }, []);
  return active;
}
