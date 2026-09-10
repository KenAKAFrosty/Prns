fn main() {
    println!(
        "{}",
        serde_json::json!({
            "app": prns_app::contract::CONTRACT_FINGERPRINT,
            "host": prns_app::contract::HOST_CONTRACT_FINGERPRINT,
        })
    );
}
