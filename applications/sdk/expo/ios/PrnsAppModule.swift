import ExpoModulesCore
import Foundation

private final class PrnsAppException: GenericException<String>, @unchecked Sendable {
  override var reason: String {
    param
  }
}

public final class PrnsAppModule: Module {
  private static let nativeQueue = DispatchQueue(
    label: "rs.reticulum.prns.app.native",
    attributes: .concurrent
  )

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

    AsyncFunction("start") { () throws -> String in
      let storageURL = try Self.developmentStorageURL(create: true)
      return try Self.withUtf8Bytes(storageURL.path) { pointer, count in
        try Self.consume(prns_app_start(pointer, count))
      }
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

    AsyncFunction("stop") { () throws -> String in
      try Self.consume(prns_app_stop())
    }.runOnQueue(Self.nativeQueue)

    AsyncFunction("reset") { () throws -> String in
      let storageURL = try Self.developmentStorageURL(create: false)
      return try Self.withUtf8Bytes(storageURL.path) { pointer, count in
        try Self.consume(prns_app_reset(pointer, count))
      }
    }.runOnQueue(Self.nativeQueue)

    OnDestroy {
      Self.stopForTeardown()
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

  private static func developmentStorageURL(create: Bool) throws -> URL {
    do {
      let applicationSupport = try FileManager.default.url(
        for: .applicationSupportDirectory,
        in: .userDomainMask,
        appropriateFor: nil,
        create: true
      )
      let storageURL = applicationSupport
        .appendingPathComponent("prns", isDirectory: true)
        .appendingPathComponent("development", isDirectory: true)
      if create {
        try FileManager.default.createDirectory(
          at: storageURL,
          withIntermediateDirectories: true
        )
      }
      return storageURL
    } catch {
      throw PrnsAppException("Unable to prepare the prns development storage directory.")
        .causedBy(error)
    }
  }

  private static func invokeJSON(
    _ inputJSON: String,
    operation: (UnsafePointer<UInt8>?, Int) -> PrnsAppBytes
  ) throws -> String {
    try withUtf8Bytes(inputJSON) { pointer, count in
      try consume(operation(pointer, count))
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

  private static func stopForTeardown() {
    nativeQueue.sync(flags: .barrier) {
      let bytes = prns_app_stop()
      prns_app_bytes_free(bytes)
    }
  }
}
