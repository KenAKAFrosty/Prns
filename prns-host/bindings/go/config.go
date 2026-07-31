package prns

import "path/filepath"

type Limits struct {
	PendingCommands    int
	ApplicationEvents  int
	RetainedEventBytes int
	Diagnostics        int
}

func BalancedLimits() Limits {
	return Limits{
		PendingCommands:    BalancedPendingCommands,
		ApplicationEvents:  BalancedApplicationEvents,
		RetainedEventBytes: BalancedRetainedEventBytes,
		Diagnostics:        BalancedDiagnostics,
	}
}

type HostOptions struct {
	Role                 HostRole
	Identity             IdentityConfig
	Persistence          PersistenceConfig
	Destinations         []DestinationConfig
	RequiredCapabilities []Capability
	Limits               Limits
}

func EphemeralEndpoint(
	destinations []DestinationConfig,
	requiredCapabilities []Capability,
) HostOptions {
	return HostOptions{
		Role:                 HostRoleEndpoint,
		Identity:             IdentityConfigGenerateEphemeral{},
		Persistence:          PersistenceConfigEphemeral{},
		Destinations:         destinations,
		RequiredCapabilities: requiredCapabilities,
		Limits:               BalancedLimits(),
	}
}

func PersistentEndpoint(
	root string,
	destinations []DestinationConfig,
	requiredCapabilities []Capability,
) HostOptions {
	return HostOptions{
		Role:                 HostRoleEndpoint,
		Identity:             IdentityConfigLoadOrCreate{Path: filepath.Join(root, "identity")},
		Persistence:          PersistenceConfigDirectory{Path: filepath.Join(root, "state")},
		Destinations:         destinations,
		RequiredCapabilities: requiredCapabilities,
		Limits:               BalancedLimits(),
	}
}

type ConfigErrorKind uint8

const (
	ConfigMissingIdentity ConfigErrorKind = iota + 1
	ConfigUnknownIdentity
	ConfigUnknownDestination
	ConfigUnknownDestinationIdentity
	ConfigInvalidLimits
	ConfigAllocationFailed
	ConfigInvalidRequestPolicy
	ConfigUnknownPersistence
)

type ConfigError struct {
	Kind  ConfigErrorKind
	Field string
}

func (failure ConfigError) Error() string {
	return "personal-rns: invalid host configuration: " + failure.Field
}
