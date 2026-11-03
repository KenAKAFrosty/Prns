import { Link } from "expo-router";
import { View } from "react-native";
import type { prns_identities as identities } from "@prns/app-bindings";
import { contactKey, pinnedFirst } from "./contacts";
import type { SavedContact } from "./contacts";
import { identityKey } from "./identity-reference";
import { useStyles } from "./ui";
import { Badge, Button, ListRow, PageHeader, Text } from "./ui/primitives";

export function ContactList({ identity, contacts, active, onOpenContact }: {
  identity: identities.IdentityClientLike; displayName: string; active: boolean;
  onOpenContact?: ((contact: SavedContact) => void) | undefined;
  contacts: readonly SavedContact[]; onChange: (contacts: readonly SavedContact[]) => void;
}) {
  const styles = useStyles();
  const owner = identityKey(identity.reference());
  return <View style={styles.section}>
    <PageHeader title="Contacts" help={{ label: "About contacts", content: <Text>Contacts stay with this messaging identity. Use a signed contact card to add someone, or find people announcing on the network.</Text> }}>
      <Link href={{ pathname: "/contacts/add", params: { owner } }} asChild><Button title="Add contact" icon="add" tone="primary" disabled={!active} /></Link>
      <Link href={{ pathname: "/contacts/discover", params: { owner } }} asChild><Button title="Find people" tone="quiet" disabled={!active} /></Link>
      <Link href={{ pathname: "/contacts/card", params: { owner } }} asChild><Button title="My card" accessibilityLabel="My contact card" icon="card" tone="quiet" disabled={!active} /></Link>
    </PageHeader>
    {contacts.length === 0 && <Text>No saved contacts yet. Add someone’s contact card to invite them to a conversation.</Text>}
    {pinnedFirst(contacts).map((contact) => <ListRow key={contactKey(contact)} title={contact.alias || contact.claims.displayName || "Unnamed contact"}
        accessibilityLabel={`Contact details: ${contact.alias || contact.claims.displayName || "Unnamed contact"}`}
        badge={contact.pinned === true ? <Badge>Pinned</Badge> : undefined}
        disabled={!active || onOpenContact === undefined} onPress={() => { if (active) onOpenContact?.(contact); }} />)}
  </View>;
}
