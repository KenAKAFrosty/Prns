mod codec;
mod model;
mod stamp;

pub use codec::{
    decode_advertisement, decode_envelope, encode_advertisement, encode_encrypted_envelope,
    encode_plaintext_envelope, DiscoveryDecodeError, DiscoveryEncodeError, DiscoveryEnvelope,
    DiscoveryEnvelopeBody, DiscoveryEnvelopeError, DiscoveryField,
};
pub use model::{
    AdvertisedInterfaceType, AdvertisedTransport, AdvertisementDetails, DiscoveryAdvertisement,
    GeographicLocation, PublishedIfac,
};
pub use stamp::{
    generate_stamp, stamp_value, validate_stamp, AdvertisementHash, GeneratedStamp, StampCost,
    StampCostError, StampGeneration, StampValidation, StampValue, DEFAULT_STAMP_COST, STAMP_SIZE,
    WORKBLOCK_EXPAND_ROUNDS,
};

pub const APP_NAME: &str = "rnstransport";
pub const APP_ASPECTS: &[&str] = &["discovery", "interface"];
