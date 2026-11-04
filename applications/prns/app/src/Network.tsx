import { Button, Text } from "./ui/primitives";
import { useState } from "react";
import { View } from "react-native";
import type { prns_service_runtime_uniffi as owners } from "@prns/app-bindings";
import { Connections } from "./connections/Connections";
import { NetworkInspection } from "./NetworkInspection";
import { Nodes } from "./nodes/Nodes";
import { useStyles } from "./ui";

/** Saved settings remain mounted when their live host generation changes. */
export function Network({ owner, active }: { owner?: owners.ServiceRuntimeLike | undefined; active: boolean }) {
  const styles = useStyles();
  const [tab, setTab] = useState<"Connections" | "Nodes" | "Inspection">("Connections");
  return <View style={styles.section}>
    <View style={styles.row} accessibilityRole="tablist">
      {(["Connections", "Nodes", "Inspection"] as const).map((name) => <Button key={name} title={name}
        accessibilityState={{ selected: tab === name }} onPress={() => { setTab(name); }} />)}
    </View>
    <View style={tab === "Connections" ? styles.section : styles.hidden}>
      <Connections owner={owner} active={active && tab === "Connections"} />
    </View>
    <View style={tab === "Nodes" ? styles.section : styles.hidden}>
      {owner === undefined ? <Text>Open the installation to manage nodes.</Text>
        : <Nodes owner={owner} active={active && tab === "Nodes"} />}
    </View>
    {tab === "Inspection" && (owner === undefined
      ? <Text>Open the installation to inspect the live network.</Text>
      : <NetworkInspection owner={owner} active={active} />)}
  </View>;
}
