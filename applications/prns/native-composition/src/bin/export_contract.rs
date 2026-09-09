fn main() {
    if std::env::args().any(|argument| argument == "--fingerprints") {
        println!(
            "{}",
            serde_json::json!({
                "app": prns_app::contract::CONTRACT_FINGERPRINT,
                "host": prns_app::contract::HOST_CONTRACT_FINGERPRINT,
            })
        );
    } else {
        print!("{}", prns_app::contract::export_typescript());
    }
}
