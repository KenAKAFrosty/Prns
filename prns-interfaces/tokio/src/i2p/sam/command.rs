use super::reply::SamReplyKind;
use super::value::{I2pAddress, I2pPrivateDestination, I2pPublicDestination, SamSessionId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SamSessionDestination {
    Transient,
    Persistent(I2pPrivateDestination),
}

impl SamSessionDestination {
    fn command_fragment(&self) -> String {
        match self {
            Self::Transient => String::from("DESTINATION=TRANSIENT SIGNATURE_TYPE=7"),
            Self::Persistent(destination) => {
                format!("DESTINATION={}", destination.as_str())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SamCommand {
    HelloVersion,
    DestinationGenerate,
    SessionCreate {
        id: SamSessionId,
        destination: SamSessionDestination,
    },
    StreamConnect {
        id: SamSessionId,
        destination: I2pPublicDestination,
    },
    StreamAccept {
        id: SamSessionId,
    },
    NamingLookup {
        name: I2pAddress,
    },
}

impl SamCommand {
    pub const fn reply_kind(&self) -> SamReplyKind {
        match self {
            Self::HelloVersion => SamReplyKind::Hello,
            Self::DestinationGenerate => SamReplyKind::Destination,
            Self::SessionCreate { .. } => SamReplyKind::Session,
            Self::StreamConnect { .. } | Self::StreamAccept { .. } => SamReplyKind::Stream,
            Self::NamingLookup { .. } => SamReplyKind::Naming,
        }
    }

    pub fn encode(&self) -> String {
        match self {
            Self::HelloVersion => String::from("HELLO VERSION MIN=3.1 MAX=3.1\n"),
            Self::DestinationGenerate => String::from("DEST GENERATE SIGNATURE_TYPE=7\n"),
            Self::SessionCreate { id, destination } => {
                format!(
                    "SESSION CREATE STYLE=STREAM ID={} {}\n",
                    id.as_str(),
                    destination.command_fragment()
                )
            }
            Self::StreamConnect { id, destination } => format!(
                "STREAM CONNECT ID={} DESTINATION={} SILENT=false\n",
                id.as_str(),
                destination.as_str()
            ),
            Self::StreamAccept { id } => {
                format!("STREAM ACCEPT ID={} SILENT=false\n", id.as_str())
            }
            Self::NamingLookup { name } => {
                format!("NAMING LOOKUP NAME={}\n", name.as_str())
            }
        }
    }
}
