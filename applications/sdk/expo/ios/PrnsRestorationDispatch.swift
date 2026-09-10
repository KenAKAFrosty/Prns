struct PrnsRestorationDispatch {
  private(set) var launchRequested = false
  private var claimed = false

  mutating func request(_ requested: Bool) {
    launchRequested = launchRequested || requested
  }

  mutating func claimIfAuthorized(_ authorized: Bool) -> Bool {
    guard launchRequested, authorized, !claimed else {
      return false
    }
    claimed = true
    return true
  }

  mutating func rearmAfterAuthorizationLoss() {
    claimed = false
  }

  mutating func cancel() {
    launchRequested = false
    claimed = false
  }
}
