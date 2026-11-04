import Foundation
import PrnsHostExpo

/// Product path selection only. Rust validates the installation and owns every
/// marker, controller key and domain file below this private platform root.
enum PrnsAppStorage {
  static func root(create: Bool) throws -> URL {
    let support = try FileManager.default.url(
      for: .applicationSupportDirectory, in: .userDomainMask,
      appropriateFor: nil, create: create
    )
    let root = support.appendingPathComponent("prns-app", isDirectory: true)
    if create {
      do {
        try FileManager.default.createDirectory(
          at: root, withIntermediateDirectories: false,
          attributes: [.posixPermissions: 0o700,
                       .protectionKey: FileProtectionType.completeUntilFirstUserAuthentication]
        )
      } catch let error as CocoaError where error.code == .fileWriteFileExists { /* Existing roots are validated by Rust. */ }
    }
    return root
  }

  static func installationPresent() throws -> Bool {
    do {
      let installation = try root(create: false).appendingPathComponent(appInstallationDirectory())
      _ = try FileManager.default.attributesOfItem(atPath: installation.path)
      return true
    } catch let error as CocoaError where error.code == .fileReadNoSuchFile { return false }
  }
}
