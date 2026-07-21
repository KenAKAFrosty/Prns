use std::ffi::OsString;
use std::path::Path;

const REQUIRED_ENVIRONMENT: &str = "PRNS_ORACLE_REQUIRED";

pub fn reference_python(environment: &str, fallback: &str) -> Option<OsString> {
    let fallback = Path::new(env!("CARGO_MANIFEST_DIR")).join(fallback);
    if let Some(interpreter) = std::env::var_os(environment) {
        assert!(
            !interpreter.is_empty(),
            "{environment} must name a Python interpreter"
        );
        return Some(interpreter);
    }
    if fallback.is_file() {
        return Some(fallback.into_os_string());
    }
    assert!(
        std::env::var_os(REQUIRED_ENVIRONMENT).is_none(),
        "{environment} is required for this oracle lane and the developer fallback is missing at {}",
        fallback.display()
    );
    None
}
