/// Host-declared facts about what an interface can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub receives: bool,
    pub transmits: bool,
    pub forwards: bool,
    pub repeats: bool,
}
