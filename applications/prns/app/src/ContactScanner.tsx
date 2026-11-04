import { Button, Text } from "./ui/primitives";
import { useEffect, useRef, useState } from "react";
import { ActivityIndicator, Linking, StyleSheet, View } from "react-native";
import { CameraView, useCameraPermissions } from "expo-camera";
import type { BarcodeScanningResult } from "expo-camera";
import { useStyles } from "./ui";

/** Mounted only for one explicit foreground scan. The first QR result closes
 * this attempt before verification so repeated camera events cannot overwrite
 * the reviewed card or repeatedly invoke native signature verification. */
export function ContactScanner({ onScan, onCancel, onFailure }: {
  onScan: (data: string) => void; onCancel: () => void; onFailure: (message: string) => void;
}) {
  const styles = useStyles();
  const [permission, requestPermission] = useCameraPermissions();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const mounted = useRef(true);
  const handled = useRef(false);
  const requesting = useRef(false);
  useEffect(() => {
    mounted.current = true; handled.current = false;
    return () => { mounted.current = false; handled.current = true; };
  }, []);
  const request = async () => {
    if (requesting.current || !mounted.current) return;
    requesting.current = true; setBusy(true); setError(undefined);
    try {
      const result = await requestPermission();
      if (mounted.current && !result.granted) setError("Camera access was not allowed. You can still paste a contact link.");
    } catch { if (mounted.current) setError("Camera permission could not be checked. Try again or paste a contact link."); }
    finally { requesting.current = false; if (mounted.current) setBusy(false); }
  };
  const scanned = (result: BarcodeScanningResult) => {
    if (!mounted.current || handled.current || result.type !== "qr") return;
    handled.current = true;
    onScan(result.data);
  };
  return <View style={styles.card}>
    <Text accessibilityRole="header" style={styles.title}>Scan a contact card</Text>
    <Text>Point your camera at a prns contact QR code. You will review the verified identity before saving it.</Text>
    {error !== undefined && <Text accessibilityRole="alert" style={styles.error}>{error}</Text>}
    {permission === null && <ActivityIndicator accessibilityLabel="Checking camera permission" />}
    {permission !== null && !permission.granted && <>
      <Text>Camera access is only needed to scan. Pasting a link works without it.</Text>
      {permission.canAskAgain
        ? <Button title="Allow camera for scanning" disabled={busy} onPress={() => { void request(); }} />
        : <><Text>Camera access is disabled for this app. You can enable it in device settings.</Text>
          <Button title="Open camera settings" onPress={() => { void Linking.openSettings().catch(() => {
            if (mounted.current) setError("Settings could not be opened. Enable camera access in your device settings, or paste the link.");
          }); }} /></>}
    </>}
    {permission?.granted && <CameraView style={scanner.camera} facing="back" mode="picture" mute
      barcodeScannerSettings={{ barcodeTypes: ["qr"] }} onBarcodeScanned={scanned}
      onMountError={() => {
        if (!mounted.current || handled.current) return;
        handled.current = true;
        onFailure("The camera is unavailable. Try scanning again, or paste a contact link instead.");
      }} accessibilityLabel="Contact QR camera" />}
    <Button title="Cancel scan" onPress={() => { handled.current = true; onCancel(); }} />
  </View>;
}
const scanner = StyleSheet.create({ camera: { width: "100%", height: 300, borderRadius: 10, overflow: "hidden" } });
