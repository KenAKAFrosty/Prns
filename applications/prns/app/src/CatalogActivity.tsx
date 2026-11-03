import { Button, Text } from "./ui/primitives";
import { useCallback, useEffect, useRef, useState } from "react";
import { View } from "react-native";
import { prns_identities_core as core, prns_identities as identities } from "@prns/app-bindings";
import { useStyles } from "./ui";

/** Acknowledgement is catalog-wide; never jump over pending work or a key lease. */
export function completedPrefix(operations: readonly identities.IdentityOperationRecord[]): bigint | undefined {
  const ordered = [...operations].sort((left, right) => left.token.sequence < right.token.sequence ? -1 : left.token.sequence > right.token.sequence ? 1 : 0);
  let through: bigint | undefined;
  for (const operation of ordered) {
    if (operation.state.tag !== core.IdentityOperationState_Tags.Completed || operation.unpublishedKey !== undefined) break;
    through = operation.token.sequence;
  }
  return through;
}
function label(operation: identities.IdentityOperationRecord): string {
  if (identities.IdentityCommand.Create.instanceOf(operation.command)) return `Create ${operation.command.inner[0].metadata.localLabel}`;
  const target = operation.command.inner[0].target.tag;
  return target === "Enable" ? "Start messaging" : target === "Pause" ? "Pause messaging" : target;
}
function result(state: core.IdentityOperationState): string {
  if (state.tag !== core.IdentityOperationState_Tags.Completed) return state.tag === "Reserved" ? "Waiting for confirmation" : "Finishing";
  const outcome = state.inner[0].tag;
  const labels: Record<string, string> = { Created: "Created", Existing: "Existing identity recovered", Enabled: "Messaging started",
    Paused: "Messaging paused", Retired: "Identity retired", RetirementBlocked: "Retirement blocked", ActivationFailed: "Messaging could not start",
    ServiceAdded: "Service added", ServiceRemoved: "Service removed", ServiceRemovalBlocked: "Service removal blocked", Cancelled: "Cancelled before starting" };
  return labels[outcome] ?? outcome;
}

export function CatalogActivity({ catalog, active }: { catalog: identities.IdentityCatalogClientLike; active: boolean }) {
  const styles = useStyles();
  const [operations, setOperations] = useState<identities.IdentityOperationRecord[]>([]);
  const [expanded, setExpanded] = useState(false);
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const generation = useRef(0);
  const reading = useRef(false);
  const mutating = useRef(false);
  const refresh = useCallback(async () => {
    const ticket = ++generation.current;
    const rows = await catalog.operations();
    if (ticket === generation.current) setOperations(rows);
  }, [catalog]);
  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    const poll = async () => {
      if (reading.current || mutating.current) return;
      reading.current = true;
      try { await refresh(); }
      catch (cause) { if (!cancelled) setError(cause instanceof Error ? cause.message : String(cause)); }
      finally { reading.current = false; }
    };
    void poll();
    const timer = setInterval(() => { void poll(); }, 1500);
    return () => { cancelled = true; generation.current++; clearInterval(timer); };
  }, [active, refresh]);
  const through = completedPrefix(operations);
  if (operations.length === 0 && error === undefined) return null;
  return <View style={styles.card}>
    <Button title={expanded ? "Hide identity activity" : `Review identity activity (${operations.length})`} onPress={() => { setExpanded(!expanded); }} />
    {error !== undefined && <Text accessibilityRole="alert" style={styles.error}>{error}</Text>}
    {expanded && <>
      <Text>Completed activity can be cleared for all your identities. This does not delete identities or message history.</Text>
      {operations.map((operation) => <Text key={operation.token.sequence.toString()}>{label(operation)} · {result(operation.state)}</Text>)}
      <Button title="Acknowledge completed identity results" disabled={busy || through === undefined} onPress={() => {
        if (through === undefined || mutating.current) return;
        mutating.current = true; generation.current++; setBusy(true); setError(undefined);
        void (async () => {
          try { await catalog.pruneCompleted(through); await refresh(); }
          catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
          finally { mutating.current = false; setBusy(false); }
        })();
      }} />
      {through === undefined && <Text>Finish or cancel pending actions before clearing later results.</Text>}
    </>}
  </View>;
}
