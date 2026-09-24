import Foundation
#if canImport(UIKit)
import UIKit
#endif

public enum PrnsNativeStartPhase: String {
  case failed, notRequested, running, starting, stopping
}

public enum PrnsNativeStartAdmissionError: Error {
  case stopInProgress, supersededByStop, bluetoothPermissionNeedsForeground
  case restorationNotAuthorized, resultUnavailable, tooManyWaiters
}

/// Process-owned startup admission independent of an application's result type.
/// One generation owns every coalesced waiter; incomplete stop retains admission.
@MainActor
public final class PrnsBluetoothRuntimeCoordinator<Outcome> {
  public struct Status {
    public let authorization: PrnsBluetoothAuthorization
    public let nativeStart: PrnsNativeStartPhase
    public let restorationAttemptRequested: Bool
    public let revision: UInt64
  }

  private typealias NativeStartPhase = PrnsNativeStartPhase
  private let readAuthorization: () -> PrnsBluetoothAuthorization
  private let applicationActive: () -> Bool
  private let cacheOutcome: (Outcome) -> Outcome?
  private let onStatus: (Status) -> Void
  private let maximumWaiters: Int
  #if canImport(UIKit)
  private var activationObserver: NSObjectProtocol?
  #endif

  private var observingAuthorization = false
  private var authorization = PrnsBluetoothAuthorization.notDetermined
  private var nativeStartPhase = NativeStartPhase.notRequested
  private var nativeStartCompletions: [(Result<Outcome, Error>) -> Void] = []
  private var nativeStartGeneration: UInt64 = 0
  private var completedStartOutcome: Outcome?
  private var restorationDispatch = PrnsRestorationDispatch()
  private var restorationReady: (() -> Void)?
  private var statusRevision: UInt64 = 0

  public init(
    authorization: @escaping () -> PrnsBluetoothAuthorization,
    applicationActive: @escaping () -> Bool,
    cacheOutcome: @escaping (Outcome) -> Outcome?,
    maximumWaiters: Int = 8,
    onStatus: @escaping (Status) -> Void
  ) {
    precondition(maximumWaiters > 0)
    self.readAuthorization = authorization
    self.applicationActive = applicationActive
    self.cacheOutcome = cacheOutcome
    self.maximumWaiters = maximumWaiters
    self.onStatus = onStatus
  }

  deinit {
    #if canImport(UIKit)
    if let activationObserver { NotificationCenter.default.removeObserver(activationObserver) }
    #endif
  }

  public var status: Status {
    Status(authorization: authorization, nativeStart: nativeStartPhase,
      restorationAttemptRequested: restorationDispatch.attemptRequested, revision: statusRevision)
  }

  public func activate(
    restorationAttemptRequested: Bool,
    restorationReady: @escaping () -> Void
  ) {
    restorationDispatch.request(restorationAttemptRequested)
    self.restorationReady = restorationReady
    if !observingAuthorization {
      observingAuthorization = true
      #if canImport(UIKit)
      activationObserver = NotificationCenter.default.addObserver(
        forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main
      ) { [weak self] _ in
        MainActor.assumeIsolated { self?.applicationDidBecomeActive() }
      }
      #endif
    }
    refreshAuthorization()
    publish()
    dispatchRestorationStartIfReady()
  }

  public func applicationDidBecomeActive() {
    // Observe permission changes without creating another Bluetooth manager.
    refreshAuthorization()
    dispatchRestorationStartIfReady()
  }

  public func refreshAuthorization() {
    let current = readAuthorization()
    guard authorization != current else { return }
    authorization = current
    publish()
  }

  public func requireRestorationAuthorized() throws {
    refreshAuthorization()
    guard authorization.permitsAutomaticRestoration else {
      throw PrnsNativeStartAdmissionError.restorationNotAuthorized
    }
  }

  public func restorationStartAuthorizationDidClose() {
    restorationDispatch.rearmAfterAuthorizationLoss()
    publish()
    dispatchRestorationStartIfReady()
  }

  public func requestNativeStart(
    restorationOnly: Bool,
    _ completion: @escaping (Result<Outcome, Error>) -> Void
  ) throws -> UInt64? {
    try requireStartAllowed(restorationOnly: restorationOnly)
    switch nativeStartPhase {
    case .running:
      guard let completedStartOutcome else {
        throw PrnsNativeStartAdmissionError.resultUnavailable
      }
      completion(.success(completedStartOutcome))
      return nil
    case .starting:
      guard nativeStartCompletions.count < maximumWaiters else {
        throw PrnsNativeStartAdmissionError.tooManyWaiters
      }
      nativeStartCompletions.append(completion)
      return nil
    case .stopping:
      throw PrnsNativeStartAdmissionError.stopInProgress
    case .failed, .notRequested:
      nativeStartGeneration &+= 1
      nativeStartPhase = .starting
      nativeStartCompletions = [completion]
      completedStartOutcome = nil
      publish()
      return nativeStartGeneration
    }
  }

  public func requireStartAllowed(startGeneration: UInt64, restorationOnly: Bool) throws {
    guard
      nativeStartGeneration == startGeneration,
      nativeStartPhase == .starting
    else {
      throw PrnsNativeStartAdmissionError.supersededByStop
    }
    try requireStartAllowed(restorationOnly: restorationOnly)
  }

  private func requireStartAllowed(restorationOnly: Bool) throws {
    refreshAuthorization()
    guard authorization.permitsStart(
      restorationOnly: restorationOnly,
      applicationActive: applicationActive()
    ) else {
      throw restorationOnly
        ? PrnsNativeStartAdmissionError.restorationNotAuthorized
        : PrnsNativeStartAdmissionError.bluetoothPermissionNeedsForeground
    }
  }

  public func nativeStartDidFinish(outcome: Outcome, startGeneration: UInt64) {
    guard
      nativeStartGeneration == startGeneration,
      nativeStartPhase == .starting
    else {
      return
    }
    let cachedOutcome = cacheOutcome(outcome)
    nativeStartPhase = cachedOutcome == nil ? .failed : .running
    completedStartOutcome = cachedOutcome
    let completions = nativeStartCompletions
    nativeStartCompletions.removeAll(keepingCapacity: false)
    for completion in completions {
      completion(.success(outcome))
    }
    refreshAuthorization()
    publish()
  }

  public func nativeStartDidFail(_ error: Error, startGeneration: UInt64) {
    guard
      nativeStartGeneration == startGeneration,
      nativeStartPhase == .starting
    else {
      return
    }
    nativeStartPhase = .failed
    completedStartOutcome = nil
    let completions = nativeStartCompletions
    nativeStartCompletions.removeAll(keepingCapacity: false)
    for completion in completions {
      completion(.failure(error))
    }
    refreshAuthorization()
    publish()
  }

  public func nativeStopWillBegin() {
    restorationDispatch.cancel()
    nativeStartGeneration &+= 1
    nativeStartPhase = .stopping
    completedStartOutcome = nil
    let completions = nativeStartCompletions
    nativeStartCompletions.removeAll(keepingCapacity: false)
    let error = PrnsNativeStartAdmissionError.supersededByStop
    for completion in completions {
      completion(.failure(error))
    }
    publish()
  }

  public func nativeStopDidFinish(joined: Bool) {
    // Incomplete shutdown cannot admit another native generation.
    nativeStartPhase = joined ? .notRequested : .stopping
    completedStartOutcome = nil
    publish()
  }

  private func dispatchRestorationStartIfReady() {
    guard
      let restorationReady,
      restorationDispatch.claimIfAuthorized(authorization.permitsAutomaticRestoration)
    else {
      return
    }
    restorationReady()
  }

  private func publish() {
    statusRevision &+= 1
    onStatus(status)
  }
}
