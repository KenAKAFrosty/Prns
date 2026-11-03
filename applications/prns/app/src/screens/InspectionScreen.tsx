import { NetworkInspection } from "../NetworkInspection";
import { useRouteActive } from "../navigation";
import { useInstallation } from "../runtime/InstallationProvider";
import { PageHeader, Screen, Text } from "../ui/primitives";

export function InspectionScreen() {
  const { owner, networkKey, networkActivity, clearNetworkActivity } = useInstallation();
  const active = useRouteActive();
  return <Screen>
    {owner === undefined ? <><PageHeader title="Network activity" /><Text>Open this phone’s installation to inspect its network.</Text></> : <NetworkInspection owner={owner} active={active} networkKey={networkKey} activity={networkActivity} clearActivity={clearNetworkActivity} />}
  </Screen>;
}
