// route-id: contacts.index

import { ContactsScreen } from "@/features/contacts/contacts-screen";
import { CatalogRouteGuard } from "@/features/placeholder-screen";

export default function ContactsRoute() {
  return (
    <CatalogRouteGuard screenId="contacts.index">
      <ContactsScreen />
    </CatalogRouteGuard>
  );
}
