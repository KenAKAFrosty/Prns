import { useRef, useState } from "react";
import { Linking } from "react-native";

import { useDevelopmentRuntime } from "@/native/development-runtime-context";
import { BodyText, Button, Card, Subheading } from "@/ui/primitives";

export function AndroidNodeControls() {
  const runtime = useDevelopmentRuntime();
  const [notificationPending, setNotificationPending] = useState(false);
  const [settingsFailure, setSettingsFailure] = useState(false);
  const requestPending = useRef(false);
  if (runtime.androidRuntime === null) return null;
  const platform = runtime.androidRuntime;
  const notification = platform.status?.connectionNotification;
  const requestNotification = async () => {
    if (requestPending.current) return;
    requestPending.current = true;
    setNotificationPending(true);
    setSettingsFailure(false);
    try {
      if (notification === "blocked") await Linking.openSettings();
      else await platform.requestConnectionNotificationPermission();
    } catch {
      setSettingsFailure(true);
    } finally {
      requestPending.current = false;
      setNotificationPending(false);
    }
  };

  return (
    <Card>
      <Subheading>Node controls</Subheading>
      <BodyText>
        Stopping pauses this device&apos;s connections without deleting your identity, pairings, or
        messages.
      </BodyText>
      <Button
        disabled={!runtime.canStopNode}
        onPress={() => void runtime.stopNode()}
        tone="secondary"
      >
        {runtime.stoppingNode ? "Stopping…" : "Stop node"}
      </Button>
      {runtime.stopFailure === null ? null : (
        <BodyText>The node could not stop. Try again.</BodyText>
      )}
      {notification !== undefined && notification !== "enabled" ? (
        <>
          <BodyText>
            {notification === "blocked"
              ? "Turn on notifications in Android Settings to show the running-node status and its Stop control. You can always stop the node here."
              : "Allow the running-node notification to see its status and Stop control outside the app. This does not enable message alerts."}
          </BodyText>
          <Button
            disabled={notificationPending}
            tone="secondary"
            onPress={() => void requestNotification()}
          >
            {notificationPending
              ? "Please wait…"
              : notification === "blocked"
                ? "Open notification settings"
                : "Allow node notification"}
          </Button>
        </>
      ) : null}
      {platform.failure === null ? null : (
        <BodyText>Device status could not be checked. Refresh its status and try again.</BodyText>
      )}
      {settingsFailure ? (
        <BodyText>
          Settings could not open. Check this app&apos;s notification settings in Android Settings.
        </BodyText>
      ) : null}
    </Card>
  );
}
