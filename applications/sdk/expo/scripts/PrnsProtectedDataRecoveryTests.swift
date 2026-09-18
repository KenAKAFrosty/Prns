@main
struct PrnsProtectedDataRecoveryTests {
  static func main() {
    var unlockedBeforeFailureHandler = PrnsProtectedDataRecovery()
    unlockedBeforeFailureHandler.beginAttempt(protectedDataAvailable: false)
    expect(
      unlockedBeforeFailureHandler.action(
        for: .native(stage: "storage")
      ),
      .waitForAvailability,
      "a storage failure must retry when unlock wins the failure-handler race"
    )
    unlockedBeforeFailureHandler.beginAttempt(protectedDataAvailable: true)
    expect(
      unlockedBeforeFailureHandler.action(
        for: .native(stage: "storage")
      ),
      .doNotRetry,
      "the recovery retry must be bounded to one attempt"
    )

    var lockedDuringAttempt = PrnsProtectedDataRecovery()
    lockedDuringAttempt.beginAttempt(protectedDataAvailable: true)
    lockedDuringAttempt.protectedDataWillBecomeUnavailable()
    expect(
      lockedDuringAttempt.action(
        for: .native(stage: "identity")
      ),
      .waitForAvailability,
      "an identity failure after a lock transition must wait for protected data"
    )
    lockedDuringAttempt.beginAttempt(protectedDataAvailable: true)
    expect(
      lockedDuringAttempt.action(
        for: .native(stage: "persistenceRestore")
      ),
      .doNotRetry,
      "waiting for unlock must consume the single recovery attempt up front"
    )

    for stage in ["contract", "runtime", "bluetooth", "node", nil] {
      var unrelatedFailure = PrnsProtectedDataRecovery()
      unrelatedFailure.beginAttempt(protectedDataAvailable: false)
      expect(
        unrelatedFailure.action(
          for: .native(stage: stage)
        ),
        .doNotRetry,
        "\(stage ?? "missing") failures must not be mistaken for protected-data loss"
      )
      precondition(!unrelatedFailure.consumed)
    }

    var bridgeFailure = PrnsProtectedDataRecovery()
    bridgeFailure.beginAttempt(protectedDataAvailable: false)
    expect(
      bridgeFailure.action(for: .bridge),
      .waitForAvailability,
      "a thrown storage bridge must receive the same bounded unlock recovery"
    )

    var stopSupersession = PrnsProtectedDataRecovery()
    stopSupersession.beginAttempt(protectedDataAvailable: false)
    expect(
      stopSupersession.action(for: .cancelled),
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
    restorationDispatch.request(true)
    precondition(restorationDispatch.claimIfAuthorized(true))
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
    precondition(!cancelledRestorationDispatch.launchRequested)
  }

  private static func expect(
    _ actual: PrnsProtectedDataRecovery.Action,
    _ expected: PrnsProtectedDataRecovery.Action,
    _ message: String
  ) {
    precondition(actual == expected, "\(message): expected \(expected), got \(actual)")
  }
}
