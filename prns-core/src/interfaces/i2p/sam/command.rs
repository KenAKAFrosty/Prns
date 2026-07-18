use alloc::format;
use alloc::string::String;

use super::value::{I2pAddress, I2pPrivateDestination, I2pPublicDestination, SamSessionId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SamSessionDestination {
    Transient,
    Persistent(I2pPrivateDestination),
}

impl SamSessionDestination {
    fn as_str(&self) -> &str {
        match self {
            Self::Transient => "TRANSIENT",
            Self::Persistent(destination) => destination.as_str(),
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
    pub fn encode(&self) -> String {
        match self {
            Self::HelloVersion => String::from("HELLO VERSION MIN=3.1 MAX=3.1\n"),
            Self::DestinationGenerate => String::from("DEST GENERATE SIGNATURE_TYPE=7\n"),
            Self::SessionCreate { id, destination } => {
                format!(
                    "SESSION CREATE STYLE=STREAM ID={} DESTINATION={} \n",
                    id.as_str(),
                    destination.as_str()
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
