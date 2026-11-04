//! A host-only consumer for an independent telemetry application's contact URI.
//! The codec and all operations below use caller-owned fixed buffers; this
//! example uses std only for fixture loading, errors and diagnostic printing.

use prns_contact_card::{
    decode_uri, encode_uri, service_name_hash, verify, MAX_CARD_BYTES, MAX_URI_BYTES,
};

fn main() -> Result<(), String> {
    let document: serde_json::Value =
        serde_json::from_str(include_str!("../tests/vectors/portable-v1.json"))
            .map_err(|error| error.to_string())?;
    let uri = document["vectors"]
        .as_array()
        .ok_or("Missing vectors array")?
        .iter()
        .find(|vector| vector["service"] == "example.telemetry")
        .ok_or("Missing telemetry vector")?["uri"]
        .as_str()
        .ok_or("Missing telemetry URI")?;

    let mut wire = [0_u8; MAX_CARD_BYTES];
    let length = decode_uri(uri, &mut wire).map_err(|error| format!("{error:?}"))?;
    let card = verify(&wire[..length]).map_err(|error| error.to_string())?;
    // Application policy is separate from signature verification: this
    // telemetry application only accepts its own service destination.
    if *card.service_name_hash() != service_name_hash("example.telemetry") {
        return Err("This application does not support the card's service".into());
    }
    let mut canonical = [0_u8; MAX_URI_BYTES];
    let length = encode_uri(card.wire(), &mut canonical).map_err(|error| format!("{error:?}"))?;
    assert_eq!(&canonical[..length], uri.as_bytes());

    println!("Self-asserted label: {}", card.display_name());
    println!("Identity: {}", hex::encode(card.identity_hash()));
    println!(
        "Service destination: {}",
        hex::encode(card.destination_hash())
    );
    // A real consumer now asks for user or application approval, then persists
    // its own contact record. This does not learn a route or admit group access.
    Ok(())
}
