import Foundation

public struct Limits: Hashable, Sendable {
    public let pendingCommands: Int
    public let applicationEvents: Int
    public let retainedEventBytes: Int
    public let diagnostics: Int

    public init(
        pendingCommands: Int,
        applicationEvents: Int,
        retainedEventBytes: Int,
        diagnostics: Int
    ) throws {
        guard pendingCommands > 0,
              applicationEvents > 0,
              retainedEventBytes > 0,
              diagnostics > 0
        else {
            throw ConfigurationError.invalidLimits
        }
        self.pendingCommands = pendingCommands
        self.applicationEvents = applicationEvents
        self.retainedEventBytes = retainedEventBytes
        self.diagnostics = diagnostics
    }

    public static var balanced: Self {
        get throws {
            try Self(
                pendingCommands: HostContract.balancedPendingCommands,
                applicationEvents: HostContract.balancedApplicationEvents,
                retainedEventBytes: HostContract.balancedRetainedEventBytes,
                diagnostics: HostContract.balancedDiagnostics
            )
        }
    }
}

public struct HostOptions: Sendable {
    public let role: HostRole
    public let identity: IdentityConfig
    public let destinations: [DestinationConfig]
    public let requiredCapabilities: [Capability]
    public let limits: Limits
    public let persistence: PersistenceConfig

    public init(
        role: HostRole,
        identity: IdentityConfig,
        destinations: [DestinationConfig],
        requiredCapabilities: [Capability],
        limits: Limits,
        persistence: PersistenceConfig = .ephemeral
    ) {
        self.role = role
        self.identity = identity
        self.destinations = destinations
        self.requiredCapabilities = requiredCapabilities
        self.limits = limits
        self.persistence = persistence
    }

    public static func ephemeralEndpoint(
        destinations: [DestinationConfig] = [],
        requiredCapabilities: [Capability] = []
    ) throws -> Self {
        Self(
            role: .endpoint,
            identity: .generateEphemeral,
            destinations: destinations,
            requiredCapabilities: requiredCapabilities,
            limits: try .balanced
        )
    }

    public static func persistentEndpoint(
        root: String,
        destinations: [DestinationConfig] = [],
        requiredCapabilities: [Capability] = []
    ) throws -> Self {
        let rootURL = URL(fileURLWithPath: root, isDirectory: true)
        return Self(
            role: .endpoint,
            identity: .loadOrCreate(
                path: rootURL.appendingPathComponent("identity").path
            ),
            destinations: destinations,
            requiredCapabilities: requiredCapabilities,
            limits: try .balanced,
            persistence: .directory(
                path: rootURL.appendingPathComponent("state").path
            )
        )
    }
}

public enum ConfigurationError: Error, Equatable {
    case invalidLimits
    case unknownIdentity
    case unknownDestination
    case unknownDestinationIdentity
    case unknownBitrate
    case unknownCommand
    case allocationFailed
}
