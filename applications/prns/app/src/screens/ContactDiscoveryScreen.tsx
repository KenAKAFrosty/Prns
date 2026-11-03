import { useEffect, useMemo, useRef, useState } from "react";
import { useLocalSearchParams, useRouter } from "expo-router";
import { contactKey, loadContacts } from "../contacts";
import type { SavedContact } from "../contacts";
import { identityKey } from "../identity-reference";
import { hex } from "../bytes";
import { scalar, useRouteLease } from "../navigation";
import { useInstallation } from "../runtime/InstallationProvider";
import { IdentityAnnouncements } from "../IdentityAnnouncements";
import { ActionMenu, Badge, Button, PageHeader, Panel, Screen, Text } from "../ui/primitives";

export function ContactDiscoveryScreen() {
  const installation = useInstallation();
  const params = useLocalSearchParams<{ owner?: string }>();
  const reference = scalar(params.owner);
  const record = installation.records.find((item) => identityKey(item) === reference);
  const owner = installation.owner;
  const { active, isCurrent } = useRouteLease();
  const router = useRouter();
  const [retry, setRetry] = useState(0);
  const [saved, setSaved] = useState<{ owner: typeof owner; reference: string | undefined; revision: number; rows: readonly SavedContact[]; error?: string }>();
  const contactsRevision = installation.contactsRevision;
  const identity = useMemo(() => record === undefined ? undefined : { id: record.id, publicHash: record.publicHash }, [reference, owner, record === undefined]);
  const generation = installation.networkKey;
  const route = useRef({ owner, reference, generation, contactsRevision });
  route.current = { owner, reference, generation, contactsRevision };
  const canNavigate = () => isCurrent() && route.current.owner === owner && route.current.reference === reference &&
    route.current.generation === generation && route.current.contactsRevision === contactsRevision;
  useEffect(() => {
    if (!active || owner === undefined || identity === undefined) return;
    let cancelled = false;
    void loadContacts(identity).then((rows) => {
      if (!cancelled) setSaved({ owner, reference, revision: contactsRevision, rows });
    }).catch((cause: unknown) => {
      if (!cancelled) setSaved((previous) => ({ owner, reference, revision: contactsRevision,
        rows: previous?.owner === owner && previous.reference === reference ? previous.rows : [], error: String(cause) }));
    });
    return () => { cancelled = true; };
  }, [owner, reference, identity, active, contactsRevision, retry]);
  const savedCurrent = saved !== undefined && saved.owner === owner && saved.reference === reference;
  const savedReady = savedCurrent && saved?.revision === contactsRevision && saved.error === undefined;
  const savedByKey = new Map((savedCurrent ? saved?.rows ?? [] : []).map((contact) => [contactKey(contact), contact]));
  const clearDiscoveredPeople = installation.clearDiscoveredPeople;
  const activity = installation.networkActivity;
  const current = installation.networkKey !== undefined && activity.generation === installation.networkKey;
  const local = new Set(installation.records.map((item) => hex(item.publicHash)));
  const people = current ? activity.discovered.filter((item) => !local.has(hex(item.contact.claims.identityHash))) : [];
  return <Screen><PageHeader title="Find people" help={{ label: "About finding people", content: <Text>People appear when this phone receives their signed contact announcements. A valid signature confirms the identity address, not who the person is. Review a card and confirm it with the person before saving. Network activity also includes other announcements; only compatible, verified messaging contact cards appear here. Clearing recent discoveries leaves saved contacts unchanged. New announcements can make people appear again.</Text> }}
    trailing={<ActionMenu label="Discovery options" disabled={!active || !current || owner === undefined || record === undefined || clearDiscoveredPeople === undefined}
      items={[{ title: "Clear recent discoveries", onPress: () => { if (canNavigate()) clearDiscoveredPeople?.(); } }]} />} />
    {record === undefined || owner === undefined || identity === undefined ? <Text>Choose an available messaging identity from Contacts first.</Text> : <>
      <Text>Recent people heard by this phone appear automatically. Saved contacts stay in your address book; discoveries expire after a day or when networking restarts.</Text>
      <IdentityAnnouncements owner={owner} reference={identity} active={active} compact />
      {!savedCurrent && <Text>Checking saved contacts…</Text>}
      {savedCurrent && saved?.error !== undefined && <><Text accessibilityRole="alert">{saved.error}</Text>
        <Button title="Retry loading saved contacts" disabled={!active} onPress={() => setRetry((value) => value + 1)} /></>}
      {!current && <Text>Start networking from Connections to find people.</Text>}
      {current && (activity.error ?? activity.discoveryError) !== undefined && <Text accessibilityRole="alert">{activity.error ?? activity.discoveryError}</Text>}
      {current && people.length === 0 && <Text>No people discovered yet. Ask the other person to announce their messaging identity, or add their contact card.</Text>}
      {current && activity.dropped > 0n && <Text>Some announcements were missed while this phone was busy. People can announce again or share their contact card.</Text>}
      {people.map(({ contact, observation }) => {
        const savedContact = savedByKey.get(contactKey(contact));
        const name = savedContact?.alias || savedContact?.claims.displayName || contact.claims.displayName || "Unnamed person";
        return <Panel key={contactKey(contact)}>
          <Text accessibilityRole="header">{name}</Text>
          {savedContact === undefined ? <Text>Discovered contact</Text> : <Badge>Saved</Badge>}
          <Text>{observation.event.inner.hops} hops away</Text>
          {savedContact === undefined
            ? <Button title={`Review contact: ${name}`} icon="add" tone="primary" size="compact" disabled={!active || !savedReady} onPress={() => {
              if (savedReady && canNavigate()) router.push({ pathname: "/contacts/add", params: { owner: reference!, card: contact.uri } });
            }} />
            : <Button title="Open contact" accessibilityLabel={`Open contact: ${name}`} tone="quiet" size="compact" disabled={!active || !savedReady} onPress={() => {
              if (savedReady && canNavigate()) router.push({ pathname: "/contacts/[contact]", params: { owner: reference!, contact: contactKey(savedContact) } });
            }} />}
        </Panel>;
      })}
    </>}
  </Screen>;
}
