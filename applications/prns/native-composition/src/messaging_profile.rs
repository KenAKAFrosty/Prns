use crate::contract::{LocalMessagingProfile, PrimaryIdentityState};

pub(crate) const DEFAULT_MESSAGING_NAME: &str = "prns";
pub(crate) const MAX_MESSAGING_NAME_BYTES: usize = 64;

pub(crate) fn normalize_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty()
        || name.len() > MAX_MESSAGING_NAME_BYTES
        || name.chars().any(char::is_control)
    {
        return Err(format!(
            "Use a messaging name containing 1 to {MAX_MESSAGING_NAME_BYTES} UTF-8 bytes without control characters."
        ));
    }
    Ok(name.to_owned())
}

pub(crate) fn messaging_destination(identity: [u8; 16]) -> Option<[u8; 16]> {
    personal_rns::routing::announce::derive_single_destination_hash(
        &personal_rns::identity::IdentityHash::new(identity),
        prns_lxmf::wire::LXMF_APP_NAME,
        prns_lxmf::wire::LXMF_DELIVERY_ASPECTS,
    )
    .ok()
    .map(|destination| *destination.as_bytes())
}

pub(crate) fn profile(name: String, identity: &PrimaryIdentityState) -> LocalMessagingProfile {
    let destination = match identity {
        PrimaryIdentityState::Present { identity_hash } => identity_hash
            .as_slice()
            .try_into()
            .ok()
            .and_then(messaging_destination),
        _ => None,
    };
    LocalMessagingProfile {
        display_name: name,
        destination,
    }
}

pub(crate) fn announce_data(name: &str) -> Result<Vec<u8>, String> {
    let name = normalize_name(name)?;
    let mut output = [0_u8; prns_lxmf::direct::MAX_ANNOUNCE_APP_DATA_BYTES];
    let length = prns_lxmf::wire::encode_current_lxmf_announce(name.as_bytes(), &mut output)
        .map_err(|error| format!("Could not encode the messaging name: {error:?}"))?;
    Ok(output[..length].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messaging_names_are_trimmed_and_bounded_by_encoded_bytes() {
        assert_eq!(normalize_name("  Phone  ").unwrap(), "Phone");
        assert!(normalize_name(" \n ").is_err());
        assert!(normalize_name("Phone\nName").is_err());
        assert!(normalize_name(&"é".repeat(32)).is_ok());
        assert!(normalize_name(&"é".repeat(33)).is_err());
        let encoded = announce_data(&"é".repeat(32)).unwrap();
        let parsed = prns_lxmf::wire::parse_lxmf_announce(&encoded, 255).unwrap();
        assert_eq!(parsed.display_name, Some("é".repeat(32).as_bytes()));
    }
}
