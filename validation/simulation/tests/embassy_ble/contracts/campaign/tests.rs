use super::cases::{cases, Action::*, Case, Profile};

#[test]
fn generation_has_a_stable_complete_bounded_manifest() {
    let expected: Vec<_> = [
        [Expire, Cancel, Reconnect],
        [Expire, Reconnect, Cancel],
        [Cancel, Expire, Reconnect],
        [Cancel, Reconnect, Expire],
        [Reconnect, Expire, Cancel],
        [Reconnect, Cancel, Expire],
    ]
    .into_iter()
    .flat_map(|actions| {
        [Profile::ShortDeadline, Profile::Fragmented]
            .into_iter()
            .map(move |profile| Case { actions, profile })
    })
    .collect();
    assert_eq!(cases(), expected);
}

#[test]
fn profiles_cover_distinct_payloads_and_deadlines() {
    assert_eq!(
        [
            Profile::ShortDeadline.timeout_ms(),
            Profile::Fragmented.timeout_ms()
        ],
        [1, 50]
    );
    assert_eq!(Profile::ShortDeadline.payload(3), vec![3]);
    assert_eq!(
        Profile::Fragmented.payload(0),
        (0..=u8::MAX).collect::<Vec<_>>()
    );
    assert_ne!(
        Profile::Fragmented.payload(0),
        Profile::Fragmented.payload(1)
    );
}
