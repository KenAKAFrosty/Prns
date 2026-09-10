// route-id: inbox.index

import { InboxScreen } from "@/features/inbox/inbox-screen";
import { CatalogRouteGuard } from "@/features/placeholder-screen";

export default function InboxRoute() {
  return (
    <CatalogRouteGuard screenId="inbox.index">
      <InboxScreen />
    </CatalogRouteGuard>
  );
}
