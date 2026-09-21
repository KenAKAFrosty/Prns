import * as Bindings from "@prns-internal/expo";

import { BodyText, Card, KeyValue, Subheading } from "@/ui/primitives";

export function NodeOverviewCard({ overview }: { readonly overview: Bindings.RemoteNodeOverview }) {
  return (
    <Card>
      <Subheading>Overview</Subheading>
      {overview.firmware === undefined ? null : (
        <KeyValue label="Firmware" value={overview.firmware || "Not reported"} />
      )}
      {overview.power === undefined ? null : (
        <>
          <KeyValue
            label="Battery"
            value={
              overview.power.batteryPercent === undefined
                ? "Not reported"
                : `${overview.power.batteryPercent}%`
            }
          />
          <KeyValue
            label="External power"
            value={externalPowerLabel(overview.power.externalPower)}
          />
        </>
      )}
      {overview.firmware === undefined && overview.power === undefined ? (
        <BodyText>This node does not report firmware or power information.</BodyText>
      ) : null}
    </Card>
  );
}

function externalPowerLabel(power: Bindings.RemoteExternalPower): string {
  switch (power) {
    case Bindings.RemoteExternalPower.Unknown:
      return "Not reported";
    case Bindings.RemoteExternalPower.Absent:
      return "Not connected";
    case Bindings.RemoteExternalPower.Present:
      return "Connected";
    case Bindings.RemoteExternalPower.Charging:
      return "Connected · Charging";
    case Bindings.RemoteExternalPower.Idle:
      return "Connected · Not charging";
  }
}
