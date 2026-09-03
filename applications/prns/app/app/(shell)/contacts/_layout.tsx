import { Slot } from "expo-router";

import { ContactRuntimeProvider } from "@/native/contact-runtime-context";

export default function ContactsLayout() {
  return (
    <ContactRuntimeProvider>
      <Slot />
    </ContactRuntimeProvider>
  );
}
