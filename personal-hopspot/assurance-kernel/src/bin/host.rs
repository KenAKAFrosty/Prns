fn main() {
    match personal_hopspot_assurance_kernel::run() {
        Ok(evidence) => println!("{evidence}"),
        Err(error) => {
            eprintln!("PRNS_ISA_ERROR {error:?}");
            std::process::exit(1);
        }
    }
}
