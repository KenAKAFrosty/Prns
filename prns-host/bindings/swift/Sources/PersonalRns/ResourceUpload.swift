import CPrnsHost
import Foundation

public final class ResourceUpload: @unchecked Sendable {
    private let lock = NSLock()
    private var pointer: OpaquePointer?
    private var finished = false

    init(pointer: OpaquePointer) {
        self.pointer = pointer
    }

    deinit {
        close()
    }

    public func write(_ chunk: [UInt8]) async throws {
        while true {
            try Task.checkCancellation()
            let status: Status = try lock.withLock {
                guard let pointer, !finished else {
                    throw StatusFailure(
                        operation: "resourceUpload",
                        status: .stopped
                    )
                }
                let arena = NativeArena()
                return Status(
                    rawValue: prns_resource_upload_write(
                        pointer,
                        try arena.bytes(chunk)
                    )
                ) ?? .backendFailed
            }
            if status == .ok {
                return
            }
            guard status == .wouldBlock else {
                throw StatusFailure(operation: "writeResourceUpload", status: status)
            }
            await Task.yield()
        }
    }

    public func finish() async throws -> CommandSettlement {
        let command = try lock.withLock {
            guard let pointer, !finished else {
                throw StatusFailure(operation: "resourceUpload", status: .stopped)
            }
            var output: OpaquePointer?
            try checkedStatus(
                prns_resource_upload_finish(pointer, &output),
                operation: "finishResourceUpload"
            )
            guard let output else {
                throw StatusFailure(
                    operation: "finishResourceUpload",
                    status: .backendFailed
                )
            }
            finished = true
            return try Command(pointer: output)
        }
        defer {
            command.close()
            close()
        }
        return try await command.value()
    }

    public func abort() {
        lock.withLock {
            guard let pointer, !finished else {
                return
            }
            prns_resource_upload_abort(pointer)
            finished = true
        }
    }

    public func close() {
        lock.withLock {
            guard let pointer else {
                return
            }
            if !finished {
                prns_resource_upload_abort(pointer)
            }
            prns_resource_upload_release(pointer)
            self.pointer = nil
        }
    }
}
