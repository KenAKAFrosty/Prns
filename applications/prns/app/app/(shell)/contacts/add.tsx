// route-id: contacts.add

import { AddContactScreen } from "@/features/contacts/contacts-screen";
import { CatalogRouteGuard } from "@/features/placeholder-screen";

export default function AddContactRoute() {
  return (
    <CatalogRouteGuard screenId="contacts.add">
      <AddContactScreen />
    </CatalogRouteGuard>
  );
}
