import Foundation

@main
struct PrnsNativeStartDispatchTests {
  enum Failure: Error { case superseded, unauthorized, preparation }

  final class Events: @unchecked Sendable {
    private let lock = NSLock()
    private var values: [String] = []
    func append(_ value: String) {
      lock.lock()
      defer { lock.unlock() }
      values.append(value)
    }
    func read() -> [String] {
      lock.lock()
      defer { lock.unlock() }
      return values
    }
  }

  @MainActor
  static func main() async {
    await check()
    await check(restoration: false)
    await check(stopBeforePreparation: true)
    await check(stopDuringPreparation: true)
    await check(authorizationLostDuringPreparation: true)
    await check(preparationFails: true)
    print("native startup dispatch: 6 queue, restoration and admission checks passed")
  }

  @MainActor
  private static func check(
    restoration: Bool = true,
    stopBeforePreparation: Bool = false,
    stopDuringPreparation: Bool = false,
    authorizationLostDuringPreparation: Bool = false,
    preparationFails: Bool = false
  ) async {
    let queue = DispatchQueue(label: "prns.test.native-start", attributes: .concurrent)
    let events = Events()
    var generation = 1
    var authorized = true
    queue.suspend()
    let result: Result<Int, Error> = await withCheckedContinuation { continuation in
      PrnsNativeStartDispatch.enqueue(
        on: queue,
        validate: {
          precondition(Thread.isMainThread)
          events.append("validate")
          guard generation == 1 else { throw Failure.superseded }
          guard authorized else { throw Failure.unauthorized }
        },
        prepare: restoration ? {
          precondition(!Thread.isMainThread)
          events.append("prepare")
          // Hold real background preparation until the main queue progresses.
          // MainActor preparation would deadlock this bounded check.
          let resumed = DispatchSemaphore(value: 0)
          DispatchQueue.main.async {
            events.append("mainResponsive")
            if stopDuringPreparation { generation += 1 }
            if authorizationLostDuringPreparation { authorized = false }
            resumed.signal()
          }
          precondition(resumed.wait(timeout: .now() + 5) == .success)
          if preparationFails { throw Failure.preparation }
        } : nil,
        start: {
          precondition(!Thread.isMainThread)
          events.append("start")
          return 42
        },
        completion: {
          precondition(Thread.isMainThread)
          events.append("completion")
          continuation.resume(returning: $0)
        }
      )
      if stopBeforePreparation { generation += 1 }
      queue.resume()
    }

    let expected: [String]
    if stopBeforePreparation {
      guard case .failure(Failure.superseded) = result else { fatalError("queued Stop was ignored") }
      expected = ["validate", "completion"]
    } else if preparationFails {
      guard case .failure(Failure.preparation) = result else { fatalError("preparation failure was lost") }
      expected = ["validate", "prepare", "mainResponsive", "completion"]
    } else if stopDuringPreparation || authorizationLostDuringPreparation {
      if stopDuringPreparation {
        guard case .failure(Failure.superseded) = result else { fatalError("Stop was ignored") }
      } else {
        guard case .failure(Failure.unauthorized) = result else { fatalError("authorization loss was ignored") }
      }
      expected = ["validate", "prepare", "mainResponsive", "validate", "completion"]
    } else {
      guard case .success(42) = result else { fatalError("startup failed") }
      expected = restoration
        ? ["validate", "prepare", "mainResponsive", "validate", "start", "completion"]
        : ["validate", "start", "completion"]
    }
    precondition(events.read() == expected, "unexpected startup ordering: \(events.read())")
  }
}
