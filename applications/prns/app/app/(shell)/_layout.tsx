import { ShellLayout } from "@/ui/shell";
import { ContactRuntimeProvider } from "@/native/contact-runtime-context";
import { DevelopmentRuntimeProvider } from "@/native/development-runtime-context";

export default function ProductShellLayout() {
  return (
    <DevelopmentRuntimeProvider>
      <ContactRuntimeProvider>
        <ShellLayout />
      </ContactRuntimeProvider>
    </DevelopmentRuntimeProvider>
  );
}
