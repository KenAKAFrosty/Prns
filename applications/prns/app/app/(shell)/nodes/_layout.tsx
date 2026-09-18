import { NodesScreen } from "@/features/nodes/nodes-screen";
import { MasterDetailLayout } from "@/ui/master-detail-layout";

export default function NodesLayout() {
  return (
    <MasterDetailLayout
      emptyDescription="Choose this device or a paired node to view its details."
      emptyTitle="Select a node"
      master={<NodesScreen />}
      rootPath="/nodes"
      sectionLabel="Nodes"
    />
  );
}
