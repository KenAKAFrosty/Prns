@main
struct PrnsProtectedDataRecoveryTests {
  static func main() {
    var unlockedBeforeFailureHandler = PrnsProtectedDataRecovery()
    unlockedBeforeFailureHandler.beginAttempt(protectedDataAvailable: false)
    expect(
      unlockedBeforeFailureHandler.action(
        recoveryEligible: true
      ),
      .waitForAvailability,
      "a recoverable failure must retry when unlock wins the failure-handler race"
    )
    unlockedBeforeFailureHandler.beginAttempt(protectedDataAvailable: true)
    expect(
      unlockedBeforeFailureHandler.action(
        recoveryEligible: true
      ),
      .doNotRetry,
      "the recovery retry must be bounded to one attempt"
    )

    var lockedDuringAttempt = PrnsProtectedDataRecovery()
    lockedDuringAttempt.beginAttempt(protectedDataAvailable: true)
    lockedDuringAttempt.protectedDataWillBecomeUnavailable()
    expect(
      lockedDuringAttempt.action(
        recoveryEligible: true
      ),
      .waitForAvailability,
      "a recoverable failure after a lock transition must wait for protected data"
    )
    lockedDuringAttempt.beginAttempt(protectedDataAvailable: true)
    expect(
      lockedDuringAttempt.action(
        recoveryEligible: true
      ),
      .doNotRetry,
      "waiting for unlock must consume the single recovery attempt up front"
    )

    var unrelatedFailure = PrnsProtectedDataRecovery()
    unrelatedFailure.beginAttempt(protectedDataAvailable: false)
    expect(
      unrelatedFailure.action(recoveryEligible: false),
      .doNotRetry,
      "a failure classified as unrelated must not trigger protected-data recovery"
    )
    precondition(!unrelatedFailure.consumed)

    var bridgeFailure = PrnsProtectedDataRecovery()
    bridgeFailure.beginAttempt(protectedDataAvailable: false)
    expect(
      bridgeFailure.action(recoveryEligible: true),
      .waitForAvailability,
      "a failure classified as recoverable must receive the same bounded unlock recovery"
    )

    var stopSupersession = PrnsProtectedDataRecovery()
    stopSupersession.beginAttempt(protectedDataAvailable: false)
    expect(
      stopSupersession.action(recoveryEligible: false),
      .doNotRetry,
      "an explicit stop supersession must never schedule protected-data recovery"
    )
    precondition(!stopSupersession.consumed)

    var relockedButReadable = PrnsProtectedDataRecovery()
    relockedButReadable.beginAttempt(protectedDataAvailable: false)
    relockedButReadable.finishAttempt()
    precondition(
      !relockedButReadable.consumed,
      "a successful attempt while ordinarily relocked must not consume recovery"
    )

    var restorationDispatch = PrnsRestorationDispatch()
    restorationDispatch.request(false)
    precondition(!restorationDispatch.claimIfAuthorized(true))
    restorationDispatch.request(true)
    precondition(!restorationDispatch.claimIfAuthorized(false))
    precondition(restorationDispatch.claimIfAuthorized(true))
    // A second caller or scene activation does not repeat process startup.
    restorationDispatch.request(true)
    precondition(!restorationDispatch.claimIfAuthorized(true))
    restorationDispatch.rearmAfterAuthorizationLoss()
    precondition(!restorationDispatch.claimIfAuthorized(false))
    precondition(restorationDispatch.claimIfAuthorized(true))
    precondition(!restorationDispatch.claimIfAuthorized(true))

    var cancelledRestorationDispatch = PrnsRestorationDispatch()
    cancelledRestorationDispatch.request(true)
    precondition(cancelledRestorationDispatch.claimIfAuthorized(true))
    cancelledRestorationDispatch.rearmAfterAuthorizationLoss()
    cancelledRestorationDispatch.cancel()
    precondition(!cancelledRestorationDispatch.claimIfAuthorized(true))
    precondition(!cancelledRestorationDispatch.attemptRequested)
    cancelledRestorationDispatch.rearmAfterAuthorizationLoss()
    precondition(!cancelledRestorationDispatch.claimIfAuthorized(true))

    var cancelledBeforeAuthorization = PrnsRestorationDispatch()
    cancelledBeforeAuthorization.request(true)
    precondition(!cancelledBeforeAuthorization.claimIfAuthorized(false))
    cancelledBeforeAuthorization.cancel()
    precondition(!cancelledBeforeAuthorization.claimIfAuthorized(true))
  }

  private static func expect(
    _ actual: PrnsProtectedDataRecovery.Action,
    _ expected: PrnsProtectedDataRecovery.Action,
    _ message: String
  ) {
    precondition(actual == expected, "\(message): expected \(expected), got \(actual)")
  }
}
