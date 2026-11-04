import { Text } from "../ui/primitives";
import { View } from "react-native";
import { prns_host_uniffi as rc } from "@prns/app-bindings";
import { hex } from "../bytes";
import { useStyles } from "../ui";

export const label = (value: string): string => value.replace(/([a-z0-9])([A-Z])/g, "$1 $2");
export const requestLabel = (kind: rc.RemoteControlRequestKind): string => label(rc.RemoteControlRequestKind[kind]);

/** Rendering only: canonical records remain the source of every field. */
export function detail(value: unknown): string {
  if (value === undefined || value === null) return "Unavailable";
  if (value instanceof Uint8Array) return hex(value);
  if (typeof value === "bigint") return value.toString();
  if (typeof value !== "object") return String(value);
  if ("tag" in value) {
    const tagged = value as { tag: string; inner?: unknown };
    return `${label(tagged.tag)}${tagged.inner === undefined ? "" : `: ${detail(tagged.inner)}`}`;
  }
  if (value instanceof Error) return value.message;
  return Object.entries(value).map(([key, item]) => `${label(key)}: ${key === "password" ? "Hidden" : detail(item)}`).join(" · ");
}

/** Only missing-route link rejection permits contextual path discovery. */
export function nodeRouteMissing(failure: rc.RemoteControlHostRemoteControlError): boolean {
  if (failure.tag !== rc.RemoteControlHostRemoteControlError_Tags.Connect) return false;
  const connect = failure.inner.value;
  if (connect.tag !== rc.RemoteControlConnectRemoteControlTargetError_Tags.EstablishLink) return false;
  const link = connect.inner.value;
  if (link.tag !== rc.RemoteControlSendErrorRemoteControlEstablishLinkFailure_Tags.Failed) return false;
  const failed = link.inner.value;
  return failed.tag === rc.RemoteControlEstablishLinkFailure_Tags.Rejected &&
    failed.inner.value === rc.RemoteControlEstablishLinkRejection.NoRouteToDestination;
}

export function Facts({ value, depth = 0 }: { value: unknown; depth?: number }) {
  const styles = useStyles();
  if (value === undefined || value === null || typeof value !== "object" || value instanceof Uint8Array || depth >= 5) {
    return <Text selectable>{detail(value)}</Text>;
  }
  if (Array.isArray(value)) return <View style={styles.section}>{value.length === 0 ? <Text>None</Text> : value.slice(0, 64).map((item, index) => <Facts key={index} value={item} depth={depth + 1} />)}
    {value.length > 64 && <Text>{value.length - 64} more entries in this result.</Text>}</View>;
  if ("tag" in value) {
    const tagged = value as { tag: string; inner?: unknown };
    return <View><Text>{label(tagged.tag)}</Text>{tagged.inner !== undefined && <Facts value={tagged.inner} depth={depth + 1} />}</View>;
  }
  return <View style={styles.section}>{Object.entries(value).map(([key, item]) => <View key={key}>
    <Text style={styles.title}>{label(key)}</Text>{key === "password" ? <Text>Hidden</Text> : <Facts value={item} depth={depth + 1} />}
  </View>)}</View>;
}

export function Permissions({ authority, permittedRequests }: rc.RemoteControlPairingPermissions) {
  return <View><Text>Authority: {label(rc.RemoteControlControllerAuthority[authority])}</Text>
    <Text>Exact permitted actions: {permittedRequests.kinds.map(requestLabel).join(", ") || "None"}</Text>
  </View>;
}

export function operationPending(state: rc.RemoteControlOperationState): boolean {
  return state.tag === rc.RemoteControlOperationState_Tags.Pending || state.tag === rc.RemoteControlOperationState_Tags.Submitted;
}
export function operationUncertain(state: rc.RemoteControlOperationState): boolean {
  return operationPending(state) || state.tag === rc.RemoteControlOperationState_Tags.OutcomeUnknown;
}
export function isTargetOperation(operation: rc.RemoteControlOperationSnapshot, target: Uint8Array): boolean {
  return operation.destination.tag === rc.RemoteControlOperationDestination_Tags.Target && hex(operation.destination.inner.identity.value) === hex(target);
}
