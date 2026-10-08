import { usePathname } from "expo-router";

import { ShellLayout } from "@/ui/shell";
import { ContactRuntimeProvider } from "@/native/contact-runtime-context";
import {
  DevelopmentRuntimeProvider,
  routeConsumesDevelopmentSnapshot,
} from "@/native/development-runtime-context";

export default function ProductShellLayout() {
  const pathname = usePathname();

  return (
    <DevelopmentRuntimeProvider refreshActive={routeConsumesDevelopmentSnapshot(pathname)}>
      <ContactRuntimeProvider>
        <ShellLayout />
      </ContactRuntimeProvider>
    </DevelopmentRuntimeProvider>
  );
}
