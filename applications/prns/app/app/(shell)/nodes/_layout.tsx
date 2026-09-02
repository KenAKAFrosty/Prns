import { Slot } from "expo-router";

import { DevelopmentRuntimeProvider } from "@/native/development-runtime-context";

export default function NodesLayout() {
  return (
    <DevelopmentRuntimeProvider>
      <Slot />
    </DevelopmentRuntimeProvider>
  );
}
