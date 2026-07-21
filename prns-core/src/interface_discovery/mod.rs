mod advertisement;
#[cfg(feature = "interface-discovery-archive")]
mod archive;
mod autoconnect;
mod catalog;
mod codec;
mod coordinator;
mod intake;
mod policy;
mod protocol;
mod publication;
mod stamp;
mod storage;

pub use crate::interfaces::InterfaceOriginKind;
pub use advertisement::{
    AdvertisedInterfaceType, AdvertisedTransport, AdvertisementDetails, DiscoveryAdvertisement,
    GeographicLocation, PublishedIfac,
};
#[cfg(feature = "interface-discovery-archive")]
pub use archive::{
    discovered_interface_configuration, ArchiveFileOperation, ArchiveRecordError, DiscoveryArchive,
    DiscoveryArchiveError, DiscoveryArchiveFileState, DiscoveryArchiveRecord, HexDecodeError,
    LoadedDiscoveryArchive, DISCOVERED_INTERFACES_FILE,
};
pub use autoconnect::{
    ActiveDiscoveredInterface, DiscoveredConnectionAccess, DiscoveredConnectionEndpoint,
    DiscoveredConnectionEndpointId, DiscoveredConnectionHealth, DiscoveredConnectionKind,
    DiscoveredConnectionPlan, DiscoveredConnectionRegistrationError,
    DISCOVERED_INTERFACE_DETACH_AFTER,
};
pub use catalog::{
    DiscoveryCatalog, DiscoveryCatalogRefresh, DiscoveryCatalogRestoreError, DiscoveryCatalogSeed,
    DiscoveryCatalogStoreError, DiscoveryCatalogUpdate, DiscoveryObservationCount, DiscoveryRecord,
};
pub use codec::{
    decode_advertisement, decode_envelope, encode_advertisement, encode_encrypted_envelope,
    encode_plaintext_envelope, DiscoveryDecodeError, DiscoveryEncodeError, DiscoveryEnvelope,
    DiscoveryEnvelopeBody, DiscoveryEnvelopeError, DiscoveryField,
};
pub use coordinator::{
    DiscoveryAttachmentRegistrationFailure, DiscoveryCoordinator, DiscoveryCoordinatorAction,
    DiscoveryCoordinatorEvent, DiscoveryCoordinatorOutput, DiscoveryEndpointReservation,
    DiscoveryEndpointReservationError, DiscoveryIngressEligibility, DiscoveryIngressFilter,
};
pub use intake::{
    ingest_discovery_announce, DiscoveredInterface, DiscoveredInterfaceId,
    DiscoveryDecryptionError, DiscoveryEnvelopeSecurity, DiscoveryIntake, DiscoveryNotApplicable,
    DiscoveryProvenance, DiscoveryRejection, DiscoveryRejectionKind, InterfaceOrigin,
};
pub use policy::{
    discovered_interface_status, AutoConnectPolicy, DiscoveredInterfaceStatus,
    DiscoverySourceAllowList, DiscoverySourcePolicy, EnabledDiscoveryPolicy,
    InterfaceDiscoveryPolicy, DISCOVERY_EXPIRES_AFTER, DISCOVERY_STALE_AFTER,
    DISCOVERY_UNKNOWN_AFTER,
};
pub use protocol::{discovery_destination_hash, APP_ASPECTS, APP_NAME, DOTTED_NAME_HASH};
pub use publication::{
    frame_discovery_publication, prepare_discovery_publication,
    prepare_discovery_publication_with_stamp_cache, DiscoveryPublicationEncryptionError,
    DiscoveryPublicationFrameError, DiscoveryPublicationPreparation,
    DiscoveryPublicationRegistration, DiscoveryPublicationSchedule,
    DiscoveryPublicationScheduleError, DiscoveryPublicationSecurity, DiscoveryPublicationTiming,
    PreparedDiscoveryAdvertisement,
};
pub use stamp::{
    generate_stamp, stamp_value, validate_stamp, AdvertisementHash, GeneratedStamp, StampCost,
    StampCostError, StampGeneration, StampValidation, StampValue, StampValueError,
    DEFAULT_STAMP_COST, STAMP_SIZE, WORKBLOCK_EXPAND_ROUNDS,
};
pub use storage::{
    discovery_validation_index_buckets, DiscoveredConnectionTable, DiscoveredEndpointSet,
    DiscoveryCatalogTable, DiscoveryValidationCache, FixedDiscoveryValidationCache,
    GrowableInterfaceDiscoveryStorage, HeapDiscoveredConnectionTable, HeapDiscoveredEndpointSet,
    HeapDiscoveryCatalogTable, HeapDiscoveryValidationCache, InterfaceDiscoveryStorage,
    RNS_VALIDATION_CACHE_CAPACITY,
};
