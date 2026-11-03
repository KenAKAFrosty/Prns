import CoreBluetooth
import Foundation

/// Explicit OS permission request; ordinary status reads never construct a manager.
@MainActor
public final class PrnsBluetoothPermission: NSObject, @preconcurrency CBCentralManagerDelegate {
  private var manager: CBCentralManager?
  private var waiter: CheckedContinuation<Void, Never>?
  public enum Failure: Error { case requestInProgress }

  public func request() async throws {
    guard CBManager.authorization == .notDetermined else { return }
    guard waiter == nil else { throw Failure.requestInProgress }
    await withCheckedContinuation { continuation in
      waiter = continuation
      if manager == nil {
        manager = CBCentralManager(delegate: self, queue: .main,
          options: [CBCentralManagerOptionShowPowerAlertKey: false])
      }
    }
  }

  public func centralManagerDidUpdateState(_ central: CBCentralManager) {
    guard CBManager.authorization != .notDetermined else { return }
    manager = nil
    let completed = waiter
    waiter = nil
    completed?.resume()
  }
}
