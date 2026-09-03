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

    AsyncFunction("contractFingerprint") { () throws -> String in
      try Self.contractFingerprint()
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("hostContractFingerprint") { () throws -> String in
      try Self.hostContractFingerprint()
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("inspectIdentity") { () throws -> String in
      let storageURL = try Self.developmentStorageURL(create: true)
      return try Self.withUtf8Bytes(storageURL.path) { pointer, count in
        try Self.consume(prns_app_inspect_identity(pointer, count))
      }
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("previewIdentityImport") { (identity: [UInt8]) throws -> String in
      try Self.withBytes(identity) { pointer, count in
        try Self.consume(prns_app_preview_identity_import(pointer, count))
      }
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("createGeneratedIdentity") { () throws -> String in
      let storageURL = try Self.developmentStorageURL(create: true)
      return try Self.withUtf8Bytes(storageURL.path) { pointer, count in
        try Self.consume(prns_app_create_generated_identity(pointer, count))
      }
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("createImportedIdentity") { (identity: [UInt8]) throws -> String in
      let storageURL = try Self.developmentStorageURL(create: true)
      return try Self.withUtf8Bytes(storageURL.path) { pathPointer, pathCount in
        try Self.withBytes(identity) { identityPointer, identityCount in
          try Self.consume(
            prns_app_create_imported_identity(
              pathPointer,
              pathCount,
              identityPointer,
              identityCount
            )
          )
        }
      }
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("start") { (inputJSON: String) throws -> String in
      try Self.startWithRestoration(inputJSON)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("snapshot") { () throws -> String in
      try Self.consume(prns_app_snapshot())
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("initiatePairing") { (inputJSON: String) throws -> String in
      try Self.invokeJSON(inputJSON, operation: prns_app_initiate_pairing)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("approvePairing") { (inputJSON: String) throws -> String in
      try Self.invokeJSON(inputJSON, operation: prns_app_approve_pairing)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("rejectPairing") { (inputJSON: String) throws -> String in
      try Self.invokeJSON(inputJSON, operation: prns_app_reject_pairing)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("describeTarget") { (inputJSON: String) throws -> String in
      try Self.invokeJSON(inputJSON, operation: prns_app_describe_target)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("saveObservedDestination") { (inputJSON: String) throws -> String in
      try Self.invokePathJSON(inputJSON, operation: prns_app_save_observed_destination)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("createManualContact") { (inputJSON: String) throws -> String in
      try Self.invokePathJSON(inputJSON, operation: prns_app_create_manual_contact)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("setContactAlias") { (inputJSON: String) throws -> String in
      try Self.invokePathJSON(inputJSON, operation: prns_app_set_contact_alias)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("setContactPinned") { (inputJSON: String) throws -> String in
      try Self.invokePathJSON(inputJSON, operation: prns_app_set_contact_pinned)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("deleteContact") { (inputJSON: String) throws -> String in
      try Self.invokePathJSON(inputJSON, operation: prns_app_delete_contact)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("getContact") { (inputJSON: String) throws -> String in
      try Self.invokePathJSON(inputJSON, operation: prns_app_get_contact)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("listContacts") { () throws -> String in
      let storageURL = try Self.developmentStorageURL(create: true)
      return try Self.withUtf8Bytes(storageURL.path) { pointer, count in
        try Self.consume(prns_app_list_contacts(pointer, count))
      }
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("listLxmfPeers") { () throws -> String in
      try Self.consume(prns_app_list_lxmf_peers())
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("listLxmfMessages") { (inputJSON: String) throws -> String in
      try Self.invokePathJSON(inputJSON, operation: prns_app_list_lxmf_messages)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("retryLxmfMessage") { (inputJSON: String) throws -> String in
      try Self.invokePathJSON(inputJSON, operation: prns_app_retry_lxmf_message)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("cancelLxmfMessage") { (inputJSON: String) throws -> String in
      try Self.invokePathJSON(inputJSON, operation: prns_app_cancel_lxmf_message)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("announceLxmf") { () throws -> String in
      try Self.consume(prns_app_announce_lxmf())
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("measureLxmfText") { (inputJSON: String) throws -> String in
      try Self.invokeJSON(inputJSON, operation: prns_app_measure_lxmf_text)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("sendDirectText") { (inputJSON: String) throws -> String in
      try Self.invokeJSON(inputJSON, operation: prns_app_send_direct_text)
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("stop") { () throws -> String in
      try Self.consume(prns_app_stop())
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("reset") { () throws -> String in
      let storageURL = try Self.developmentStorageURL(create: false)
      let outcome = try Self.withUtf8Bytes(storageURL.path) { pointer, count in
        try Self.consume(prns_app_reset(pointer, count))
      }
      Self.invalidatePreparedStorage(at: storageURL)
      return outcome
    }.runOnQueue(Self.nativeQueue)

    #if targetEnvironment(simulator)
    OnCreate {
      guard ProcessInfo.processInfo.environment["PRNS_IOS_NATIVE_SMOKE"] == "1" else {
        return
      }
      Self.nativeQueue.async(flags: .barrier) {
        do {
          try Self.runSimulatorLifecycleSmoke()
        } catch {
          Self.writeSimulatorSmokeMarker("PRNS_IOS_NATIVE_SMOKE_FAILED detail=\(error)")
        }
      }
    }
    #endif

  }

  static func configuredStartInputJSON() throws -> String {
    let data = try JSONSerialization.data(
      withJSONObject: ["developmentTcpTarget": NSNull()],
      options: [.sortedKeys]
    )
    guard let json = String(data: data, encoding: .utf8) else {
      throw PrnsAppException("Unable to encode the native restoration start input.")
    }
    return json
  }

  static func startWithRestoration(_ inputJSON: String) throws -> String {
    let storageURL = try developmentStorageURL(create: true)
    let identifiers = try restorationIdentifiers()
    return try withUtf8Bytes(storageURL.path) { pathPointer, pathCount in
      try withUtf8Bytes(inputJSON) { inputPointer, inputCount in
        try withUtf8Bytes(identifiers.central) { centralPointer, centralCount in
          try withUtf8Bytes(identifiers.peripheral) { peripheralPointer, peripheralCount in
            try consume(
              prns_app_start_with_apple_restoration(
                pathPointer,
                pathCount,
                inputPointer,
                inputCount,
                centralPointer,
                centralCount,
                peripheralPointer,
                peripheralCount
              )
            )
          }
        }
      }
    }
  }

  static func prepareBluetoothRestoration() throws -> String {
    let storageURL = try restorationStorageURL()
    let identifiers = try restorationIdentifiers()
    return try withUtf8Bytes(storageURL.path) { pathPointer, pathCount in
      try withUtf8Bytes(identifiers.central) { centralPointer, centralCount in
        try withUtf8Bytes(identifiers.peripheral) { peripheralPointer, peripheralCount in
          try consume(
            prns_app_prepare_apple_bluetooth_restoration(
              pathPointer,
              pathCount,
              centralPointer,
              centralCount,
              peripheralPointer,
              peripheralCount
            )
          )
        }
      }
    }
  }

  private static func contractFingerprint() throws -> String {
    guard let pointer = prns_app_contract_fingerprint() else {
      throw PrnsAppException("Native contract fingerprint pointer is null.")
    }
    return String(cString: pointer)
  }

  private static func hostContractFingerprint() throws -> String {
    guard let pointer = prns_app_host_contract_fingerprint() else {
      throw PrnsAppException("Native Host contract fingerprint pointer is null.")
    }
    return String(cString: pointer)
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
      !peripheral.isEmpty
    else {
      throw PrnsAppException("The app is missing its peripheral Bluetooth restoration identifier.")
    }
    guard central != peripheral else {
      throw PrnsAppException("The app's Bluetooth restoration identifiers must be distinct.")
    }
    return (central, peripheral)
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

  private static func invokeJSON(
    _ inputJSON: String,
    operation: (UnsafePointer<UInt8>?, Int) -> PrnsAppBytes
  ) throws -> String {
    try withUtf8Bytes(inputJSON) { pointer, count in
      try consume(operation(pointer, count))
    }
  }

  private static func invokePathJSON(
    _ inputJSON: String,
    operation: (
      UnsafePointer<UInt8>?,
      Int,
      UnsafePointer<UInt8>?,
      Int
    ) -> PrnsAppBytes
  ) throws -> String {
    let storageURL = try developmentStorageURL(create: true)
    return try withUtf8Bytes(storageURL.path) { pathPointer, pathCount in
      try withUtf8Bytes(inputJSON) { inputPointer, inputCount in
        try consume(operation(pathPointer, pathCount, inputPointer, inputCount))
      }
    }
  }

  private static func withUtf8Bytes<Result>(
    _ value: String,
    operation: (UnsafePointer<UInt8>?, Int) throws -> Result
  ) rethrows -> Result {
    let bytes = Array(value.utf8)
    return try bytes.withUnsafeBytes { rawBuffer in
      try operation(rawBuffer.bindMemory(to: UInt8.self).baseAddress, rawBuffer.count)
    }
  }

  private static func withBytes<Result>(
    _ bytes: [UInt8],
    operation: (UnsafePointer<UInt8>?, Int) throws -> Result
  ) rethrows -> Result {
    try bytes.withUnsafeBytes { rawBuffer in
      try operation(rawBuffer.bindMemory(to: UInt8.self).baseAddress, rawBuffer.count)
    }
  }

  private static func consume(_ bytes: PrnsAppBytes) throws -> String {
    defer {
      prns_app_bytes_free(bytes)
    }
    guard bytes.len == 0 || bytes.ptr != nil else {
      throw PrnsAppException("Native result has a null pointer with a nonzero length.")
    }
    let data: Data
    if let pointer = bytes.ptr {
      data = Data(bytes: pointer, count: bytes.len)
    } else {
      data = Data()
    }
    guard let json = String(data: data, encoding: .utf8) else {
      throw PrnsAppException("Native result is not valid UTF-8.")
    }
    return json
  }

  #if targetEnvironment(simulator)
  private static func runSimulatorLifecycleSmoke() throws {
    let fileManager = FileManager.default
    let temporaryRoot = fileManager.temporaryDirectory
      .appendingPathComponent("prns-native-smoke-\(UUID().uuidString)", isDirectory: true)
    let storageURL = temporaryRoot
      .appendingPathComponent("prns", isDirectory: true)
      .appendingPathComponent("development", isDirectory: true)
    try fileManager.createDirectory(at: storageURL, withIntermediateDirectories: true)
    defer {
      try? fileManager.removeItem(at: temporaryRoot)
    }

    let contract = try contractFingerprint()
    let hostContract = try hostContractFingerprint()
    guard !contract.isEmpty, !hostContract.isEmpty else {
      throw PrnsAppException("Native contract fingerprints must not be empty.")
    }
    let identity = try withUtf8Bytes(storageURL.path) { pointer, count in
      try consume(prns_app_create_generated_identity(pointer, count))
    }
    try requireTag(identity, operation: "identity", expected: "created")

    let startInput = #"{"developmentTcpTarget":null}"#
    for generation in 1 ... 2 {
      let started = try invokePathJSON(
        startInput,
        storageURL: storageURL,
        operation: restoringSmokeStart
      )
      try requireTag(started, operation: "start \(generation)", expected: "started")

      let snapshotStartedAt = Date()
      let firstSnapshot = try consume(prns_app_snapshot())
      let secondSnapshot = try consume(prns_app_snapshot())
      guard Date().timeIntervalSince(snapshotStartedAt) < 5 else {
        throw PrnsAppException("Native snapshot reads exceeded the simulator smoke bound.")
      }
      try requireField(
        firstSnapshot,
        operation: "snapshot \(generation).1",
        key: "runtime",
        expected: "running"
      )
      try requireField(
        secondSnapshot,
        operation: "snapshot \(generation).2",
        key: "runtime",
        expected: "running"
      )

      let stopped = try consume(prns_app_stop())
      try requireTag(stopped, operation: "stop \(generation)", expected: "stopped")
    }

    let bluetoothIdentity = storageURL
      .appendingPathComponent("identities", isDirectory: true)
      .appendingPathComponent("bluetooth-auto.identity")
    try Data(repeating: 0, count: 40).write(to: bluetoothIdentity)
    let failedStart = try invokePathJSON(
      startInput,
      storageURL: storageURL,
      operation: restoringSmokeStart
    )
    try requireTag(failedStart, operation: "malformed identity start", expected: "failed")
    try requireField(
      failedStart,
      operation: "malformed identity start",
      key: "stage",
      expected: "identity"
    )

    let failedSnapshot = try consume(prns_app_snapshot())
    try requireField(
      failedSnapshot,
      operation: "failed startup snapshot",
      key: "runtime",
      expected: "failed"
    )
    let reset = try withUtf8Bytes(storageURL.path) { pointer, count in
      try consume(prns_app_reset(pointer, count))
    }
    try requireTag(reset, operation: "failed startup reset", expected: "alreadyStopped")
    guard !fileManager.fileExists(atPath: storageURL.path) else {
      throw PrnsAppException("Simulator smoke storage survived reset.")
    }

    writeSimulatorSmokeMarker(
      "PRNS_IOS_NATIVE_SMOKE_OK contract=\(contract) starts=2 snapshots=5 stops=2 cleanup=reset"
    )
  }

  private static func invokePathJSON(
    _ inputJSON: String,
    storageURL: URL,
    operation: (
      UnsafePointer<UInt8>?,
      Int,
      UnsafePointer<UInt8>?,
      Int
    ) -> PrnsAppBytes
  ) throws -> String {
    try withUtf8Bytes(storageURL.path) { pathPointer, pathCount in
      try withUtf8Bytes(inputJSON) { inputPointer, inputCount in
        try consume(operation(pathPointer, pathCount, inputPointer, inputCount))
      }
    }
  }

  private static func restoringSmokeStart(
    _ pathPointer: UnsafePointer<UInt8>?,
    _ pathCount: Int,
    _ inputPointer: UnsafePointer<UInt8>?,
    _ inputCount: Int
  ) -> PrnsAppBytes {
    let central = "rs.reticulum.prns.smoke.bluetooth-auto.central.v1"
    let peripheral = "rs.reticulum.prns.smoke.bluetooth-auto.peripheral.v1"
    return withUtf8Bytes(central) { centralPointer, centralCount in
      withUtf8Bytes(peripheral) { peripheralPointer, peripheralCount in
        prns_app_start_with_apple_restoration(
          pathPointer,
          pathCount,
          inputPointer,
          inputCount,
          centralPointer,
          centralCount,
          peripheralPointer,
          peripheralCount
        )
      }
    }
  }

  private static func requireTag(
    _ json: String,
    operation: String,
    expected: String
  ) throws {
    try requireField(json, operation: operation, key: "type", expected: expected)
  }

  private static func requireField(
    _ json: String,
    operation: String,
    key: String,
    expected: String
  ) throws {
    guard
      let data = json.data(using: .utf8),
      let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
      object[key] as? String == expected
    else {
      throw PrnsAppException("\(operation) did not return \(key)=\(expected).")
    }
  }

  private static func writeSimulatorSmokeMarker(_ marker: String) {
    guard let data = "\(marker)\n".data(using: .utf8) else {
      return
    }
    FileHandle.standardError.write(data)
  }
  #endif

}
