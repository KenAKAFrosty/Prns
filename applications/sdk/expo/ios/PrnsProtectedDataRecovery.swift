struct PrnsProtectedDataRecovery {
  enum Action: Equatable {
    case doNotRetry
    case waitForAvailability
  }

  enum Failure {
    case bridge
    case cancelled
    case native(stage: String?)
  }

  private(set) var consumed = false
  private var attemptInFlight = false
  private var unavailableObserved = false

  mutating func beginAttempt(protectedDataAvailable: Bool) {
    attemptInFlight = true
    unavailableObserved = !protectedDataAvailable
  }

  mutating func protectedDataWillBecomeUnavailable() {
    guard attemptInFlight else {
      return
    }
    unavailableObserved = true
  }

  mutating func finishAttempt() {
    attemptInFlight = false
    unavailableObserved = false
  }

  mutating func action(for failure: Failure) -> Action {
    let eligible = Self.isRecoveryEligible(failure)
    let observedUnavailable = unavailableObserved
    finishAttempt()
    guard !consumed, eligible, observedUnavailable else {
      return .doNotRetry
    }
    consumed = true
    return .waitForAvailability
  }

  private static func isRecoveryEligible(_ failure: Failure) -> Bool {
    switch failure {
    case .bridge:
      return true
    case .cancelled:
      return false
    case .native(let stage):
      return stage == "storage" || stage == "identity" || stage == "persistenceRestore"
    }
  }
}
