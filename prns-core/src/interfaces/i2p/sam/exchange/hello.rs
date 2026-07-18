use alloc::format;

use super::{accepted_reply, private, unexpected, SamExchange};
use crate::interfaces::i2p::sam::{
    SamCommand, SamProtocolError, SamReply, SamReplyKind, SamVersion,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamHello;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamV3_1;

impl private::Sealed for SamHello {}

impl SamExchange for SamHello {
    type Output = SamV3_1;

    fn command(&self) -> SamCommand {
        SamCommand::HelloVersion
    }

    fn conclude(self, reply: SamReply) -> Result<Self::Output, SamProtocolError> {
        match accepted_reply(SamReplyKind::Hello, reply)? {
            SamReply::Hello(SamVersion::V3_1) => Ok(SamV3_1),
            SamReply::Hello(version) => Err(SamProtocolError::InvalidVersion(format!(
                "{}.{}",
                version.major, version.minor
            ))),
            reply => Err(unexpected(SamReplyKind::Hello, reply)),
        }
    }
}
