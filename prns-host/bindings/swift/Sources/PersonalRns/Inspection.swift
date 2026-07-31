import CPrnsHost
import Foundation

func decodeBackendInfo(_ value: PrnsBackendInfo) throws -> BackendInfo {
    guard let backend = BackendKind(rawValue: value.backend) else {
        throw StatusFailure(operation: "decodeBackendInfo", status: .backendFailed)
    }
    let nativeCapabilities = UnsafeBufferPointer(
        start: value.capabilities,
        count: value.capability_count
    )
    let capabilities = nativeCapabilities.compactMap(Capability.init(rawValue:))
    let nativeKinds = UnsafeBufferPointer(
        start: value.interface_kinds,
        count: value.interface_kind_count
    )
    let kinds = nativeKinds.compactMap(InterfaceKind.init(rawValue:))
    guard capabilities.count == value.capability_count,
          kinds.count == value.interface_kind_count
    else {
        throw StatusFailure(operation: "decodeBackendInfo", status: .backendFailed)
    }
    return BackendInfo(
        backend: backend,
        capabilities: capabilities,
        interfaceKinds: kinds
    )
}

func decodeHostSnapshot(_ value: PrnsHostSnapshot) throws -> HostSnapshot {
    let nativeInterfaces = UnsafeBufferPointer(
        start: value.interfaces,
        count: value.interface_count
    )
    let interfaces = try nativeInterfaces.map { item in
        guard let health = InterfaceHealth(rawValue: item.health) else {
            throw StatusFailure(
                operation: "decodeInterfaceSnapshot",
                status: .backendFailed
            )
        }
        return InterfaceSnapshot(
            interfaceId: try InterfaceId(copyBytes(item.interface_id)),
            name: item.has_name == 0 ? nil : copyString(item.name),
            kind: item.has_kind == 0 ? nil : InterfaceKind(rawValue: item.kind),
            health: health,
            failureDetail: item.has_failure_detail == 0 ? nil :
                copyString(item.failure_detail),
            rxBytes: item.rx_bytes,
            txBytes: item.tx_bytes,
            rxBps: item.has_rx_bps == 0 ? nil : item.rx_bps,
            txBps: item.has_tx_bps == 0 ? nil : item.tx_bps,
            routeCount: item.route_count,
            linkCount: item.link_count,
            transportedLinkCount: item.transported_link_count
        )
    }
    let nativeRoutes = UnsafeBufferPointer(
        start: value.routes,
        count: value.route_count
    )
    let routes = try nativeRoutes.map { item in
        RouteSnapshot(
            destination: try DestinationHash(copyBytes(item.destination)),
            hops: item.hops,
            viaIdentity: item.has_via_identity == 0 ? nil :
                try IdentityHash(copyBytes(item.via_identity)),
            interfaceId: try InterfaceId(copyBytes(item.interface_id)),
            learnedAtMillis: item.learned_at_millis,
            lastRelayedAtMillis: item.last_relayed_at_millis,
            expiresAtMillis: item.expires_at_millis
        )
    }
    let nativeIdentities = UnsafeBufferPointer(
        start: value.destination_identities,
        count: value.destination_identity_count
    )
    let identities = try nativeIdentities.map { item in
        DestinationIdentitySnapshot(
            destination: try DestinationHash(copyBytes(item.destination)),
            identity: try IdentityHash(copyBytes(item.identity))
        )
    }
    let runtime = value.runtime
    let persistence = value.persistence
    return HostSnapshot(
        revision: value.revision,
        backend: try decodeBackendInfo(value.backend),
        interfaces: interfaces,
        routes: routes,
        activeLinkCount: value.active_link_count,
        destinationIdentities: identities,
        runtime: RuntimeHealthSnapshot(
            running: runtime.running != 0,
            uptimeMillis: runtime.uptime_millis,
            interfaceCount: runtime.interface_count,
            onlineInterfaceCount: runtime.online_interface_count,
            routeCount: runtime.route_count,
            linkCount: runtime.link_count,
            transportedLinkCount: runtime.transported_link_count,
            rxBytes: runtime.rx_bytes,
            txBytes: runtime.tx_bytes,
            rxBps: runtime.rx_bps,
            txBps: runtime.tx_bps
        ),
        persistence: PersistenceSnapshot(
            persistent: persistence.persistent != 0,
            restored: persistence.restored != 0,
            lastFlushCause: persistence.has_last_flush_cause == 0 ? nil :
                PersistenceFlushCause(rawValue: persistence.last_flush_cause),
            lastFailureDetail: persistence.has_last_failure_detail == 0 ? nil :
                copyString(persistence.last_failure_detail)
        )
    )
}
