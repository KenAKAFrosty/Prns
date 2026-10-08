import { useLocalSearchParams, useRouter } from "expo-router";
import type { ReactNode } from "react";
import {
  type RawRouteParams,
  routeParamsAreValid,
  type ScreenCatalogEntry,
  type ScreenId,
  screenById,
} from "@/navigation/catalog";
import { Badge, BodyText, Button, Card, Screen, ScreenHeading } from "@/ui/primitives";

export function NotFoundScreen({ backPath = "/inbox" }: { readonly backPath?: string }) {
  const router = useRouter();
  return (
    <Screen>
      <Badge tone="warning">Not found</Badge>
      <ScreenHeading>This page is not available</ScreenHeading>
      <BodyText>The link may be incomplete or no longer valid.</BodyText>
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
      <Badge tone="warning">Not available yet</Badge>
      <ScreenHeading>{entry.label}</ScreenHeading>
      <Card>
        <BodyText>{entry.summary}</BodyText>
        <BodyText muted>{entry.limitation}</BodyText>
      </Card>
      <Button tone="secondary" onPress={() => router.replace(entry.backPath)}>
        Go back
      </Button>
    </Screen>
  );
}

export function CatalogRouteGuard({
  children,
  screenId,
}: {
  readonly children: ReactNode;
  readonly screenId: ScreenId;
}) {
  const entry = screenById(screenId);
  const params: RawRouteParams = useLocalSearchParams();
  if (!routeParamsAreValid(entry, params)) {
    return <NotFoundScreen backPath={entry.backPath} />;
  }
  return children;
}

export function CatalogPlaceholderRoute({ screenId }: { readonly screenId: ScreenId }) {
  const entry = screenById(screenId);
  const params: RawRouteParams = useLocalSearchParams();
  if (!routeParamsAreValid(entry, params)) {
    return <NotFoundScreen backPath={entry.backPath} />;
  }
  return <NotYetImplementedScreen entry={entry} />;
}
