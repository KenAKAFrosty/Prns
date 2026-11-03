import { Link, useLocalSearchParams, usePathname, useRouter } from "expo-router";
import { useCallback, useRef } from "react";
import { Nodes, nodeSections } from "../nodes/Nodes";
import type { NodeSection } from "../nodes/Nodes";
import { nodeDetailTabs } from "../nodes/Manage";
import type { NodeDetailTab } from "../nodes/Manage";
import { scalar, useRouteLease } from "../navigation";
import { useInstallation } from "../runtime/InstallationProvider";
import { Button, Heading, ListRow, Screen, Text } from "../ui/primitives";

export function NodesScreen() {
  const { owner, networkKey } = useInstallation();
  const params = useLocalSearchParams<{ nodeId?: string; section?: string; tab?: string }>();
  const targetId = scalar(params.nodeId)?.toLowerCase();
  const section = scalar(params.section) ?? "Nodes";
  const tab = scalar(params.tab) ?? "Overview";
  const router = useRouter();
  const pathname = usePathname();
  const { active, isCurrent } = useRouteLease();
  const scope = `${pathname}:${targetId ?? ""}:${section}:${tab}`;
  const currentScope = useRef(scope); currentScope.current = scope;
  const canNavigate = useCallback(() => isCurrent() && currentScope.current === scope, [isCurrent, scope]);
  const openTarget = useCallback((nodeId: string) => {
    if (canNavigate()) router.push({ pathname: "/nodes/managed/[nodeId]", params: { nodeId } });
  }, [router, canNavigate]);
  const openSection = useCallback((section: NodeSection) => {
    if (canNavigate()) router.push({ pathname: "/nodes", params: { section } });
  }, [router, canNavigate]);
  const openTab = useCallback((tab: NodeDetailTab) => {
    if (canNavigate() && targetId !== undefined) router.replace({ pathname: "/nodes/managed/[nodeId]", params: { nodeId: targetId, tab } });
  }, [router, canNavigate, targetId]);
  if ((params.nodeId !== undefined && (targetId === undefined || !/^[0-9a-f]{32}$/.test(targetId))) ||
      (params.section !== undefined && scalar(params.section) === undefined) || !nodeSections.some((value) => value === section) ||
      (params.tab !== undefined && (targetId === undefined || scalar(params.tab) === undefined)) || !nodeDetailTabs.some((value) => value === tab) ||
      (targetId !== undefined && section !== "Nodes")) {
    return <Screen><Heading>Node unavailable</Heading><Text>The node or section reference is invalid. Choose a saved node from Nodes.</Text><Link href="/nodes" asChild><Button title="Back to Nodes" /></Link></Screen>;
  }
  return <Screen><Heading help={{ label: "About nodes", content: <Text>A radio board is optional for messaging. Pair a node to use its supported controls from this phone. Check the node you selected before making changes.</Text> }}>{targetId === undefined ? "Nodes" : "Manage node"}</Heading>
    {targetId !== undefined && <Link href="/nodes" asChild><Button title="Back to Nodes" /></Link>}
    {targetId === undefined && section === "Nodes" && <Link href="/nodes/phone" asChild><ListRow title="This phone" subtitle="Local network and controller status" /></Link>}
    {owner === undefined ? <Link href="/setup" asChild><Button title="Open phone setup" /></Link> : <Nodes owner={owner} active={active} networkKey={networkKey}
      section={section as NodeSection} targetId={targetId} detailTab={tab as NodeDetailTab} onOpenTarget={openTarget} onSectionChange={openSection} onDetailTabChange={openTab} />}
  </Screen>;
}
