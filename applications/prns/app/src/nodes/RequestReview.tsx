import { View } from "react-native";
import { Text } from "../ui/primitives";
import { prns_host_uniffi as rc } from "@prns/app-bindings";
import { hex } from "../bytes";
import { LoRaFacts } from "./LoRa";
import { Facts, label, Permissions } from "./presentation";

/** User-facing meaning of a proposed canonical request, never a protocol copy. */
export function RequestReview({ request }: { request: rc.RemoteControlRequest }) {
  let content;
  switch (request.tag) {
    case rc.RemoteControlRequest_Tags.AuthorizeController:
      content = <><Text selectable>Controller: {hex(rc.remoteControlControllerIdentityHash(request.inner.controller).value)}</Text><Permissions authority={rc.RemoteControlControllerAuthority.Operator} permittedRequests={request.inner.permittedRequests} /><Text>This changes access on the remote node. Share target access information separately after confirming the result.</Text></>; break;
    case rc.RemoteControlRequest_Tags.RevokeController:
      content = <><Text selectable>Controller: {hex(request.inner.hash.value)}</Text><Text>Remove this controller's access on the remote node. It must be authorized again to reconnect. Protected access may be refused.</Text></>; break;
    case rc.RemoteControlRequest_Tags.SetInterfaceLoRaProfile:
      content = <><Text selectable>Interface: {hex(request.inner.id.value)}</Text><LoRaFacts profile={rc.remoteControlLoraProfileBand(request.inner.profile)} /></>; break;
    case rc.RemoteControlRequest_Tags.ConfigureRadio:
      content = <><Text selectable>Interface: {hex(request.inner.id.value)}</Text>{request.inner.configuration.tag === rc.RemoteControlRadioConfiguration_Tags.Profile ? <LoRaFacts profile={rc.remoteControlLoraProfileBand(request.inner.configuration.inner.value)} /> : <Text>Clear the saved radio configuration. This may make the radio unavailable until configured again.</Text>}</>; break;
    case rc.RemoteControlRequest_Tags.SetSystemPower:
      content = <Text>Node power: {label(rc.RemoteControlSystemPower[request.inner.power])}. Putting it to sleep may require local access to wake it.</Text>; break;
    case rc.RemoteControlRequest_Tags.SetGnssPower:
      content = <Text>GPS: {label(rc.RemoteControlGnssPower[request.inner.power])}</Text>; break;
    case rc.RemoteControlRequest_Tags.SetDisplayVisibility:
      content = <Text>Display: {label(rc.RemoteControlDisplayVisibility[request.inner.visibility])}</Text>; break;
    case rc.RemoteControlRequest_Tags.SetDisplayAutoOff:
      content = <Text>Display auto-off: {label(rc.RemoteControlDisplayAutoOff[request.inner.autoOff])}</Text>; break;
    case rc.RemoteControlRequest_Tags.SetEspRadioMode:
      content = <Text>Wireless mode: {label(rc.RemoteControlEspRadioMode[request.inner.mode])}. This may close the current wireless connection.</Text>; break;
    case rc.RemoteControlRequest_Tags.SetInterfacePower:
      content = <Text>Interface {hex(request.inner.id.value)} power: {label(rc.RemoteControlInterfacePower[request.inner.power])}</Text>; break;
    case rc.RemoteControlRequest_Tags.SetInterfaceMode:
      content = <Text>Interface {hex(request.inner.id.value)} mode: {label(rc.RemoteControlInterfaceMode[request.inner.mode])}</Text>; break;
    case rc.RemoteControlRequest_Tags.SetStationUplink:
      content = <Text>Interface {hex(request.inner.id.value)} saved Wi-Fi uplink: {label(rc.RemoteControlStationUplink[request.inner.uplink])}</Text>; break;
    default: content = "inner" in request ? <Facts value={request.inner} /> : undefined;
  }
  return <View><Text>{label(request.tag)}</Text>{content}</View>;
}
