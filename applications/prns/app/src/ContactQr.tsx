import { Component } from "react";
import { StyleSheet, View, useWindowDimensions } from "react-native";
import { Text } from "./ui/primitives";
import QRCode from "react-native-qrcode-svg";

class ImageBoundary extends Component<{ uri: string; size: number }, { failed: boolean }> {
  override state = { failed: false };
  static getDerivedStateFromError() { return { failed: true }; }
  override render() {
    if (this.state.failed) return <Text accessibilityRole="alert" style={qr.fallback}>This card could not be shown as a QR code. Share or copy the exact contact link below instead.</Text>;
    return <View accessible accessibilityRole="image" accessibilityLabel="Your signed contact QR code">
      <QRCode value={this.props.uri} size={this.props.size} quietZone={20} ecl="M" color="#000000" backgroundColor="#ffffff" />
    </View>;
  }
}
/** Encode the exact signed URI, with no URL rewriting, image logo, or tracking. */
export function ContactQr({ uri }: { uri: string }) {
  const { width } = useWindowDimensions();
  const size = Math.max(140, Math.min(280, width - 120));
  return <View style={qr.frame}>
    <ImageBoundary key={uri} uri={uri} size={size} />
  </View>;
}
const qr = StyleSheet.create({
  frame: { alignSelf: "center", backgroundColor: "#ffffff", padding: 4, maxWidth: "100%" },
  fallback: { color: "#000000" },
});
