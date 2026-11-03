import { View } from "react-native";
import { identityKey } from "../identity-reference";
import { useContactLinks } from "../contact-links";
import { ContactLinkReview } from "../ContactLinkReview";
import { useForeground } from "../use-foreground";
import { useInstallation } from "./InstallationProvider";
import { Button, Text } from "../ui/primitives";

/** One incoming-link queue survives navigation and preserves explicit identity review. */
export function IncomingContacts() {
  const links = useContactLinks();
  const installation = useInstallation();
  const active = useForeground();
  return <View>
    {links.initialError !== undefined && <><Text accessibilityRole="alert">{links.initialError}</Text>
      <Button title="Retry opening contact link" onPress={links.retryInitial} /></>}
    {links.notice !== undefined && <><Text accessibilityRole="alert">{links.notice}</Text>
      <Button title="Dismiss contact link notice" onPress={links.clearNotice} /></>}
    {links.current !== undefined && <ContactLinkReview key={links.current.id} link={links.current} active={active}
      owner={installation.owner} catalogRevision={JSON.stringify(installation.records.map((record) => [identityKey(record), record.metadata.localLabel]))} onDone={links.dismiss} onSaved={installation.contactSaved} />}
    {links.waiting > 0 && <Text>{links.waiting} more contact links waiting for review.</Text>}
  </View>;
}
