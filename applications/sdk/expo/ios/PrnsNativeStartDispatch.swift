import Foundation

/// Queue work only: startup authority and generation remain with the accessory
/// coordinator. Restoration is scheduled directly by native launch, before JS.
enum PrnsNativeStartDispatch {
  static func enqueue<Output>(
    on queue: DispatchQueue,
    validate: @escaping @MainActor () throws -> Void,
    prepare: (() throws -> Void)?,
    start: @escaping () throws -> Output,
    completion: @escaping @MainActor (Result<Output, Error>) -> Void
  ) {
    queue.async(flags: .barrier) {
      let result = Result {
        try DispatchQueue.main.sync { try validate() }
        // Filesystem preparation, supervisor locking and manager creation may
        // block. None of those operations may run on the main actor.
        if let prepare {
          try prepare()
          try DispatchQueue.main.sync { try validate() }
        }
        return try start()
      }
      DispatchQueue.main.async { completion(result) }
    }
  }
}
