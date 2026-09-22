import * as Bindings from "@prns-internal/expo";

import { Badge, BodyText, Button, Card, CardStack, KeyValue, Subheading } from "@/ui/primitives";
import { formatBytes } from "../format";
import { ConfirmAction } from "./confirm-action";
import type { RemoteChangeHandler } from "./node-controls";

export type ControllerAccessCardProps = {
  readonly page?: Bindings.RemoteControllerPage | undefined;
  readonly controllerIdentityFingerprint: Uint8Array;
  readonly availableRequests: readonly Bindings.RemoteControlRequestKind[];
  readonly busy: boolean;
  readonly onLoad: () => void;
  readonly onLoadMore?: (() => void) | undefined;
  readonly onChange: RemoteChangeHandler;
};

/** Inventory reports identities only; never infer another device's role or permissions. */
export function ControllerAccessCard({
  page,
  controllerIdentityFingerprint,
  availableRequests,
  busy,
  onLoad,
  onLoadMore,
  onChange,
}: ControllerAccessCardProps) {
  if (!availableRequests.includes(Bindings.RemoteControlRequestKind.InventoryControllers)) {
    return null;
  }
  const canRemove =
    controllerIdentityFingerprint.length === 16 &&
    availableRequests.includes(Bindings.RemoteControlRequestKind.RevokeController);
  const currentIdentity = formatBytes(controllerIdentityFingerprint);

  return (
    <Card>
      <Subheading>Devices with access</Subheading>
      <BodyText muted>
        The node identifies devices by their IDs. Some devices have protected access that cannot be
        removed remotely.
      </BodyText>
      <Button disabled={busy} onPress={onLoad} tone="secondary">
        {page === undefined ? "Load devices" : "Refresh devices"}
      </Button>
      {page?.identities.length === 0 ? <BodyText>No devices were reported.</BodyText> : null}
      {page?.identities.map((identity) => {
        const formatted = formatBytes(identity);
        const isCurrent = formatted === currentIdentity;
        const label = isCurrent ? "This phone" : `Device ${formatted.slice(0, 8)}`;
        return (
          <CardStack key={formatted}>
            <Subheading>{label}</Subheading>
            <KeyValue label="Device ID" value={formatted} />
            {isCurrent ? (
              <Badge>Current device</Badge>
            ) : canRemove ? (
              <ConfirmAction
                label={`Remove access for ${formatted.slice(0, 8)}`}
                warning={`Remove this device's access to the node? It will need to be authorized again to reconnect. Device ID: ${formatted}`}
                confirmationLabel="Remove device access"
                disabled={busy}
                onConfirm={() =>
                  onChange(
                    Bindings.RemoteNodeChange.RevokeController.new({
                      controllerIdentityFingerprint: identity,
                    }),
                  )
                }
              />
            ) : null}
          </CardStack>
        );
      })}
      {page?.next !== undefined && onLoadMore !== undefined ? (
        <Button disabled={busy} onPress={onLoadMore} tone="secondary">
          Load more devices
        </Button>
      ) : null}
    </Card>
  );
}
