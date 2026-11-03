import { useCallback, useMemo, useRef } from "react";
import { useLocalSearchParams, useRouter } from "expo-router";
import { ContactExchange } from "../ContactExchange";
import { identityKey } from "../identity-reference";
import { scalar, useRouteLease } from "../navigation";
import { useInstallation } from "../runtime/InstallationProvider";
import { PageHeader, Screen, Text } from "../ui/primitives";

/** The route owns a full immutable identity reference, never a changing selection. */
export function ContactWorkflowScreen({ mode }: { mode: "add" | "card" }) {
  const installation = useInstallation();
  const params = useLocalSearchParams<{ owner?: string; card?: string }>();
  const reference = scalar(params.owner);
  const initialCard = mode === "add" ? scalar(params.card) : undefined;
  const record = installation.records.find((item) => identityKey(item) === reference);
  const owner = installation.owner;
  const resolved = record === undefined ? undefined : identityKey(record);
  const identity = useMemo(() => {
    if (owner === undefined || record === undefined) return undefined;
    return owner.identities().identity({ id: record.id, publicHash: record.publicHash });
  }, [owner, resolved]);
  const { active, isCurrent } = useRouteLease();
  const router = useRouter();
  const current = useRef({ owner, resolved, mode, initialCard });
  current.current = { owner, resolved, mode, initialCard };
  const saved = useCallback(() => {
    installation.contactSaved();
    if (isCurrent() && current.current.owner === owner && current.current.resolved === resolved && current.current.mode === mode && current.current.initialCard === initialCard && resolved !== undefined) {
      router.replace({ pathname: "/contacts", params: { owner: resolved } });
    }
  }, [owner, resolved, mode, initialCard, isCurrent, installation.contactSaved, router]);
  return <Screen><PageHeader title={mode === "add" ? "Add contact" : "My contact card"} help={{ label: "About contact cards", content: <Text>A signed card proves which identity published it. Confirm the person through a channel you trust. Saving a card does not join a group.</Text> }} />
    {record === undefined || identity === undefined ? <Text>Open this screen from the contacts of an available messaging identity.</Text> : <>
      <ContactExchange key={`${resolved}:${mode}:${initialCard ?? ""}`} mode={mode} initialCard={initialCard} identity={identity} displayName={record.metadata.publishedName} active={active} onChange={saved} />
    </>}
  </Screen>;
}
