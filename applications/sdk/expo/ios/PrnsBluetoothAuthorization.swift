/// App authorization is separate from radio availability and peer discovery.
/// Reading this policy never creates a Bluetooth manager or requests permission.
enum PrnsBluetoothAuthorization: String, CaseIterable {
  case notDetermined
  case restricted
  case denied
  case allowedAlways

  var permitsAutomaticRestoration: Bool { self == .allowedAlways }

  func permitsStart(restorationOnly: Bool, applicationActive: Bool) -> Bool {
    if restorationOnly {
      return permitsAutomaticRestoration
    }
    // Explicit local-node startup also works offline or with denied Bluetooth.
    // Only an unresolved permission requires the app to be in the foreground:
    // the actual Rust-owned managers, not a separate probe, present the prompt.
    return self != .notDetermined || applicationActive
  }
}
