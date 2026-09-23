/// A process-launch attempt, not evidence that CoreBluetooth restored state.
/// Scene connection, foregrounding and JavaScript reload must not request it.
struct PrnsRestorationDispatch {
  private(set) var attemptRequested = false
  private var claimed = false

  mutating func request(_ requested: Bool) {
    attemptRequested = attemptRequested || requested
  }

  mutating func claimIfAuthorized(_ authorized: Bool) -> Bool {
    guard attemptRequested, authorized, !claimed else {
      return false
    }
    claimed = true
    return true
  }

  mutating func rearmAfterAuthorizationLoss() {
    claimed = false
  }

  mutating func cancel() {
    attemptRequested = false
    claimed = false
  }
}
