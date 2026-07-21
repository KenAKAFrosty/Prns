pub const APP_NAME: &str = "rnstransport";
pub const APP_ASPECTS: &[&str] = &["discovery", "interface"];
pub const DOTTED_NAME_HASH: crate::routing::announce::DottedNameHash =
    crate::routing::announce::DottedNameHash::new([
        0x55, 0xaa, 0x39, 0xe8, 0x5c, 0x3e, 0x04, 0x5e, 0x9c, 0xb1,
    ]);

pub fn discovery_destination_hash(
    identity: &crate::identity::IdentityHash,
) -> crate::wire::DestinationHash {
    crate::routing::announce::derive_destination_hash(identity, &DOTTED_NAME_HASH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_discovery_name_hash_matches_the_shared_name_derivation() {
        assert_eq!(
            crate::routing::announce::expand_name(APP_NAME, APP_ASPECTS),
            Ok(DOTTED_NAME_HASH)
        );
    }
}
