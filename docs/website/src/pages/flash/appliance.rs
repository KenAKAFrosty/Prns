use dioxus::prelude::*;

use crate::components::MarkdownBody;

const G4_SLUG: &str = "thinknode-g4";
const HELTEC_SLUG: &str = "heltec-ht-hd01-v2";

pub(super) enum Appliance {
    ThinkNodeG4,
    HeltecHtHd01V2,
}

impl Appliance {
    pub(super) fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            G4_SLUG => Some(Self::ThinkNodeG4),
            HELTEC_SLUG => Some(Self::HeltecHtHd01V2),
            _ => None,
        }
    }

    fn quick_guide(&self) -> &'static str {
        match self {
            Self::ThinkNodeG4 => G4_QUICK_GUIDE,
            Self::HeltecHtHd01V2 => HELTEC_QUICK_GUIDE,
        }
    }

    fn extended_guide(&self) -> &'static str {
        match self {
            Self::ThinkNodeG4 => G4_EXTENDED_GUIDE,
            Self::HeltecHtHd01V2 => HELTEC_EXTENDED_GUIDE,
        }
    }
}

const G4_QUICK_GUIDE: &str =
    include_str!("../../../../../personal-hopspot/headless/docs/g4-quick-installation.md");
const HELTEC_QUICK_GUIDE: &str =
    include_str!("../../../../../personal-hopspot/headless/docs/heltec-quick-installation.md");
const SHARED_QUICK_GUIDE: &str =
    include_str!("../../../../../personal-hopspot/headless/docs/halow-quick-installation.md");
const G4_EXTENDED_GUIDE: &str =
    include_str!("../../../../../personal-hopspot/headless/docs/g4-installation.md");
const HELTEC_EXTENDED_GUIDE: &str =
    include_str!("../../../../../personal-hopspot/headless/docs/heltec-installation.md");
const SHARED_EXTENDED_GUIDE: &str =
    include_str!("../../../../../personal-hopspot/appliance/docs/guided-installation.md");

pub(super) fn installation_guide(appliance: &Appliance) -> Element {
    rsx! {
        MarkdownBody { heading_offset: 1, source: format!("{}\n\n{SHARED_QUICK_GUIDE}", appliance.quick_guide()) }
        details { class: "mt-10",
            summary { class: "cursor-pointer text-sm text-soft hover:text-accent transition-colors",
                "Extended guide: every check, every failure case, and the reasoning behind each step"
            }
            div { class: "mt-6",
                MarkdownBody { heading_offset: 1, source: format!("{}\n\n{SHARED_EXTENDED_GUIDE}", appliance.extended_guide()) }
            }
        }
    }
}
