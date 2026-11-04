import Foundation
import Network

/// A process-owned path observer supplies invalidation, never another socket owner.
@MainActor
public final class PrnsLanNetworkMonitor {
  private let changed: @Sendable () -> Void
  private let queue = DispatchQueue(label: "rs.reticulum.prns.lan-network")
  private var monitor: NWPathMonitor?
  private var generation: UInt64 = 0

  public init(changed: @escaping @Sendable () -> Void) { self.changed = changed }

  public func start() {
    guard monitor == nil else { return }
    generation &+= 1
    let current = generation
    let next = NWPathMonitor()
    next.pathUpdateHandler = { [weak self] _ in
      Task { @MainActor [weak self] in
        guard let self, self.monitor != nil, self.generation == current else { return }
        self.changed()
      }
    }
    monitor = next
    next.start(queue: queue)
  }

  public func stop() {
    generation &+= 1
    let retired = monitor
    monitor = nil
    retired?.cancel()
  }
}
