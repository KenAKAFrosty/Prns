import { ContactsScreen } from "@/features/contacts/contacts-screen";
import { MasterDetailLayout } from "@/ui/master-detail-layout";

export default function ContactsLayout() {
  return (
    <MasterDetailLayout
      emptyDescription="Choose a contact from the list to view or edit it."
      emptyTitle="Select a contact"
      master={<ContactsScreen />}
      rootPath="/contacts"
      sectionLabel="Contacts"
    />
  );
}
