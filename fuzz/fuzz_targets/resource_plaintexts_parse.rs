#![no_main]

use libfuzzer_sys::fuzz_target;
use prns_core::routing::links::resources::advertisement::{
    parse_hashmap_update_plaintext, write_hashmap_update_plaintext, ResourceAdvertisement,
};
use prns_core::routing::links::resources::control::{
    parse_cancel_plaintext, parse_part_request_plaintext, parse_proof_plaintext,
    write_cancel_plaintext, write_part_request_plaintext, write_proof_plaintext,
    PART_REQUEST_PLAINTEXT_CAP, PROOF_PLAINTEXT_LEN,
};
use prns_core::routing::links::resources::{
    ADVERTISEMENT_OVERHEAD, HASHMAP_MAX_LEN, MAP_HASH_LEN, RESOURCE_HASH_LEN, WINDOW_MAX,
};

fuzz_target!(|data: &[u8]| {
    if let Ok(advertisement) = ResourceAdvertisement::parse(data) {
        let hashmap_is_writable = advertisement.hashmap.len() <= HASHMAP_MAX_LEN * MAP_HASH_LEN
            && advertisement.hashmap.len().is_multiple_of(MAP_HASH_LEN);
        if hashmap_is_writable {
            let mut out = vec![0u8; ADVERTISEMENT_OVERHEAD + advertisement.hashmap.len() + 64];
            if let Ok(written) = advertisement.write(&mut out) {
                let reparsed = ResourceAdvertisement::parse(&out[..written])
                    .expect("a freshly written resource advertisement must parse");
                assert_eq!(reparsed, advertisement);
            }
        }
    }

    if let Ok(update) = parse_hashmap_update_plaintext(data) {
        let hashmap_is_writable = update.hashmap.len() <= HASHMAP_MAX_LEN * MAP_HASH_LEN
            && update.hashmap.len().is_multiple_of(MAP_HASH_LEN);
        if hashmap_is_writable {
            let mut out = vec![0u8; RESOURCE_HASH_LEN + update.hashmap.len() + 32];
            if let Ok(written) = write_hashmap_update_plaintext(
                &update.hash,
                update.segment,
                update.hashmap,
                &mut out,
            ) {
                let reparsed = parse_hashmap_update_plaintext(&out[..written])
                    .expect("a freshly written hashmap update must parse");
                assert_eq!(reparsed.hash, update.hash);
                assert_eq!(reparsed.segment, update.segment);
                assert_eq!(reparsed.hashmap, update.hashmap);
            }
        }
    }

    if let Ok(request) = parse_part_request_plaintext(data) {
        let requested_is_writable = request.requested.len() <= WINDOW_MAX * MAP_HASH_LEN
            && request.requested.len().is_multiple_of(MAP_HASH_LEN);
        if requested_is_writable {
            let mut out = [0u8; PART_REQUEST_PLAINTEXT_CAP];
            if let Ok(written) = write_part_request_plaintext(
                &request.hash,
                request.last_known_map_hash.as_ref(),
                request.requested,
                &mut out,
            ) {
                let reparsed = parse_part_request_plaintext(&out[..written])
                    .expect("a freshly written part request must parse");
                assert_eq!(reparsed.hash, request.hash);
                assert_eq!(reparsed.last_known_map_hash, request.last_known_map_hash);
                assert_eq!(reparsed.requested, request.requested);
            }
        }
    }

    if let Ok((hash, proof)) = parse_proof_plaintext(data) {
        let mut out = [0u8; PROOF_PLAINTEXT_LEN];
        assert_eq!(
            write_proof_plaintext(&hash, &proof, &mut out),
            Ok(PROOF_PLAINTEXT_LEN)
        );
        assert_eq!(parse_proof_plaintext(&out), Ok((hash, proof)));
    }

    if let Ok(hash) = parse_cancel_plaintext(data) {
        let mut out = [0u8; RESOURCE_HASH_LEN];
        assert_eq!(
            write_cancel_plaintext(&hash, &mut out),
            Ok(RESOURCE_HASH_LEN)
        );
        assert_eq!(parse_cancel_plaintext(&out), Ok(hash));
    }
});
