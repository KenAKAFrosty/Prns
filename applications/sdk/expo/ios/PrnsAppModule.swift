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

    Events("onBluetoothAuthorizationStatus")

    OnStartObserving("onBluetoothAuthorizationStatus") {
      NotificationCenter.default.addObserver(
        self,
        selector: #selector(self.handleBluetoothAuthorizationStatus(_:)),
        name: prnsBluetoothAuthorizationStatusDidChange,
        object: nil
      )
    }

    OnStopObserving("onBluetoothAuthorizationStatus") {
      NotificationCenter.default.removeObserver(
        self,
        name: prnsBluetoothAuthorizationStatusDidChange,
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

    AsyncFunction("bluetoothAuthorizationStatus") { (promise: Promise) in
      DispatchQueue.main.async {
        PrnsBluetoothCoordinator.shared.refreshAuthorization()
        promise.resolve(PrnsBluetoothCoordinator.shared.statusJSON())
      }
    }

    AsyncFunction("start") { (inputBytes: [UInt8], promise: Promise) in
      do {
        let input = try Self.validatedStartInput(Self.decodeStartInput(inputBytes))
        DispatchQueue.main.async {
          Self.startNative(input) { result in
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
    // already admitted by process-owned native startup.
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
  private func handleBluetoothAuthorizationStatus(_ notification: Notification) {
    guard let status = notification.userInfo?["status"] as? String else { return }
    sendEvent("onBluetoothAuthorizationStatus", ["status": status])
  }

  @MainActor
  static func startNative(
    _ input: DevelopmentNodeStartInput,
    restorationOnly: Bool = false,
    prepareRestoration: (() throws -> Void)? = nil,
    completion: @escaping (Result<DevelopmentNodeStartOutcome, Error>) -> Void
  ) {
    let startGeneration: UInt64
    do {
      guard
        let requestedGeneration = try PrnsBluetoothCoordinator.shared.requestNativeStart(
          restorationOnly: restorationOnly,
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
    PrnsNativeStartDispatch.enqueue(
      on: nativeQueue,
      validate: {
        try PrnsBluetoothCoordinator.shared.requireStartAllowed(
          startGeneration: startGeneration,
          restorationOnly: restorationOnly
        )
      },
      prepare: prepareRestoration,
      start: { try startWithBluetoothRestoration(input) },
      completion: { result in
        switch result {
        case .success(let outcome):
          PrnsBluetoothCoordinator.shared.nativeStartDidFinish(
            outcome: outcome,
            startGeneration: startGeneration
          )
        case .failure(let error):
          PrnsBluetoothCoordinator.shared.nativeStartDidFail(
            error,
            startGeneration: startGeneration
          )
        }
      }
    )
  }

  static func configuredStartInput() -> DevelopmentNodeStartInput {
    #if DEBUG
    let target = Bundle.main.object(forInfoDictionaryKey: "PRNSDevelopmentTcpTarget") as? String
    return DevelopmentNodeStartInput(developmentTcpTarget: target)
    #else
    DevelopmentNodeStartInput(developmentTcpTarget: nil)
    #endif
  }

  private static func validatedStartInput(
    _ input: DevelopmentNodeStartInput
  ) throws -> DevelopmentNodeStartInput {
    let configured = configuredStartInput()
    if let requested = input.developmentTcpTarget,
      requested != configured.developmentTcpTarget
    {
      throw PrnsAppException("The iOS test peer is set by the installed build. Rebuild the app to change it.")
    }
    return configured
  }

  static func startWithBluetoothRestoration(
    _ input: DevelopmentNodeStartInput
  ) throws -> DevelopmentNodeStartOutcome {
    let identifiers = try restorationIdentifiers()
    return nativeStartWithAppleBluetoothRestoration(
      storageRoot: try developmentStorageURL(create: true).path,
      input: input,
      centralIdentifier: identifiers.central,
      peripheralIdentifier: identifiers.peripheral
    )
  }

  static func prepareBluetoothRestoration() throws -> AppleBluetoothRestorationPreparationOutcome {
    let identifiers = try restorationIdentifiers()
    return nativePrepareAppleBluetoothRestoration(
      storageRoot: try restorationStorageURL().path,
      centralIdentifier: identifiers.central,
      peripheralIdentifier: identifiers.peripheral
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

  static func restorationIdentifiers() throws -> (central: String, peripheral: String) {
    let centralKey = "PRNSCoreBluetoothCentralRestorationIdentifier"
    let peripheralKey = "PRNSCoreBluetoothPeripheralRestorationIdentifier"
    guard
      let central = Bundle.main.object(forInfoDictionaryKey: centralKey) as? String,
      !central.isEmpty
    else {
      throw PrnsAppException("The app is missing its central Bluetooth restoration identifier.")
    }
    guard
      let peripheral = Bundle.main.object(forInfoDictionaryKey: peripheralKey) as? String,
      !peripheral.isEmpty,
      peripheral != central
    else {
      throw PrnsAppException("The app is missing a distinct peripheral Bluetooth restoration identifier.")
    }
    return (central, peripheral)
  }

  static func developmentStorageURL(create: Bool) throws -> URL {
    try storageURL(create: create, migrateExistingContents: true)
  }

  private static func restorationStorageURL() throws -> URL {
    try storageURL(create: false, migrateExistingContents: false)
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
      PrnsBluetoothCoordinator.shared.nativeStopWillBegin()
    }
  }

  private static func finishNativeStop(_ outcome: DevelopmentNodeStopOutcome) {
    DispatchQueue.main.sync {
      PrnsBluetoothCoordinator.shared.nativeStopDidFinish(outcome: outcome)
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
      let started = nativeStartWithAppleBluetoothRestoration(
          storageRoot: path, input: configuredStartInput(),
          centralIdentifier: "rs.reticulum.prns.smoke.bluetooth-auto.central.v1",
          peripheralIdentifier: "rs.reticulum.prns.smoke.bluetooth-auto.peripheral.v1"
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
    let failedStart = nativeStartWithAppleBluetoothRestoration(
        storageRoot: path, input: configuredStartInput(),
        centralIdentifier: "rs.reticulum.prns.smoke.bluetooth-auto.central.v1",
        peripheralIdentifier: "rs.reticulum.prns.smoke.bluetooth-auto.peripheral.v1"
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
