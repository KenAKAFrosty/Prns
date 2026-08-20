fn through_hopspot(
    value: Option<personal_rns::DestinationHash>,
) -> Option<hopspot::DestinationHash> {
    value
}

#[test]
fn preserves_personal_rns_type_identity() {
    assert_eq!(through_hopspot(None), None);
}
