pub(crate) fn embedded_docs_mode() -> bool {
    option_env!("PRNS_EMBEDDED_SITE")
        .is_some_and(|value| matches!(value, "1" | "true" | "TRUE" | "yes" | "YES"))
}
