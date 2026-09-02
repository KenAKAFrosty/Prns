fn main() {
    println!("cargo:rerun-if-env-changed=PRNS_LORA_PROFILE");
    if let Ok(profile) = std::env::var("PRNS_LORA_PROFILE") {
        println!("cargo:rustc-env=PRNS_LORA_PROFILE={profile}");
    }
}
