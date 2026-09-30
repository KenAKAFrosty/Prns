import type { DestinationHash } from "personal-rns/contract";

import { NotYetImplementedScreen } from "@/features/placeholder-screen";
import { screenById } from "@/navigation/catalog";

export function InboxScreen() {
  return <NotYetImplementedScreen entry={screenById("inbox.index")} />;
}

export function ConversationScreen(_props: { readonly destination: DestinationHash }) {
  return <NotYetImplementedScreen entry={screenById("inbox.conversation")} />;
}

export function ComposeScreen(_props: { readonly initialDestination: string | null }) {
  return <NotYetImplementedScreen entry={screenById("inbox.compose")} />;
}
