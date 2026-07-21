use super::*;

fn sibling_binary(name: &str) -> std::path::PathBuf {
    let mut path = std::env::current_exe().expect("own path");
    path.set_file_name(name);
    path
}

fn external_node(impl_dir: &str, binary: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("external")
        .join(impl_dir)
        .join("interop")
        .join(binary)
}

fn reference_python(script: &str) -> Command {
    let reference = Path::new(env!("CARGO_MANIFEST_DIR")).join("reference");
    let python: OsString = std::env::var_os("RNS_REFERENCE_PYTHON")
        .filter(|p| Path::new(p).exists())
        .or_else(|| {
            [
                reference.join(".venv").join("bin").join("python"),
                reference.join(".venv").join("Scripts").join("python.exe"),
            ]
            .into_iter()
            .find(|p| p.exists())
            .map(|p| p.into_os_string())
        })
        .unwrap_or_else(|| OsString::from("python3"));
    let mut c = Command::new(python);
    c.arg("-u");
    c.arg(reference.join(script));
    c
}

pub(super) struct Implementation {
    pub(super) name: &'static str,
    pub(super) slug: &'static str,
    pub(super) label: &'static str,
    pub(super) interop_roles: &'static [&'static str],
    pub(super) interop_mechanisms: Option<&'static [&'static str]>,
    pub(super) interop_self_only: bool,
}

const BOTH_ROLES: &[&str] = &["initiator", "responder"];
const BOTH_MECHANISMS: &[&str] = &["single", "link"];

pub(super) fn implementation(name: &str) -> Implementation {
    match name {
        "self" => Implementation {
            name: "self",
            slug: "personal-rns",
            label: "Prns",
            interop_roles: BOTH_ROLES,
            interop_mechanisms: None,
            interop_self_only: false,
        },
        "reference" => Implementation {
            name: "reference",
            slug: "rns-1.4.0",
            label: "RNS 1.4.0",
            interop_roles: BOTH_ROLES,
            interop_mechanisms: None,
            interop_self_only: false,
        },
        "go-reticulum" => Implementation {
            name: "go-reticulum",
            slug: "go-reticulum",
            label: "go-reticulum",
            interop_roles: BOTH_ROLES,
            interop_mechanisms: Some(BOTH_MECHANISMS),
            interop_self_only: false,
        },
        "leviculum" => Implementation {
            name: "leviculum",
            slug: "leviculum",
            label: "Leviculum 0.6.3",
            interop_roles: BOTH_ROLES,
            interop_mechanisms: Some(BOTH_MECHANISMS),
            interop_self_only: false,
        },
        "rns-cr" => Implementation {
            name: "rns-cr",
            slug: "rns-cr",
            label: "rns-cr 0.1.0",
            interop_roles: &["initiator"],
            interop_mechanisms: Some(&["single"]),
            interop_self_only: false,
        },
        "lxmf-rs" => Implementation {
            name: "lxmf-rs",
            slug: "lxmf-rs",
            label: "LXMF-rs 0.2.0",
            interop_roles: BOTH_ROLES,
            interop_mechanisms: Some(&["link"]),
            interop_self_only: true,
        },
        other => {
            panic!(
                "unknown implementation {other:?} \
                 (self|reference|go-reticulum|leviculum|rns-cr|lxmf-rs)"
            )
        }
    }
}

pub(super) fn unsupported_pairing(
    initiator: &Implementation,
    responder: &Implementation,
    mechanism: &str,
) -> Option<String> {
    if initiator
        .interop_mechanisms
        .is_some_and(|m| !m.contains(&mechanism))
    {
        return Some(format!("{} fields no {mechanism} node", initiator.name));
    }
    if !initiator.interop_roles.contains(&"initiator") {
        return Some(format!("{} fields no initiator", initiator.name));
    }
    if responder
        .interop_mechanisms
        .is_some_and(|m| !m.contains(&mechanism))
    {
        return Some(format!("{} fields no {mechanism} node", responder.name));
    }
    if !responder.interop_roles.contains(&"responder") {
        return Some(format!("{} fields no responder", responder.name));
    }
    if (initiator.interop_self_only || responder.interop_self_only)
        && initiator.name != responder.name
    {
        let odd = if initiator.interop_self_only {
            initiator.name
        } else {
            responder.name
        };
        return Some(format!(
            "{odd}'s {mechanism} wire interoperates only with itself (the mechanism is not one \
             protocol across impls)"
        ));
    }
    None
}

impl Implementation {
    pub(super) fn interop_command(&self) -> Option<Command> {
        match self.name {
            "self" => Some(Command::new(sibling_binary("scenario_node"))),
            "reference" => Some(reference_python("scenario_node.py")),
            "go-reticulum" => Some(Command::new(external_node("go-reticulum", "go-node"))),
            "leviculum" => Some(Command::new(external_node("leviculum", "leviculum-node"))),
            "rns-cr" => Some(Command::new(external_node("rns-cr", "rnscr-node"))),
            "lxmf-rs" => Some(Command::new(external_node("lxmf-rs", "lxmf-node"))),
            _ => None,
        }
    }
}
