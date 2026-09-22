import * as Bindings from "@prns-internal/expo";

import { ActionRow, BodyText, Button, Card, CardSection, Subheading } from "@/ui/primitives";
import { ConfirmAction } from "./confirm-action";

export type RemoteChangeHandler = (change: Bindings.RemoteNodeChange) => void;

export function NodeControls({
  availableRequests,
  busy,
  onChange,
}: {
  readonly availableRequests: readonly Bindings.RemoteControlRequestKind[];
  readonly busy: boolean;
  readonly onChange: RemoteChangeHandler;
}) {
  const allows = (kind: Bindings.RemoteControlRequestKind) => availableRequests.includes(kind);
  const positioning = allows(Bindings.RemoteControlRequestKind.SetGnssPower);
  const visibility = allows(Bindings.RemoteControlRequestKind.SetDisplayVisibility);
  const displayAutoOff = allows(Bindings.RemoteControlRequestKind.SetDisplayAutoOff);
  const systemPower = allows(Bindings.RemoteControlRequestKind.SetSystemPower);
  const radioMode = allows(Bindings.RemoteControlRequestKind.SetEspRadioMode);
  const sleepRadios = allows(Bindings.RemoteControlRequestKind.SleepRadios);
  const wakeRadios = allows(Bindings.RemoteControlRequestKind.WakeRadios);

  return (
    <>
      {positioning ||
      visibility ||
      displayAutoOff ||
      systemPower ||
      radioMode ||
      sleepRadios ||
      wakeRadios ? null : (
        <BodyText>
          No device controls are available. Refresh the node information to check again.
        </BodyText>
      )}
      {positioning || visibility || displayAutoOff ? (
        <Card>
          <Subheading>Display and positioning</Subheading>
          <BodyText muted>Current settings are not reported by this node.</BodyText>
          {positioning ? (
            <ActionPair
              title="Satellite positioning"
              first="Turn positioning on"
              second="Turn positioning off"
              busy={busy}
              onFirst={() => onChange(Bindings.RemoteNodeChange.GnssPower.new({ enabled: true }))}
              onSecond={() => onChange(Bindings.RemoteNodeChange.GnssPower.new({ enabled: false }))}
            />
          ) : null}
          {visibility ? (
            <ActionPair
              title="Display"
              first="Show display"
              second="Hide display"
              busy={busy}
              onFirst={() =>
                onChange(Bindings.RemoteNodeChange.DisplayVisibility.new({ visible: true }))
              }
              onSecond={() =>
                onChange(Bindings.RemoteNodeChange.DisplayVisibility.new({ visible: false }))
              }
            />
          ) : null}
          {displayAutoOff ? (
            <ActionPair
              title="Automatic display-off"
              first="Allow display to turn off"
              second="Keep display on"
              busy={busy}
              onFirst={() =>
                onChange(Bindings.RemoteNodeChange.DisplayAutoOff.new({ enabled: true }))
              }
              onSecond={() =>
                onChange(Bindings.RemoteNodeChange.DisplayAutoOff.new({ enabled: false }))
              }
            />
          ) : null}
        </Card>
      ) : null}
      {systemPower || radioMode || sleepRadios || wakeRadios ? (
        <Card>
          <Subheading>Connections and power</Subheading>
          {systemPower ? (
            <CardSection title="Node power">
              <ConfirmAction
                label="Sleep node"
                warning="The node may stop responding. Make sure you can wake it using its controls or another working connection."
                confirmationLabel="Put node to sleep"
                disabled={busy}
                onConfirm={() =>
                  onChange(Bindings.RemoteNodeChange.SystemPower.new({ awake: false }))
                }
              />
              <Button
                disabled={busy}
                tone="secondary"
                onPress={() => onChange(Bindings.RemoteNodeChange.SystemPower.new({ awake: true }))}
              >
                Wake node
              </Button>
              <BodyText muted>
                Wake requests work only while a connection to the node is still available.
              </BodyText>
            </CardSection>
          ) : null}
          {radioMode ? (
            <CardSection title="Connection mode">
              <ConfirmAction
                label="Use Bluetooth"
                warning="Switching to Bluetooth may turn off the node's Wi-Fi hotspot. Make sure Bluetooth is available before continuing."
                disabled={busy}
                onConfirm={() =>
                  onChange(
                    Bindings.RemoteNodeChange.RadioMode.new({
                      mode: Bindings.RemoteRadioMode.Bluetooth,
                    }),
                  )
                }
              />
              <ConfirmAction
                label="Use Wi-Fi hotspot"
                warning="Switching to the Wi-Fi hotspot may disconnect Bluetooth. You may need to join the node's hotspot or use its controls to reconnect."
                disabled={busy}
                onConfirm={() =>
                  onChange(
                    Bindings.RemoteNodeChange.RadioMode.new({
                      mode: Bindings.RemoteRadioMode.AccessPoint,
                    }),
                  )
                }
              />
            </CardSection>
          ) : null}
          {sleepRadios || wakeRadios ? (
            <CardSection title="Radios">
              {sleepRadios ? (
                <ConfirmAction
                  label="Pause radios"
                  warning="Paused radios may disconnect the app. Make sure you can use the node's controls or another connection to resume them."
                  confirmationLabel="Pause node radios"
                  disabled={busy}
                  onConfirm={() => onChange(Bindings.RemoteNodeChange.SleepRadios.new())}
                />
              ) : null}
              {wakeRadios ? (
                <>
                  <Button
                    disabled={busy}
                    tone="secondary"
                    onPress={() => onChange(Bindings.RemoteNodeChange.WakeRadios.new())}
                  >
                    Resume radios
                  </Button>
                  <BodyText muted>
                    The node must still be reachable to receive this request.
                  </BodyText>
                </>
              ) : null}
            </CardSection>
          ) : null}
        </Card>
      ) : null}
    </>
  );
}

function ActionPair({
  title,
  first,
  second,
  busy,
  onFirst,
  onSecond,
}: {
  readonly title: string;
  readonly first: string;
  readonly second: string;
  readonly busy: boolean;
  readonly onFirst: () => void;
  readonly onSecond: () => void;
}) {
  return (
    <CardSection title={title}>
      <ActionRow>
        <Button disabled={busy} onPress={onFirst} tone="secondary">
          {first}
        </Button>
        <Button disabled={busy} onPress={onSecond} tone="secondary">
          {second}
        </Button>
      </ActionRow>
    </CardSection>
  );
}
