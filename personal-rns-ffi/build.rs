fn main() {
    // Generates the uniffi scaffolding (`prns.uniffi.rs`) consumed by
    // `include_scaffolding!` in `src/lib.rs`.
    uniffi::generate_scaffolding("./src/prns.udl").expect("uniffi scaffolding generation failed");
}
