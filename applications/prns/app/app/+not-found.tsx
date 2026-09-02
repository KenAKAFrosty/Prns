// route-kind: not-found
import { NotFoundScreen } from "@/features/placeholder-screen";
import { useScaffoldState } from "@/state/scaffold-state-context";

export default function NotFoundRoute() {
  const { state } = useScaffoldState();
  const backPath =
    state.onboardingPreview.status === "notStarted" ? "/onboarding/welcome" : "/inbox";
  return <NotFoundScreen backPath={backPath} />;
}
