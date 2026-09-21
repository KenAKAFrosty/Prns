import { useState } from "react";

import { BodyText, Button, CardStack } from "@/ui/primitives";

/** Confirmation is presentation only; the native owner still admits the change. */
export function ConfirmAction({
  label,
  warning,
  confirmationLabel = "Apply change",
  disabled,
  onConfirm,
}: {
  readonly label: string;
  readonly warning: string;
  readonly confirmationLabel?: string;
  readonly disabled: boolean;
  readonly onConfirm: () => void;
}) {
  const [confirming, setConfirming] = useState(false);

  return (
    <CardStack>
      <Button disabled={disabled} onPress={() => setConfirming(true)} tone="secondary">
        {label}
      </Button>
      {confirming ? (
        <>
          <BodyText>{warning}</BodyText>
          <Button
            disabled={disabled}
            onPress={() => {
              setConfirming(false);
              onConfirm();
            }}
            tone="destructive"
          >
            {confirmationLabel}
          </Button>
          <Button onPress={() => setConfirming(false)} tone="secondary">
            Cancel
          </Button>
        </>
      ) : null}
    </CardStack>
  );
}
