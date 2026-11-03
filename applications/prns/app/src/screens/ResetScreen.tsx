import { Link } from "expo-router";
import { ActivityIndicator } from "react-native";
import { useInstallation } from "../runtime/InstallationProvider";
import { Button, Heading, Panel, Screen, Text } from "../ui/primitives";

export function ResetScreen() {
  const installation = useInstallation();
  const { reset } = installation;
  const { state } = reset;
  return <Screen><Heading>Reset this phone</Heading>
    <Panel><Text>Reset permanently removes this app’s identities, contacts, messages, conversation drafts, saved connections and node pairing on this phone.</Text>
      <Text>It does not close groups, notify peers, remove other people’s copies or change a connected node. There is no undo or supported backup restoration.</Text></Panel>
    {installation.resetPending && <Text>Your reset is not finished. Complete it before setting up this phone again.</Text>}
    {state.error !== undefined && <Text accessibilityRole="alert">{state.error}</Text>}
    {state.busy && <ActivityIndicator accessibilityLabel={state.localDeleted ? "Finishing reset preferences" : "Resetting this phone"} />}
    {!state.busy && !state.localDeleted && !state.reviewed && <Button title="Review local data reset" onPress={() => { void reset.prepare(); }} />}
    {state.reviewed && <Panel><Text>Confirm permanent deletion of this phone’s app data. Connections and pending activity will be stopped first.</Text>
      <Button tone="destructive" title={installation.resetPending ? "Retry confirmed local reset" : "Permanently reset this phone"} disabled={state.busy} onPress={() => { void reset.confirm(); }} />
      {!installation.resetPending && <Button title="Keep this phone’s data" disabled={state.busy} onPress={reset.cancel} />}
    </Panel>}
    {state.localDeleted && !state.busy && <><Text>Identities, messages and contacts were deleted. Finish clearing preferences and drafts.</Text>
      <Button title="Finish reset cleanup" onPress={() => { void reset.resumeCleanup(); }} /></>}
    {!installation.resetPending && !state.reviewed && !state.busy && <Link href="/setup" asChild><Button title="Open phone setup" /></Link>}
  </Screen>;
}
