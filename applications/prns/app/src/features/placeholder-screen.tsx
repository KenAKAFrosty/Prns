import { useLocalSearchParams } from "expo-router";

import {
  routeParamsAreValid,
  screenById,
  type RawRouteParams,
  type ScreenCatalogEntry,
  type ScreenId,
} from "@/navigation/catalog";
import { Badge, BodyText, Button, Card, Screen, ScreenHeading } from "@/ui/primitives";
import { useRouter } from "expo-router";

export function NotFoundScreen({ backPath = "/inbox" }: { readonly backPath?: string }) {
  const router = useRouter();
  return (
    <Screen>
      <Badge tone="warning">Not found</Badge>
      <ScreenHeading>This route is not available</ScreenHeading>
      <BodyText>
        The address is unknown or contains an invalid, empty, or repeated parameter. No command was
        issued.
      </BodyText>
      <Button tone="secondary" onPress={() => router.replace(backPath)}>
        Return to a safe screen
      </Button>
    </Screen>
  );
}

export function NotYetImplementedScreen({ entry }: { readonly entry: ScreenCatalogEntry }) {
  const router = useRouter();
  return (
    <Screen>
      <Badge tone="warning">Not yet implemented</Badge>
      <ScreenHeading>{entry.label}</ScreenHeading>
      <Card>
        <BodyText>{entry.summary}</BodyText>
        <BodyText muted>{entry.limitation}</BodyText>
      </Card>
      <BodyText muted>
        This route is retained so planned navigation stays visible. It has no mock protocol result,
        command, or fallback service.
      </BodyText>
      <Button tone="secondary" onPress={() => router.replace(entry.backPath)}>
        Go back
      </Button>
    </Screen>
  );
}

export function CatalogPlaceholderRoute({ screenId }: { readonly screenId: ScreenId }) {
  const entry = screenById(screenId);
  const params: RawRouteParams = useLocalSearchParams();
  if (!routeParamsAreValid(entry, params)) {
    return <NotFoundScreen backPath={entry.backPath} />;
  }
  return <NotYetImplementedScreen entry={entry} />;
}
