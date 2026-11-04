//! Offline signed service claims for enrollment, independent of LXMF or a host.
//!
//! Verification proves a service claim signed by an identity. Selecting a
//! recipient remains a consumer consent/trust decision. The claim grants no
//! membership, supplies no route and does not replace live Link authentication.

use prns_contact_card::VerifiedContactCard;
use prns_host::IdentityHash;

/// The exact enrollment service advertised by the ordinary group carrier.
pub fn enrollment_service_name_hash() -> [u8; prns_contact_card::SERVICE_NAME_HASH_BYTES] {
    prns_contact_card::service_name_hash(&format!(
        "{}.{}",
        crate::APP_NAME,
        crate::ENROLLMENT_ASPECTS.join(".")
    ))
}

/// Extract a recipient only from a verified claim for this enrollment service.
///
/// A valid claim for another service is not a group contact. The returned public
/// identity is suitable for the ordinary group Invite intent after user consent;
/// importing the card itself does not send or accept an invitation.
pub fn enrollment_recipient(card: &VerifiedContactCard<'_>) -> Option<IdentityHash> {
    (card.service_name_hash() == &enrollment_service_name_hash())
        .then(|| IdentityHash::new(*card.identity_hash()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::identity::PrivateIdentityMaterial;

    #[test]
    fn signed_card_recipient_and_destination_match_the_enrollment_carrier() {
        let identity = PrivateIdentityMaterial::from_bytes([19; 64]);
        let mut wire = [0; prns_contact_card::MAX_CARD_BYTES];
        let length = prns_contact_card::encode_identity(
            &identity,
            &enrollment_service_name_hash(),
            "A group peer",
            &mut wire,
        )
        .unwrap();
        let card = prns_contact_card::verify(&wire[..length]).unwrap();
        let recipient = enrollment_recipient(&card).unwrap();
        assert_eq!(recipient.as_bytes(), identity.identity_hash().as_bytes());
        assert_eq!(
            crate::enrollment_destination_for(recipient)
                .unwrap()
                .as_bytes(),
            card.destination_hash()
        );
        assert_ne!(
            crate::destination_for(recipient).unwrap().as_bytes(),
            card.destination_hash()
        );
        wire[84] ^= 1;
        assert!(prns_contact_card::verify(&wire[..length]).is_err());
    }

    #[test]
    fn an_otherwise_valid_service_claim_does_not_select_a_group_recipient() {
        let identity = PrivateIdentityMaterial::from_bytes([20; 64]);
        for service in ["prns.groups", "lxmf.delivery", "example.telemetry"] {
            let mut wire = [0; prns_contact_card::MAX_CARD_BYTES];
            let length = prns_contact_card::encode_identity(
                &identity,
                &prns_contact_card::service_name_hash(service),
                "peer",
                &mut wire,
            )
            .unwrap();
            assert!(
                enrollment_recipient(&prns_contact_card::verify(&wire[..length]).unwrap())
                    .is_none()
            );
        }
    }
}
