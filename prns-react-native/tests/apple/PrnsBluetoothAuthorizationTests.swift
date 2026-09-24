@main
enum PrnsBluetoothAuthorizationTests {
  static func main() {
    for authorization in PrnsBluetoothAuthorization.allCases {
      // Every permission choice admits an explicit foreground local-node start.
      precondition(authorization.permitsStart(restorationOnly: false, applicationActive: true))
      // Launch/restoration must never be the source of a new permission prompt.
      for active in [false, true] {
        precondition(
          authorization.permitsStart(restorationOnly: true, applicationActive: active)
            == (authorization == .allowedAlways)
        )
      }
      precondition(
        authorization.permitsStart(restorationOnly: false, applicationActive: false)
          == (authorization != .notDetermined)
      )
    }
    // A permission grant after launch dispatches once, never once per foreground.
    var dispatch = PrnsRestorationDispatch()
    dispatch.request(true)
    precondition(!dispatch.claimIfAuthorized(PrnsBluetoothAuthorization.notDetermined.permitsAutomaticRestoration))
    precondition(dispatch.claimIfAuthorized(PrnsBluetoothAuthorization.allowedAlways.permitsAutomaticRestoration))
    precondition(!dispatch.claimIfAuthorized(PrnsBluetoothAuthorization.allowedAlways.permitsAutomaticRestoration))
    // Revocation cannot launch a permission prompt and a stop cancels re-arming.
    dispatch.rearmAfterAuthorizationLoss()
    precondition(!dispatch.claimIfAuthorized(PrnsBluetoothAuthorization.denied.permitsAutomaticRestoration))
    dispatch.cancel()
    precondition(!dispatch.claimIfAuthorized(PrnsBluetoothAuthorization.allowedAlways.permitsAutomaticRestoration))
    print("Bluetooth authorization policy checks passed")
  }
}
