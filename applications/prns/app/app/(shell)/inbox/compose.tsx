// route-id: inbox.compose

import { useLocalSearchParams } from "expo-router";

import { parseDestinationHash } from "@/features/contacts/format";
import { ComposeScreen } from "@/features/inbox/inbox-screen";
import { NotFoundScreen } from "@/features/placeholder-screen";

export default function ComposeRoute() {
  const params = useLocalSearchParams<{ readonly destination?: string | string[] }>();
  if (Array.isArray(params.destination)) {
    return <NotFoundScreen backPath="/inbox" />;
  }
  if (params.destination !== undefined && parseDestinationHash(params.destination) === null) {
    return <NotFoundScreen backPath="/inbox" />;
  }
  return <ComposeScreen initialDestination={params.destination ?? null} />;
}
