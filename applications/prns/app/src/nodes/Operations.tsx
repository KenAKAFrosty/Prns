import { Button, Text } from "../ui/primitives";
import { useState } from "react";
import { View } from "react-native";
import { prns_host_uniffi as rc } from "@prns/app-bindings";
import { useStyles } from "../ui";
import { detail, Facts, label, operationPending, requestLabel } from "./presentation";

export function Operations({ operations, records, changed }: {
  operations: rc.RemoteControlOperationsLike; records: rc.RemoteControlOperationSnapshot[]; changed(): void;
}) {
  const styles = useStyles();
  const [retiring, setRetiring] = useState<bigint>();
  const [error, setError] = useState<string>();
  function cancel(id: bigint) {
    try { setError(detail(operations.cancel(id))); changed(); } catch (cause) { setError(detail(cause)); }
  }
  function retire(id: bigint) {
    try { setError(detail(operations.retire(id))); setRetiring(undefined); changed(); } catch (cause) { setError(detail(cause)); }
  }
  return <View style={styles.section}>
    <Text style={styles.heading}>Management changes</Text>
    <Text>Results stay available while the app is running. An unknown result is not retried automatically. Check the node’s current settings before trying again.</Text>
    {error !== undefined && <Text accessibilityRole="alert">{error}</Text>}
    {records.length === 0 && <Text>No management changes to show.</Text>}
    {records.map((operation) => <View style={styles.card} key={operation.operationId.toString()}>
      <Text style={styles.title}>{requestLabel(operation.requestKind)} · change {operation.operationId.toString()}</Text>
      <Facts value={operation.destination} />
      <Text>{label(operation.state.tag)}</Text>
      {operation.state.tag === rc.RemoteControlOperationState_Tags.OutcomeUnknown && <Text accessibilityRole="alert">The request may have reached the node. Cancellation or removing this record cannot undo it.</Text>}
      {"inner" in operation.state && <Facts value={operation.state.inner} />}
      {operationPending(operation.state) ? <Button title={`Cancel waiting for change ${operation.operationId}`} onPress={() => cancel(operation.operationId)} /> :
        retiring === operation.operationId ? <View style={styles.section}>
          <Text>Remove this local result? This does not undo the remote action. An unknown result will no longer be available here.</Text>
          <Button title="Keep result" onPress={() => setRetiring(undefined)} />
          <Button title="Remove result" onPress={() => retire(operation.operationId)} />
        </View> : <Button title={`Review removal of change ${operation.operationId}`} onPress={() => setRetiring(operation.operationId)} />}
    </View>)}
  </View>;
}
