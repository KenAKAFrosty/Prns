public struct PrnsProtectedDataRecovery {
  public init() {}
  public enum Action: Equatable {
    case doNotRetry
    case waitForAvailability
  }

  public private(set) var consumed = false
  private var attemptInFlight = false
  private var unavailableObserved = false

  public mutating func beginAttempt(protectedDataAvailable: Bool) {
    attemptInFlight = true
    unavailableObserved = !protectedDataAvailable
  }

  public mutating func protectedDataWillBecomeUnavailable() {
    guard attemptInFlight else {
      return
    }
    unavailableObserved = true
  }

  public mutating func finishAttempt() {
    attemptInFlight = false
    unavailableObserved = false
  }

  public mutating func action(recoveryEligible eligible: Bool) -> Action {
    let observedUnavailable = unavailableObserved
    finishAttempt()
    guard !consumed, eligible, observedUnavailable else {
      return .doNotRetry
    }
    consumed = true
    return .waitForAvailability
  }

}
