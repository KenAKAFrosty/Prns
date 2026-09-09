import ExpoModulesCore
import Foundation

final class PrnsAppException: GenericException<String>, @unchecked Sendable {
  override var reason: String {
    param
  }
}

public final class PrnsAppModule: Module {
  static let nativeQueue = DispatchQueue(
    label: "rs.reticulum.prns.app.native",
    attributes: .concurrent
  )
  private static let storagePreparationLock = NSLock()
  private static var preparedStorageIdentifiers: [String: AnyHashable] = [:]

  public func definition() -> ModuleDefinition {
    Name("PrnsApp")

    Events("onAccessorySetupStatus")

    OnStartObserving("onAccessorySetupStatus") {
      NotificationCenter.default.addObserver(
        self,
        selector: #selector(self.handleAccessorySetupStatus(_:)),
        name: prnsAccessorySetupStatusDidChange,
        object: nil
      )
    }

    OnStopObserving("onAccessorySetupStatus") {
      NotificationCenter.default.removeObserver(
        self,
        name: prnsAccessorySetupStatusDidChange,
        object: nil
      )
    }

    AsyncFunction("prepareStorage") { () throws -> [UInt8] in
      let path = try Self.developmentStorageURL(create: true).path
      return Self.encode(nativePrepareStorage(storageRoot: path), FfiConverterTypeNativeStoragePreparationOutcome.write)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("inspectIdentity") { () throws -> [UInt8] in
      let path = try Self.developmentStorageURL(create: true).path
      return Self.encode(nativeInspectIdentity(storageRoot: path), FfiConverterTypePrimaryIdentityState.write)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("createGeneratedIdentity") { () throws -> [UInt8] in
      let path = try Self.developmentStorageURL(create: true).path
      return Self.encode(nativeCreateGeneratedIdentity(storageRoot: path), FfiConverterTypeIdentityCreationOutcome.write)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("createImportedIdentity") { (identity: [UInt8]) throws -> [UInt8] in
      let path = try Self.developmentStorageURL(create: true).path
      return Self.encode(nativeCreateImportedIdentity(storageRoot: path, identity: Data(identity)), FfiConverterTypeIdentityCreationOutcome.write)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("accessorySetupStatus") { (promise: Promise) in
      DispatchQueue.main.async {
        promise.resolve(PrnsAccessorySetupCoordinator.shared.statusJSON())
      }
    }

    AsyncFunction("showAccessorySetupPicker") { (promise: Promise) in
      DispatchQueue.main.async {
        PrnsAccessorySetupCoordinator.shared.showPicker { outcome in
          promise.resolve(outcome)
        }
      }
    }

    AsyncFunction("start") { (inputBytes: [UInt8], promise: Promise) in
      do {
        let input = try Self.decodeStartInput(inputBytes)
        DispatchQueue.main.async {
          Self.startAuthorized(input) { result in
            switch result {
            case .success(let outcome):
              promise.resolve(Self.encode(outcome, FfiConverterTypeDevelopmentNodeStartOutcome.write))
            case .failure(let error):
              promise.reject(error)
            }
          }
        }
      } catch {
        promise.reject(error)
      }
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("stop") { () -> [UInt8] in
      Self.beginNativeStop()
      let outcome = nativeStop()
      Self.finishNativeStop(outcome)
      return Self.encode(outcome, FfiConverterTypeDevelopmentNodeStopOutcome.write)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("reset") { () throws -> [UInt8] in
      let storageURL = try Self.developmentStorageURL(create: false)
      Self.beginNativeStop()
      let outcome = nativeReset(storageRoot: storageURL.path)
      Self.invalidatePreparedStorage(at: storageURL)
      Self.finishNativeStop(outcome)
      return Self.encode(outcome, FfiConverterTypeDevelopmentNodeStopOutcome.write)
    }.runOnQueue(Self.nativeQueue)

    // Android refreshes its foreground Bluetooth owner here. Apple's owner is
    // already admitted by native startup and accessory authorization.
    AsyncFunction("prepareOutbound") { () in }

    #if targetEnvironment(simulator)
    OnCreate {
      guard ProcessInfo.processInfo.environment["PRNS_IOS_NATIVE_SMOKE"] == "1" else { return }
      Self.nativeQueue.async(flags: .barrier) {
        do { try Self.runSimulatorLifecycleSmoke() }
        catch { Self.writeSimulatorSmokeMarker("PRNS_IOS_NATIVE_SMOKE_FAILED detail=\(error)") }
      }
    }
    #endif
  }

  @objc
  private func handleAccessorySetupStatus(_ notification: Notification) {
    guard let status = notification.userInfo?["status"] as? String else { return }
    sendEvent("onAccessorySetupStatus", ["status": status])
  }

  @MainActor
  static func startAuthorized(
    _ input: DevelopmentNodeStartInput,
    completion: @escaping (Result<DevelopmentNodeStartOutcome, Error>) -> Void
  ) {
    let startGeneration: UInt64
    do {
      guard
        let requestedGeneration = try PrnsAccessorySetupCoordinator.shared.requestNativeStart(
          completion
        )
      else {
        return
      }
      startGeneration = requestedGeneration
    } catch {
      completion(.failure(error))
      return
    }
    nativeQueue.async(flags: .barrier) {
      do {
        try DispatchQueue.main.sync {
          try PrnsAccessorySetupCoordinator.shared.requireAuthorized(
            startGeneration: startGeneration
          )
        }
        let outcome = try startWithCentralRestoration(input)
        DispatchQueue.main.async {
          PrnsAccessorySetupCoordinator.shared.nativeStartDidFinish(
            outcome: outcome,
            startGeneration: startGeneration
          )
        }
      } catch {
        DispatchQueue.main.async {
          PrnsAccessorySetupCoordinator.shared.nativeStartDidFail(
            error,
            startGeneration: startGeneration
          )
        }
      }
    }
  }

  static func configuredStartInput() -> DevelopmentNodeStartInput {
    DevelopmentNodeStartInput(developmentTcpTarget: nil)
  }

  static func startWithCentralRestoration(
    _ input: DevelopmentNodeStartInput
  ) throws -> DevelopmentNodeStartOutcome {
    nativeStartWithAppleBluetoothCentralRestoration(
      storageRoot: try developmentStorageURL(create: true).path,
      input: input,
      centralIdentifier: try restorationIdentifier()
    )
  }

  static func prepareBluetoothCentralRestoration() throws -> AppleBluetoothRestorationPreparationOutcome {
    nativePrepareAppleBluetoothCentralRestoration(
      storageRoot: try restorationStorageURL().path,
      centralIdentifier: try restorationIdentifier()
    )
  }

  // UniFFI owns the encoding; Expo only transports the owned bytes for calls
  // which need platform admission or filesystem work on the native queue.
  private static func encode<T>(
    _ value: T,
    _ write: (T, inout [UInt8]) -> Void
  ) -> [UInt8] {
    var bytes: [UInt8] = []
    write(value, &bytes)
    return bytes
  }

  private static func decodeStartInput(_ bytes: [UInt8]) throws -> DevelopmentNodeStartInput {
    var buffer = (data: Data(bytes), offset: Data.Index(0))
    let input = try FfiConverterTypeDevelopmentNodeStartInput.read(from: &buffer)
    guard buffer.offset == buffer.data.endIndex else {
      throw PrnsAppException("Unexpected bytes after the native start input.")
    }
    return input
  }

  static func restorationIdentifier() throws -> String {
    let centralKey = "PRNSCoreBluetoothCentralRestorationIdentifier"
    guard
      let central = Bundle.main.object(forInfoDictionaryKey: centralKey) as? String,
      !central.isEmpty
    else {
      throw PrnsAppException("The app is missing its central Bluetooth restoration identifier.")
    }
    return central
  }

  static func developmentStorageURL(create: Bool) throws -> URL {
    try storageURL(create: create, migrateExistingContents: true)
  }

  private static func restorationStorageURL() throws -> URL {
    try storageURL(create: true, migrateExistingContents: false)
  }

  private static func storageURL(
    create: Bool,
    migrateExistingContents: Bool
  ) throws -> URL {
    do {
      let fileManager = FileManager.default
      let applicationSupport = try fileManager.url(
        for: .applicationSupportDirectory,
        in: .userDomainMask,
        appropriateFor: nil,
        create: true
      )
      let storageURL = applicationSupport
        .appendingPathComponent("prns", isDirectory: true)
        .appendingPathComponent("development", isDirectory: true)
      if create {
        try fileManager.createDirectory(
          at: storageURL,
          withIntermediateDirectories: true
        )
      }
      if fileManager.fileExists(atPath: storageURL.path) {
        if migrateExistingContents {
          try prepareStoragePolicy(at: storageURL, fileManager: fileManager)
        } else {
          try prepareStorageRootPolicy(at: storageURL, fileManager: fileManager)
        }
      }
      return storageURL
    } catch {
      throw PrnsAppException("Unable to prepare the prns development storage directory.")
        .causedBy(error)
    }
  }

  private static func prepareStoragePolicy(at storageURL: URL, fileManager: FileManager) throws {
    storagePreparationLock.lock()
    defer { storagePreparationLock.unlock() }

    let resourceIdentifier = try storageURL.resourceValues(
      forKeys: [.fileResourceIdentifierKey]
    ).fileResourceIdentifier as? AnyHashable
    if let resourceIdentifier,
      preparedStorageIdentifiers[storageURL.path] == resourceIdentifier
    {
      return
    }
    preparedStorageIdentifiers.removeValue(forKey: storageURL.path)

    try prepareStorageRootPolicyUnlocked(at: storageURL, fileManager: fileManager)

    let keys: [URLResourceKey] = [.isSymbolicLinkKey]
    var traversalError: Error?
    guard
      let enumerator = fileManager.enumerator(
        at: storageURL,
        includingPropertiesForKeys: keys,
        options: [],
        errorHandler: { _, error in
          traversalError = error
          return false
        }
      )
    else {
      throw PrnsAppException("Unable to inspect the existing prns storage directory.")
    }
    for case let childURL as URL in enumerator {
      let values = try childURL.resourceValues(forKeys: Set(keys))
      if values.isSymbolicLink == true {
        enumerator.skipDescendants()
        continue
      }
      try applyProtection(to: childURL, fileManager: fileManager)
    }
    if let traversalError {
      throw traversalError
    }
    if let resourceIdentifier {
      preparedStorageIdentifiers[storageURL.path] = resourceIdentifier
    }
  }

  private static func prepareStorageRootPolicy(
    at storageURL: URL,
    fileManager: FileManager
  ) throws {
    storagePreparationLock.lock()
    defer { storagePreparationLock.unlock() }
    try prepareStorageRootPolicyUnlocked(at: storageURL, fileManager: fileManager)
  }

  private static func prepareStorageRootPolicyUnlocked(
    at storageURL: URL,
    fileManager: FileManager
  ) throws {
    try applyProtection(to: storageURL, fileManager: fileManager)
    var rootValues = URLResourceValues()
    rootValues.isExcludedFromBackup = true
    var mutableStorageURL = storageURL
    try mutableStorageURL.setResourceValues(rootValues)
  }

  private static func applyProtection(to url: URL, fileManager: FileManager) throws {
    try fileManager.setAttributes(
      [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication],
      ofItemAtPath: url.path
    )
  }

  private static func invalidatePreparedStorage(at storageURL: URL) {
    storagePreparationLock.lock()
    preparedStorageIdentifiers.removeValue(forKey: storageURL.path)
    storagePreparationLock.unlock()
  }

  private static func beginNativeStop() {
    DispatchQueue.main.sync {
      PrnsAppLifecycleCoordinator.shared.nativeStopWillBegin(application: .shared)
      PrnsAccessorySetupCoordinator.shared.nativeStopWillBegin()
    }
  }

  private static func finishNativeStop(_ outcome: DevelopmentNodeStopOutcome) {
    DispatchQueue.main.sync {
      PrnsAccessorySetupCoordinator.shared.nativeStopDidFinish(outcome: outcome)
    }
  }

  #if targetEnvironment(simulator)
  private final class SimulatorSnapshotWaiter: @unchecked Sendable {
    let ready = DispatchSemaphore(value: 0)
    var snapshot: DevelopmentNodeSnapshot?
  }

  private static func simulatorSnapshot() throws -> DevelopmentNodeSnapshot {
    let waiter = SimulatorSnapshotWaiter()
    Task {
      waiter.snapshot = await readSnapshot()
      waiter.ready.signal()
    }
    guard waiter.ready.wait(timeout: .now() + 5) == .success,
      let snapshot = waiter.snapshot
    else { throw PrnsAppException("Generated snapshot exceeded the simulator smoke bound.") }
    return snapshot
  }

  private static func runSimulatorLifecycleSmoke() throws {
    let fileManager = FileManager.default
    let temporaryRoot = fileManager.temporaryDirectory.appendingPathComponent("prns-native-smoke-\(UUID().uuidString)", isDirectory: true)
    let storageURL = temporaryRoot.appendingPathComponent("prns/development", isDirectory: true)
    try fileManager.createDirectory(at: storageURL, withIntermediateDirectories: true)
    defer { try? fileManager.removeItem(at: temporaryRoot) }
    let path = storageURL.path
    let contract = bindingContract()
    guard !contract.app.isEmpty, !contract.host.isEmpty else {
      throw PrnsAppException("Native contract fingerprints must not be empty.")
    }
    let identity = nativeCreateGeneratedIdentity(storageRoot: path)
    guard case .created = identity else { throw PrnsAppException("Identity creation failed.") }
    for _ in 1...2 {
      let started = nativeStartWithAppleBluetoothCentralRestoration(
          storageRoot: path, input: configuredStartInput(),
          centralIdentifier: "rs.reticulum.prns.smoke.bluetooth-auto.central.v1"
        )
      guard case .started(let owner) = started else { throw PrnsAppException("Native start failed.") }
      let first = try simulatorSnapshot()
      let second = try simulatorSnapshot()
      guard first.runtime == .running, second.runtime == .running,
        first.generationId == owner.generationId, first.primaryIdentity == owner.primaryIdentity
      else { throw PrnsAppException("Generated snapshot does not share the native owner.") }
      let stopped = nativeStop()
      guard case .stopped = stopped else { throw PrnsAppException("Native stop failed.") }
    }
    let bluetoothIdentity = storageURL.appendingPathComponent("identities/bluetooth-auto.identity")
    try Data(repeating: 0, count: 40).write(to: bluetoothIdentity)
    let failedStart = nativeStartWithAppleBluetoothCentralRestoration(
        storageRoot: path, input: configuredStartInput(),
        centralIdentifier: "rs.reticulum.prns.smoke.bluetooth-auto.central.v1"
      )
    guard case .failed(stage: .identity, detail: _) = failedStart else {
      throw PrnsAppException("Malformed identity did not fail native start.")
    }
    guard try simulatorSnapshot().runtime == .failed else { throw PrnsAppException("Failed startup snapshot is not failed.") }
    let reset = nativeReset(storageRoot: path)
    guard case .alreadyStopped = reset, !fileManager.fileExists(atPath: path) else {
      throw PrnsAppException("Failed-start reset did not remove storage.")
    }
    writeSimulatorSmokeMarker("PRNS_IOS_NATIVE_SMOKE_OK contract=\(contract.app) starts=2 snapshots=5 stops=2 cleanup=reset")
  }

  private static func writeSimulatorSmokeMarker(_ marker: String) {
    FileHandle.standardError.write(Data("\(marker)\n".utf8))
  }
  #endif
}
