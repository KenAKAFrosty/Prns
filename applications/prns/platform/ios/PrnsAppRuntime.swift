import CoreBluetooth
import Foundation
import PrnsHostExpo
import UIKit

/// Process-owned platform admission. The Rust registry remains the sole native
/// installation owner; React reloads never create or close another owner.
@MainActor
final class PrnsAppRuntime {
  static let shared = PrnsAppRuntime()
  private let construction = DispatchQueue(label: "rs.reticulum.prns.app.installation")
  private let defaults = UserDefaults.standard
  private var protectedRetry: (() -> Void)?
  private var bluetoothAttachment: Task<Void, Error>?
  private var stopping: Task<Void, Error>?
  private var preparingLocal = false
  private let bluetoothPermission = PrnsBluetoothPermission()
  private var bluetoothChanging = false
  private var appliedGroups: [String]?
  private var permissionEpoch: UInt64 = 0
  private lazy var lan = PrnsLanNetworkMonitor(changed: { mobileAutoLanNetworkChanged() })
  private lazy var protectedData = PrnsProtectedDataCoordinator(
    retry: { [weak self] _ in
      let retry = self?.protectedRetry
      self?.protectedRetry = nil
      retry?()
    }, onEvent: { _ in }
  )
  private lazy var admission = PrnsBluetoothRuntimeCoordinator<ServiceRuntime>(
    authorization: Self.authorization,
    applicationActive: { UIApplication.shared.applicationState == .active },
    cacheOutcome: { $0.networkStatus().phase == .ready ? $0 : nil },
    onStatus: { _ in }
  )

  private enum Failure: LocalizedError {
    case invalidMode, protectedDataUnavailable, transport(String)
    var errorDescription: String? {
      switch self {
      case .invalidMode: return "Choose create or open for this installation."
      case .protectedDataUnavailable: return "Unlock the device before opening PRNS."
      case .transport(let detail): return detail
      }
    }
  }

  private static func authorization() -> PrnsBluetoothAuthorization {
    switch CBManager.authorization {
    case .notDetermined: return .notDetermined
    case .restricted: return .restricted
    case .denied: return .denied
    case .allowedAlways: return .allowedAlways
    @unknown default: return .restricted
    }
  }
  func bluetoothSelected() -> Bool { defaults.bool(forKey: "prns.app.bluetooth") }

  func bluetoothAuthorization() -> String {
    let value = Self.authorization()
    return value == .allowedAlways ? "allowed" : value.rawValue
  }

  func activate(restorationRequested: Bool) {
    configureAppleMulticast(enabled: Bundle.main.object(forInfoDictionaryKey: "PRNSMulticastEnabled") as? Bool ?? false)
    protectedData.observe(application: .shared)
    admission.activate(
      restorationAttemptRequested: restorationRequested && defaults.bool(forKey: "prns.app.running") &&
        defaults.bool(forKey: "prns.app.bluetooth")
    ) { [weak self] in
      self?.start(mode: "open", bluetooth: true, restorationOnly: true) { _ in }
    }
  }

  func start(
    mode: String, bluetooth: Bool, restorationOnly: Bool = false,
    completion: @escaping (Result<Void, Error>) -> Void
  ) {
    guard mode == "create" || mode == "open" else { completion(.failure(Failure.invalidMode)); return }
    prepareLocal(mode: mode, reviewId: nil, bluetooth: bluetooth,
      restorationOnly: restorationOnly, completion: completion)
  }

  func startImported(reviewId: String, bluetooth: Bool,
    completion: @escaping (Result<Void, Error>) -> Void
  ) {
    guard reviewId.utf8.count == 64, reviewId.utf8.allSatisfy({
      (48...57).contains($0) || (97...102).contains($0)
    }) else { completion(.failure(Failure.invalidMode)); return }
    prepareLocal(mode: "create", reviewId: reviewId, bluetooth: bluetooth,
      restorationOnly: false, completion: completion)
  }

  /// Local data readiness does not depend on radio permissions or network admission.
  /// An imported capability is consumed only here; all subsequent starts strictly open.
  private func prepareLocal(mode: String, reviewId: String?, bluetooth: Bool,
    restorationOnly: Bool, completion: @escaping (Result<Void, Error>) -> Void
  ) {
    guard !preparingLocal, !bluetoothChanging, stopping == nil else {
      completion(.failure(PrnsNativeStartAdmissionError.stopInProgress)); return
    }
    preparingLocal = true
    let epoch = permissionEpoch
    activateIfNeeded()
    protectedData.beginAttempt(application: .shared)
    PrnsNativeStartDispatch.enqueue(on: construction,
      validate: { [weak self] in
        guard let self, self.permissionEpoch == epoch, self.stopping == nil else {
          throw PrnsNativeStartAdmissionError.supersededByStop
        }
        // Automatic restoration must not open data before its existing authorization.
        if restorationOnly { try self.admission.requireRestorationAuthorized() }
        guard UIApplication.shared.isProtectedDataAvailable else { throw Failure.protectedDataUnavailable }
      }, prepare: nil,
      start: {
        let root = try PrnsAppStorage.root(create: mode == "create")
        if let reviewId { return try appPrepareRetainedPhoneIdentityImport(storageRoot: root.path, id: reviewId) }
        return try mode == "create" ? appPrepareNew(storageRoot: root.path) : appPrepareExisting(storageRoot: root.path)
      }, completion: { [weak self] result in
        guard let self else { return }
        switch result {
        case .failure(let error):
          self.preparingLocal = false
          if self.permissionEpoch == epoch, case Failure.protectedDataUnavailable = error {
            self.protectedRetry = { [weak self] in
              self?.prepareLocal(mode: mode, reviewId: reviewId, bluetooth: bluetooth,
                restorationOnly: restorationOnly) { _ in }
            }
            self.protectedData.recoverIfNeeded(eligible: true, application: .shared)
          } else { self.protectedData.finishAttempt() }
          completion(.failure(error))
        case .success(let owner):
          Task { @MainActor in
            do {
              try await owner.ready()
              if let reviewId { try appCancelPhoneIdentityImport(id: reviewId) }
              guard self.permissionEpoch == epoch, self.stopping == nil else {
                throw PrnsNativeStartAdmissionError.supersededByStop
              }
              self.preparingLocal = false
              self.protectedData.finishAttempt()
              self.startNetworking(mode: "open", bluetooth: bluetooth,
                restorationOnly: restorationOnly, completion: completion)
            } catch {
              self.preparingLocal = false
              self.protectedData.finishAttempt()
              completion(.failure(error))
            }
          }
        }
      })
  }

  private func startNetworking(
    mode: String, bluetooth: Bool, restorationOnly: Bool = false,
    completion: @escaping (Result<Void, Error>) -> Void
  ) {
    guard mode == "create" || mode == "open" else { completion(.failure(Failure.invalidMode)); return }
    activateIfNeeded()
    do {
      let generation = try admission.requestNativeStart(
        restorationOnly: restorationOnly, bluetoothRequested: bluetooth
      ) { [weak self] result in
        guard let self else { return }
        switch result {
        case .failure(let error): completion(.failure(error))
        case .success(let owner):
          // Capture the revocable network handle before any queued platform work.
          do {
            let host = try owner.host()
            let epoch = self.permissionEpoch
            Task { @MainActor in
              do {
                try await self.attachBluetooth(host, requested: bluetooth, epoch: epoch)
                completion(.success(()))
              } catch { completion(.failure(error)) }
            }
          } catch { completion(.failure(error)) }
        }
      }
      guard let generation else { return }
      protectedData.beginAttempt(application: .shared)
      PrnsNativeStartDispatch.enqueue(
        on: construction,
        validate: { [weak self] in
          guard let self else { throw PrnsNativeStartAdmissionError.supersededByStop }
          try self.admission.requireStartAllowed(
            startGeneration: generation, restorationOnly: restorationOnly, bluetoothRequested: bluetooth
          )
          guard UIApplication.shared.isProtectedDataAvailable else { throw Failure.protectedDataUnavailable }
        }, prepare: nil,
        start: {
          let root = try PrnsAppStorage.root(create: mode == "create")
          return try mode == "create" ? appStartNew(storageRoot: root.path) : appOpenExisting(storageRoot: root.path)
        },
        completion: { [weak self] result in
          guard let self, self.admission.isCurrentStart(generation) else { return }
          switch result {
          case .failure(let error):
            self.admission.nativeStartDidFail(error, startGeneration: generation)
            self.rearmRestorationIfNeeded(error, restorationOnly: restorationOnly)
            // Retry only a pre-construction protected-data refusal. A failure
            // after publication must never retry Create or infer a fresh store.
            if case Failure.protectedDataUnavailable = error {
              self.protectedRetry = { [weak self] in
                self?.start(mode: mode, bluetooth: bluetooth, restorationOnly: restorationOnly) { _ in }
              }
              self.protectedData.recoverIfNeeded(eligible: true, application: .shared)
            } else { self.protectedData.finishAttempt() }
          case .success(let owner):
            Task { @MainActor in
              do {
                try await owner.ready()
                try await owner.networkReady()
                try self.admission.requireStartAllowed(
                  startGeneration: generation, restorationOnly: restorationOnly, bluetoothRequested: bluetooth
                )
                self.defaults.set(true, forKey: "prns.app.running")
                if !restorationOnly { self.defaults.set(bluetooth, forKey: "prns.app.bluetooth") }
                self.protectedData.finishAttempt()
                self.lan.start()
                self.admission.nativeStartDidFinish(outcome: owner, startGeneration: generation)
              } catch {
                guard self.admission.isCurrentStart(generation) else { return }
                self.protectedData.finishAttempt()
                self.admission.nativeStartDidFail(error, startGeneration: generation)
                self.rearmRestorationIfNeeded(error, restorationOnly: restorationOnly)
              }
            }
          }
        }
      )
    } catch {
      rearmRestorationIfNeeded(error, restorationOnly: restorationOnly)
      completion(.failure(error))
    }
  }

  private func rearmRestorationIfNeeded(_ error: Error, restorationOnly: Bool) {
    if restorationOnly, case PrnsNativeStartAdmissionError.restorationNotAuthorized = error {
      admission.restorationStartAuthorizationDidClose()
    }
  }

  private func activateIfNeeded() {
    // Repeated calls only refresh observation; restoration claims remain in the
    // SDK coordinator. No Bluetooth manager is created by this permission read.
    protectedData.observe(application: .shared)
    configureAppleMulticast(enabled: Bundle.main.object(forInfoDictionaryKey: "PRNSMulticastEnabled") as? Bool ?? false)
  }

  private func attachBluetooth(_ host: HostClientHandle, requested: Bool, epoch: UInt64) async throws {
    guard epoch == permissionEpoch else { throw PrnsNativeStartAdmissionError.supersededByStop }
    // LAN/TCP activation follows the native profile owner. Platform selection
    // only owns this explicit Bluetooth permission and attachment path.
    guard requested else { return }
    if bluetoothAttachment == nil {
      bluetoothAttachment = Task { @MainActor in
        do {
          let groups = try self.savedDiscoveryGroups()
          try await Self.attach(host, config: .configuredBluetoothLe(discoveryGroups: groups))
          guard epoch == self.permissionEpoch else { throw PrnsNativeStartAdmissionError.supersededByStop }
          self.appliedGroups = groups
        } catch {
          if epoch == self.permissionEpoch { self.bluetoothAttachment = nil }
          throw error
        }
      }
    }
    try await bluetoothAttachment!.value
  }

  private static func attach(_ host: HostClientHandle, config: InterfaceConfig) async throws {
    if try await host.snapshot().interfaces.contains(where: { $0.kind == .automaticBluetoothLe }) { return }
    let settlement = try await host.execute(command: .attachInterface(config: config, routing: nil))
    if case .failed(let failure) = settlement { throw Failure.transport(String(describing: failure)) }
  }

  func requestBluetoothPermission() async throws {
    try await bluetoothPermission.request()
    admission.refreshAuthorization()
  }

  private func groupsPath() throws -> URL {
    try PrnsAppStorage.root(create: true).appendingPathComponent("bluetooth-groups.json")
  }

  private func savedDiscoveryGroups() throws -> [String] {
    let path = try groupsPath()
    guard FileManager.default.fileExists(atPath: path.path) else { return ["reticulum"] }
    let data = try Data(contentsOf: path)
    guard data.count <= 800 else { throw Failure.transport("Saved discovery groups are invalid") }
    return try validateBluetoothDiscoveryGroups(groups: JSONDecoder().decode([String].self, from: data))
  }

  private func bluetoothAttached() async -> Bool {
    guard let host = try? appOwner().host(), let snapshot = try? await host.snapshot() else { return false }
    return snapshot.interfaces.contains { $0.kind == .automaticBluetoothLe }
  }

  func platformStatus() async -> [String: Any] {
    ["bluetooth": ["authorization": bluetoothAuthorization(), "availability": "unknown",
      "selected": bluetoothSelected(), "attached": await bluetoothAttached()],
      "notifications": "notApplicable"]
  }

  func discoveryGroups() async throws -> [String: Any] {
    let applied = await bluetoothAttached() ? appliedGroups : nil
    return ["saved": try savedDiscoveryGroups(), "applied": applied as Any? ?? NSNull()]
  }

  func setBluetoothEnabled(_ enabled: Bool) async throws {
    guard !preparingLocal, !bluetoothChanging, stopping == nil, admission.status.nativeStart != .starting else { throw Failure.transport("Startup or a Bluetooth change is still finishing") }
    bluetoothChanging = true
    defer { bluetoothChanging = false }
    let epoch = permissionEpoch
    if enabled { try await requestBluetoothPermission() }
    guard epoch == permissionEpoch else { throw PrnsNativeStartAdmissionError.supersededByStop }
    defaults.set(enabled, forKey: "prns.app.bluetooth")
    try await reconfigureBluetooth(epoch: epoch)
  }

  func setDiscoveryGroups(_ names: [String]) async throws -> [String: Any] {
    let groups = try validateBluetoothDiscoveryGroups(groups: names)
    guard !preparingLocal, !bluetoothChanging, stopping == nil, admission.status.nativeStart != .starting else { throw Failure.transport("Startup or a Bluetooth change is still finishing") }
    bluetoothChanging = true
    defer { bluetoothChanging = false }
    let epoch = permissionEpoch
    try JSONEncoder().encode(groups).write(to: groupsPath(), options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    try await reconfigureBluetooth(epoch: epoch)
    return try await discoveryGroups()
  }

  private func reconfigureBluetooth(epoch: UInt64) async throws {
    // The captured handle stays tied to this exact network generation across awaits.
    guard let owner = try? appOwner(), let host = try? owner.host() else { appliedGroups = nil; return }
    _ = await bluetoothAttachment?.result
    guard epoch == permissionEpoch else { throw PrnsNativeStartAdmissionError.supersededByStop }
    appliedGroups = nil
    let snapshot = try await host.snapshot()
    for interface in snapshot.interfaces where interface.kind == .automaticBluetoothLe {
      let outcome = try await host.execute(command: .detachInterface(interface: interface.interfaceId))
      if case .failed(let failure) = outcome { throw Failure.transport(String(describing: failure)) }
    }
    bluetoothAttachment = nil
    guard epoch == permissionEpoch else { throw PrnsNativeStartAdmissionError.supersededByStop }
    if bluetoothSelected() {
      guard Self.authorization() == .allowedAlways else { throw Failure.transport("Bluetooth permission was not granted") }
      let groups = try savedDiscoveryGroups()
      let outcome = try await host.execute(command: .attachInterface(config: .configuredBluetoothLe(discoveryGroups: groups), routing: nil))
      if case .failed(let failure) = outcome { throw Failure.transport(String(describing: failure)) }
      guard epoch == permissionEpoch else { throw PrnsNativeStartAdmissionError.supersededByStop }
      appliedGroups = groups
    }
  }

  func openSettings(_ destination: String) async throws {
    guard ["application", "bluetooth", "location", "notifications"].contains(destination) else { throw Failure.invalidMode }
    let target: String
    if #available(iOS 16.0, *), destination == "notifications" { target = UIApplication.openNotificationSettingsURLString }
    else { target = UIApplication.openSettingsURLString }
    guard let url = URL(string: target), await UIApplication.shared.open(url) else { throw Failure.transport("System settings could not be opened") }
  }

  func prepareReset() async throws {
    try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
      construction.async {
        do {
          try appCaptureResetRoot(storageRoot: PrnsAppStorage.root(create: false).path)
          continuation.resume(returning: ())
        } catch { continuation.resume(throwing: error) }
      }
    }
  }

  func resetPreferences() async throws {
    guard !preparingLocal, stopping == nil, !bluetoothChanging, bluetoothAttachment == nil,
      admission.status.nativeStart == .notRequested else {
      throw Failure.transport("Stop networking before clearing platform preferences")
    }
    let reset: Bool
    do { _ = try appOwner(); reset = false }
    catch AppBootstrapError.NotStarted { reset = true }
    guard reset else { throw Failure.transport("Complete the installation reset before clearing platform preferences") }
    permissionEpoch &+= 1
    let path = try PrnsAppStorage.root(create: false).appendingPathComponent("bluetooth-groups.json")
    do { try FileManager.default.removeItem(at: path) }
    catch let error as CocoaError where error.code == .fileNoSuchFile || error.code == .fileReadNoSuchFile { }
    defaults.removeObject(forKey: "prns.app.running")
    defaults.removeObject(forKey: "prns.app.bluetooth")
    protectedRetry = nil
    appliedGroups = nil
  }

  func stop(completion: @escaping (Result<Void, Error>) -> Void) {
    if stopping == nil {
      permissionEpoch &+= 1
      appliedGroups = nil
      admission.nativeStopWillBegin()
      defaults.set(false, forKey: "prns.app.running")
      protectedRetry = nil
      protectedData.stop(application: .shared)
      lan.stop()
      // Fence immediately when an owner has been published. The serial barrier
      // below is still required: Stop can win after startup validation but just
      // before the native constructor publishes its process-owned handle.
      _ = try? appStop()
      stopping = Task { @MainActor in
        defer { self.stopping = nil }
        do {
          let close: ServiceRuntimeNetworkChange? = try await withCheckedThrowingContinuation { continuation in
            construction.async {
              do { continuation.resume(returning: try appStop()) }
              catch AppBootstrapError.NotStarted { continuation.resume(returning: nil) }
              catch { continuation.resume(throwing: error) }
            }
          }
          guard let close else { finishStop(joined: true); return }
          let result: Result<Void, Error>
          do { try await close.wait(); result = .success(()) }
          catch { result = .failure(error) }
          let joined = close.status().phase == .offline
          if joined {
            // Await only existing platform command observers. This stops their
            // late continuations changing preferences for a replacement owner.
            _ = await bluetoothAttachment?.result
          }
          finishStop(joined: joined)
          try result.get()
        } catch {
          if admission.status.nativeStart == .stopping { finishStop(joined: false) }
          throw error
        }
      }
    }
    let pending = stopping!
    Task { @MainActor in
      do { try await pending.value; completion(.success(())) }
      catch { completion(.failure(error)) }
    }
  }

  private func finishStop(joined: Bool) {
    if joined { bluetoothAttachment = nil; appliedGroups = nil }
    admission.nativeStopDidFinish(joined: joined)
  }
}
