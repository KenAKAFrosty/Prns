import type { NativeStoragePreparationOutcome } from "@prns-internal/native-bindings";
import { NavigationLink } from "@/ui/navigation-link";
import { Badge, BodyText, Card } from "@/ui/primitives";

/** Native bootstrap failure stays separate from an individual domain command. */
export function StoragePreparationFailure({
  outcome,
}: {
  readonly outcome: NativeStoragePreparationOutcome;
}) {
  if (outcome.tag === "Prepared") return null;
  const resetRequired = outcome.tag === "DevelopmentResetRequired";
  return (
    <Card>
      <Badge tone="warning">{resetRequired ? "App reset required" : "Storage unavailable"}</Badge>
      <BodyText>
        {resetRequired
          ? "This app's data needs to be reset before it can be opened."
          : "Storage is temporarily unavailable. Your data has not changed. Try again."}
      </BodyText>
      {resetRequired ? <NavigationLink href="/recovery">Open recovery</NavigationLink> : null}
    </Card>
  );
}
