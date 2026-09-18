import { InboxScreen } from "@/features/inbox/inbox-screen";
import { MasterDetailLayout } from "@/ui/master-detail-layout";

export default function InboxLayout() {
  return (
    <MasterDetailLayout
      emptyDescription="Choose a conversation from the list or start a new message."
      emptyTitle="Select a conversation"
      master={<InboxScreen />}
      rootPath="/inbox"
      sectionLabel="Inbox"
    />
  );
}
