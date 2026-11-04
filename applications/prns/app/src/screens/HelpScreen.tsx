import { Link } from "expo-router";
import { Button, Heading, Panel, Screen, Text } from "../ui/primitives";

export function HelpScreen() {
  return <Screen><Heading>Help</Heading>
    <Panel><Text accessibilityRole="header">Start a conversation</Text>
      <Text>Create a messaging identity, then choose Set up messaging and Start messaging. In Contacts, show your signed QR card or scan someone else’s. Create a conversation and invite a saved contact. They choose whether to join.</Text>
      <Link href="/more/identities" asChild><Button title="Open identities" /></Link>
      <Link href="/contacts" asChild><Button title="Open contacts" /></Link></Panel>
    <Panel><Text accessibilityRole="header">Connections and delivery</Text>
      <Text>Phones can communicate without a radio board. Enable local discovery, use a TCP server or allow Bluetooth in Interfaces. Keep the app open for reliable delivery during this alpha.</Text>
      <Text>A connection or route does not prove a message arrived. “Received” means another device saved the message; it does not mean someone read it.</Text>
      <Text>Stopping networking keeps this phone’s identities, contacts and saved history available. Start networking again in Interfaces when you want to reconnect.</Text>
      <Link href="/more/interfaces" asChild><Button title="Check connections and permissions" /></Link></Panel>
    <Panel><Text accessibilityRole="header">Groups and saved history</Text>
      <Text>The group owner controls membership. Leaving asks the owner to remove you; closing permanently ends the group for everyone. Deleting local history removes only this phone’s copy. New members do not automatically receive older messages.</Text>
      <Text>If you are unsure whether an action finished, check its saved result before repeating it. Retry continues the same action.</Text></Panel>
    <Panel><Text accessibilityRole="header">Optional radio nodes</Text>
      <Text>Pairing a board grants supported controls. Saving an access card, verifying a live connection and configuring its radio are separate steps. Pairing does not make the board a member of your chat groups.</Text>
      <Link href="/nodes" asChild><Button title="Open nodes" /></Link></Panel>
    <Link href="/more/settings/about" asChild><Button title="Build details and shareable diagnostics" /></Link>
  </Screen>;
}
