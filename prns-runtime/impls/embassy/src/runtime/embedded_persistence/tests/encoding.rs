use super::*;

#[test]
fn shared_route_encoder_preserves_bytes_and_refuses_every_short_buffer() {
    for app_len in [0, 1, MAX_ANNOUNCE_APP_DATA_LEN] {
        let app_data = std::vec![0x42; app_len];
        let row = signed_route(7, &app_data);
        let mut durable = row.clone();
        durable.announce_id_ring = AnnounceIdRing::Table(&[]);
        let required = routing_table_snapshot_len(core::iter::once(durable.clone()));
        let mut expected = [0xa5; RECORD_SCRATCH_LEN];
        let written =
            write_routing_table_snapshot(core::iter::once(durable), &mut expected[..required])
                .unwrap();
        let mut actual = [0xa5; RECORD_SCRATCH_LEN];
        assert_eq!(encode_route_upsert(row.clone(), &mut actual), Ok(written));
        assert_eq!(actual, expected);
        for len in 0..required {
            assert_eq!(
                encode_route_upsert(row.clone(), &mut actual[..len]),
                Err(())
            );
        }
    }
}

#[test]
fn shared_ratchet_encoder_preserves_bytes_and_refuses_every_short_buffer() {
    let destination = DestinationHash::new([0x42; TRUNCATED_HASH_BYTE_LEN]);
    for count in [0, 1, 8] {
        let secrets: Vec<_> = (0..count)
            .map(|seed| X25519SecretKey::new([seed; 32]))
            .collect();
        for last_rotated in [LastRotated::Never, LastRotated::At(InstantMillis(1234))] {
            let required = TRUNCATED_HASH_BYTE_LEN + self_ratchets_snapshot_len(secrets.len());
            let mut expected = Zeroizing::new([0xa5; RECORD_SCRATCH_LEN]);
            expected[..TRUNCATED_HASH_BYTE_LEN].copy_from_slice(destination.as_bytes());
            let written = write_self_ratchets_snapshot(
                last_rotated,
                &secrets,
                &mut expected[TRUNCATED_HASH_BYTE_LEN..required],
            )
            .unwrap();
            let mut actual = Zeroizing::new([0xa5; RECORD_SCRATCH_LEN]);
            assert_eq!(
                encode_ratchet_row(destination, last_rotated, &secrets, &mut *actual),
                Ok(TRUNCATED_HASH_BYTE_LEN + written),
            );
            assert_eq!(*actual, *expected);
            for len in 0..required {
                assert_eq!(
                    encode_ratchet_row(destination, last_rotated, &secrets, &mut actual[..len]),
                    Err(()),
                );
            }
        }
    }
}
