import CPrnsHost
import Foundation

private enum NativeEventPoll<Element> {
    case value(Element)
    case waiting
    case stopped
}

final class NativeEventStream: @unchecked Sendable {
    private let stateLock = NSLock()
    private let waitLock = NSLock()
    private let readiness: NativeReadiness
    private var pointer: OpaquePointer?

    init(pointer: OpaquePointer) throws {
        do {
            readiness = try NativeReadiness.eventStream(pointer)
        } catch {
            prns_event_stream_release(pointer)
            throw error
        }
        self.pointer = pointer
    }

    deinit {
        close()
    }

    private func snapshot() throws -> OpaquePointer {
        stateLock.lock()
        defer { stateLock.unlock() }
        guard let pointer else {
            throw StatusFailure(operation: "eventStream", status: .stopped)
        }
        return pointer
    }

    private func interruptWait() {
        stateLock.lock()
        if let pointer {
            prns_event_stream_interrupt_wait(pointer)
        }
        stateLock.unlock()
    }

    func next<Element: Sendable>(
        decode: @escaping @Sendable (OpaquePointer) throws -> Element
    ) async throws -> Element? {
        return try await withTaskCancellationHandler {
            while true {
                switch try poll(decode: decode) {
                case .value(let value):
                    return value
                case .waiting:
                    await readiness.wait()
                case .stopped:
                    return nil
                }
            }
        } onCancel: {
            self.interruptWait()
        }
    }

    private func poll<Element: Sendable>(
        decode: @escaping @Sendable (OpaquePointer) throws -> Element
    ) throws -> NativeEventPoll<Element> {
        try waitLock.withLock {
            let pointer = try self.snapshot()
            var event: OpaquePointer?
            let status = Status(
                rawValue: prns_event_stream_next(
                    pointer,
                    0,
                    &event
                )
            )
            if status == .stopped {
                return .stopped
            }
            if status == .wouldBlock {
                return .waiting
            }
            if status == .interrupted {
                throw CancellationError()
            }
            guard status == .ok, let event else {
                throw StatusFailure(
                    operation: "nextEvent",
                    status: status ?? .backendFailed
                )
            }
            defer { prns_event_release(event) }
            return .value(try decode(event))
        }
    }

    func close() {
        stateLock.lock()
        let pointer = pointer
        self.pointer = nil
        if let pointer {
            prns_event_stream_interrupt_wait(pointer)
        }
        stateLock.unlock()
        guard let pointer else {
            return
        }
        waitLock.withLock {
            readiness.close()
            prns_event_stream_release(pointer)
        }
    }
}

public final class EventSequence<Element: Sendable>: AsyncSequence, @unchecked Sendable {
    public struct AsyncIterator: AsyncIteratorProtocol {
        private let source: NativeEventStream?
        private let decode: (@Sendable (OpaquePointer) throws -> Element)?
        private var initialFailure: StatusFailure?

        init(
            source: NativeEventStream?,
            decode: (@Sendable (OpaquePointer) throws -> Element)?,
            initialFailure: StatusFailure?
        ) {
            self.source = source
            self.decode = decode
            self.initialFailure = initialFailure
        }

        public mutating func next() async throws -> Element? {
            if let failure = initialFailure {
                initialFailure = nil
                throw failure
            }
            guard let source, let decode else {
                return nil
            }
            return try await source.next(decode: decode)
        }
    }

    private let claimLock = NSLock()
    private let native: NativeEventStream
    private let decode: @Sendable (OpaquePointer) throws -> Element
    private var claimed = false

    init(
        native: NativeEventStream,
        decode: @escaping @Sendable (OpaquePointer) throws -> Element
    ) {
        self.native = native
        self.decode = decode
    }

    public func makeAsyncIterator() -> AsyncIterator {
        claimLock.lock()
        defer { claimLock.unlock() }
        guard !claimed else {
            return AsyncIterator(
                source: nil,
                decode: nil,
                initialFailure: StatusFailure(
                    operation: "makeAsyncIterator",
                    status: .alreadyClaimed
                )
            )
        }
        claimed = true
        return AsyncIterator(
            source: native,
            decode: decode,
            initialFailure: nil
        )
    }

    public func close() {
        native.close()
    }
}

struct EventReader {
    let pointer: OpaquePointer

    var kind: UInt32 {
        prns_event_kind(pointer)
    }

    func bytes(_ field: EventField) throws -> [UInt8] {
        var view = PrnsByteView(data: nil, length: 0)
        try checkedStatus(
            prns_event_bytes(pointer, field.rawValue, &view),
            operation: "eventBytes"
        )
        return copyBytes(view)
    }

    func optionalBytes(_ field: EventField) throws -> [UInt8]? {
        var view = PrnsByteView(data: nil, length: 0)
        let status = Status(
            rawValue: prns_event_bytes(pointer, field.rawValue, &view)
        )
        if status == .invalidArgument {
            return nil
        }
        guard status == .ok else {
            throw StatusFailure(
                operation: "eventBytes",
                status: status ?? .backendFailed
            )
        }
        return copyBytes(view)
    }

    func string(_ field: EventField) throws -> String {
        var view = PrnsStringView(data: nil, length: 0)
        try checkedStatus(
            prns_event_string(pointer, field.rawValue, &view),
            operation: "eventString"
        )
        return copyString(view)
    }

    func u64(_ field: EventField) throws -> UInt64 {
        var value: UInt64 = 0
        try checkedStatus(
            prns_event_u64(pointer, field.rawValue, &value),
            operation: "eventInteger"
        )
        return value
    }

    func u16(_ field: EventField) throws -> UInt16 {
        guard let value = UInt16(exactly: try u64(field)) else {
            throw StatusFailure(
                operation: "eventInteger",
                status: .backendFailed
            )
        }
        return value
    }

    func u128(_ field: EventField) throws -> UInt128 {
        var low: UInt64 = 0
        var high: UInt64 = 0
        try checkedStatus(
            prns_event_u128(
                pointer,
                field.rawValue,
                &low,
                &high
            ),
            operation: "eventInteger"
        )
        return UInt128(low) | UInt128(high) << 64
    }

    func resourceStream() throws -> any ResourceStream {
        var stream: OpaquePointer?
        try checkedStatus(
            prns_event_resource_stream(pointer, &stream),
            operation: "resourceStream"
        )
        guard let stream else {
            throw StatusFailure(
                operation: "resourceStream",
                status: .backendFailed
            )
        }
        return NativeResourceBody(
            pointer: stream,
            totalBytes: try u64(.totalBytes)
        )
    }
}

final class NativeResourceBody: ResourceStream, @unchecked Sendable {
    struct AsyncIterator: AsyncIteratorProtocol {
        private let source: NativeResourceBody?
        private var initialFailure: StatusFailure?

        init(source: NativeResourceBody?, initialFailure: StatusFailure?) {
            self.source = source
            self.initialFailure = initialFailure
        }

        mutating func next() async throws -> [UInt8]? {
            if let failure = initialFailure {
                initialFailure = nil
                throw failure
            }
            return try source?.nextChunk()
        }
    }

    private let lock = NSLock()
    private var pointer: OpaquePointer?
    private var iteratorClaimed = false
    let totalBytes: UInt64

    init(pointer: OpaquePointer, totalBytes: UInt64) {
        self.pointer = pointer
        self.totalBytes = totalBytes
    }

    deinit {
        close()
    }

    func makeAsyncIterator() -> AsyncIterator {
        lock.lock()
        defer { lock.unlock() }
        guard !iteratorClaimed else {
            return AsyncIterator(
                source: nil,
                initialFailure: StatusFailure(
                    operation: "makeResourceIterator",
                    status: .alreadyClaimed
                )
            )
        }
        iteratorClaimed = true
        return AsyncIterator(source: self, initialFailure: nil)
    }

    private func nextChunk() throws -> [UInt8]? {
        lock.lock()
        defer { lock.unlock() }
        guard let pointer else {
            return nil
        }
        var view = PrnsByteView(data: nil, length: 0)
        var finished: UInt8 = 0
        try checkedStatus(
            prns_resource_stream_next(
                pointer,
                64 * 1024,
                &view,
                &finished
            ),
            operation: "resourceNext"
        )
        if finished != 0 {
            return nil
        }
        return copyBytes(view)
    }

    func close() {
        lock.lock()
        let pointer = pointer
        self.pointer = nil
        lock.unlock()
        if let pointer {
            prns_resource_stream_release(pointer)
        }
    }
}

func decodeApplicationEvent(_ pointer: OpaquePointer) throws -> ApplicationEvent {
    let event = EventReader(pointer: pointer)
    guard let kind = ApplicationEventKind(rawValue: event.kind) else {
        throw StatusFailure(
            operation: "decodeApplicationEvent",
            status: .backendFailed
        )
    }
    switch kind {
    case .singleDelivery:
        return .singleDelivery(
            destination: try DestinationHash(event.bytes(.destination)),
            sourceInterface: try InterfaceId(event.bytes(.sourceInterface)),
            plaintext: try event.bytes(.plaintext)
        )
    case .linkDelivery:
        return .linkDelivery(
            linkId: try LinkId(event.bytes(.linkId)),
            sourceInterface: try InterfaceId(event.bytes(.sourceInterface)),
            plaintext: try event.bytes(.plaintext)
        )
    case .request:
        let requester = try event.optionalBytes(.requester).map {
            try IdentityHash($0)
        }
        return .request(
            destination: try DestinationHash(event.bytes(.destination)),
            linkId: try LinkId(event.bytes(.linkId)),
            requestId: try RequestId(event.bytes(.requestId)),
            requester: requester,
            pathHash: try RequestPathHash(event.bytes(.pathHash)),
            rttMillis: try event.u64(.rttMillis),
            data: try event.bytes(.data)
        )
    case .response:
        return .response(
            linkId: try LinkId(event.bytes(.linkId)),
            requestId: try RequestId(event.bytes(.requestId)),
            data: try event.bytes(.data)
        )
    case .responseSegment:
        return .responseSegment(
            linkId: try LinkId(event.bytes(.linkId)),
            requestId: try RequestId(event.bytes(.requestId)),
            segmentIndex: try event.u64(.segmentIndex),
            totalSegments: try event.u64(.totalSegments),
            data: try event.bytes(.data)
        )
    case .resourceAvailable:
        return .resourceAvailable(
            linkId: try LinkId(event.bytes(.linkId)),
            hash: try ResourceHash(event.bytes(.hash)),
            metadata: try event.optionalBytes(.metadata),
            resource: try event.resourceStream()
        )
    case .resourceSegment:
        return .resourceSegment(
            linkId: try LinkId(event.bytes(.linkId)),
            originalHash: try ResourceHash(event.bytes(.originalHash)),
            segmentIndex: try event.u64(.segmentIndex),
            totalSegments: try event.u64(.totalSegments),
            metadata: try event.optionalBytes(.metadata),
            data: try event.bytes(.data)
        )
    case .resourceNeedsDecompression:
        return .resourceNeedsDecompression(
            linkId: try LinkId(event.bytes(.linkId)),
            hash: try ResourceHash(event.bytes(.hash)),
            stream: try event.bytes(.stream),
            uncompressedDataBytes: try event.u64(.uncompressedDataBytes)
        )
    case .channelMessage:
        return .channelMessage(
            linkId: try LinkId(event.bytes(.linkId)),
            messageType: try event.u16(.messageType),
            data: try event.bytes(.data)
        )
    }
}

func decodeDiagnosticEvent(_ pointer: OpaquePointer) throws -> DiagnosticEvent {
    let event = EventReader(pointer: pointer)
    guard let kind = DiagnosticEventKind(rawValue: event.kind) else {
        throw StatusFailure(
            operation: "decodeDiagnosticEvent",
            status: .backendFailed
        )
    }
    switch kind {
    case .announceHeard:
        guard let hops = UInt8(exactly: try event.u64(.hops)) else {
            throw StatusFailure(
                operation: "decodeAnnounceHops",
                status: .backendFailed
            )
        }
        return .announceHeard(
            destination: try DestinationHash(event.bytes(.destination)),
            hops: hops,
            sourceInterface: try InterfaceId(event.bytes(.sourceInterface)),
            appData: try event.bytes(.appData)
        )
    case .linkEstablished:
        return .linkEstablished(
            linkId: try LinkId(event.bytes(.linkId)),
            rttMillis: try event.u64(.rttMillis)
        )
    case .peerIdentified:
        return .peerIdentified(
            linkId: try LinkId(event.bytes(.linkId)),
            identity: try IdentityHash(event.bytes(.identity))
        )
    case .linkClosed:
        let rawReason = try event.u64(.reason)
        guard let narrowedReason = UInt32(exactly: rawReason),
              let reason = LinkClosedReason(rawValue: narrowedReason)
        else {
            throw StatusFailure(
                operation: "decodeDiagnosticEvent",
                status: .backendFailed
            )
        }
        return .linkClosed(
            linkId: try LinkId(event.bytes(.linkId)),
            reason: reason
        )
    case .linkInterfaceMismatch:
        return .linkInterfaceMismatch(
            linkId: try LinkId(event.bytes(.linkId)),
            attachedInterface: try InterfaceId(
                event.bytes(.attachedInterface)
            ),
            arrivedOn: try InterfaceId(event.bytes(.arrivedOn))
        )
    case .resourceAssembled:
        return .resourceAssembled(
            linkId: try LinkId(event.bytes(.linkId)),
            originalHash: try ResourceHash(event.bytes(.originalHash)),
            totalSizeBytes: try event.u64(.totalSizeBytes)
        )
    case .resourceFailed:
        return .resourceFailed(
            linkId: try LinkId(event.bytes(.linkId)),
            hash: try ResourceHash(event.bytes(.hash)),
            cause: try event.string(.cause)
        )
    case .resourceSendProgress:
        return .resourceSendProgress(
            linkId: try LinkId(event.bytes(.linkId)),
            transferredBytes: try event.u64(.transferredBytes),
            totalBytes: try event.u64(.totalBytes),
            physicalTransferredBytes: try event.u64(
                .physicalTransferredBytes
            ),
            segmentIndex: try event.u64(.segmentIndex),
            totalSegments: try event.u64(.totalSegments)
        )
    case .selfRatchetRotated:
        return .selfRatchetRotated(
            destination: try DestinationHash(event.bytes(.destination))
        )
    case .announceHeldDropped:
        return .announceHeldDropped(
            destination: try DestinationHash(event.bytes(.destination)),
            sourceInterface: try InterfaceId(event.bytes(.sourceInterface)),
            cause: try event.string(.cause)
        )
    case .delivered:
        return .delivered(detail: try event.string(.detail))
    case .routeExpired:
        return .routeExpired(
            destination: try DestinationHash(event.bytes(.destination))
        )
    case .routeEvicted:
        return .routeEvicted(
            destination: try DestinationHash(event.bytes(.destination))
        )
    case .routeInterfaceGone:
        return .routeInterfaceGone(
            destination: try DestinationHash(event.bytes(.destination))
        )
    case .routeDropped:
        return .routeDropped(
            destination: try DestinationHash(event.bytes(.destination))
        )
    case .backendDiagnostic:
        return .backendDiagnostic(
            kind: try event.string(.kind),
            detail: try event.string(.detail)
        )
    case .diagnosticsDropped:
        return .diagnosticsDropped(count: try event.u128(.droppedCount))
    case .persistenceRestored:
        return .persistenceRestored(
            routes: try event.u64(.routes),
            destinationIdentities: try event.u64(.destinationIdentities),
            tunnels: try event.u64(.tunnels),
            ratchets: try event.u64(.ratchets),
            refused: try event.u64(.refused),
            dropped: try event.u64(.dropped)
        )
    case .persistenceFlushed:
        return .persistenceFlushed(
            cause: try persistenceCause(event),
            target: try persistenceTarget(event)
        )
    case .persistenceFlushFailed:
        return .persistenceFlushFailed(
            cause: try persistenceCause(event),
            target: try persistenceTarget(event)
        )
    }
}

private func persistenceCause(_ event: EventReader) throws -> PersistenceFlushCause {
    let raw = try event.u64(.persistenceCause)
    guard let narrowed = UInt32(exactly: raw),
          let value = PersistenceFlushCause(rawValue: narrowed)
    else {
        throw StatusFailure(
            operation: "decodePersistenceCause",
            status: .backendFailed
        )
    }
    return value
}

private func persistenceTarget(_ event: EventReader) throws -> PersistenceFlushTarget {
    let raw = try event.u64(.persistenceTarget)
    guard let narrowed = UInt32(exactly: raw),
          let value = PersistenceFlushTarget(rawValue: narrowed)
    else {
        throw StatusFailure(
            operation: "decodePersistenceTarget",
            status: .backendFailed
        )
    }
    return value
}
