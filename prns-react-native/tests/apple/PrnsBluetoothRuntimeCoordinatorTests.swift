import Foundation

@main
struct PrnsBluetoothRuntimeCoordinatorTests {
  @MainActor static func main() throws {
    var authorization = PrnsBluetoothAuthorization.notDetermined
    var active = false
    var restored = 0
    var completions: [Result<Int, Error>] = []
    let runtime = PrnsBluetoothRuntimeCoordinator<Int>(
      authorization: { authorization }, applicationActive: { active },
      cacheOutcome: { $0 >= 0 ? $0 : nil }, maximumWaiters: 2, onStatus: { _ in }
    )
    runtime.activate(restorationAttemptRequested: true) { restored += 1 }
    precondition(restored == 0)
    do {
      _ = try runtime.requestNativeStart(restorationOnly: false) { completions.append($0) }
      preconditionFailure("an unresolved permission cannot begin in background")
    } catch PrnsNativeStartAdmissionError.bluetoothPermissionNeedsForeground {}
    active = true
    let generation = try runtime.requestNativeStart(restorationOnly: false) { completions.append($0) }!
    let coalesced = try runtime.requestNativeStart(restorationOnly: false) { completions.append($0) }
    precondition(coalesced == nil)
    do {
      _ = try runtime.requestNativeStart(restorationOnly: false) { _ in }
      preconditionFailure("coalesced admission must stay bounded")
    } catch PrnsNativeStartAdmissionError.tooManyWaiters {}
    runtime.nativeStopWillBegin()
    precondition(completions.count == 2)
    for completion in completions {
      guard case .failure(PrnsNativeStartAdmissionError.supersededByStop) = completion else {
        preconditionFailure("stop must settle every old-generation waiter")
      }
    }
    runtime.nativeStartDidFinish(outcome: 4, startGeneration: generation)
    precondition(runtime.status.nativeStart == .stopping)
    runtime.nativeStopDidFinish(joined: false)
    do {
      _ = try runtime.requestNativeStart(restorationOnly: false) { _ in }
      preconditionFailure("failed stop cannot admit another owner")
    } catch PrnsNativeStartAdmissionError.stopInProgress {}
    runtime.nativeStopDidFinish(joined: true)
    completions.removeAll()
    let next = try runtime.requestNativeStart(restorationOnly: false) { completions.append($0) }!
    precondition(next != generation)
    runtime.nativeStartDidFinish(outcome: 8, startGeneration: next)
    let completed = try completions.first!.get()
    precondition(completed == 8)
    var cached: Int?
    let existing = try runtime.requestNativeStart(restorationOnly: false) { cached = try? $0.get() }
    precondition(existing == nil)
    precondition(cached == 8)
    authorization = .allowedAlways
    runtime.applicationDidBecomeActive()
    precondition(restored == 0, "explicit stop cancelled automatic restoration")
    runtime.activate(restorationAttemptRequested: true) { restored += 1 }
    precondition(restored == 1)
    runtime.applicationDidBecomeActive()
    precondition(restored == 1)
    runtime.restorationStartAuthorizationDidClose()
    precondition(restored == 2)
  }
}
