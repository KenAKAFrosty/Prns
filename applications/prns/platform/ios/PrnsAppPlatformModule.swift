import ExpoModulesCore

public final class PrnsAppPlatformModule: Module {
  public func definition() -> ModuleDefinition {
    Name("PrnsAppPlatform")
    AsyncFunction("status") { (promise: Promise) in
      Task { @MainActor in promise.resolve(await PrnsAppRuntime.shared.platformStatus()) }
    }
    AsyncFunction("requestBluetoothPermission") { (promise: Promise) in
      Task { @MainActor in
        do {
          try await PrnsAppRuntime.shared.requestBluetoothPermission()
          promise.resolve(await PrnsAppRuntime.shared.platformStatus())
        } catch { promise.reject("ERR_PRNS_BLUETOOTH_PERMISSION", error.localizedDescription) }
      }
    }
    AsyncFunction("requestNotificationPermission") { (promise: Promise) in
      Task { @MainActor in promise.resolve(await PrnsAppRuntime.shared.platformStatus()) }
    }
    AsyncFunction("openSettings") { (destination: String, promise: Promise) in
      Task { @MainActor in
        do { try await PrnsAppRuntime.shared.openSettings(destination); promise.resolve(nil) }
        catch { promise.reject("ERR_PRNS_SETTINGS", error.localizedDescription) }
      }
    }
    AsyncFunction("setBluetoothEnabled") { (enabled: Bool, promise: Promise) in
      Task { @MainActor in
        do { try await PrnsAppRuntime.shared.setBluetoothEnabled(enabled); promise.resolve(nil) }
        catch { promise.reject("ERR_PRNS_BLUETOOTH", error.localizedDescription) }
      }
    }
    AsyncFunction("bluetoothDiscoveryGroups") { (promise: Promise) in
      Task { @MainActor in
        do { promise.resolve(try await PrnsAppRuntime.shared.discoveryGroups()) }
        catch { promise.reject("ERR_PRNS_BLUETOOTH", error.localizedDescription) }
      }
    }
    AsyncFunction("setBluetoothDiscoveryGroups") { (groups: [String], promise: Promise) in
      Task { @MainActor in
        do { promise.resolve(try await PrnsAppRuntime.shared.setDiscoveryGroups(groups)) }
        catch { promise.reject("ERR_PRNS_BLUETOOTH", error.localizedDescription) }
      }
    }
    AsyncFunction("installationPresent") { () throws -> Bool in
      try PrnsAppStorage.installationPresent()
    }
    AsyncFunction("bluetoothSelected") { () async -> Bool in
      await PrnsAppRuntime.shared.bluetoothSelected()
    }
    AsyncFunction("bluetoothAuthorization") { () async -> String in
      await PrnsAppRuntime.shared.bluetoothAuthorization()
    }
    AsyncFunction("start") { (mode: String, bluetooth: Bool, promise: Promise) in
      Task { @MainActor in
        PrnsAppRuntime.shared.start(mode: mode, bluetooth: bluetooth) { result in
          switch result {
          case .success: promise.resolve(nil)
          case .failure(let error): promise.reject("ERR_PRNS_START", error.localizedDescription)
          }
        }
      }
    }
    AsyncFunction("startImported") { (reviewId: String, bluetooth: Bool, promise: Promise) in
      Task { @MainActor in
        PrnsAppRuntime.shared.startImported(reviewId: reviewId, bluetooth: bluetooth) { result in
          switch result {
          case .success: promise.resolve(nil)
          case .failure(let error): promise.reject("ERR_PRNS_START", error.localizedDescription)
          }
        }
      }
    }
    AsyncFunction("prepareReset") { (promise: Promise) in
      Task { @MainActor in
        do { try await PrnsAppRuntime.shared.prepareReset(); promise.resolve(nil) }
        catch { promise.reject("ERR_PRNS_RESET_REVIEW", error.localizedDescription) }
      }
    }
    AsyncFunction("resetPreferences") { (promise: Promise) in
      Task { @MainActor in
        do { try await PrnsAppRuntime.shared.resetPreferences(); promise.resolve(nil) }
        catch { promise.reject("ERR_PRNS_RESET_PREFERENCES", error.localizedDescription) }
      }
    }
    AsyncFunction("stop") { (promise: Promise) in
      Task { @MainActor in
        PrnsAppRuntime.shared.stop { result in
          switch result {
          case .success: promise.resolve(nil)
          case .failure(let error): promise.reject("ERR_PRNS_STOP", error.localizedDescription)
          }
        }
      }
    }
  }
}
