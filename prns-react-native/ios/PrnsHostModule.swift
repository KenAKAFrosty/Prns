import ExpoModulesCore
import Foundation

public final class PrnsHostModule: Module {
  public func definition() -> ModuleDefinition {
    Name("PrnsHostPlatform")
    Constants(["nativeImage": PrnsNativeImage.name])
    AsyncFunction("defaultStoragePath") { () throws -> String in
      let support = try FileManager.default.url(
        for: .applicationSupportDirectory, in: .userDomainMask,
        appropriateFor: nil, create: true
      )
      let root = support.appendingPathComponent("prns", isDirectory: true)
      try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
      return root.path
    }
  }
}
