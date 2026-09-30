/// A process-launch attempt, not evidence that CoreBluetooth restored state.
/// Scene connection, foregrounding and JavaScript reload must not request it.
public struct PrnsRestorationDispatch {
  public init() {}
  public private(set) var attemptRequested = false
  private var claimed = false

  public mutating func request(_ requested: Bool) {
    attemptRequested = attemptRequested || requested
  }

  public mutating func claimIfAuthorized(_ authorized: Bool) -> Bool {
    guard attemptRequested, authorized, !claimed else {
      return false
    }
    claimed = true
    return true
  }

  public mutating func rearmAfterAuthorizationLoss() {
    claimed = false
  }

  public mutating func cancel() {
    attemptRequested = false
    claimed = false
  }
}
