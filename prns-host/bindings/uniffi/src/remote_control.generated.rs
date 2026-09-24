// Generated from public prns-core Remote Control declarations. Do not edit.
// Uniform fallible converters retain ? for nested validation and error conversion.
#![allow(clippy::needless_question_mark)]
pub const REMOTE_CONTROL_SEMANTIC_FINGERPRINT: &str =
    "becd93f1924c3ec2e50562f2df71a57b80c6cffb81f20399e616abbdc4e3df5c";
use crate::transport::BindingError;
pub struct RemoteControlSecretText(zeroize::Zeroizing<String>);
uniffi::custom_type!(RemoteControlSecretText, String, {
    lower: |value| value.0.to_string(),
    try_lift: |value| Ok(RemoteControlSecretText(zeroize::Zeroizing::new(value))),
});
pub struct RemoteControlSecretCode(zeroize::Zeroizing<u32>);
uniffi::custom_type!(RemoteControlSecretCode, u32, {
    lower: |value| *value.0,
    try_lift: |value| Ok(RemoteControlSecretCode(zeroize::Zeroizing::new(value))),
});
fn invalid(name: &str) -> BindingError {
    BindingError::InvalidInput {
        field: name.into(),
        detail: "invalid protocol value".into(),
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlRequest {
    Describe,
    AnnounceSelf,
    InventoryInterfaces {
        page: RemoteControlInterfacePage,
    },
    SetInterfacePower {
        id: RemoteControlInterfaceId,
        power: RemoteControlInterfacePower,
    },
    SetInterfaceMode {
        id: RemoteControlInterfaceId,
        mode: RemoteControlInterfaceMode,
    },
    SetInterfaceGroup {
        id: RemoteControlInterfaceId,
        group: RemoteControlInterfaceGroup,
    },
    InventoryInterfaceDiscoveryGroups {
        id: RemoteControlInterfaceId,
    },
    ReplaceInterfaceDiscoveryGroups {
        id: RemoteControlInterfaceId,
        groups: RemoteControlDiscoveryGroups,
    },
    InventoryInterfacePeers {
        id: RemoteControlInterfaceId,
        page: RemoteControlPeerPage,
    },
    InventoryInterfaceConfig {
        id: RemoteControlInterfaceId,
    },
    SetInterfaceLoRaProfile {
        id: RemoteControlInterfaceId,
        profile: RemoteControlLoRaProfile,
    },
    SetInterfaceWifiStation {
        id: RemoteControlInterfaceId,
        station: RemoteControlWifiStation,
    },
    InventoryControllers {
        page: RemoteControlControllerPage,
    },
    AuthorizeController {
        controller: RemoteControlControllerIdentity,
        permitted_requests: RemoteControlRequestSet,
    },
    RevokeController {
        hash: RemoteControlIdentityHash,
    },
    DescribeBuild,
    DescribePower,
    SleepRadios,
    WakeRadios,
    SetSystemPower {
        power: RemoteControlSystemPower,
    },
    SetGnssPower {
        power: RemoteControlGnssPower,
    },
    SetDisplayVisibility {
        visibility: RemoteControlDisplayVisibility,
    },
    SetDisplayAutoOff {
        auto_off: RemoteControlDisplayAutoOff,
    },
    SetStationUplink {
        id: RemoteControlInterfaceId,
        uplink: RemoteControlStationUplink,
    },
    SetEspRadioMode {
        mode: RemoteControlEspRadioMode,
    },
    StageWifiCredentials {
        station: RemoteControlWifiStation,
    },
    ActivateWifiCredentials {
        revision: RemoteControlWifiCredentialRevision,
    },
    ConfirmWifiCredentials {
        revision: RemoteControlWifiCredentialRevision,
    },
    CancelWifiCredentials {
        revision: RemoteControlWifiCredentialRevision,
    },
    InspectWifiTransaction,
}
impl TryFrom<RemoteControlRequest> for prns_core::remote_control::RemoteControlRequest {
    type Error = BindingError;
    fn try_from(value: RemoteControlRequest) -> Result<Self, Self::Error> {
        Ok(match value {
            RemoteControlRequest::Describe => {
                prns_core::remote_control::RemoteControlRequest::Describe
            }
            RemoteControlRequest::AnnounceSelf => {
                prns_core::remote_control::RemoteControlRequest::AnnounceSelf
            }
            RemoteControlRequest::InventoryInterfaces { page } => {
                prns_core::remote_control::RemoteControlRequest::InventoryInterfaces {
                    page: page.try_into()?,
                }
            }
            RemoteControlRequest::SetInterfacePower { id, power } => {
                prns_core::remote_control::RemoteControlRequest::SetInterfacePower {
                    id: id.try_into()?,
                    power: power.try_into()?,
                }
            }
            RemoteControlRequest::SetInterfaceMode { id, mode } => {
                prns_core::remote_control::RemoteControlRequest::SetInterfaceMode {
                    id: id.try_into()?,
                    mode: mode.try_into()?,
                }
            }
            RemoteControlRequest::SetInterfaceGroup { id, group } => {
                prns_core::remote_control::RemoteControlRequest::SetInterfaceGroup {
                    id: id.try_into()?,
                    group: group.try_into()?,
                }
            }
            RemoteControlRequest::InventoryInterfaceDiscoveryGroups { id } => {
                prns_core::remote_control::RemoteControlRequest::InventoryInterfaceDiscoveryGroups {
                    id: id.try_into()?,
                }
            }
            RemoteControlRequest::ReplaceInterfaceDiscoveryGroups { id, groups } => {
                prns_core::remote_control::RemoteControlRequest::ReplaceInterfaceDiscoveryGroups {
                    id: id.try_into()?,
                    groups: groups.try_into()?,
                }
            }
            RemoteControlRequest::InventoryInterfacePeers { id, page } => {
                prns_core::remote_control::RemoteControlRequest::InventoryInterfacePeers {
                    id: id.try_into()?,
                    page: page.try_into()?,
                }
            }
            RemoteControlRequest::InventoryInterfaceConfig { id } => {
                prns_core::remote_control::RemoteControlRequest::InventoryInterfaceConfig {
                    id: id.try_into()?,
                }
            }
            RemoteControlRequest::SetInterfaceLoRaProfile { id, profile } => {
                prns_core::remote_control::RemoteControlRequest::SetInterfaceLoRaProfile {
                    id: id.try_into()?,
                    profile: profile.try_into()?,
                }
            }
            RemoteControlRequest::SetInterfaceWifiStation { id, station } => {
                prns_core::remote_control::RemoteControlRequest::SetInterfaceWifiStation {
                    id: id.try_into()?,
                    station: station.try_into()?,
                }
            }
            RemoteControlRequest::InventoryControllers { page } => {
                prns_core::remote_control::RemoteControlRequest::InventoryControllers {
                    page: page.try_into()?,
                }
            }
            RemoteControlRequest::AuthorizeController {
                controller,
                permitted_requests,
            } => prns_core::remote_control::RemoteControlRequest::AuthorizeController {
                controller: controller.try_into()?,
                permitted_requests: permitted_requests.try_into()?,
            },
            RemoteControlRequest::RevokeController { hash } => {
                prns_core::remote_control::RemoteControlRequest::RevokeController {
                    hash: hash.try_into()?,
                }
            }
            RemoteControlRequest::DescribeBuild => {
                prns_core::remote_control::RemoteControlRequest::DescribeBuild
            }
            RemoteControlRequest::DescribePower => {
                prns_core::remote_control::RemoteControlRequest::DescribePower
            }
            RemoteControlRequest::SleepRadios => {
                prns_core::remote_control::RemoteControlRequest::SleepRadios
            }
            RemoteControlRequest::WakeRadios => {
                prns_core::remote_control::RemoteControlRequest::WakeRadios
            }
            RemoteControlRequest::SetSystemPower { power } => {
                prns_core::remote_control::RemoteControlRequest::SetSystemPower {
                    power: power.try_into()?,
                }
            }
            RemoteControlRequest::SetGnssPower { power } => {
                prns_core::remote_control::RemoteControlRequest::SetGnssPower {
                    power: power.try_into()?,
                }
            }
            RemoteControlRequest::SetDisplayVisibility { visibility } => {
                prns_core::remote_control::RemoteControlRequest::SetDisplayVisibility {
                    visibility: visibility.try_into()?,
                }
            }
            RemoteControlRequest::SetDisplayAutoOff { auto_off } => {
                prns_core::remote_control::RemoteControlRequest::SetDisplayAutoOff {
                    auto_off: auto_off.try_into()?,
                }
            }
            RemoteControlRequest::SetStationUplink { id, uplink } => {
                prns_core::remote_control::RemoteControlRequest::SetStationUplink {
                    id: id.try_into()?,
                    uplink: uplink.try_into()?,
                }
            }
            RemoteControlRequest::SetEspRadioMode { mode } => {
                prns_core::remote_control::RemoteControlRequest::SetEspRadioMode {
                    mode: mode.try_into()?,
                }
            }
            RemoteControlRequest::StageWifiCredentials { station } => {
                prns_core::remote_control::RemoteControlRequest::StageWifiCredentials {
                    station: station.try_into()?,
                }
            }
            RemoteControlRequest::ActivateWifiCredentials { revision } => {
                prns_core::remote_control::RemoteControlRequest::ActivateWifiCredentials {
                    revision: revision.try_into()?,
                }
            }
            RemoteControlRequest::ConfirmWifiCredentials { revision } => {
                prns_core::remote_control::RemoteControlRequest::ConfirmWifiCredentials {
                    revision: revision.try_into()?,
                }
            }
            RemoteControlRequest::CancelWifiCredentials { revision } => {
                prns_core::remote_control::RemoteControlRequest::CancelWifiCredentials {
                    revision: revision.try_into()?,
                }
            }
            RemoteControlRequest::InspectWifiTransaction => {
                prns_core::remote_control::RemoteControlRequest::InspectWifiTransaction
            }
        })
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlInterfacePage {
    First,
    After { value: RemoteControlInterfaceCursor },
}
impl TryFrom<RemoteControlInterfacePage> for prns_core::remote_control::RemoteControlInterfacePage {
    type Error = BindingError;
    fn try_from(value: RemoteControlInterfacePage) -> Result<Self, Self::Error> {
        Ok(match value {
            RemoteControlInterfacePage::First => {
                prns_core::remote_control::RemoteControlInterfacePage::First
            }
            RemoteControlInterfacePage::After { value } => {
                prns_core::remote_control::RemoteControlInterfacePage::After(value.try_into()?)
            }
        })
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlInterfaceCursor {
    pub value: RemoteControlInterfaceId,
}
impl TryFrom<RemoteControlInterfaceCursor>
    for prns_core::remote_control::RemoteControlInterfaceCursor
{
    type Error = BindingError;
    fn try_from(value: RemoteControlInterfaceCursor) -> Result<Self, Self::Error> {
        Ok(prns_core::remote_control::RemoteControlInterfaceCursor::after(value.value.try_into()?))
    }
}
impl From<prns_core::remote_control::RemoteControlInterfaceCursor>
    for RemoteControlInterfaceCursor
{
    fn from(value: prns_core::remote_control::RemoteControlInterfaceCursor) -> Self {
        Self {
            value: value.id().into(),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlInterfaceId {
    pub value: Vec<u8>,
}
impl TryFrom<RemoteControlInterfaceId> for prns_core::interfaces::InterfaceId {
    type Error = BindingError;
    fn try_from(value: RemoteControlInterfaceId) -> Result<Self, Self::Error> {
        Ok(prns_core::interfaces::InterfaceId::new(
            value.value.try_into().map_err(|_| invalid("InterfaceId"))?,
        ))
    }
}
impl From<prns_core::interfaces::InterfaceId> for RemoteControlInterfaceId {
    fn from(value: prns_core::interfaces::InterfaceId) -> Self {
        Self {
            value: value.as_bytes().to_vec(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlInterfacePower {
    Off,
    On,
}
impl TryFrom<RemoteControlInterfacePower>
    for prns_core::remote_control::RemoteControlInterfacePower
{
    type Error = BindingError;
    fn try_from(value: RemoteControlInterfacePower) -> Result<Self, Self::Error> {
        Ok(match value {
            RemoteControlInterfacePower::Off => {
                prns_core::remote_control::RemoteControlInterfacePower::Off
            }
            RemoteControlInterfacePower::On => {
                prns_core::remote_control::RemoteControlInterfacePower::On
            }
        })
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlInterfaceMode {
    Full,
    PointToPoint,
    AccessPoint,
    Roaming,
    Boundary,
    Gateway,
    Internal,
}
impl TryFrom<RemoteControlInterfaceMode> for prns_core::interfaces::InterfaceMode {
    type Error = BindingError;
    fn try_from(value: RemoteControlInterfaceMode) -> Result<Self, Self::Error> {
        Ok(match value {
            RemoteControlInterfaceMode::Full => prns_core::interfaces::InterfaceMode::Full,
            RemoteControlInterfaceMode::PointToPoint => {
                prns_core::interfaces::InterfaceMode::PointToPoint
            }
            RemoteControlInterfaceMode::AccessPoint => {
                prns_core::interfaces::InterfaceMode::AccessPoint
            }
            RemoteControlInterfaceMode::Roaming => prns_core::interfaces::InterfaceMode::Roaming,
            RemoteControlInterfaceMode::Boundary => prns_core::interfaces::InterfaceMode::Boundary,
            RemoteControlInterfaceMode::Gateway => prns_core::interfaces::InterfaceMode::Gateway,
            RemoteControlInterfaceMode::Internal => prns_core::interfaces::InterfaceMode::Internal,
        })
    }
}
impl From<prns_core::interfaces::InterfaceMode> for RemoteControlInterfaceMode {
    fn from(value: prns_core::interfaces::InterfaceMode) -> Self {
        match value {
            prns_core::interfaces::InterfaceMode::Full => RemoteControlInterfaceMode::Full,
            prns_core::interfaces::InterfaceMode::PointToPoint => {
                RemoteControlInterfaceMode::PointToPoint
            }
            prns_core::interfaces::InterfaceMode::AccessPoint => {
                RemoteControlInterfaceMode::AccessPoint
            }
            prns_core::interfaces::InterfaceMode::Roaming => RemoteControlInterfaceMode::Roaming,
            prns_core::interfaces::InterfaceMode::Boundary => RemoteControlInterfaceMode::Boundary,
            prns_core::interfaces::InterfaceMode::Gateway => RemoteControlInterfaceMode::Gateway,
            prns_core::interfaces::InterfaceMode::Internal => RemoteControlInterfaceMode::Internal,
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlInterfaceGroup {
    pub value: String,
}
impl TryFrom<RemoteControlInterfaceGroup>
    for prns_core::remote_control::RemoteControlInterfaceGroup
{
    type Error = BindingError;
    fn try_from(value: RemoteControlInterfaceGroup) -> Result<Self, Self::Error> {
        Ok(
            prns_core::remote_control::RemoteControlInterfaceGroup::parse(&value.value)
                .ok_or_else(|| invalid("RemoteControlInterfaceGroup"))?,
        )
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlDiscoveryGroups {
    pub groups: Vec<String>,
}
impl TryFrom<RemoteControlDiscoveryGroups>
    for prns_core::remote_control::RemoteControlDiscoveryGroups
{
    type Error = BindingError;
    fn try_from(value: RemoteControlDiscoveryGroups) -> Result<Self, Self::Error> {
        Ok({
            let groups = value
                .groups
                .into_iter()
                .map(|text| {
                    prns_core::interfaces::DiscoveryGroupId::parse(&text)
                        .map_err(|_| invalid("DiscoveryGroup"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            prns_core::remote_control::RemoteControlDiscoveryGroups::new(
                prns_core::interfaces::DiscoveryGroupSet::try_from_slice(&groups)
                    .map_err(|_| invalid("DiscoveryGroups"))?,
            )
        })
    }
}
impl From<prns_core::remote_control::RemoteControlDiscoveryGroups>
    for RemoteControlDiscoveryGroups
{
    fn from(value: prns_core::remote_control::RemoteControlDiscoveryGroups) -> Self {
        Self {
            groups: value
                .groups()
                .iter()
                .map(|group| group.as_str().to_owned())
                .collect(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPeerPage {
    First,
    After { value: RemoteControlPeerCursor },
}
impl TryFrom<RemoteControlPeerPage> for prns_core::remote_control::RemoteControlPeerPage {
    type Error = BindingError;
    fn try_from(value: RemoteControlPeerPage) -> Result<Self, Self::Error> {
        Ok(match value {
            RemoteControlPeerPage::First => prns_core::remote_control::RemoteControlPeerPage::First,
            RemoteControlPeerPage::After { value } => {
                prns_core::remote_control::RemoteControlPeerPage::After(value.try_into()?)
            }
        })
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlPeerCursor {
    pub value: RemoteControlInterfaceId,
}
impl TryFrom<RemoteControlPeerCursor> for prns_core::remote_control::RemoteControlPeerCursor {
    type Error = BindingError;
    fn try_from(value: RemoteControlPeerCursor) -> Result<Self, Self::Error> {
        Ok(prns_core::remote_control::RemoteControlPeerCursor::after(
            value.value.try_into()?,
        ))
    }
}
impl From<prns_core::remote_control::RemoteControlPeerCursor> for RemoteControlPeerCursor {
    fn from(value: prns_core::remote_control::RemoteControlPeerCursor) -> Self {
        Self {
            value: value.id().into(),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlLoRaProfile {
    pub value: String,
}
impl TryFrom<RemoteControlLoRaProfile> for prns_core::remote_control::RemoteControlLoRaProfile {
    type Error = BindingError;
    fn try_from(value: RemoteControlLoRaProfile) -> Result<Self, Self::Error> {
        Ok(
            prns_core::remote_control::RemoteControlLoRaProfile::parse(&value.value)
                .ok_or_else(|| invalid("RemoteControlLoRaProfile"))?,
        )
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlWifiStation {
    pub ssid: String,
    pub password: RemoteControlSecretText,
}
impl TryFrom<RemoteControlWifiStation> for prns_core::remote_control::RemoteControlWifiStation {
    type Error = BindingError;
    fn try_from(value: RemoteControlWifiStation) -> Result<Self, Self::Error> {
        Ok(prns_core::remote_control::RemoteControlWifiStation::parse(
            &value.ssid,
            &value.password.0,
        )
        .map_err(|_| invalid("WifiStation"))?)
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlControllerPage {
    First,
    After {
        value: RemoteControlControllerCursor,
    },
}
impl TryFrom<RemoteControlControllerPage>
    for prns_core::remote_control::RemoteControlControllerPage
{
    type Error = BindingError;
    fn try_from(value: RemoteControlControllerPage) -> Result<Self, Self::Error> {
        Ok(match value {
            RemoteControlControllerPage::First => {
                prns_core::remote_control::RemoteControlControllerPage::First
            }
            RemoteControlControllerPage::After { value } => {
                prns_core::remote_control::RemoteControlControllerPage::After(value.try_into()?)
            }
        })
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlControllerCursor {
    pub value: RemoteControlIdentityHash,
}
impl TryFrom<RemoteControlControllerCursor>
    for prns_core::remote_control::RemoteControlControllerCursor
{
    type Error = BindingError;
    fn try_from(value: RemoteControlControllerCursor) -> Result<Self, Self::Error> {
        Ok(
            prns_core::remote_control::RemoteControlControllerCursor::after(
                value.value.try_into()?,
            ),
        )
    }
}
impl From<prns_core::remote_control::RemoteControlControllerCursor>
    for RemoteControlControllerCursor
{
    fn from(value: prns_core::remote_control::RemoteControlControllerCursor) -> Self {
        Self {
            value: value.identity().into(),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlIdentityHash {
    pub value: Vec<u8>,
}
impl TryFrom<RemoteControlIdentityHash> for prns_core::identity::IdentityHash {
    type Error = BindingError;
    fn try_from(value: RemoteControlIdentityHash) -> Result<Self, Self::Error> {
        Ok(prns_core::identity::IdentityHash::new(
            value
                .value
                .try_into()
                .map_err(|_| invalid("IdentityHash"))?,
        ))
    }
}
impl From<prns_core::identity::IdentityHash> for RemoteControlIdentityHash {
    fn from(value: prns_core::identity::IdentityHash) -> Self {
        Self {
            value: value.as_bytes().to_vec(),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlControllerIdentity {
    pub public_keys: Vec<u8>,
}
impl TryFrom<RemoteControlControllerIdentity>
    for prns_core::remote_control::RemoteControlControllerIdentity
{
    type Error = BindingError;
    fn try_from(value: RemoteControlControllerIdentity) -> Result<Self, Self::Error> {
        Ok(
            prns_core::remote_control::RemoteControlControllerIdentity::new(
                prns_core::identity::PublicIdentityMaterial::from_slice(&value.public_keys)
                    .map_err(|_| invalid("publicKeys"))?
                    .public_keys(),
            ),
        )
    }
}
impl From<prns_core::remote_control::RemoteControlControllerIdentity>
    for RemoteControlControllerIdentity
{
    fn from(value: prns_core::remote_control::RemoteControlControllerIdentity) -> Self {
        Self {
            public_keys: value.public_keys().public_key_bytes().to_vec(),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlRequestSet {
    pub kinds: Vec<RemoteControlRequestKind>,
}
impl TryFrom<RemoteControlRequestSet> for prns_core::remote_control::RemoteControlRequestSet {
    type Error = BindingError;
    fn try_from(value: RemoteControlRequestSet) -> Result<Self, Self::Error> {
        Ok({
            let mut set = prns_core::remote_control::RemoteControlRequestSet::empty();
            for kind in value.kinds {
                set.insert(kind.try_into()?);
            }
            set
        })
    }
}
impl From<prns_core::remote_control::RemoteControlRequestSet> for RemoteControlRequestSet {
    fn from(value: prns_core::remote_control::RemoteControlRequestSet) -> Self {
        Self {
            kinds: value.iter().map(Into::into).collect(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlRequestKind {
    Describe,
    AnnounceSelf,
    InventoryInterfaces,
    SetInterfacePower,
    SleepRadios,
    WakeRadios,
    SetInterfaceMode,
    SetInterfaceGroup,
    InventoryInterfacePeers,
    InventoryInterfaceConfig,
    SetInterfaceLoRaProfile,
    DescribeBuild,
    SetInterfaceWifiStation,
    InventoryControllers,
    AuthorizeController,
    RevokeController,
    DescribePower,
    SetSystemPower,
    SetGnssPower,
    SetDisplayVisibility,
    SetDisplayAutoOff,
    SetStationUplink,
    SetEspRadioMode,
    StageWifiCredentials,
    ActivateWifiCredentials,
    ConfirmWifiCredentials,
    CancelWifiCredentials,
    InspectWifiTransaction,
    InventoryInterfaceDiscoveryGroups,
    ReplaceInterfaceDiscoveryGroups,
}
impl TryFrom<RemoteControlRequestKind> for prns_core::remote_control::RemoteControlRequestKind {
    type Error = BindingError;
    fn try_from(value: RemoteControlRequestKind) -> Result<Self, Self::Error> {
        Ok(
match value {
RemoteControlRequestKind::Describe => prns_core::remote_control::RemoteControlRequestKind::Describe,
RemoteControlRequestKind::AnnounceSelf => prns_core::remote_control::RemoteControlRequestKind::AnnounceSelf,
RemoteControlRequestKind::InventoryInterfaces => prns_core::remote_control::RemoteControlRequestKind::InventoryInterfaces,
RemoteControlRequestKind::SetInterfacePower => prns_core::remote_control::RemoteControlRequestKind::SetInterfacePower,
RemoteControlRequestKind::SleepRadios => prns_core::remote_control::RemoteControlRequestKind::SleepRadios,
RemoteControlRequestKind::WakeRadios => prns_core::remote_control::RemoteControlRequestKind::WakeRadios,
RemoteControlRequestKind::SetInterfaceMode => prns_core::remote_control::RemoteControlRequestKind::SetInterfaceMode,
RemoteControlRequestKind::SetInterfaceGroup => prns_core::remote_control::RemoteControlRequestKind::SetInterfaceGroup,
RemoteControlRequestKind::InventoryInterfacePeers => prns_core::remote_control::RemoteControlRequestKind::InventoryInterfacePeers,
RemoteControlRequestKind::InventoryInterfaceConfig => prns_core::remote_control::RemoteControlRequestKind::InventoryInterfaceConfig,
RemoteControlRequestKind::SetInterfaceLoRaProfile => prns_core::remote_control::RemoteControlRequestKind::SetInterfaceLoRaProfile,
RemoteControlRequestKind::DescribeBuild => prns_core::remote_control::RemoteControlRequestKind::DescribeBuild,
RemoteControlRequestKind::SetInterfaceWifiStation => prns_core::remote_control::RemoteControlRequestKind::SetInterfaceWifiStation,
RemoteControlRequestKind::InventoryControllers => prns_core::remote_control::RemoteControlRequestKind::InventoryControllers,
RemoteControlRequestKind::AuthorizeController => prns_core::remote_control::RemoteControlRequestKind::AuthorizeController,
RemoteControlRequestKind::RevokeController => prns_core::remote_control::RemoteControlRequestKind::RevokeController,
RemoteControlRequestKind::DescribePower => prns_core::remote_control::RemoteControlRequestKind::DescribePower,
RemoteControlRequestKind::SetSystemPower => prns_core::remote_control::RemoteControlRequestKind::SetSystemPower,
RemoteControlRequestKind::SetGnssPower => prns_core::remote_control::RemoteControlRequestKind::SetGnssPower,
RemoteControlRequestKind::SetDisplayVisibility => prns_core::remote_control::RemoteControlRequestKind::SetDisplayVisibility,
RemoteControlRequestKind::SetDisplayAutoOff => prns_core::remote_control::RemoteControlRequestKind::SetDisplayAutoOff,
RemoteControlRequestKind::SetStationUplink => prns_core::remote_control::RemoteControlRequestKind::SetStationUplink,
RemoteControlRequestKind::SetEspRadioMode => prns_core::remote_control::RemoteControlRequestKind::SetEspRadioMode,
RemoteControlRequestKind::StageWifiCredentials => prns_core::remote_control::RemoteControlRequestKind::StageWifiCredentials,
RemoteControlRequestKind::ActivateWifiCredentials => prns_core::remote_control::RemoteControlRequestKind::ActivateWifiCredentials,
RemoteControlRequestKind::ConfirmWifiCredentials => prns_core::remote_control::RemoteControlRequestKind::ConfirmWifiCredentials,
RemoteControlRequestKind::CancelWifiCredentials => prns_core::remote_control::RemoteControlRequestKind::CancelWifiCredentials,
RemoteControlRequestKind::InspectWifiTransaction => prns_core::remote_control::RemoteControlRequestKind::InspectWifiTransaction,
RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups => prns_core::remote_control::RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups,
RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups => prns_core::remote_control::RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups,
}
)
    }
}
impl From<prns_core::remote_control::RemoteControlRequestKind> for RemoteControlRequestKind {
    fn from(value: prns_core::remote_control::RemoteControlRequestKind) -> Self {
        match value {
prns_core::remote_control::RemoteControlRequestKind::Describe => RemoteControlRequestKind::Describe,
prns_core::remote_control::RemoteControlRequestKind::AnnounceSelf => RemoteControlRequestKind::AnnounceSelf,
prns_core::remote_control::RemoteControlRequestKind::InventoryInterfaces => RemoteControlRequestKind::InventoryInterfaces,
prns_core::remote_control::RemoteControlRequestKind::SetInterfacePower => RemoteControlRequestKind::SetInterfacePower,
prns_core::remote_control::RemoteControlRequestKind::SleepRadios => RemoteControlRequestKind::SleepRadios,
prns_core::remote_control::RemoteControlRequestKind::WakeRadios => RemoteControlRequestKind::WakeRadios,
prns_core::remote_control::RemoteControlRequestKind::SetInterfaceMode => RemoteControlRequestKind::SetInterfaceMode,
prns_core::remote_control::RemoteControlRequestKind::SetInterfaceGroup => RemoteControlRequestKind::SetInterfaceGroup,
prns_core::remote_control::RemoteControlRequestKind::InventoryInterfacePeers => RemoteControlRequestKind::InventoryInterfacePeers,
prns_core::remote_control::RemoteControlRequestKind::InventoryInterfaceConfig => RemoteControlRequestKind::InventoryInterfaceConfig,
prns_core::remote_control::RemoteControlRequestKind::SetInterfaceLoRaProfile => RemoteControlRequestKind::SetInterfaceLoRaProfile,
prns_core::remote_control::RemoteControlRequestKind::DescribeBuild => RemoteControlRequestKind::DescribeBuild,
prns_core::remote_control::RemoteControlRequestKind::SetInterfaceWifiStation => RemoteControlRequestKind::SetInterfaceWifiStation,
prns_core::remote_control::RemoteControlRequestKind::InventoryControllers => RemoteControlRequestKind::InventoryControllers,
prns_core::remote_control::RemoteControlRequestKind::AuthorizeController => RemoteControlRequestKind::AuthorizeController,
prns_core::remote_control::RemoteControlRequestKind::RevokeController => RemoteControlRequestKind::RevokeController,
prns_core::remote_control::RemoteControlRequestKind::DescribePower => RemoteControlRequestKind::DescribePower,
prns_core::remote_control::RemoteControlRequestKind::SetSystemPower => RemoteControlRequestKind::SetSystemPower,
prns_core::remote_control::RemoteControlRequestKind::SetGnssPower => RemoteControlRequestKind::SetGnssPower,
prns_core::remote_control::RemoteControlRequestKind::SetDisplayVisibility => RemoteControlRequestKind::SetDisplayVisibility,
prns_core::remote_control::RemoteControlRequestKind::SetDisplayAutoOff => RemoteControlRequestKind::SetDisplayAutoOff,
prns_core::remote_control::RemoteControlRequestKind::SetStationUplink => RemoteControlRequestKind::SetStationUplink,
prns_core::remote_control::RemoteControlRequestKind::SetEspRadioMode => RemoteControlRequestKind::SetEspRadioMode,
prns_core::remote_control::RemoteControlRequestKind::StageWifiCredentials => RemoteControlRequestKind::StageWifiCredentials,
prns_core::remote_control::RemoteControlRequestKind::ActivateWifiCredentials => RemoteControlRequestKind::ActivateWifiCredentials,
prns_core::remote_control::RemoteControlRequestKind::ConfirmWifiCredentials => RemoteControlRequestKind::ConfirmWifiCredentials,
prns_core::remote_control::RemoteControlRequestKind::CancelWifiCredentials => RemoteControlRequestKind::CancelWifiCredentials,
prns_core::remote_control::RemoteControlRequestKind::InspectWifiTransaction => RemoteControlRequestKind::InspectWifiTransaction,
prns_core::remote_control::RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups => RemoteControlRequestKind::InventoryInterfaceDiscoveryGroups,
prns_core::remote_control::RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups => RemoteControlRequestKind::ReplaceInterfaceDiscoveryGroups,
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlSystemPower {
    Awake,
    Asleep,
}
impl TryFrom<RemoteControlSystemPower> for prns_core::remote_control::RemoteControlSystemPower {
    type Error = BindingError;
    fn try_from(value: RemoteControlSystemPower) -> Result<Self, Self::Error> {
        Ok(match value {
            RemoteControlSystemPower::Awake => {
                prns_core::remote_control::RemoteControlSystemPower::Awake
            }
            RemoteControlSystemPower::Asleep => {
                prns_core::remote_control::RemoteControlSystemPower::Asleep
            }
        })
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlGnssPower {
    Off,
    On,
}
impl TryFrom<RemoteControlGnssPower> for prns_core::remote_control::RemoteControlGnssPower {
    type Error = BindingError;
    fn try_from(value: RemoteControlGnssPower) -> Result<Self, Self::Error> {
        Ok(match value {
            RemoteControlGnssPower::Off => prns_core::remote_control::RemoteControlGnssPower::Off,
            RemoteControlGnssPower::On => prns_core::remote_control::RemoteControlGnssPower::On,
        })
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlDisplayVisibility {
    Hidden,
    Visible,
}
impl TryFrom<RemoteControlDisplayVisibility>
    for prns_core::remote_control::RemoteControlDisplayVisibility
{
    type Error = BindingError;
    fn try_from(value: RemoteControlDisplayVisibility) -> Result<Self, Self::Error> {
        Ok(match value {
            RemoteControlDisplayVisibility::Hidden => {
                prns_core::remote_control::RemoteControlDisplayVisibility::Hidden
            }
            RemoteControlDisplayVisibility::Visible => {
                prns_core::remote_control::RemoteControlDisplayVisibility::Visible
            }
        })
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlDisplayAutoOff {
    Disabled,
    Enabled,
}
impl TryFrom<RemoteControlDisplayAutoOff>
    for prns_core::remote_control::RemoteControlDisplayAutoOff
{
    type Error = BindingError;
    fn try_from(value: RemoteControlDisplayAutoOff) -> Result<Self, Self::Error> {
        Ok(match value {
            RemoteControlDisplayAutoOff::Disabled => {
                prns_core::remote_control::RemoteControlDisplayAutoOff::Disabled
            }
            RemoteControlDisplayAutoOff::Enabled => {
                prns_core::remote_control::RemoteControlDisplayAutoOff::Enabled
            }
        })
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlStationUplink {
    Disabled,
    Enabled,
}
impl TryFrom<RemoteControlStationUplink> for prns_core::remote_control::RemoteControlStationUplink {
    type Error = BindingError;
    fn try_from(value: RemoteControlStationUplink) -> Result<Self, Self::Error> {
        Ok(match value {
            RemoteControlStationUplink::Disabled => {
                prns_core::remote_control::RemoteControlStationUplink::Disabled
            }
            RemoteControlStationUplink::Enabled => {
                prns_core::remote_control::RemoteControlStationUplink::Enabled
            }
        })
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlEspRadioMode {
    Bluetooth,
    AccessPoint,
}
impl TryFrom<RemoteControlEspRadioMode> for prns_core::remote_control::RemoteControlEspRadioMode {
    type Error = BindingError;
    fn try_from(value: RemoteControlEspRadioMode) -> Result<Self, Self::Error> {
        Ok(match value {
            RemoteControlEspRadioMode::Bluetooth => {
                prns_core::remote_control::RemoteControlEspRadioMode::Bluetooth
            }
            RemoteControlEspRadioMode::AccessPoint => {
                prns_core::remote_control::RemoteControlEspRadioMode::AccessPoint
            }
        })
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlWifiCredentialRevision {
    pub value: u32,
}
impl TryFrom<RemoteControlWifiCredentialRevision>
    for prns_core::remote_control::RemoteControlWifiCredentialRevision
{
    type Error = BindingError;
    fn try_from(value: RemoteControlWifiCredentialRevision) -> Result<Self, Self::Error> {
        Ok(
            prns_core::remote_control::RemoteControlWifiCredentialRevision::new(value.value)
                .ok_or_else(|| invalid("RemoteControlWifiCredentialRevision"))?,
        )
    }
}
impl From<prns_core::remote_control::RemoteControlWifiCredentialRevision>
    for RemoteControlWifiCredentialRevision
{
    fn from(value: prns_core::remote_control::RemoteControlWifiCredentialRevision) -> Self {
        Self { value: value.get() }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlResponse {
    Describe {
        value: RemoteControlDescription,
    },
    AnnounceSelf {
        value: RemoteControlAnnounceSelfOutcome,
    },
    InventoryInterfaces {
        value: RemoteControlInterfaceInventory,
    },
    SetInterfacePower {
        value: RemoteControlPowerOutcome,
    },
    SetInterfaceMode {
        value: RemoteControlModeOutcome,
    },
    SetInterfaceGroup {
        value: RemoteControlGroupOutcome,
    },
    InventoryInterfaceDiscoveryGroups {
        value: RemoteControlDiscoveryGroupsInventoryOutcome,
    },
    ReplaceInterfaceDiscoveryGroups {
        value: RemoteControlDiscoveryGroupsReplaceOutcome,
    },
    InventoryInterfacePeers {
        value: RemoteControlInterfacePeersOutcome,
    },
    InventoryInterfaceConfig {
        value: RemoteControlInterfaceConfigOutcome,
    },
    SetInterfaceLoRaProfile {
        value: RemoteControlLoRaOutcome,
    },
    SetInterfaceWifiStation {
        value: RemoteControlWifiStationOutcome,
    },
    InventoryControllers {
        value: RemoteControlControllerInventory,
    },
    AuthorizeController {
        value: RemoteControlAuthorizeControllerOutcome,
    },
    RevokeController {
        value: RemoteControlRevokeControllerOutcome,
    },
    DescribeBuild {
        value: RemoteControlBuildVersion,
    },
    DescribePower {
        value: RemoteControlPowerSnapshot,
    },
    SleepRadios {
        value: RemoteControlSleepOutcome,
    },
    WakeRadios {
        value: RemoteControlSleepOutcome,
    },
    SetSystemPower {
        value: RemoteControlApplyOutcome,
    },
    SetGnssPower {
        value: RemoteControlApplyOutcome,
    },
    SetDisplayVisibility {
        value: RemoteControlApplyOutcome,
    },
    SetDisplayAutoOff {
        value: RemoteControlApplyOutcome,
    },
    SetStationUplink {
        value: RemoteControlApplyOutcome,
    },
    SetEspRadioMode {
        value: RemoteControlApplyOutcome,
    },
    StageWifiCredentials {
        value: RemoteControlWifiStageOutcome,
    },
    ActivateWifiCredentials {
        value: RemoteControlApplyOutcome,
    },
    ConfirmWifiCredentials {
        value: RemoteControlApplyOutcome,
    },
    CancelWifiCredentials {
        value: RemoteControlApplyOutcome,
    },
    InspectWifiTransaction {
        value: RemoteControlWifiTransactionStatus,
    },
    ProtocolError {
        value: RemoteControlProtocolError,
    },
}
impl From<prns_core::remote_control::RemoteControlResponse> for RemoteControlResponse {
    fn from(value: prns_core::remote_control::RemoteControlResponse) -> Self {
        match value {
            prns_core::remote_control::RemoteControlResponse::Describe(value) => {
                RemoteControlResponse::Describe {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::AnnounceSelf(value) => {
                RemoteControlResponse::AnnounceSelf {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::InventoryInterfaces(value) => {
                RemoteControlResponse::InventoryInterfaces {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::SetInterfacePower(value) => {
                RemoteControlResponse::SetInterfacePower {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::SetInterfaceMode(value) => {
                RemoteControlResponse::SetInterfaceMode {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::SetInterfaceGroup(value) => {
                RemoteControlResponse::SetInterfaceGroup {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::InventoryInterfaceDiscoveryGroups(
                value,
            ) => RemoteControlResponse::InventoryInterfaceDiscoveryGroups {
                value: value.into(),
            },
            prns_core::remote_control::RemoteControlResponse::ReplaceInterfaceDiscoveryGroups(
                value,
            ) => RemoteControlResponse::ReplaceInterfaceDiscoveryGroups {
                value: value.into(),
            },
            prns_core::remote_control::RemoteControlResponse::InventoryInterfacePeers(value) => {
                RemoteControlResponse::InventoryInterfacePeers {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::InventoryInterfaceConfig(value) => {
                RemoteControlResponse::InventoryInterfaceConfig {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::SetInterfaceLoRaProfile(value) => {
                RemoteControlResponse::SetInterfaceLoRaProfile {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::SetInterfaceWifiStation(value) => {
                RemoteControlResponse::SetInterfaceWifiStation {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::InventoryControllers(value) => {
                RemoteControlResponse::InventoryControllers {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::AuthorizeController(value) => {
                RemoteControlResponse::AuthorizeController {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::RevokeController(value) => {
                RemoteControlResponse::RevokeController {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::DescribeBuild(value) => {
                RemoteControlResponse::DescribeBuild {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::DescribePower(value) => {
                RemoteControlResponse::DescribePower {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::SleepRadios(value) => {
                RemoteControlResponse::SleepRadios {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::WakeRadios(value) => {
                RemoteControlResponse::WakeRadios {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::SetSystemPower(value) => {
                RemoteControlResponse::SetSystemPower {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::SetGnssPower(value) => {
                RemoteControlResponse::SetGnssPower {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::SetDisplayVisibility(value) => {
                RemoteControlResponse::SetDisplayVisibility {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::SetDisplayAutoOff(value) => {
                RemoteControlResponse::SetDisplayAutoOff {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::SetStationUplink(value) => {
                RemoteControlResponse::SetStationUplink {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::SetEspRadioMode(value) => {
                RemoteControlResponse::SetEspRadioMode {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::StageWifiCredentials(value) => {
                RemoteControlResponse::StageWifiCredentials {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::ActivateWifiCredentials(value) => {
                RemoteControlResponse::ActivateWifiCredentials {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::ConfirmWifiCredentials(value) => {
                RemoteControlResponse::ConfirmWifiCredentials {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::CancelWifiCredentials(value) => {
                RemoteControlResponse::CancelWifiCredentials {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::InspectWifiTransaction(value) => {
                RemoteControlResponse::InspectWifiTransaction {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlResponse::ProtocolError(value) => {
                RemoteControlResponse::ProtocolError {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlDescription {
    pub available_requests: RemoteControlRequestSet,
}
impl From<prns_core::remote_control::RemoteControlDescription> for RemoteControlDescription {
    fn from(value: prns_core::remote_control::RemoteControlDescription) -> Self {
        Self {
            available_requests: (*value.available_requests()).into(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlAnnounceSelfOutcome {
    Announced,
    Unavailable,
    Rejected,
    WriteFailed,
}
impl From<prns_core::remote_control::RemoteControlAnnounceSelfOutcome>
    for RemoteControlAnnounceSelfOutcome
{
    fn from(value: prns_core::remote_control::RemoteControlAnnounceSelfOutcome) -> Self {
        match value {
            prns_core::remote_control::RemoteControlAnnounceSelfOutcome::Announced => {
                RemoteControlAnnounceSelfOutcome::Announced
            }
            prns_core::remote_control::RemoteControlAnnounceSelfOutcome::Unavailable => {
                RemoteControlAnnounceSelfOutcome::Unavailable
            }
            prns_core::remote_control::RemoteControlAnnounceSelfOutcome::Rejected => {
                RemoteControlAnnounceSelfOutcome::Rejected
            }
            prns_core::remote_control::RemoteControlAnnounceSelfOutcome::WriteFailed => {
                RemoteControlAnnounceSelfOutcome::WriteFailed
            }
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlInterfaceInventory {
    pub entries: Vec<RemoteControlInterfaceEntry>,
    pub continuation: RemoteControlInterfaceContinuation,
}
impl From<prns_core::remote_control::RemoteControlInterfaceInventory>
    for RemoteControlInterfaceInventory
{
    fn from(value: prns_core::remote_control::RemoteControlInterfaceInventory) -> Self {
        Self {
            entries: value
                .entries()
                .iter()
                .cloned()
                .map(|item| item.into())
                .collect(),
            continuation: value.continuation().into(),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlInterfaceEntry {
    pub id: RemoteControlInterfaceId,
    pub kind: RemoteControlInterfaceKind,
    pub mode: RemoteControlInterfaceMode,
    pub connection: RemoteControlConnectionState,
    pub enabled: bool,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub links: u32,
    pub rate_bytes_per_sec: u32,
}
impl From<prns_core::remote_control::RemoteControlInterfaceEntry> for RemoteControlInterfaceEntry {
    fn from(value: prns_core::remote_control::RemoteControlInterfaceEntry) -> Self {
        Self {
            id: value.id.into(),
            kind: value.kind.into(),
            mode: value.mode.into(),
            connection: value.connection.into(),
            enabled: value.enabled,
            tx_bytes: value.tx_bytes,
            rx_bytes: value.rx_bytes,
            links: value.links,
            rate_bytes_per_sec: value.rate_bytes_per_sec,
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlInterfaceKind {
    Loopback,
    TcpClient,
    TcpServer,
    Udp,
    Serial,
    UsbAutoHost,
    UsbAutoDevice,
    AutoWifi,
    WifiPeer,
    LocalServer,
    LocalClient,
    TcpServerPeer,
    BluetoothAuto,
    BluetoothPeer,
    LoRa,
    Kiss,
    Ax25Kiss,
    Pipe,
    Rnode,
    BackboneServer,
    BackboneServerPeer,
    BackboneClient,
    EspNow,
    WebSocketClient,
    WebSocketServer,
    WebSocketServerPeer,
    WifiDirect,
    WifiDirectPeer,
    WifiAware,
    WifiAwarePeer,
    I2p,
    I2pPeer,
    Weave,
    WeavePeer,
}
impl From<prns_core::interfaces::InterfaceKind> for RemoteControlInterfaceKind {
    fn from(value: prns_core::interfaces::InterfaceKind) -> Self {
        match value {
            prns_core::interfaces::InterfaceKind::Loopback => RemoteControlInterfaceKind::Loopback,
            prns_core::interfaces::InterfaceKind::TcpClient => {
                RemoteControlInterfaceKind::TcpClient
            }
            prns_core::interfaces::InterfaceKind::TcpServer => {
                RemoteControlInterfaceKind::TcpServer
            }
            prns_core::interfaces::InterfaceKind::Udp => RemoteControlInterfaceKind::Udp,
            prns_core::interfaces::InterfaceKind::Serial => RemoteControlInterfaceKind::Serial,
            prns_core::interfaces::InterfaceKind::UsbAutoHost => {
                RemoteControlInterfaceKind::UsbAutoHost
            }
            prns_core::interfaces::InterfaceKind::UsbAutoDevice => {
                RemoteControlInterfaceKind::UsbAutoDevice
            }
            prns_core::interfaces::InterfaceKind::AutoWifi => RemoteControlInterfaceKind::AutoWifi,
            prns_core::interfaces::InterfaceKind::WifiPeer => RemoteControlInterfaceKind::WifiPeer,
            prns_core::interfaces::InterfaceKind::LocalServer => {
                RemoteControlInterfaceKind::LocalServer
            }
            prns_core::interfaces::InterfaceKind::LocalClient => {
                RemoteControlInterfaceKind::LocalClient
            }
            prns_core::interfaces::InterfaceKind::TcpServerPeer => {
                RemoteControlInterfaceKind::TcpServerPeer
            }
            prns_core::interfaces::InterfaceKind::BluetoothAuto => {
                RemoteControlInterfaceKind::BluetoothAuto
            }
            prns_core::interfaces::InterfaceKind::BluetoothPeer => {
                RemoteControlInterfaceKind::BluetoothPeer
            }
            prns_core::interfaces::InterfaceKind::LoRa => RemoteControlInterfaceKind::LoRa,
            prns_core::interfaces::InterfaceKind::Kiss => RemoteControlInterfaceKind::Kiss,
            prns_core::interfaces::InterfaceKind::Ax25Kiss => RemoteControlInterfaceKind::Ax25Kiss,
            prns_core::interfaces::InterfaceKind::Pipe => RemoteControlInterfaceKind::Pipe,
            prns_core::interfaces::InterfaceKind::Rnode => RemoteControlInterfaceKind::Rnode,
            prns_core::interfaces::InterfaceKind::BackboneServer => {
                RemoteControlInterfaceKind::BackboneServer
            }
            prns_core::interfaces::InterfaceKind::BackboneServerPeer => {
                RemoteControlInterfaceKind::BackboneServerPeer
            }
            prns_core::interfaces::InterfaceKind::BackboneClient => {
                RemoteControlInterfaceKind::BackboneClient
            }
            prns_core::interfaces::InterfaceKind::EspNow => RemoteControlInterfaceKind::EspNow,
            prns_core::interfaces::InterfaceKind::WebSocketClient => {
                RemoteControlInterfaceKind::WebSocketClient
            }
            prns_core::interfaces::InterfaceKind::WebSocketServer => {
                RemoteControlInterfaceKind::WebSocketServer
            }
            prns_core::interfaces::InterfaceKind::WebSocketServerPeer => {
                RemoteControlInterfaceKind::WebSocketServerPeer
            }
            prns_core::interfaces::InterfaceKind::WifiDirect => {
                RemoteControlInterfaceKind::WifiDirect
            }
            prns_core::interfaces::InterfaceKind::WifiDirectPeer => {
                RemoteControlInterfaceKind::WifiDirectPeer
            }
            prns_core::interfaces::InterfaceKind::WifiAware => {
                RemoteControlInterfaceKind::WifiAware
            }
            prns_core::interfaces::InterfaceKind::WifiAwarePeer => {
                RemoteControlInterfaceKind::WifiAwarePeer
            }
            prns_core::interfaces::InterfaceKind::I2p => RemoteControlInterfaceKind::I2p,
            prns_core::interfaces::InterfaceKind::I2pPeer => RemoteControlInterfaceKind::I2pPeer,
            prns_core::interfaces::InterfaceKind::Weave => RemoteControlInterfaceKind::Weave,
            prns_core::interfaces::InterfaceKind::WeavePeer => {
                RemoteControlInterfaceKind::WeavePeer
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlConnectionState {
    Initializing,
    Connected,
    Degraded,
    Reconnecting,
    Failed,
    Disconnected,
    Disabled,
    Unknown,
}
impl From<prns_core::interfaces::ConnectionState> for RemoteControlConnectionState {
    fn from(value: prns_core::interfaces::ConnectionState) -> Self {
        match value {
            prns_core::interfaces::ConnectionState::Initializing => {
                RemoteControlConnectionState::Initializing
            }
            prns_core::interfaces::ConnectionState::Connected => {
                RemoteControlConnectionState::Connected
            }
            prns_core::interfaces::ConnectionState::Degraded => {
                RemoteControlConnectionState::Degraded
            }
            prns_core::interfaces::ConnectionState::Reconnecting => {
                RemoteControlConnectionState::Reconnecting
            }
            prns_core::interfaces::ConnectionState::Failed => RemoteControlConnectionState::Failed,
            prns_core::interfaces::ConnectionState::Disconnected => {
                RemoteControlConnectionState::Disconnected
            }
            prns_core::interfaces::ConnectionState::Disabled => {
                RemoteControlConnectionState::Disabled
            }
            prns_core::interfaces::ConnectionState::Unknown => {
                RemoteControlConnectionState::Unknown
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlInterfaceContinuation {
    Complete,
    More { value: RemoteControlInterfaceCursor },
}
impl From<prns_core::remote_control::RemoteControlInterfaceContinuation>
    for RemoteControlInterfaceContinuation
{
    fn from(value: prns_core::remote_control::RemoteControlInterfaceContinuation) -> Self {
        match value {
            prns_core::remote_control::RemoteControlInterfaceContinuation::Complete => {
                RemoteControlInterfaceContinuation::Complete
            }
            prns_core::remote_control::RemoteControlInterfaceContinuation::More(value) => {
                RemoteControlInterfaceContinuation::More {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPowerOutcome {
    Applied,
    UnknownInterface,
    Failed,
    Unchanged,
    Scheduled,
}
impl From<prns_core::remote_control::RemoteControlPowerOutcome> for RemoteControlPowerOutcome {
    fn from(value: prns_core::remote_control::RemoteControlPowerOutcome) -> Self {
        match value {
            prns_core::remote_control::RemoteControlPowerOutcome::Applied => {
                RemoteControlPowerOutcome::Applied
            }
            prns_core::remote_control::RemoteControlPowerOutcome::UnknownInterface => {
                RemoteControlPowerOutcome::UnknownInterface
            }
            prns_core::remote_control::RemoteControlPowerOutcome::Failed => {
                RemoteControlPowerOutcome::Failed
            }
            prns_core::remote_control::RemoteControlPowerOutcome::Unchanged => {
                RemoteControlPowerOutcome::Unchanged
            }
            prns_core::remote_control::RemoteControlPowerOutcome::Scheduled => {
                RemoteControlPowerOutcome::Scheduled
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlModeOutcome {
    Applied,
    UnknownInterface,
    Failed,
}
impl From<prns_core::remote_control::RemoteControlModeOutcome> for RemoteControlModeOutcome {
    fn from(value: prns_core::remote_control::RemoteControlModeOutcome) -> Self {
        match value {
            prns_core::remote_control::RemoteControlModeOutcome::Applied => {
                RemoteControlModeOutcome::Applied
            }
            prns_core::remote_control::RemoteControlModeOutcome::UnknownInterface => {
                RemoteControlModeOutcome::UnknownInterface
            }
            prns_core::remote_control::RemoteControlModeOutcome::Failed => {
                RemoteControlModeOutcome::Failed
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlGroupOutcome {
    Applied,
    UnknownInterface,
    Failed,
}
impl From<prns_core::remote_control::RemoteControlGroupOutcome> for RemoteControlGroupOutcome {
    fn from(value: prns_core::remote_control::RemoteControlGroupOutcome) -> Self {
        match value {
            prns_core::remote_control::RemoteControlGroupOutcome::Applied => {
                RemoteControlGroupOutcome::Applied
            }
            prns_core::remote_control::RemoteControlGroupOutcome::UnknownInterface => {
                RemoteControlGroupOutcome::UnknownInterface
            }
            prns_core::remote_control::RemoteControlGroupOutcome::Failed => {
                RemoteControlGroupOutcome::Failed
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlDiscoveryGroupsInventoryOutcome {
    Groups { value: RemoteControlDiscoveryGroups },
    UnknownInterface,
    Unsupported,
}
impl From<prns_core::remote_control::RemoteControlDiscoveryGroupsInventoryOutcome>
    for RemoteControlDiscoveryGroupsInventoryOutcome
{
    fn from(
        value: prns_core::remote_control::RemoteControlDiscoveryGroupsInventoryOutcome,
    ) -> Self {
        match value {
prns_core::remote_control::RemoteControlDiscoveryGroupsInventoryOutcome::Groups(value) => RemoteControlDiscoveryGroupsInventoryOutcome::Groups { value: value.into() },
prns_core::remote_control::RemoteControlDiscoveryGroupsInventoryOutcome::UnknownInterface => RemoteControlDiscoveryGroupsInventoryOutcome::UnknownInterface,
prns_core::remote_control::RemoteControlDiscoveryGroupsInventoryOutcome::Unsupported => RemoteControlDiscoveryGroupsInventoryOutcome::Unsupported,
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlDiscoveryGroupsReplaceOutcome {
    Applied,
    Unchanged,
    UnknownInterface,
    Unsupported,
}
impl From<prns_core::remote_control::RemoteControlDiscoveryGroupsReplaceOutcome>
    for RemoteControlDiscoveryGroupsReplaceOutcome
{
    fn from(value: prns_core::remote_control::RemoteControlDiscoveryGroupsReplaceOutcome) -> Self {
        match value {
prns_core::remote_control::RemoteControlDiscoveryGroupsReplaceOutcome::Applied => RemoteControlDiscoveryGroupsReplaceOutcome::Applied,
prns_core::remote_control::RemoteControlDiscoveryGroupsReplaceOutcome::Unchanged => RemoteControlDiscoveryGroupsReplaceOutcome::Unchanged,
prns_core::remote_control::RemoteControlDiscoveryGroupsReplaceOutcome::UnknownInterface => RemoteControlDiscoveryGroupsReplaceOutcome::UnknownInterface,
prns_core::remote_control::RemoteControlDiscoveryGroupsReplaceOutcome::Unsupported => RemoteControlDiscoveryGroupsReplaceOutcome::Unsupported,
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlInterfacePeersOutcome {
    Page {
        value: RemoteControlInterfacePeerPage,
    },
    UnknownInterface,
}
impl From<prns_core::remote_control::RemoteControlInterfacePeersOutcome>
    for RemoteControlInterfacePeersOutcome
{
    fn from(value: prns_core::remote_control::RemoteControlInterfacePeersOutcome) -> Self {
        match value {
            prns_core::remote_control::RemoteControlInterfacePeersOutcome::Page(value) => {
                RemoteControlInterfacePeersOutcome::Page {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlInterfacePeersOutcome::UnknownInterface => {
                RemoteControlInterfacePeersOutcome::UnknownInterface
            }
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlInterfacePeerPage {
    pub id: RemoteControlInterfaceId,
    pub peers: Vec<RemoteControlInterfacePeer>,
    pub continuation: RemoteControlPeerContinuation,
}
impl From<prns_core::remote_control::RemoteControlInterfacePeerPage>
    for RemoteControlInterfacePeerPage
{
    fn from(value: prns_core::remote_control::RemoteControlInterfacePeerPage) -> Self {
        Self {
            id: value.id.into(),
            peers: value
                .peers
                .iter()
                .cloned()
                .map(|item| item.into())
                .collect(),
            continuation: value.continuation().into(),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlInterfacePeer {
    pub id: RemoteControlInterfaceId,
    pub connection: RemoteControlConnectionState,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub links: u32,
    pub destinations: u32,
    pub rate_bytes_per_sec: u32,
    pub radio: RemoteControlRadioIndication,
    pub details: RemoteControlPeerDetails,
}
impl From<prns_core::remote_control::RemoteControlInterfacePeer> for RemoteControlInterfacePeer {
    fn from(value: prns_core::remote_control::RemoteControlInterfacePeer) -> Self {
        Self {
            id: value.id.into(),
            connection: value.connection.into(),
            tx_bytes: value.tx_bytes,
            rx_bytes: value.rx_bytes,
            links: value.links,
            destinations: value.destinations,
            rate_bytes_per_sec: value.rate_bytes_per_sec,
            radio: value.radio.into(),
            details: value.details.into(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlRadioIndication {
    NotRadio,
    Bluetooth {
        value: RemoteControlBluetoothIndication,
    },
    Wifi {
        value: RemoteControlWifiIndication,
    },
    LoRa {
        value: RemoteControlLoRaIndication,
    },
}
impl From<prns_core::interfaces::RadioIndication> for RemoteControlRadioIndication {
    fn from(value: prns_core::interfaces::RadioIndication) -> Self {
        match value {
            prns_core::interfaces::RadioIndication::NotRadio => {
                RemoteControlRadioIndication::NotRadio
            }
            prns_core::interfaces::RadioIndication::Bluetooth(value) => {
                RemoteControlRadioIndication::Bluetooth {
                    value: value.into(),
                }
            }
            prns_core::interfaces::RadioIndication::Wifi(value) => {
                RemoteControlRadioIndication::Wifi {
                    value: value.into(),
                }
            }
            prns_core::interfaces::RadioIndication::LoRa(value) => {
                RemoteControlRadioIndication::LoRa {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlBluetoothIndication {
    Pending,
    Rssi { value: RemoteControlRssiDbm },
}
impl From<prns_core::interfaces::BluetoothIndication> for RemoteControlBluetoothIndication {
    fn from(value: prns_core::interfaces::BluetoothIndication) -> Self {
        match value {
            prns_core::interfaces::BluetoothIndication::Pending => {
                RemoteControlBluetoothIndication::Pending
            }
            prns_core::interfaces::BluetoothIndication::Rssi(value) => {
                RemoteControlBluetoothIndication::Rssi {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlRssiDbm {
    pub value: i16,
}
impl From<prns_core::interfaces::RssiDbm> for RemoteControlRssiDbm {
    fn from(value: prns_core::interfaces::RssiDbm) -> Self {
        Self { value: value.get() }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlWifiIndication {
    Unavailable,
    Pending,
    Rssi { value: RemoteControlRssiDbm },
}
impl From<prns_core::interfaces::WifiIndication> for RemoteControlWifiIndication {
    fn from(value: prns_core::interfaces::WifiIndication) -> Self {
        match value {
            prns_core::interfaces::WifiIndication::Unavailable => {
                RemoteControlWifiIndication::Unavailable
            }
            prns_core::interfaces::WifiIndication::Pending => RemoteControlWifiIndication::Pending,
            prns_core::interfaces::WifiIndication::Rssi(value) => {
                RemoteControlWifiIndication::Rssi {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlLoRaIndication {
    Pending,
    Sample {
        rssi: RemoteControlRssiDbm,
        snr: Option<RemoteControlSnrQuarterDb>,
        quality: Option<RemoteControlSignalQualityTenthsPercent>,
    },
}
impl From<prns_core::interfaces::LoRaIndication> for RemoteControlLoRaIndication {
    fn from(value: prns_core::interfaces::LoRaIndication) -> Self {
        match value {
            prns_core::interfaces::LoRaIndication::Pending => RemoteControlLoRaIndication::Pending,
            prns_core::interfaces::LoRaIndication::Sample { rssi, snr, quality } => {
                RemoteControlLoRaIndication::Sample {
                    rssi: rssi.into(),
                    snr: snr.map(|item| item.into()),
                    quality: quality.map(|item| item.into()),
                }
            }
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlSnrQuarterDb {
    pub value: i16,
}
impl From<prns_core::interfaces::SnrQuarterDb> for RemoteControlSnrQuarterDb {
    fn from(value: prns_core::interfaces::SnrQuarterDb) -> Self {
        Self {
            value: value.quarters(),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlSignalQualityTenthsPercent {
    pub value: u16,
}
impl From<prns_core::interfaces::SignalQualityTenthsPercent>
    for RemoteControlSignalQualityTenthsPercent
{
    fn from(value: prns_core::interfaces::SignalQualityTenthsPercent) -> Self {
        Self {
            value: value.tenths_percent(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPeerDetails {
    NotApplicable,
    Unknown,
    BleGatt,
    BleCoc,
    WifiRfChannel { value: u8 },
}
impl From<prns_core::interfaces::PeerDetails> for RemoteControlPeerDetails {
    fn from(value: prns_core::interfaces::PeerDetails) -> Self {
        match value {
            prns_core::interfaces::PeerDetails::NotApplicable => {
                RemoteControlPeerDetails::NotApplicable
            }
            prns_core::interfaces::PeerDetails::Unknown => RemoteControlPeerDetails::Unknown,
            prns_core::interfaces::PeerDetails::BleGatt => RemoteControlPeerDetails::BleGatt,
            prns_core::interfaces::PeerDetails::BleCoc => RemoteControlPeerDetails::BleCoc,
            prns_core::interfaces::PeerDetails::WifiRfChannel(value) => {
                RemoteControlPeerDetails::WifiRfChannel { value }
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPeerContinuation {
    Complete,
    More { value: RemoteControlPeerCursor },
}
impl From<prns_core::remote_control::RemoteControlPeerContinuation>
    for RemoteControlPeerContinuation
{
    fn from(value: prns_core::remote_control::RemoteControlPeerContinuation) -> Self {
        match value {
            prns_core::remote_control::RemoteControlPeerContinuation::Complete => {
                RemoteControlPeerContinuation::Complete
            }
            prns_core::remote_control::RemoteControlPeerContinuation::More(value) => {
                RemoteControlPeerContinuation::More {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlInterfaceConfigOutcome {
    Card { value: RemoteControlInterfaceCard },
    UnknownInterface,
}
impl From<prns_core::remote_control::RemoteControlInterfaceConfigOutcome>
    for RemoteControlInterfaceConfigOutcome
{
    fn from(value: prns_core::remote_control::RemoteControlInterfaceConfigOutcome) -> Self {
        match value {
            prns_core::remote_control::RemoteControlInterfaceConfigOutcome::Card(value) => {
                RemoteControlInterfaceConfigOutcome::Card {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlInterfaceConfigOutcome::UnknownInterface => {
                RemoteControlInterfaceConfigOutcome::UnknownInterface
            }
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlInterfaceCard {
    pub name: String,
    pub group: String,
    pub config: String,
    pub failure: String,
    pub destinations: u32,
    pub transported_links: u32,
}
impl From<prns_core::remote_control::RemoteControlInterfaceCard> for RemoteControlInterfaceCard {
    fn from(value: prns_core::remote_control::RemoteControlInterfaceCard) -> Self {
        Self {
            name: value.name.to_string(),
            group: value.group.to_string(),
            config: value.config.to_string(),
            failure: value.failure.to_string(),
            destinations: value.destinations,
            transported_links: value.transported_links,
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlLoRaOutcome {
    Applied,
    UnknownInterface,
    Failed,
}
impl From<prns_core::remote_control::RemoteControlLoRaOutcome> for RemoteControlLoRaOutcome {
    fn from(value: prns_core::remote_control::RemoteControlLoRaOutcome) -> Self {
        match value {
            prns_core::remote_control::RemoteControlLoRaOutcome::Applied => {
                RemoteControlLoRaOutcome::Applied
            }
            prns_core::remote_control::RemoteControlLoRaOutcome::UnknownInterface => {
                RemoteControlLoRaOutcome::UnknownInterface
            }
            prns_core::remote_control::RemoteControlLoRaOutcome::Failed => {
                RemoteControlLoRaOutcome::Failed
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlWifiStationOutcome {
    Applied,
    UnknownInterface,
    Failed,
}
impl From<prns_core::remote_control::RemoteControlWifiStationOutcome>
    for RemoteControlWifiStationOutcome
{
    fn from(value: prns_core::remote_control::RemoteControlWifiStationOutcome) -> Self {
        match value {
            prns_core::remote_control::RemoteControlWifiStationOutcome::Applied => {
                RemoteControlWifiStationOutcome::Applied
            }
            prns_core::remote_control::RemoteControlWifiStationOutcome::UnknownInterface => {
                RemoteControlWifiStationOutcome::UnknownInterface
            }
            prns_core::remote_control::RemoteControlWifiStationOutcome::Failed => {
                RemoteControlWifiStationOutcome::Failed
            }
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlControllerInventory {
    pub hashes: Vec<RemoteControlIdentityHash>,
    pub continuation: RemoteControlControllerContinuation,
}
impl From<prns_core::remote_control::RemoteControlControllerInventory>
    for RemoteControlControllerInventory
{
    fn from(value: prns_core::remote_control::RemoteControlControllerInventory) -> Self {
        Self {
            hashes: value
                .hashes()
                .iter()
                .cloned()
                .map(|item| item.into())
                .collect(),
            continuation: value.continuation().into(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlControllerContinuation {
    Complete,
    More {
        value: RemoteControlControllerCursor,
    },
}
impl From<prns_core::remote_control::RemoteControlControllerContinuation>
    for RemoteControlControllerContinuation
{
    fn from(value: prns_core::remote_control::RemoteControlControllerContinuation) -> Self {
        match value {
            prns_core::remote_control::RemoteControlControllerContinuation::Complete => {
                RemoteControlControllerContinuation::Complete
            }
            prns_core::remote_control::RemoteControlControllerContinuation::More(value) => {
                RemoteControlControllerContinuation::More {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlAuthorizeControllerOutcome {
    Applied,
    CapacityExhausted,
    Failed,
    Forbidden,
    Busy,
}
impl From<prns_core::remote_control::RemoteControlAuthorizeControllerOutcome>
    for RemoteControlAuthorizeControllerOutcome
{
    fn from(value: prns_core::remote_control::RemoteControlAuthorizeControllerOutcome) -> Self {
        match value {
prns_core::remote_control::RemoteControlAuthorizeControllerOutcome::Applied => RemoteControlAuthorizeControllerOutcome::Applied,
prns_core::remote_control::RemoteControlAuthorizeControllerOutcome::CapacityExhausted => RemoteControlAuthorizeControllerOutcome::CapacityExhausted,
prns_core::remote_control::RemoteControlAuthorizeControllerOutcome::Failed => RemoteControlAuthorizeControllerOutcome::Failed,
prns_core::remote_control::RemoteControlAuthorizeControllerOutcome::Forbidden => RemoteControlAuthorizeControllerOutcome::Forbidden,
prns_core::remote_control::RemoteControlAuthorizeControllerOutcome::Busy => RemoteControlAuthorizeControllerOutcome::Busy,
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlRevokeControllerOutcome {
    Applied,
    NotFound,
    Forbidden,
    Failed,
    Busy,
}
impl From<prns_core::remote_control::RemoteControlRevokeControllerOutcome>
    for RemoteControlRevokeControllerOutcome
{
    fn from(value: prns_core::remote_control::RemoteControlRevokeControllerOutcome) -> Self {
        match value {
            prns_core::remote_control::RemoteControlRevokeControllerOutcome::Applied => {
                RemoteControlRevokeControllerOutcome::Applied
            }
            prns_core::remote_control::RemoteControlRevokeControllerOutcome::NotFound => {
                RemoteControlRevokeControllerOutcome::NotFound
            }
            prns_core::remote_control::RemoteControlRevokeControllerOutcome::Forbidden => {
                RemoteControlRevokeControllerOutcome::Forbidden
            }
            prns_core::remote_control::RemoteControlRevokeControllerOutcome::Failed => {
                RemoteControlRevokeControllerOutcome::Failed
            }
            prns_core::remote_control::RemoteControlRevokeControllerOutcome::Busy => {
                RemoteControlRevokeControllerOutcome::Busy
            }
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlBuildVersion {
    pub value: String,
}
impl From<prns_core::remote_control::RemoteControlBuildVersion> for RemoteControlBuildVersion {
    fn from(value: prns_core::remote_control::RemoteControlBuildVersion) -> Self {
        Self {
            value: value.as_str().unwrap_or("").to_owned(),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlPowerSnapshot {
    pub battery: Option<RemoteControlBatteryPercent>,
    pub external_power: RemoteControlExternalPowerState,
}
impl From<prns_core::capabilities::power::PowerSnapshot> for RemoteControlPowerSnapshot {
    fn from(value: prns_core::capabilities::power::PowerSnapshot) -> Self {
        Self {
            battery: value.battery().map(|item| item.into()),
            external_power: value.external_power().into(),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlBatteryPercent {
    pub value: u8,
}
impl From<prns_core::capabilities::power::BatteryPercent> for RemoteControlBatteryPercent {
    fn from(value: prns_core::capabilities::power::BatteryPercent) -> Self {
        Self { value: value.get() }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlExternalPowerState {
    Absent,
    Present {
        charging: RemoteControlChargingState,
    },
    Unknown,
}
impl From<prns_core::capabilities::power::ExternalPowerState> for RemoteControlExternalPowerState {
    fn from(value: prns_core::capabilities::power::ExternalPowerState) -> Self {
        match value {
            prns_core::capabilities::power::ExternalPowerState::Absent => {
                RemoteControlExternalPowerState::Absent
            }
            prns_core::capabilities::power::ExternalPowerState::Present { charging } => {
                RemoteControlExternalPowerState::Present {
                    charging: charging.into(),
                }
            }
            prns_core::capabilities::power::ExternalPowerState::Unknown => {
                RemoteControlExternalPowerState::Unknown
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlChargingState {
    Charging,
    Idle,
    Unknown,
}
impl From<prns_core::capabilities::power::ChargingState> for RemoteControlChargingState {
    fn from(value: prns_core::capabilities::power::ChargingState) -> Self {
        match value {
            prns_core::capabilities::power::ChargingState::Charging => {
                RemoteControlChargingState::Charging
            }
            prns_core::capabilities::power::ChargingState::Idle => RemoteControlChargingState::Idle,
            prns_core::capabilities::power::ChargingState::Unknown => {
                RemoteControlChargingState::Unknown
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlSleepOutcome {
    Applied,
    Unavailable,
    Failed,
}
impl From<prns_core::remote_control::RemoteControlSleepOutcome> for RemoteControlSleepOutcome {
    fn from(value: prns_core::remote_control::RemoteControlSleepOutcome) -> Self {
        match value {
            prns_core::remote_control::RemoteControlSleepOutcome::Applied => {
                RemoteControlSleepOutcome::Applied
            }
            prns_core::remote_control::RemoteControlSleepOutcome::Unavailable => {
                RemoteControlSleepOutcome::Unavailable
            }
            prns_core::remote_control::RemoteControlSleepOutcome::Failed => {
                RemoteControlSleepOutcome::Failed
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlApplyOutcome {
    Applied,
    Unchanged,
    Scheduled,
}
impl From<prns_core::remote_control::RemoteControlApplyOutcome> for RemoteControlApplyOutcome {
    fn from(value: prns_core::remote_control::RemoteControlApplyOutcome) -> Self {
        match value {
            prns_core::remote_control::RemoteControlApplyOutcome::Applied => {
                RemoteControlApplyOutcome::Applied
            }
            prns_core::remote_control::RemoteControlApplyOutcome::Unchanged => {
                RemoteControlApplyOutcome::Unchanged
            }
            prns_core::remote_control::RemoteControlApplyOutcome::Scheduled => {
                RemoteControlApplyOutcome::Scheduled
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlWifiStageOutcome {
    Staged {
        value: RemoteControlWifiCredentialRevision,
    },
    InvalidCredentials,
}
impl From<prns_core::remote_control::RemoteControlWifiStageOutcome>
    for RemoteControlWifiStageOutcome
{
    fn from(value: prns_core::remote_control::RemoteControlWifiStageOutcome) -> Self {
        match value {
            prns_core::remote_control::RemoteControlWifiStageOutcome::Staged(value) => {
                RemoteControlWifiStageOutcome::Staged {
                    value: value.into(),
                }
            }
            prns_core::remote_control::RemoteControlWifiStageOutcome::InvalidCredentials => {
                RemoteControlWifiStageOutcome::InvalidCredentials
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlWifiTransactionStatus {
    FactoryProvisioning,
    Confirmed {
        revision: RemoteControlWifiCredentialRevision,
    },
    Staged {
        revision: RemoteControlWifiCredentialRevision,
    },
    AwaitingConfirmation {
        revision: RemoteControlWifiCredentialRevision,
        remaining: RemoteControlWifiConfirmationRemaining,
    },
    RollingBack {
        rejected_revision: RemoteControlWifiCredentialRevision,
    },
}
impl From<prns_core::remote_control::RemoteControlWifiTransactionStatus>
    for RemoteControlWifiTransactionStatus
{
    fn from(value: prns_core::remote_control::RemoteControlWifiTransactionStatus) -> Self {
        match value {
prns_core::remote_control::RemoteControlWifiTransactionStatus::FactoryProvisioning => RemoteControlWifiTransactionStatus::FactoryProvisioning,
prns_core::remote_control::RemoteControlWifiTransactionStatus::Confirmed { revision } => RemoteControlWifiTransactionStatus::Confirmed { revision: revision.into() },
prns_core::remote_control::RemoteControlWifiTransactionStatus::Staged { revision } => RemoteControlWifiTransactionStatus::Staged { revision: revision.into() },
prns_core::remote_control::RemoteControlWifiTransactionStatus::AwaitingConfirmation { revision, remaining } => RemoteControlWifiTransactionStatus::AwaitingConfirmation { revision: revision.into(), remaining: remaining.into() },
prns_core::remote_control::RemoteControlWifiTransactionStatus::RollingBack { rejected_revision } => RemoteControlWifiTransactionStatus::RollingBack { rejected_revision: rejected_revision.into() },
}
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlWifiConfirmationRemaining {
    pub value: u8,
}
impl From<prns_core::remote_control::RemoteControlWifiConfirmationRemaining>
    for RemoteControlWifiConfirmationRemaining
{
    fn from(value: prns_core::remote_control::RemoteControlWifiConfirmationRemaining) -> Self {
        Self {
            value: value.seconds(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlProtocolError {
    MalformedRequest,
    UnsupportedVersion { found: u8 },
    UnknownRequestKind { found: u8 },
    UnsupportedRequest { request: RemoteControlRequestKind },
    Busy { request: RemoteControlRequestKind },
    ApplyFailed { request: RemoteControlRequestKind },
    PersistenceFailed { request: RemoteControlRequestKind },
    RollbackFailed { request: RemoteControlRequestKind },
    InternalFailure { request: RemoteControlRequestKind },
}
impl From<prns_core::remote_control::RemoteControlProtocolError> for RemoteControlProtocolError {
    fn from(value: prns_core::remote_control::RemoteControlProtocolError) -> Self {
        match value {
            prns_core::remote_control::RemoteControlProtocolError::MalformedRequest => {
                RemoteControlProtocolError::MalformedRequest
            }
            prns_core::remote_control::RemoteControlProtocolError::UnsupportedVersion { found } => {
                RemoteControlProtocolError::UnsupportedVersion { found }
            }
            prns_core::remote_control::RemoteControlProtocolError::UnknownRequestKind { found } => {
                RemoteControlProtocolError::UnknownRequestKind { found }
            }
            prns_core::remote_control::RemoteControlProtocolError::UnsupportedRequest {
                request,
            } => RemoteControlProtocolError::UnsupportedRequest {
                request: request.into(),
            },
            prns_core::remote_control::RemoteControlProtocolError::Busy { request } => {
                RemoteControlProtocolError::Busy {
                    request: request.into(),
                }
            }
            prns_core::remote_control::RemoteControlProtocolError::ApplyFailed { request } => {
                RemoteControlProtocolError::ApplyFailed {
                    request: request.into(),
                }
            }
            prns_core::remote_control::RemoteControlProtocolError::PersistenceFailed {
                request,
            } => RemoteControlProtocolError::PersistenceFailed {
                request: request.into(),
            },
            prns_core::remote_control::RemoteControlProtocolError::RollbackFailed { request } => {
                RemoteControlProtocolError::RollbackFailed {
                    request: request.into(),
                }
            }
            prns_core::remote_control::RemoteControlProtocolError::InternalFailure { request } => {
                RemoteControlProtocolError::InternalFailure {
                    request: request.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlExchangeSettlement {
    Completed {
        response: RemoteControlResponse,
        rtt_millis: u64,
    },
    Failed {
        failure: RemoteControlNativeRemoteControlError,
    },
}
impl From<prns_host_native::remote_control::RemoteControlExchangeSettlement>
    for RemoteControlExchangeSettlement
{
    fn from(value: prns_host_native::remote_control::RemoteControlExchangeSettlement) -> Self {
        match value {
            prns_host_native::remote_control::RemoteControlExchangeSettlement::Completed {
                response,
                rtt_millis,
            } => RemoteControlExchangeSettlement::Completed {
                response: response.into(),
                rtt_millis,
            },
            prns_host_native::remote_control::RemoteControlExchangeSettlement::Failed {
                failure,
            } => RemoteControlExchangeSettlement::Failed {
                failure: failure.into(),
            },
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlNativeRemoteControlError {
    Admission {
        value: RemoteControlNativeSubmitError,
    },
    Connect {
        value: RemoteControlConnectRemoteControlTargetError,
    },
    Exchange {
        value: RemoteControlError,
    },
    TargetOperation {
        value: RemoteControlTargetOperationError,
    },
}
impl From<prns_host_native::remote_control::NativeRemoteControlError>
    for RemoteControlNativeRemoteControlError
{
    fn from(value: prns_host_native::remote_control::NativeRemoteControlError) -> Self {
        match value {
            prns_host_native::remote_control::NativeRemoteControlError::Admission(value) => {
                RemoteControlNativeRemoteControlError::Admission {
                    value: value.into(),
                }
            }
            prns_host_native::remote_control::NativeRemoteControlError::Connect(value) => {
                RemoteControlNativeRemoteControlError::Connect {
                    value: value.into(),
                }
            }
            prns_host_native::remote_control::NativeRemoteControlError::Exchange(value) => {
                RemoteControlNativeRemoteControlError::Exchange {
                    value: value.into(),
                }
            }
            prns_host_native::remote_control::NativeRemoteControlError::TargetOperation(value) => {
                RemoteControlNativeRemoteControlError::TargetOperation {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlNativeSubmitError {
    Busy,
    Stopped,
}
impl From<prns_host_native::NativeSubmitError> for RemoteControlNativeSubmitError {
    fn from(value: prns_host_native::NativeSubmitError) -> Self {
        match value {
            prns_host_native::NativeSubmitError::Busy => RemoteControlNativeSubmitError::Busy,
            prns_host_native::NativeSubmitError::Stopped => RemoteControlNativeSubmitError::Stopped,
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlConnectRemoteControlTargetError {
    Resolve {
        value: RemoteControlResolveRemoteControlTargetControlError,
    },
    EstablishLink {
        value: RemoteControlSendErrorRemoteControlEstablishLinkFailure,
    },
    Identify {
        value: RemoteControlSendErrorRemoteControlIdentifyFailure,
    },
}
impl From<personal_rns::runtime::ConnectRemoteControlTargetError>
    for RemoteControlConnectRemoteControlTargetError
{
    fn from(value: personal_rns::runtime::ConnectRemoteControlTargetError) -> Self {
        match value {
            personal_rns::runtime::ConnectRemoteControlTargetError::Resolve(value) => {
                RemoteControlConnectRemoteControlTargetError::Resolve {
                    value: value.into(),
                }
            }
            personal_rns::runtime::ConnectRemoteControlTargetError::EstablishLink(value) => {
                RemoteControlConnectRemoteControlTargetError::EstablishLink {
                    value: value.into(),
                }
            }
            personal_rns::runtime::ConnectRemoteControlTargetError::Identify(value) => {
                RemoteControlConnectRemoteControlTargetError::Identify {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlResolveRemoteControlTargetControlError {
    NodeStopped,
    Busy,
    Unavailable,
    TargetNotAuthorized,
}
impl From<personal_rns::runtime::ResolveRemoteControlTargetControlError>
    for RemoteControlResolveRemoteControlTargetControlError
{
    fn from(value: personal_rns::runtime::ResolveRemoteControlTargetControlError) -> Self {
        match value {
            personal_rns::runtime::ResolveRemoteControlTargetControlError::NodeStopped => {
                RemoteControlResolveRemoteControlTargetControlError::NodeStopped
            }
            personal_rns::runtime::ResolveRemoteControlTargetControlError::Busy => {
                RemoteControlResolveRemoteControlTargetControlError::Busy
            }
            personal_rns::runtime::ResolveRemoteControlTargetControlError::Unavailable => {
                RemoteControlResolveRemoteControlTargetControlError::Unavailable
            }
            personal_rns::runtime::ResolveRemoteControlTargetControlError::TargetNotAuthorized => {
                RemoteControlResolveRemoteControlTargetControlError::TargetNotAuthorized
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlSendErrorRemoteControlEstablishLinkFailure {
    PayloadTooLarge,
    NodeStopped,
    Busy,
    Failed {
        value: RemoteControlEstablishLinkFailure,
    },
}
impl From<personal_rns::runtime::SendError<prns_core::engine::EstablishLinkFailure>>
    for RemoteControlSendErrorRemoteControlEstablishLinkFailure
{
    fn from(
        value: personal_rns::runtime::SendError<prns_core::engine::EstablishLinkFailure>,
    ) -> Self {
        match value {
personal_rns::runtime::SendError::<prns_core::engine::EstablishLinkFailure>::PayloadTooLarge => RemoteControlSendErrorRemoteControlEstablishLinkFailure::PayloadTooLarge,
personal_rns::runtime::SendError::<prns_core::engine::EstablishLinkFailure>::NodeStopped => RemoteControlSendErrorRemoteControlEstablishLinkFailure::NodeStopped,
personal_rns::runtime::SendError::<prns_core::engine::EstablishLinkFailure>::Busy => RemoteControlSendErrorRemoteControlEstablishLinkFailure::Busy,
personal_rns::runtime::SendError::<prns_core::engine::EstablishLinkFailure>::Failed(value) => RemoteControlSendErrorRemoteControlEstablishLinkFailure::Failed { value: value.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlEstablishLinkFailure {
    Rejected {
        value: RemoteControlEstablishLinkRejection,
    },
    WriteFailed {
        value: RemoteControlWriteEstablishLinkRejection,
    },
    Timeout,
}
impl From<prns_core::engine::EstablishLinkFailure> for RemoteControlEstablishLinkFailure {
    fn from(value: prns_core::engine::EstablishLinkFailure) -> Self {
        match value {
            prns_core::engine::EstablishLinkFailure::Rejected(value) => {
                RemoteControlEstablishLinkFailure::Rejected {
                    value: value.into(),
                }
            }
            prns_core::engine::EstablishLinkFailure::WriteFailed(value) => {
                RemoteControlEstablishLinkFailure::WriteFailed {
                    value: value.into(),
                }
            }
            prns_core::engine::EstablishLinkFailure::Timeout => {
                RemoteControlEstablishLinkFailure::Timeout
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlEstablishLinkRejection {
    NoRouteToDestination,
    NotDirectlyReachable,
}
impl From<prns_core::engine::EstablishLinkRejection> for RemoteControlEstablishLinkRejection {
    fn from(value: prns_core::engine::EstablishLinkRejection) -> Self {
        match value {
            prns_core::engine::EstablishLinkRejection::NoRouteToDestination => {
                RemoteControlEstablishLinkRejection::NoRouteToDestination
            }
            prns_core::engine::EstablishLinkRejection::NotDirectlyReachable => {
                RemoteControlEstablishLinkRejection::NotDirectlyReachable
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlWriteEstablishLinkRejection {
    RouteVanished,
    Serialize,
    LinkTableFull,
    DuplicateLinkId,
}
impl From<prns_core::routing::links::establish::WriteEstablishLinkRejection>
    for RemoteControlWriteEstablishLinkRejection
{
    fn from(value: prns_core::routing::links::establish::WriteEstablishLinkRejection) -> Self {
        match value {
            prns_core::routing::links::establish::WriteEstablishLinkRejection::RouteVanished => {
                RemoteControlWriteEstablishLinkRejection::RouteVanished
            }
            prns_core::routing::links::establish::WriteEstablishLinkRejection::Serialize => {
                RemoteControlWriteEstablishLinkRejection::Serialize
            }
            prns_core::routing::links::establish::WriteEstablishLinkRejection::LinkTableFull => {
                RemoteControlWriteEstablishLinkRejection::LinkTableFull
            }
            prns_core::routing::links::establish::WriteEstablishLinkRejection::DuplicateLinkId => {
                RemoteControlWriteEstablishLinkRejection::DuplicateLinkId
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlSendErrorRemoteControlIdentifyFailure {
    PayloadTooLarge,
    NodeStopped,
    Busy,
    Failed { value: RemoteControlIdentifyFailure },
}
impl From<personal_rns::runtime::SendError<prns_core::engine::IdentifyFailure>>
    for RemoteControlSendErrorRemoteControlIdentifyFailure
{
    fn from(value: personal_rns::runtime::SendError<prns_core::engine::IdentifyFailure>) -> Self {
        match value {
personal_rns::runtime::SendError::<prns_core::engine::IdentifyFailure>::PayloadTooLarge => RemoteControlSendErrorRemoteControlIdentifyFailure::PayloadTooLarge,
personal_rns::runtime::SendError::<prns_core::engine::IdentifyFailure>::NodeStopped => RemoteControlSendErrorRemoteControlIdentifyFailure::NodeStopped,
personal_rns::runtime::SendError::<prns_core::engine::IdentifyFailure>::Busy => RemoteControlSendErrorRemoteControlIdentifyFailure::Busy,
personal_rns::runtime::SendError::<prns_core::engine::IdentifyFailure>::Failed(value) => RemoteControlSendErrorRemoteControlIdentifyFailure::Failed { value: value.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlIdentifyFailure {
    Rejected {
        value: RemoteControlIdentifyRejection,
    },
    WriteFailed,
}
impl From<prns_core::engine::IdentifyFailure> for RemoteControlIdentifyFailure {
    fn from(value: prns_core::engine::IdentifyFailure) -> Self {
        match value {
            prns_core::engine::IdentifyFailure::Rejected(value) => {
                RemoteControlIdentifyFailure::Rejected {
                    value: value.into(),
                }
            }
            prns_core::engine::IdentifyFailure::WriteFailed => {
                RemoteControlIdentifyFailure::WriteFailed
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlIdentifyRejection {
    NoSuchLink,
    LinkNotActive,
    NotInitiator,
    IdentityNotHeld,
}
impl From<prns_core::engine::IdentifyRejection> for RemoteControlIdentifyRejection {
    fn from(value: prns_core::engine::IdentifyRejection) -> Self {
        match value {
            prns_core::engine::IdentifyRejection::NoSuchLink => {
                RemoteControlIdentifyRejection::NoSuchLink
            }
            prns_core::engine::IdentifyRejection::LinkNotActive => {
                RemoteControlIdentifyRejection::LinkNotActive
            }
            prns_core::engine::IdentifyRejection::NotInitiator => {
                RemoteControlIdentifyRejection::NotInitiator
            }
            prns_core::engine::IdentifyRejection::IdentityNotHeld => {
                RemoteControlIdentifyRejection::IdentityNotHeld
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlError {
    UnsupportedRequestKind {
        value: RemoteControlRequestKind,
    },
    Encode {
        value: RemoteControlMessageWriteError,
    },
    Request {
        value: RemoteControlSendErrorRemoteControlSendRequestFailure,
    },
    Response {
        value: RemoteControlResponseParseError,
    },
    Remote {
        value: RemoteControlProtocolError,
    },
    UnexpectedResponse {
        expected: RemoteControlResponseKind,
        found: RemoteControlResponseKind,
    },
    AnnounceSelf {
        value: RemoteControlAnnounceSelfFailure,
    },
}
impl From<personal_rns::runtime::RemoteControlError> for RemoteControlError {
    fn from(value: personal_rns::runtime::RemoteControlError) -> Self {
        match value {
            personal_rns::runtime::RemoteControlError::UnsupportedRequestKind(value) => {
                RemoteControlError::UnsupportedRequestKind {
                    value: value.into(),
                }
            }
            personal_rns::runtime::RemoteControlError::Encode(value) => {
                RemoteControlError::Encode {
                    value: value.into(),
                }
            }
            personal_rns::runtime::RemoteControlError::Request(value) => {
                RemoteControlError::Request {
                    value: value.into(),
                }
            }
            personal_rns::runtime::RemoteControlError::Response(value) => {
                RemoteControlError::Response {
                    value: value.into(),
                }
            }
            personal_rns::runtime::RemoteControlError::Remote(value) => {
                RemoteControlError::Remote {
                    value: value.into(),
                }
            }
            personal_rns::runtime::RemoteControlError::UnexpectedResponse { expected, found } => {
                RemoteControlError::UnexpectedResponse {
                    expected: expected.into(),
                    found: found.into(),
                }
            }
            personal_rns::runtime::RemoteControlError::AnnounceSelf(value) => {
                RemoteControlError::AnnounceSelf {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlMessageWriteError {
    BufferTooShort,
    InvalidRequestSet,
}
impl From<prns_core::remote_control::RemoteControlMessageWriteError>
    for RemoteControlMessageWriteError
{
    fn from(value: prns_core::remote_control::RemoteControlMessageWriteError) -> Self {
        match value {
            prns_core::remote_control::RemoteControlMessageWriteError::BufferTooShort => {
                RemoteControlMessageWriteError::BufferTooShort
            }
            prns_core::remote_control::RemoteControlMessageWriteError::InvalidRequestSet => {
                RemoteControlMessageWriteError::InvalidRequestSet
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlSendErrorRemoteControlSendRequestFailure {
    PayloadTooLarge,
    NodeStopped,
    Busy,
    Failed {
        value: RemoteControlSendRequestFailure,
    },
}
impl From<personal_rns::runtime::SendError<prns_core::engine::SendRequestFailure>>
    for RemoteControlSendErrorRemoteControlSendRequestFailure
{
    fn from(
        value: personal_rns::runtime::SendError<prns_core::engine::SendRequestFailure>,
    ) -> Self {
        match value {
personal_rns::runtime::SendError::<prns_core::engine::SendRequestFailure>::PayloadTooLarge => RemoteControlSendErrorRemoteControlSendRequestFailure::PayloadTooLarge,
personal_rns::runtime::SendError::<prns_core::engine::SendRequestFailure>::NodeStopped => RemoteControlSendErrorRemoteControlSendRequestFailure::NodeStopped,
personal_rns::runtime::SendError::<prns_core::engine::SendRequestFailure>::Busy => RemoteControlSendErrorRemoteControlSendRequestFailure::Busy,
personal_rns::runtime::SendError::<prns_core::engine::SendRequestFailure>::Failed(value) => RemoteControlSendErrorRemoteControlSendRequestFailure::Failed { value: value.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlSendRequestFailure {
    Rejected {
        value: RemoteControlSendRequestRejection,
    },
    WriteFailed,
    Culled,
    Timeout,
    LinkClosed,
    ResponseTooLarge,
    ResponseTransferFailed {
        value: RemoteControlResourceFailureCause,
    },
    ResourceCapacity,
}
impl From<prns_core::engine::SendRequestFailure> for RemoteControlSendRequestFailure {
    fn from(value: prns_core::engine::SendRequestFailure) -> Self {
        match value {
            prns_core::engine::SendRequestFailure::Rejected(value) => {
                RemoteControlSendRequestFailure::Rejected {
                    value: value.into(),
                }
            }
            prns_core::engine::SendRequestFailure::WriteFailed => {
                RemoteControlSendRequestFailure::WriteFailed
            }
            prns_core::engine::SendRequestFailure::Culled => {
                RemoteControlSendRequestFailure::Culled
            }
            prns_core::engine::SendRequestFailure::Timeout => {
                RemoteControlSendRequestFailure::Timeout
            }
            prns_core::engine::SendRequestFailure::LinkClosed => {
                RemoteControlSendRequestFailure::LinkClosed
            }
            prns_core::engine::SendRequestFailure::ResponseTooLarge => {
                RemoteControlSendRequestFailure::ResponseTooLarge
            }
            prns_core::engine::SendRequestFailure::ResponseTransferFailed(value) => {
                RemoteControlSendRequestFailure::ResponseTransferFailed {
                    value: value.into(),
                }
            }
            prns_core::engine::SendRequestFailure::ResourceCapacity => {
                RemoteControlSendRequestFailure::ResourceCapacity
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlSendRequestRejection {
    NoSuchLink,
    LinkNotActive,
}
impl From<prns_core::engine::SendRequestRejection> for RemoteControlSendRequestRejection {
    fn from(value: prns_core::engine::SendRequestRejection) -> Self {
        match value {
            prns_core::engine::SendRequestRejection::NoSuchLink => {
                RemoteControlSendRequestRejection::NoSuchLink
            }
            prns_core::engine::SendRequestRejection::LinkNotActive => {
                RemoteControlSendRequestRejection::LinkNotActive
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlResourceFailureCause {
    CancelledBySender,
    RefusedHashmapUpdate {
        value: RemoteControlApplyHashmapUpdateError,
    },
    RetriesExhausted,
    LinkVanished,
    TransferUnopenable,
    TransferCorrupt,
    ProofUnsendable,
    DecompressionFailed,
    DecompressionTimedOut,
    OpenTimedOut,
    MetadataOverrun,
}
impl From<prns_core::routing::links::resources::ResourceFailureCause>
    for RemoteControlResourceFailureCause
{
    fn from(value: prns_core::routing::links::resources::ResourceFailureCause) -> Self {
        match value {
            prns_core::routing::links::resources::ResourceFailureCause::CancelledBySender => {
                RemoteControlResourceFailureCause::CancelledBySender
            }
            prns_core::routing::links::resources::ResourceFailureCause::RefusedHashmapUpdate(
                value,
            ) => RemoteControlResourceFailureCause::RefusedHashmapUpdate {
                value: value.into(),
            },
            prns_core::routing::links::resources::ResourceFailureCause::RetriesExhausted => {
                RemoteControlResourceFailureCause::RetriesExhausted
            }
            prns_core::routing::links::resources::ResourceFailureCause::LinkVanished => {
                RemoteControlResourceFailureCause::LinkVanished
            }
            prns_core::routing::links::resources::ResourceFailureCause::TransferUnopenable => {
                RemoteControlResourceFailureCause::TransferUnopenable
            }
            prns_core::routing::links::resources::ResourceFailureCause::TransferCorrupt => {
                RemoteControlResourceFailureCause::TransferCorrupt
            }
            prns_core::routing::links::resources::ResourceFailureCause::ProofUnsendable => {
                RemoteControlResourceFailureCause::ProofUnsendable
            }
            prns_core::routing::links::resources::ResourceFailureCause::DecompressionFailed => {
                RemoteControlResourceFailureCause::DecompressionFailed
            }
            prns_core::routing::links::resources::ResourceFailureCause::DecompressionTimedOut => {
                RemoteControlResourceFailureCause::DecompressionTimedOut
            }
            prns_core::routing::links::resources::ResourceFailureCause::OpenTimedOut => {
                RemoteControlResourceFailureCause::OpenTimedOut
            }
            prns_core::routing::links::resources::ResourceFailureCause::MetadataOverrun => {
                RemoteControlResourceFailureCause::MetadataOverrun
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlApplyHashmapUpdateError {
    BeyondPartCount,
    SkipsAhead,
    HashmapTooLong,
    HashmapRagged,
}
impl From<prns_core::routing::links::resources::table::ApplyHashmapUpdateError>
    for RemoteControlApplyHashmapUpdateError
{
    fn from(value: prns_core::routing::links::resources::table::ApplyHashmapUpdateError) -> Self {
        match value {
prns_core::routing::links::resources::table::ApplyHashmapUpdateError::BeyondPartCount => RemoteControlApplyHashmapUpdateError::BeyondPartCount,
prns_core::routing::links::resources::table::ApplyHashmapUpdateError::SkipsAhead => RemoteControlApplyHashmapUpdateError::SkipsAhead,
prns_core::routing::links::resources::table::ApplyHashmapUpdateError::HashmapTooLong => RemoteControlApplyHashmapUpdateError::HashmapTooLong,
prns_core::routing::links::resources::table::ApplyHashmapUpdateError::HashmapRagged => RemoteControlApplyHashmapUpdateError::HashmapRagged,
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlResponseParseError {
    Truncated,
    UnsupportedVersion { found: u8 },
    UnknownResponseKind { found: u8 },
    UnknownAnnounceSelfOutcome { found: u8 },
    UnknownPowerOutcome { found: u8 },
    UnknownModeOutcome { found: u8 },
    UnknownGroupOutcome { found: u8 },
    UnknownLoRaOutcome { found: u8 },
    UnknownWifiStationOutcome { found: u8 },
    UnknownAuthorizeControllerOutcome { found: u8 },
    UnknownRevokeControllerOutcome { found: u8 },
    UnknownSleepOutcome { found: u8 },
    UnknownApplyOutcome { found: u8 },
    UnknownWifiStageOutcome { found: u8 },
    UnknownWifiTransactionStatus { found: u8 },
    UnknownProtocolErrorKind { found: u8 },
    UnknownRequestKind { found: u8 },
    UnknownInterfaceKind { found: u8 },
    UnknownInterfaceMode { found: u8 },
    UnknownConnectionState { found: u8 },
    NonCanonicalRequestSet,
    NonCanonicalCursor,
    Malformed,
}
impl From<prns_core::remote_control::RemoteControlResponseParseError>
    for RemoteControlResponseParseError
{
    fn from(value: prns_core::remote_control::RemoteControlResponseParseError) -> Self {
        match value {
prns_core::remote_control::RemoteControlResponseParseError::Truncated => RemoteControlResponseParseError::Truncated,
prns_core::remote_control::RemoteControlResponseParseError::UnsupportedVersion { found } => RemoteControlResponseParseError::UnsupportedVersion { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownResponseKind { found } => RemoteControlResponseParseError::UnknownResponseKind { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownAnnounceSelfOutcome { found } => RemoteControlResponseParseError::UnknownAnnounceSelfOutcome { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownPowerOutcome { found } => RemoteControlResponseParseError::UnknownPowerOutcome { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownModeOutcome { found } => RemoteControlResponseParseError::UnknownModeOutcome { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownGroupOutcome { found } => RemoteControlResponseParseError::UnknownGroupOutcome { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownLoRaOutcome { found } => RemoteControlResponseParseError::UnknownLoRaOutcome { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownWifiStationOutcome { found } => RemoteControlResponseParseError::UnknownWifiStationOutcome { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownAuthorizeControllerOutcome { found } => RemoteControlResponseParseError::UnknownAuthorizeControllerOutcome { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownRevokeControllerOutcome { found } => RemoteControlResponseParseError::UnknownRevokeControllerOutcome { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownSleepOutcome { found } => RemoteControlResponseParseError::UnknownSleepOutcome { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownApplyOutcome { found } => RemoteControlResponseParseError::UnknownApplyOutcome { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownWifiStageOutcome { found } => RemoteControlResponseParseError::UnknownWifiStageOutcome { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownWifiTransactionStatus { found } => RemoteControlResponseParseError::UnknownWifiTransactionStatus { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownProtocolErrorKind { found } => RemoteControlResponseParseError::UnknownProtocolErrorKind { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownRequestKind { found } => RemoteControlResponseParseError::UnknownRequestKind { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownInterfaceKind { found } => RemoteControlResponseParseError::UnknownInterfaceKind { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownInterfaceMode { found } => RemoteControlResponseParseError::UnknownInterfaceMode { found },
prns_core::remote_control::RemoteControlResponseParseError::UnknownConnectionState { found } => RemoteControlResponseParseError::UnknownConnectionState { found },
prns_core::remote_control::RemoteControlResponseParseError::NonCanonicalRequestSet => RemoteControlResponseParseError::NonCanonicalRequestSet,
prns_core::remote_control::RemoteControlResponseParseError::NonCanonicalCursor => RemoteControlResponseParseError::NonCanonicalCursor,
prns_core::remote_control::RemoteControlResponseParseError::Malformed => RemoteControlResponseParseError::Malformed,
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlResponseKind {
    Describe,
    AnnounceSelf,
    InventoryInterfaces,
    SetInterfacePower,
    SleepRadios,
    WakeRadios,
    SetInterfaceMode,
    SetInterfaceGroup,
    InventoryInterfacePeers,
    InventoryInterfaceConfig,
    SetInterfaceLoRaProfile,
    DescribeBuild,
    SetInterfaceWifiStation,
    InventoryControllers,
    AuthorizeController,
    RevokeController,
    DescribePower,
    SetSystemPower,
    SetGnssPower,
    SetDisplayVisibility,
    SetDisplayAutoOff,
    SetStationUplink,
    SetEspRadioMode,
    StageWifiCredentials,
    ActivateWifiCredentials,
    ConfirmWifiCredentials,
    CancelWifiCredentials,
    InspectWifiTransaction,
    InventoryInterfaceDiscoveryGroups,
    ReplaceInterfaceDiscoveryGroups,
    ProtocolError,
}
impl From<prns_core::remote_control::RemoteControlResponseKind> for RemoteControlResponseKind {
    fn from(value: prns_core::remote_control::RemoteControlResponseKind) -> Self {
        match value {
prns_core::remote_control::RemoteControlResponseKind::Describe => RemoteControlResponseKind::Describe,
prns_core::remote_control::RemoteControlResponseKind::AnnounceSelf => RemoteControlResponseKind::AnnounceSelf,
prns_core::remote_control::RemoteControlResponseKind::InventoryInterfaces => RemoteControlResponseKind::InventoryInterfaces,
prns_core::remote_control::RemoteControlResponseKind::SetInterfacePower => RemoteControlResponseKind::SetInterfacePower,
prns_core::remote_control::RemoteControlResponseKind::SleepRadios => RemoteControlResponseKind::SleepRadios,
prns_core::remote_control::RemoteControlResponseKind::WakeRadios => RemoteControlResponseKind::WakeRadios,
prns_core::remote_control::RemoteControlResponseKind::SetInterfaceMode => RemoteControlResponseKind::SetInterfaceMode,
prns_core::remote_control::RemoteControlResponseKind::SetInterfaceGroup => RemoteControlResponseKind::SetInterfaceGroup,
prns_core::remote_control::RemoteControlResponseKind::InventoryInterfacePeers => RemoteControlResponseKind::InventoryInterfacePeers,
prns_core::remote_control::RemoteControlResponseKind::InventoryInterfaceConfig => RemoteControlResponseKind::InventoryInterfaceConfig,
prns_core::remote_control::RemoteControlResponseKind::SetInterfaceLoRaProfile => RemoteControlResponseKind::SetInterfaceLoRaProfile,
prns_core::remote_control::RemoteControlResponseKind::DescribeBuild => RemoteControlResponseKind::DescribeBuild,
prns_core::remote_control::RemoteControlResponseKind::SetInterfaceWifiStation => RemoteControlResponseKind::SetInterfaceWifiStation,
prns_core::remote_control::RemoteControlResponseKind::InventoryControllers => RemoteControlResponseKind::InventoryControllers,
prns_core::remote_control::RemoteControlResponseKind::AuthorizeController => RemoteControlResponseKind::AuthorizeController,
prns_core::remote_control::RemoteControlResponseKind::RevokeController => RemoteControlResponseKind::RevokeController,
prns_core::remote_control::RemoteControlResponseKind::DescribePower => RemoteControlResponseKind::DescribePower,
prns_core::remote_control::RemoteControlResponseKind::SetSystemPower => RemoteControlResponseKind::SetSystemPower,
prns_core::remote_control::RemoteControlResponseKind::SetGnssPower => RemoteControlResponseKind::SetGnssPower,
prns_core::remote_control::RemoteControlResponseKind::SetDisplayVisibility => RemoteControlResponseKind::SetDisplayVisibility,
prns_core::remote_control::RemoteControlResponseKind::SetDisplayAutoOff => RemoteControlResponseKind::SetDisplayAutoOff,
prns_core::remote_control::RemoteControlResponseKind::SetStationUplink => RemoteControlResponseKind::SetStationUplink,
prns_core::remote_control::RemoteControlResponseKind::SetEspRadioMode => RemoteControlResponseKind::SetEspRadioMode,
prns_core::remote_control::RemoteControlResponseKind::StageWifiCredentials => RemoteControlResponseKind::StageWifiCredentials,
prns_core::remote_control::RemoteControlResponseKind::ActivateWifiCredentials => RemoteControlResponseKind::ActivateWifiCredentials,
prns_core::remote_control::RemoteControlResponseKind::ConfirmWifiCredentials => RemoteControlResponseKind::ConfirmWifiCredentials,
prns_core::remote_control::RemoteControlResponseKind::CancelWifiCredentials => RemoteControlResponseKind::CancelWifiCredentials,
prns_core::remote_control::RemoteControlResponseKind::InspectWifiTransaction => RemoteControlResponseKind::InspectWifiTransaction,
prns_core::remote_control::RemoteControlResponseKind::InventoryInterfaceDiscoveryGroups => RemoteControlResponseKind::InventoryInterfaceDiscoveryGroups,
prns_core::remote_control::RemoteControlResponseKind::ReplaceInterfaceDiscoveryGroups => RemoteControlResponseKind::ReplaceInterfaceDiscoveryGroups,
prns_core::remote_control::RemoteControlResponseKind::ProtocolError => RemoteControlResponseKind::ProtocolError,
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlAnnounceSelfFailure {
    Unavailable,
    Rejected,
    WriteFailed,
}
impl From<personal_rns::runtime::RemoteControlAnnounceSelfFailure>
    for RemoteControlAnnounceSelfFailure
{
    fn from(value: personal_rns::runtime::RemoteControlAnnounceSelfFailure) -> Self {
        match value {
            personal_rns::runtime::RemoteControlAnnounceSelfFailure::Unavailable => {
                RemoteControlAnnounceSelfFailure::Unavailable
            }
            personal_rns::runtime::RemoteControlAnnounceSelfFailure::Rejected => {
                RemoteControlAnnounceSelfFailure::Rejected
            }
            personal_rns::runtime::RemoteControlAnnounceSelfFailure::WriteFailed => {
                RemoteControlAnnounceSelfFailure::WriteFailed
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlTargetOperationError {
    NotPermitted { value: RemoteControlRequestKind },
    Exchange { value: RemoteControlError },
}
impl From<personal_rns::runtime::RemoteControlTargetOperationError>
    for RemoteControlTargetOperationError
{
    fn from(value: personal_rns::runtime::RemoteControlTargetOperationError) -> Self {
        match value {
            personal_rns::runtime::RemoteControlTargetOperationError::NotPermitted(value) => {
                RemoteControlTargetOperationError::NotPermitted {
                    value: value.into(),
                }
            }
            personal_rns::runtime::RemoteControlTargetOperationError::Exchange(value) => {
                RemoteControlTargetOperationError::Exchange {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlNativeRemoteControlConfig {
    pub controller_identity: crate::transport::IdentityConfig,
    pub target_identity: crate::transport::IdentityConfig,
    pub initial_controller_grants: Vec<RemoteControlControllerGrant>,
    pub self_announcement: RemoteControlSelfAnnouncement,
    pub capabilities: RemoteControlCapabilities,
}
impl TryFrom<RemoteControlNativeRemoteControlConfig>
    for prns_host_native::remote_control_service::NativeRemoteControlConfig
{
    type Error = BindingError;
    fn try_from(value: RemoteControlNativeRemoteControlConfig) -> Result<Self, Self::Error> {
        Ok(Self {
            controller_identity: value.controller_identity.try_into()?,
            target_identity: value.target_identity.try_into()?,
            initial_controller_grants: value
                .initial_controller_grants
                .into_iter()
                .map(|item| Ok(item.try_into()?))
                .collect::<Result<Vec<_>, BindingError>>()?,
            self_announcement: value.self_announcement.try_into()?,
            capabilities: value.capabilities.try_into()?,
        })
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlControllerGrant {
    pub controller: RemoteControlControllerIdentity,
    pub authority: RemoteControlControllerAuthority,
    pub permitted_requests: RemoteControlRequestSet,
}
impl TryFrom<RemoteControlControllerGrant>
    for prns_core::remote_control::RemoteControlControllerGrant
{
    type Error = BindingError;
    fn try_from(value: RemoteControlControllerGrant) -> Result<Self, Self::Error> {
        Ok(
            prns_core::remote_control::RemoteControlControllerGrant::new(
                value.controller.try_into()?,
                value.authority.try_into()?,
                value.permitted_requests.try_into()?,
            )
            .map_err(|_| invalid("RemoteControlControllerGrant"))?,
        )
    }
}
impl From<prns_core::remote_control::RemoteControlControllerGrant>
    for RemoteControlControllerGrant
{
    fn from(value: prns_core::remote_control::RemoteControlControllerGrant) -> Self {
        Self {
            controller: (*value.controller()).into(),
            authority: value.authority().into(),
            permitted_requests: (*value.permitted_requests()).into(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlControllerAuthority {
    Operator,
    Administrator,
}
impl TryFrom<RemoteControlControllerAuthority>
    for prns_core::remote_control::RemoteControlControllerAuthority
{
    type Error = BindingError;
    fn try_from(value: RemoteControlControllerAuthority) -> Result<Self, Self::Error> {
        Ok(match value {
            RemoteControlControllerAuthority::Operator => {
                prns_core::remote_control::RemoteControlControllerAuthority::Operator
            }
            RemoteControlControllerAuthority::Administrator => {
                prns_core::remote_control::RemoteControlControllerAuthority::Administrator
            }
        })
    }
}
impl From<prns_core::remote_control::RemoteControlControllerAuthority>
    for RemoteControlControllerAuthority
{
    fn from(value: prns_core::remote_control::RemoteControlControllerAuthority) -> Self {
        match value {
            prns_core::remote_control::RemoteControlControllerAuthority::Operator => {
                RemoteControlControllerAuthority::Operator
            }
            prns_core::remote_control::RemoteControlControllerAuthority::Administrator => {
                RemoteControlControllerAuthority::Administrator
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlSelfAnnouncement {
    Unavailable,
    Destination { value: RemoteControlDestinationHash },
}
impl TryFrom<RemoteControlSelfAnnouncement>
    for prns_core::remote_control::RemoteControlSelfAnnouncement
{
    type Error = BindingError;
    fn try_from(value: RemoteControlSelfAnnouncement) -> Result<Self, Self::Error> {
        Ok(match value {
            RemoteControlSelfAnnouncement::Unavailable => {
                prns_core::remote_control::RemoteControlSelfAnnouncement::Unavailable
            }
            RemoteControlSelfAnnouncement::Destination { value } => {
                prns_core::remote_control::RemoteControlSelfAnnouncement::Destination(
                    value.try_into()?,
                )
            }
        })
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlDestinationHash {
    pub value: Vec<u8>,
}
impl TryFrom<RemoteControlDestinationHash> for prns_core::wire::DestinationHash {
    type Error = BindingError;
    fn try_from(value: RemoteControlDestinationHash) -> Result<Self, Self::Error> {
        Ok(prns_core::wire::DestinationHash::new(
            value
                .value
                .try_into()
                .map_err(|_| invalid("DestinationHash"))?,
        ))
    }
}
impl From<prns_core::wire::DestinationHash> for RemoteControlDestinationHash {
    fn from(value: prns_core::wire::DestinationHash) -> Self {
        Self {
            value: value.as_bytes().to_vec(),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlCapabilities {
    pub requests: RemoteControlRequestSet,
}
impl TryFrom<RemoteControlCapabilities> for prns_core::remote_control::RemoteControlCapabilities {
    type Error = BindingError;
    fn try_from(value: RemoteControlCapabilities) -> Result<Self, Self::Error> {
        Ok(
            prns_core::remote_control::RemoteControlCapabilities::from_requests(
                value.requests.try_into()?,
            )
            .map_err(|_| invalid("RemoteControlCapabilities"))?,
        )
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlNativeRemoteControlEvent {
    PairingAvailable {
        endpoint: RemoteControlPairingEndpoint,
        observed_at: RemoteControlInstantMillis,
        expires_at: RemoteControlInstantMillis,
        hops: u8,
        source_interface: RemoteControlInterfaceId,
        public_app_data: Vec<u8>,
    },
    TargetConfirmationRequired {
        confirmation: RemoteControlNativeRemoteControlConfirmation,
        window: RemoteControlTargetPairingAttemptWindow,
    },
    TargetControllerCommitted {
        attempt_id: RemoteControlPairingAttemptId,
    },
    TargetAuthorizationRequired {
        attempt_id: RemoteControlPairingAttemptId,
        grant: RemoteControlControllerGrant,
    },
    TargetAuthorizationPersisted {
        attempt_id: RemoteControlPairingAttemptId,
    },
    TargetExpiredDuringAuthorization {
        attempt_id: RemoteControlPairingAttemptId,
    },
    ControllerConfirmationRequired {
        confirmation: RemoteControlNativeRemoteControlConfirmation,
        window: RemoteControlControllerPairingAttemptWindow,
    },
    ControllerPersistenceRequired {
        attempt_id: RemoteControlPairingAttemptId,
        context: RemoteControlPairingContext,
        target: RemoteControlTargetIdentity,
        authority: RemoteControlControllerAuthority,
        permitted_requests: RemoteControlRequestSet,
    },
    ControllerAuthorizationPersisted {
        attempt_id: RemoteControlPairingAttemptId,
    },
    ControllerAuthorizationPersistenceFailed {
        attempt_id: RemoteControlPairingAttemptId,
    },
    ControllerExpired {
        aborted: RemoteControlControllerPairingAborted,
    },
    ControllerLinkClosed {
        aborted: RemoteControlControllerPairingAborted,
    },
    TargetExpired {
        aborted: RemoteControlTargetPairingAborted,
    },
    TargetLinkClosed {
        aborted: RemoteControlTargetPairingAborted,
    },
    TargetCompletionRetentionExpired {
        attempt_id: RemoteControlPairingAttemptId,
    },
    TargetCompletionLinkClosed {
        attempt_id: RemoteControlPairingAttemptId,
    },
}
impl From<prns_host_native::remote_control_service::NativeRemoteControlEvent>
    for RemoteControlNativeRemoteControlEvent
{
    fn from(value: prns_host_native::remote_control_service::NativeRemoteControlEvent) -> Self {
        match value {
prns_host_native::remote_control_service::NativeRemoteControlEvent::PairingAvailable { endpoint, observed_at, expires_at, hops, source_interface, public_app_data } => RemoteControlNativeRemoteControlEvent::PairingAvailable { endpoint: endpoint.into(), observed_at: observed_at.into(), expires_at: expires_at.into(), hops, source_interface: source_interface.into(), public_app_data },
prns_host_native::remote_control_service::NativeRemoteControlEvent::TargetConfirmationRequired { confirmation, window } => RemoteControlNativeRemoteControlEvent::TargetConfirmationRequired { confirmation: confirmation.into(), window: window.into() },
prns_host_native::remote_control_service::NativeRemoteControlEvent::TargetControllerCommitted { attempt_id } => RemoteControlNativeRemoteControlEvent::TargetControllerCommitted { attempt_id: attempt_id.into() },
prns_host_native::remote_control_service::NativeRemoteControlEvent::TargetAuthorizationRequired { attempt_id, grant } => RemoteControlNativeRemoteControlEvent::TargetAuthorizationRequired { attempt_id: attempt_id.into(), grant: grant.into() },
prns_host_native::remote_control_service::NativeRemoteControlEvent::TargetAuthorizationPersisted { attempt_id } => RemoteControlNativeRemoteControlEvent::TargetAuthorizationPersisted { attempt_id: attempt_id.into() },
prns_host_native::remote_control_service::NativeRemoteControlEvent::TargetExpiredDuringAuthorization { attempt_id } => RemoteControlNativeRemoteControlEvent::TargetExpiredDuringAuthorization { attempt_id: attempt_id.into() },
prns_host_native::remote_control_service::NativeRemoteControlEvent::ControllerConfirmationRequired { confirmation, window } => RemoteControlNativeRemoteControlEvent::ControllerConfirmationRequired { confirmation: confirmation.into(), window: window.into() },
prns_host_native::remote_control_service::NativeRemoteControlEvent::ControllerPersistenceRequired { attempt_id, context, target, authority, permitted_requests } => RemoteControlNativeRemoteControlEvent::ControllerPersistenceRequired { attempt_id: attempt_id.into(), context: context.into(), target: target.into(), authority: authority.into(), permitted_requests: permitted_requests.into() },
prns_host_native::remote_control_service::NativeRemoteControlEvent::ControllerAuthorizationPersisted { attempt_id } => RemoteControlNativeRemoteControlEvent::ControllerAuthorizationPersisted { attempt_id: attempt_id.into() },
prns_host_native::remote_control_service::NativeRemoteControlEvent::ControllerAuthorizationPersistenceFailed { attempt_id } => RemoteControlNativeRemoteControlEvent::ControllerAuthorizationPersistenceFailed { attempt_id: attempt_id.into() },
prns_host_native::remote_control_service::NativeRemoteControlEvent::ControllerExpired { aborted } => RemoteControlNativeRemoteControlEvent::ControllerExpired { aborted: aborted.into() },
prns_host_native::remote_control_service::NativeRemoteControlEvent::ControllerLinkClosed { aborted } => RemoteControlNativeRemoteControlEvent::ControllerLinkClosed { aborted: aborted.into() },
prns_host_native::remote_control_service::NativeRemoteControlEvent::TargetExpired { aborted } => RemoteControlNativeRemoteControlEvent::TargetExpired { aborted: aborted.into() },
prns_host_native::remote_control_service::NativeRemoteControlEvent::TargetLinkClosed { aborted } => RemoteControlNativeRemoteControlEvent::TargetLinkClosed { aborted: aborted.into() },
prns_host_native::remote_control_service::NativeRemoteControlEvent::TargetCompletionRetentionExpired { attempt_id } => RemoteControlNativeRemoteControlEvent::TargetCompletionRetentionExpired { attempt_id: attempt_id.into() },
prns_host_native::remote_control_service::NativeRemoteControlEvent::TargetCompletionLinkClosed { attempt_id } => RemoteControlNativeRemoteControlEvent::TargetCompletionLinkClosed { attempt_id: attempt_id.into() },
}
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlPairingEndpoint {
    pub destination_hash: RemoteControlDestinationHash,
}
impl From<prns_core::remote_control::RemoteControlPairingEndpoint>
    for RemoteControlPairingEndpoint
{
    fn from(value: prns_core::remote_control::RemoteControlPairingEndpoint) -> Self {
        Self {
            destination_hash: value.destination_hash().into(),
        }
    }
}
impl TryFrom<RemoteControlPairingEndpoint>
    for prns_core::remote_control::RemoteControlPairingEndpoint
{
    type Error = BindingError;
    fn try_from(value: RemoteControlPairingEndpoint) -> Result<Self, Self::Error> {
        Ok(
            prns_core::remote_control::RemoteControlPairingEndpoint::from_destination_hash(
                value.destination_hash.try_into()?,
            ),
        )
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlInstantMillis {
    pub value: u64,
}
impl From<prns_core::units::InstantMillis> for RemoteControlInstantMillis {
    fn from(value: prns_core::units::InstantMillis) -> Self {
        Self { value: value.0 }
    }
}
impl TryFrom<RemoteControlInstantMillis> for prns_core::units::InstantMillis {
    type Error = BindingError;
    fn try_from(value: RemoteControlInstantMillis) -> Result<Self, Self::Error> {
        Ok(prns_core::units::InstantMillis(value.value))
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlNativeRemoteControlConfirmation {
    pub attempt_id: RemoteControlPairingAttemptId,
    pub context: RemoteControlPairingContext,
    pub controller: RemoteControlControllerIdentity,
    pub target: RemoteControlTargetIdentity,
    pub permissions: RemoteControlPairingPermissions,
    pub confirmation_code: String,
}
impl From<prns_host_native::remote_control_service::NativeRemoteControlConfirmation>
    for RemoteControlNativeRemoteControlConfirmation
{
    fn from(
        value: prns_host_native::remote_control_service::NativeRemoteControlConfirmation,
    ) -> Self {
        Self {
            attempt_id: value.attempt_id.into(),
            context: value.context.into(),
            controller: value.controller.into(),
            target: value.target.into(),
            permissions: value.permissions.into(),
            confirmation_code: value.confirmation_code,
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlPairingAttemptId {
    pub value: Vec<u8>,
}
impl From<prns_core::remote_control::RemoteControlPairingAttemptId>
    for RemoteControlPairingAttemptId
{
    fn from(value: prns_core::remote_control::RemoteControlPairingAttemptId) -> Self {
        Self {
            value: value.transcript().as_bytes().to_vec(),
        }
    }
}
impl TryFrom<RemoteControlPairingAttemptId>
    for prns_core::remote_control::RemoteControlPairingAttemptId
{
    type Error = BindingError;
    fn try_from(value: RemoteControlPairingAttemptId) -> Result<Self, Self::Error> {
        Ok(
            prns_core::remote_control::RemoteControlPairingAttemptId::from_transcript_digest_bytes(
                value.value.try_into().map_err(|_| invalid("attemptId"))?,
            ),
        )
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlPairingContext {
    pub endpoint: RemoteControlPairingEndpoint,
    pub link_id: RemoteControlLinkId,
}
impl From<prns_core::remote_control::RemoteControlPairingContext> for RemoteControlPairingContext {
    fn from(value: prns_core::remote_control::RemoteControlPairingContext) -> Self {
        Self {
            endpoint: value.endpoint().into(),
            link_id: value.link_id().into(),
        }
    }
}
impl TryFrom<RemoteControlPairingContext>
    for prns_core::remote_control::RemoteControlPairingContext
{
    type Error = BindingError;
    fn try_from(value: RemoteControlPairingContext) -> Result<Self, Self::Error> {
        Ok(prns_core::remote_control::RemoteControlPairingContext::new(
            value.endpoint.try_into()?,
            value.link_id.try_into()?,
        ))
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlLinkId {
    pub value: Vec<u8>,
}
impl From<prns_core::routing::links::LinkId> for RemoteControlLinkId {
    fn from(value: prns_core::routing::links::LinkId) -> Self {
        Self {
            value: value.as_bytes().to_vec(),
        }
    }
}
impl TryFrom<RemoteControlLinkId> for prns_core::routing::links::LinkId {
    type Error = BindingError;
    fn try_from(value: RemoteControlLinkId) -> Result<Self, Self::Error> {
        Ok(prns_core::routing::links::LinkId::new(
            value.value.try_into().map_err(|_| invalid("LinkId"))?,
        ))
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlTargetIdentity {
    pub public_keys: Vec<u8>,
}
impl From<prns_core::remote_control::RemoteControlTargetIdentity> for RemoteControlTargetIdentity {
    fn from(value: prns_core::remote_control::RemoteControlTargetIdentity) -> Self {
        Self {
            public_keys: value.public_keys().public_key_bytes().to_vec(),
        }
    }
}
impl TryFrom<RemoteControlTargetIdentity>
    for prns_core::remote_control::RemoteControlTargetIdentity
{
    type Error = BindingError;
    fn try_from(value: RemoteControlTargetIdentity) -> Result<Self, Self::Error> {
        Ok(prns_core::remote_control::RemoteControlTargetIdentity::new(
            prns_core::identity::PublicIdentityMaterial::from_slice(&value.public_keys)
                .map_err(|_| invalid("publicKeys"))?
                .public_keys(),
        ))
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlPairingPermissions {
    pub authority: RemoteControlControllerAuthority,
    pub permitted_requests: RemoteControlRequestSet,
}
impl From<prns_core::remote_control::RemoteControlPairingPermissions>
    for RemoteControlPairingPermissions
{
    fn from(value: prns_core::remote_control::RemoteControlPairingPermissions) -> Self {
        Self {
            authority: value.authority().into(),
            permitted_requests: (*value.permitted_requests()).into(),
        }
    }
}
impl TryFrom<RemoteControlPairingPermissions>
    for prns_core::remote_control::RemoteControlPairingPermissions
{
    type Error = BindingError;
    fn try_from(value: RemoteControlPairingPermissions) -> Result<Self, Self::Error> {
        Ok(
            prns_core::remote_control::RemoteControlPairingPermissions::new(
                value.authority.try_into()?,
                value.permitted_requests.try_into()?,
            )
            .map_err(|_| invalid("RemoteControlPairingPermissions"))?,
        )
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlTargetPairingAttemptWindow {
    pub started_at: RemoteControlInstantMillis,
    pub attempt_timeout: RemoteControlPairingAttemptTimeout,
    pub expires_at: RemoteControlInstantMillis,
}
impl From<prns_core::remote_control::RemoteControlTargetPairingAttemptWindow>
    for RemoteControlTargetPairingAttemptWindow
{
    fn from(value: prns_core::remote_control::RemoteControlTargetPairingAttemptWindow) -> Self {
        Self {
            started_at: value.started_at().into(),
            attempt_timeout: value.attempt_timeout().into(),
            expires_at: value.expires_at().into(),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlPairingAttemptTimeout {
    pub value: u64,
}
impl From<prns_core::remote_control::RemoteControlPairingAttemptTimeout>
    for RemoteControlPairingAttemptTimeout
{
    fn from(value: prns_core::remote_control::RemoteControlPairingAttemptTimeout) -> Self {
        Self {
            value: value.duration().0,
        }
    }
}
impl TryFrom<RemoteControlPairingAttemptTimeout>
    for prns_core::remote_control::RemoteControlPairingAttemptTimeout
{
    type Error = BindingError;
    fn try_from(value: RemoteControlPairingAttemptTimeout) -> Result<Self, Self::Error> {
        Ok(
            prns_core::remote_control::RemoteControlPairingAttemptTimeout::try_from(
                prns_core::units::DurationMillis(value.value),
            )
            .map_err(|_| invalid("RemoteControlPairingAttemptTimeout"))?,
        )
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlControllerPairingAttemptWindow {
    pub offered_at: RemoteControlInstantMillis,
    pub attempt_timeout: RemoteControlPairingAttemptTimeout,
    pub expires_at: RemoteControlInstantMillis,
}
impl From<prns_core::remote_control::RemoteControlControllerPairingAttemptWindow>
    for RemoteControlControllerPairingAttemptWindow
{
    fn from(value: prns_core::remote_control::RemoteControlControllerPairingAttemptWindow) -> Self {
        Self {
            offered_at: value.offered_at().into(),
            attempt_timeout: value.attempt_timeout().into(),
            expires_at: value.expires_at().into(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlControllerPairingAborted {
    AwaitingOffer {
        context: RemoteControlPairingContext,
    },
    AwaitingApproval {
        attempt_id: RemoteControlPairingAttemptId,
        context: RemoteControlPairingContext,
    },
    AwaitingCompletion {
        attempt_id: RemoteControlPairingAttemptId,
        context: RemoteControlPairingContext,
    },
}
impl From<prns_core::remote_control::RemoteControlControllerPairingAborted>
    for RemoteControlControllerPairingAborted
{
    fn from(value: prns_core::remote_control::RemoteControlControllerPairingAborted) -> Self {
        match value {
prns_core::remote_control::RemoteControlControllerPairingAborted::AwaitingOffer { context } => RemoteControlControllerPairingAborted::AwaitingOffer { context: context.into() },
prns_core::remote_control::RemoteControlControllerPairingAborted::AwaitingApproval { attempt_id, context } => RemoteControlControllerPairingAborted::AwaitingApproval { attempt_id: attempt_id.into(), context: context.into() },
prns_core::remote_control::RemoteControlControllerPairingAborted::AwaitingCompletion { attempt_id, context } => RemoteControlControllerPairingAborted::AwaitingCompletion { attempt_id: attempt_id.into(), context: context.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlTargetPairingAborted {
    OfferPrepared {
        attempt_id: RemoteControlPairingAttemptId,
        context: RemoteControlPairingContext,
    },
    AwaitingBoth {
        attempt_id: RemoteControlPairingAttemptId,
        context: RemoteControlPairingContext,
    },
    AwaitingTargetApproval {
        attempt_id: RemoteControlPairingAttemptId,
        context: RemoteControlPairingContext,
        responder: RemoteControlTargetPairingResponder,
    },
    AwaitingControllerCommit {
        attempt_id: RemoteControlPairingAttemptId,
        context: RemoteControlPairingContext,
    },
}
impl From<prns_core::remote_control::RemoteControlTargetPairingAborted>
    for RemoteControlTargetPairingAborted
{
    fn from(value: prns_core::remote_control::RemoteControlTargetPairingAborted) -> Self {
        match value {
prns_core::remote_control::RemoteControlTargetPairingAborted::OfferPrepared { attempt_id, context } => RemoteControlTargetPairingAborted::OfferPrepared { attempt_id: attempt_id.into(), context: context.into() },
prns_core::remote_control::RemoteControlTargetPairingAborted::AwaitingBoth { attempt_id, context } => RemoteControlTargetPairingAborted::AwaitingBoth { attempt_id: attempt_id.into(), context: context.into() },
prns_core::remote_control::RemoteControlTargetPairingAborted::AwaitingTargetApproval { attempt_id, context, responder } => RemoteControlTargetPairingAborted::AwaitingTargetApproval { attempt_id: attempt_id.into(), context: context.into(), responder: responder.into() },
prns_core::remote_control::RemoteControlTargetPairingAborted::AwaitingControllerCommit { attempt_id, context } => RemoteControlTargetPairingAborted::AwaitingControllerCommit { attempt_id: attempt_id.into(), context: context.into() },
}
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlTargetPairingResponder {
    pub link_id: RemoteControlLinkId,
    pub request_id: RemoteControlRequestId,
}
impl From<prns_core::remote_control::RemoteControlTargetPairingResponder>
    for RemoteControlTargetPairingResponder
{
    fn from(value: prns_core::remote_control::RemoteControlTargetPairingResponder) -> Self {
        Self {
            link_id: value.link_id().into(),
            request_id: value.request_id().into(),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlRequestId {
    pub value: Vec<u8>,
}
impl From<prns_core::routing::links::request::RequestId> for RemoteControlRequestId {
    fn from(value: prns_core::routing::links::request::RequestId) -> Self {
        Self {
            value: value.as_bytes().to_vec(),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlBeginRemoteControlControllerPairing {
    pub context: RemoteControlPairingContext,
    pub invitation_code: RemoteControlPairingInvitationCode,
    pub pairing_expires_at: RemoteControlInstantMillis,
}
impl TryFrom<RemoteControlBeginRemoteControlControllerPairing>
    for prns_core::engine::BeginRemoteControlControllerPairing
{
    type Error = BindingError;
    fn try_from(
        value: RemoteControlBeginRemoteControlControllerPairing,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            context: value.context.try_into()?,
            invitation_code: value.invitation_code.try_into()?,
            pairing_expires_at: value.pairing_expires_at.try_into()?,
        })
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlPairingInvitationCode {
    pub value: RemoteControlSecretCode,
}
impl TryFrom<RemoteControlPairingInvitationCode>
    for prns_core::remote_control::RemoteControlPairingInvitationCode
{
    type Error = BindingError;
    fn try_from(value: RemoteControlPairingInvitationCode) -> Result<Self, Self::Error> {
        Ok(
            prns_core::remote_control::RemoteControlPairingInvitationCode::from_value(
                *value.value.0,
            ),
        )
    }
}
impl From<prns_core::remote_control::RemoteControlPairingInvitationCode>
    for RemoteControlPairingInvitationCode
{
    fn from(value: prns_core::remote_control::RemoteControlPairingInvitationCode) -> Self {
        Self {
            value: RemoteControlSecretCode(zeroize::Zeroizing::new(value.value())),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlControllerPairingResponseReceived {
    pub delivered: RemoteControlPacketReceiptDelivered,
    pub admission: RemoteControlAdmitRemoteControlControllerPairingResponseOutcome,
    pub effect: RemoteControlControllerPairingResponseEffect,
}
impl From<prns_core::engine::RemoteControlControllerPairingResponseReceived>
    for RemoteControlControllerPairingResponseReceived
{
    fn from(value: prns_core::engine::RemoteControlControllerPairingResponseReceived) -> Self {
        Self {
            delivered: value.delivered.into(),
            admission: value.admission.into(),
            effect: value.effect.into(),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlPacketReceiptDelivered {
    pub rtt: RemoteControlRttMillis,
    pub evidence: RemoteControlDeliveryEvidence,
}
impl From<prns_core::engine::PacketReceiptDelivered> for RemoteControlPacketReceiptDelivered {
    fn from(value: prns_core::engine::PacketReceiptDelivered) -> Self {
        Self {
            rtt: value.rtt.into(),
            evidence: value.evidence.into(),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlRttMillis {
    pub value: u64,
}
impl From<prns_core::units::RttMillis> for RemoteControlRttMillis {
    fn from(value: prns_core::units::RttMillis) -> Self {
        Self {
            value: value.millis(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlDeliveryEvidence {
    Proof { value: RemoteControlDeliveryProof },
    Response,
}
impl From<prns_core::engine::DeliveryEvidence> for RemoteControlDeliveryEvidence {
    fn from(value: prns_core::engine::DeliveryEvidence) -> Self {
        match value {
            prns_core::engine::DeliveryEvidence::Proof(value) => {
                RemoteControlDeliveryEvidence::Proof {
                    value: value.into(),
                }
            }
            prns_core::engine::DeliveryEvidence::Response => {
                RemoteControlDeliveryEvidence::Response
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlDeliveryProof {
    Explicit { value: RemoteControlPacketHash },
    Implicit { value: RemoteControlPacketHash },
}
impl From<prns_core::engine::DeliveryProof> for RemoteControlDeliveryProof {
    fn from(value: prns_core::engine::DeliveryProof) -> Self {
        match value {
            prns_core::engine::DeliveryProof::Explicit(value) => {
                RemoteControlDeliveryProof::Explicit {
                    value: value.into(),
                }
            }
            prns_core::engine::DeliveryProof::Implicit(value) => {
                RemoteControlDeliveryProof::Implicit {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlPacketHash {
    pub value: Vec<u8>,
}
impl From<prns_core::routing::dedup::PacketHash> for RemoteControlPacketHash {
    fn from(value: prns_core::routing::dedup::PacketHash) -> Self {
        Self {
            value: value.as_bytes().to_vec(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlAdmitRemoteControlControllerPairingResponseOutcome {
    NoActivePairing,
    UnrelatedLink {
        expected: RemoteControlLinkId,
        received: RemoteControlLinkId,
    },
    MalformedEnvelope {
        value: RemoteControlPackedBinaryParseError,
    },
    MalformedResponse {
        value: RemoteControlPairingMessageParseError,
    },
    Offer {
        value: RemoteControlReceiveRemoteControlControllerPairingOfferOutcome,
    },
    Completed {
        value: RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome,
    },
    InvariantViolation {
        value: RemoteControlControllerPairingResponseBridgeInvariantViolation,
    },
}
impl From<prns_core::engine::AdmitRemoteControlControllerPairingResponseOutcome>
    for RemoteControlAdmitRemoteControlControllerPairingResponseOutcome
{
    fn from(value: prns_core::engine::AdmitRemoteControlControllerPairingResponseOutcome) -> Self {
        match value {
prns_core::engine::AdmitRemoteControlControllerPairingResponseOutcome::NoActivePairing => RemoteControlAdmitRemoteControlControllerPairingResponseOutcome::NoActivePairing,
prns_core::engine::AdmitRemoteControlControllerPairingResponseOutcome::UnrelatedLink { expected, received } => RemoteControlAdmitRemoteControlControllerPairingResponseOutcome::UnrelatedLink { expected: expected.into(), received: received.into() },
prns_core::engine::AdmitRemoteControlControllerPairingResponseOutcome::MalformedEnvelope(value) => RemoteControlAdmitRemoteControlControllerPairingResponseOutcome::MalformedEnvelope { value: value.into() },
prns_core::engine::AdmitRemoteControlControllerPairingResponseOutcome::MalformedResponse(value) => RemoteControlAdmitRemoteControlControllerPairingResponseOutcome::MalformedResponse { value: value.into() },
prns_core::engine::AdmitRemoteControlControllerPairingResponseOutcome::Offer(value) => RemoteControlAdmitRemoteControlControllerPairingResponseOutcome::Offer { value: value.into() },
prns_core::engine::AdmitRemoteControlControllerPairingResponseOutcome::Completed(value) => RemoteControlAdmitRemoteControlControllerPairingResponseOutcome::Completed { value: value.into() },
prns_core::engine::AdmitRemoteControlControllerPairingResponseOutcome::InvariantViolation(value) => RemoteControlAdmitRemoteControlControllerPairingResponseOutcome::InvariantViolation { value: value.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPackedBinaryParseError {
    Truncated,
    NotBinary,
    LengthOutOfRange { declared: u32 },
    LengthMismatch { declared: u64, actual: u64 },
}
impl From<prns_core::routing::links::request::PackedBinaryParseError>
    for RemoteControlPackedBinaryParseError
{
    fn from(value: prns_core::routing::links::request::PackedBinaryParseError) -> Self {
        match value {
            prns_core::routing::links::request::PackedBinaryParseError::Truncated => {
                RemoteControlPackedBinaryParseError::Truncated
            }
            prns_core::routing::links::request::PackedBinaryParseError::NotBinary => {
                RemoteControlPackedBinaryParseError::NotBinary
            }
            prns_core::routing::links::request::PackedBinaryParseError::LengthOutOfRange {
                declared,
            } => RemoteControlPackedBinaryParseError::LengthOutOfRange { declared },
            prns_core::routing::links::request::PackedBinaryParseError::LengthMismatch {
                declared,
                actual,
            } => RemoteControlPackedBinaryParseError::LengthMismatch {
                declared: declared as u64,
                actual: actual as u64,
            },
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingMessageParseError {
    TooLong {
        actual: u64,
        maximum: u64,
    },
    Truncated,
    UnsupportedVersion {
        found: u8,
    },
    UnknownKind {
        found: u8,
    },
    UnexpectedKind {
        direction: RemoteControlPairingMessageDirection,
        found: RemoteControlPairingMessageKind,
    },
    InvalidSigningPublicKey {
        role: RemoteControlPairingIdentityRole,
    },
    UnknownRequestKind {
        found: u8,
    },
    UnknownAuthority {
        found: u8,
    },
    TooManyPermissionsForVersion {
        version: RemoteControlPairingProtocolVersion,
        actual: u64,
        maximum: u64,
    },
    RequestUnsupportedForVersion {
        version: RemoteControlPairingProtocolVersion,
        request: RemoteControlRequestKind,
    },
    NonCanonicalPermissions,
    InvalidPermissions {
        value: RemoteControlPairingPermissionsError,
    },
    InvalidAttemptTimeout {
        value: RemoteControlPairingAttemptTimeoutError,
    },
    TrailingBytes {
        actual: u64,
    },
}
impl From<prns_core::remote_control::RemoteControlPairingMessageParseError>
    for RemoteControlPairingMessageParseError
{
    fn from(value: prns_core::remote_control::RemoteControlPairingMessageParseError) -> Self {
        match value {
prns_core::remote_control::RemoteControlPairingMessageParseError::TooLong { actual, maximum } => RemoteControlPairingMessageParseError::TooLong { actual: actual as u64, maximum: maximum as u64 },
prns_core::remote_control::RemoteControlPairingMessageParseError::Truncated => RemoteControlPairingMessageParseError::Truncated,
prns_core::remote_control::RemoteControlPairingMessageParseError::UnsupportedVersion { found } => RemoteControlPairingMessageParseError::UnsupportedVersion { found },
prns_core::remote_control::RemoteControlPairingMessageParseError::UnknownKind { found } => RemoteControlPairingMessageParseError::UnknownKind { found },
prns_core::remote_control::RemoteControlPairingMessageParseError::UnexpectedKind { direction, found } => RemoteControlPairingMessageParseError::UnexpectedKind { direction: direction.into(), found: found.into() },
prns_core::remote_control::RemoteControlPairingMessageParseError::InvalidSigningPublicKey { role } => RemoteControlPairingMessageParseError::InvalidSigningPublicKey { role: role.into() },
prns_core::remote_control::RemoteControlPairingMessageParseError::UnknownRequestKind { found } => RemoteControlPairingMessageParseError::UnknownRequestKind { found },
prns_core::remote_control::RemoteControlPairingMessageParseError::UnknownAuthority { found } => RemoteControlPairingMessageParseError::UnknownAuthority { found },
prns_core::remote_control::RemoteControlPairingMessageParseError::TooManyPermissionsForVersion { version, actual, maximum } => RemoteControlPairingMessageParseError::TooManyPermissionsForVersion { version: version.into(), actual: actual as u64, maximum: maximum as u64 },
prns_core::remote_control::RemoteControlPairingMessageParseError::RequestUnsupportedForVersion { version, request } => RemoteControlPairingMessageParseError::RequestUnsupportedForVersion { version: version.into(), request: request.into() },
prns_core::remote_control::RemoteControlPairingMessageParseError::NonCanonicalPermissions => RemoteControlPairingMessageParseError::NonCanonicalPermissions,
prns_core::remote_control::RemoteControlPairingMessageParseError::InvalidPermissions(value) => RemoteControlPairingMessageParseError::InvalidPermissions { value: value.into() },
prns_core::remote_control::RemoteControlPairingMessageParseError::InvalidAttemptTimeout(value) => RemoteControlPairingMessageParseError::InvalidAttemptTimeout { value: value.into() },
prns_core::remote_control::RemoteControlPairingMessageParseError::TrailingBytes { actual } => RemoteControlPairingMessageParseError::TrailingBytes { actual: actual as u64 },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingMessageDirection {
    Request,
    Response,
}
impl From<prns_core::remote_control::RemoteControlPairingMessageDirection>
    for RemoteControlPairingMessageDirection
{
    fn from(value: prns_core::remote_control::RemoteControlPairingMessageDirection) -> Self {
        match value {
            prns_core::remote_control::RemoteControlPairingMessageDirection::Request => {
                RemoteControlPairingMessageDirection::Request
            }
            prns_core::remote_control::RemoteControlPairingMessageDirection::Response => {
                RemoteControlPairingMessageDirection::Response
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingMessageKind {
    Begin,
    Offer,
    Commit,
    Completed,
}
impl From<prns_core::remote_control::RemoteControlPairingMessageKind>
    for RemoteControlPairingMessageKind
{
    fn from(value: prns_core::remote_control::RemoteControlPairingMessageKind) -> Self {
        match value {
            prns_core::remote_control::RemoteControlPairingMessageKind::Begin => {
                RemoteControlPairingMessageKind::Begin
            }
            prns_core::remote_control::RemoteControlPairingMessageKind::Offer => {
                RemoteControlPairingMessageKind::Offer
            }
            prns_core::remote_control::RemoteControlPairingMessageKind::Commit => {
                RemoteControlPairingMessageKind::Commit
            }
            prns_core::remote_control::RemoteControlPairingMessageKind::Completed => {
                RemoteControlPairingMessageKind::Completed
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingIdentityRole {
    Controller,
    Target,
}
impl From<prns_core::remote_control::RemoteControlPairingIdentityRole>
    for RemoteControlPairingIdentityRole
{
    fn from(value: prns_core::remote_control::RemoteControlPairingIdentityRole) -> Self {
        match value {
            prns_core::remote_control::RemoteControlPairingIdentityRole::Controller => {
                RemoteControlPairingIdentityRole::Controller
            }
            prns_core::remote_control::RemoteControlPairingIdentityRole::Target => {
                RemoteControlPairingIdentityRole::Target
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingProtocolVersion {
    V2,
    V3,
    V4,
}
impl From<prns_core::remote_control::RemoteControlPairingProtocolVersion>
    for RemoteControlPairingProtocolVersion
{
    fn from(value: prns_core::remote_control::RemoteControlPairingProtocolVersion) -> Self {
        match value {
            prns_core::remote_control::RemoteControlPairingProtocolVersion::V2 => {
                RemoteControlPairingProtocolVersion::V2
            }
            prns_core::remote_control::RemoteControlPairingProtocolVersion::V3 => {
                RemoteControlPairingProtocolVersion::V3
            }
            prns_core::remote_control::RemoteControlPairingProtocolVersion::V4 => {
                RemoteControlPairingProtocolVersion::V4
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingPermissionsError {
    NoPermittedRequests,
    AdministratorRequestRequiresAuthority { request: RemoteControlRequestKind },
}
impl From<prns_core::remote_control::RemoteControlPairingPermissionsError>
    for RemoteControlPairingPermissionsError
{
    fn from(value: prns_core::remote_control::RemoteControlPairingPermissionsError) -> Self {
        match value {
prns_core::remote_control::RemoteControlPairingPermissionsError::NoPermittedRequests => RemoteControlPairingPermissionsError::NoPermittedRequests,
prns_core::remote_control::RemoteControlPairingPermissionsError::AdministratorRequestRequiresAuthority { request } => RemoteControlPairingPermissionsError::AdministratorRequestRequiresAuthority { request: request.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingAttemptTimeoutError {
    Zero,
    TooLong {
        actual: RemoteControlDurationMillis,
        maximum: RemoteControlDurationMillis,
    },
}
impl From<prns_core::remote_control::RemoteControlPairingAttemptTimeoutError>
    for RemoteControlPairingAttemptTimeoutError
{
    fn from(value: prns_core::remote_control::RemoteControlPairingAttemptTimeoutError) -> Self {
        match value {
            prns_core::remote_control::RemoteControlPairingAttemptTimeoutError::Zero => {
                RemoteControlPairingAttemptTimeoutError::Zero
            }
            prns_core::remote_control::RemoteControlPairingAttemptTimeoutError::TooLong {
                actual,
                maximum,
            } => RemoteControlPairingAttemptTimeoutError::TooLong {
                actual: actual.into(),
                maximum: maximum.into(),
            },
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlDurationMillis {
    pub value: u64,
}
impl From<prns_core::units::DurationMillis> for RemoteControlDurationMillis {
    fn from(value: prns_core::units::DurationMillis) -> Self {
        Self { value: value.0 }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlReceiveRemoteControlControllerPairingOfferOutcome {
    ConfirmationRequired {
        attempt_id: RemoteControlPairingAttemptId,
    },
    Expired {
        expired: RemoteControlControllerPairingAborted,
    },
    Rejected {
        reason: RemoteControlPairingOfferVerificationError,
    },
    NoActiveAttempt,
    Unexpected {
        active: RemoteControlControllerPairingActivity,
    },
    PairingUnavailable {
        reason: RemoteControlControllerPairingAttemptWindowError,
    },
}
impl From<prns_core::remote_control::ReceiveRemoteControlControllerPairingOfferOutcome>
    for RemoteControlReceiveRemoteControlControllerPairingOfferOutcome
{
    fn from(
        value: prns_core::remote_control::ReceiveRemoteControlControllerPairingOfferOutcome,
    ) -> Self {
        match value {
prns_core::remote_control::ReceiveRemoteControlControllerPairingOfferOutcome::ConfirmationRequired { attempt_id } => RemoteControlReceiveRemoteControlControllerPairingOfferOutcome::ConfirmationRequired { attempt_id: attempt_id.into() },
prns_core::remote_control::ReceiveRemoteControlControllerPairingOfferOutcome::Expired { expired } => RemoteControlReceiveRemoteControlControllerPairingOfferOutcome::Expired { expired: expired.into() },
prns_core::remote_control::ReceiveRemoteControlControllerPairingOfferOutcome::Rejected { reason } => RemoteControlReceiveRemoteControlControllerPairingOfferOutcome::Rejected { reason: reason.into() },
prns_core::remote_control::ReceiveRemoteControlControllerPairingOfferOutcome::NoActiveAttempt => RemoteControlReceiveRemoteControlControllerPairingOfferOutcome::NoActiveAttempt,
prns_core::remote_control::ReceiveRemoteControlControllerPairingOfferOutcome::Unexpected { active } => RemoteControlReceiveRemoteControlControllerPairingOfferOutcome::Unexpected { active: active.into() },
prns_core::remote_control::ReceiveRemoteControlControllerPairingOfferOutcome::PairingUnavailable { reason } => RemoteControlReceiveRemoteControlControllerPairingOfferOutcome::PairingUnavailable { reason: reason.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingOfferVerificationError {
    ProtocolVersionMismatch {
        begin: RemoteControlPairingProtocolVersion,
        offer: RemoteControlPairingProtocolVersion,
    },
    InvalidTargetSignature,
}
impl From<prns_core::remote_control::RemoteControlPairingOfferVerificationError>
    for RemoteControlPairingOfferVerificationError
{
    fn from(value: prns_core::remote_control::RemoteControlPairingOfferVerificationError) -> Self {
        match value {
prns_core::remote_control::RemoteControlPairingOfferVerificationError::ProtocolVersionMismatch { begin, offer } => RemoteControlPairingOfferVerificationError::ProtocolVersionMismatch { begin: begin.into(), offer: offer.into() },
prns_core::remote_control::RemoteControlPairingOfferVerificationError::InvalidTargetSignature => RemoteControlPairingOfferVerificationError::InvalidTargetSignature,
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlControllerPairingActivity {
    AwaitingOffer,
    AwaitingApproval {
        attempt_id: RemoteControlPairingAttemptId,
    },
    AwaitingCompletion {
        attempt_id: RemoteControlPairingAttemptId,
    },
    Persisting {
        attempt_id: RemoteControlPairingAttemptId,
    },
}
impl From<prns_core::remote_control::RemoteControlControllerPairingActivity>
    for RemoteControlControllerPairingActivity
{
    fn from(value: prns_core::remote_control::RemoteControlControllerPairingActivity) -> Self {
        match value {
prns_core::remote_control::RemoteControlControllerPairingActivity::AwaitingOffer => RemoteControlControllerPairingActivity::AwaitingOffer,
prns_core::remote_control::RemoteControlControllerPairingActivity::AwaitingApproval { attempt_id } => RemoteControlControllerPairingActivity::AwaitingApproval { attempt_id: attempt_id.into() },
prns_core::remote_control::RemoteControlControllerPairingActivity::AwaitingCompletion { attempt_id } => RemoteControlControllerPairingActivity::AwaitingCompletion { attempt_id: attempt_id.into() },
prns_core::remote_control::RemoteControlControllerPairingActivity::Persisting { attempt_id } => RemoteControlControllerPairingActivity::Persisting { attempt_id: attempt_id.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlControllerPairingAttemptWindowError {
    PairingWindowElapsed {
        offered_at: RemoteControlInstantMillis,
        pairing_expires_at: RemoteControlInstantMillis,
    },
}
impl From<prns_core::remote_control::RemoteControlControllerPairingAttemptWindowError>
    for RemoteControlControllerPairingAttemptWindowError
{
    fn from(
        value: prns_core::remote_control::RemoteControlControllerPairingAttemptWindowError,
    ) -> Self {
        match value {
prns_core::remote_control::RemoteControlControllerPairingAttemptWindowError::PairingWindowElapsed { offered_at, pairing_expires_at } => RemoteControlControllerPairingAttemptWindowError::PairingWindowElapsed { offered_at: offered_at.into(), pairing_expires_at: pairing_expires_at.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome {
    PersistenceOwed {
        attempt_id: RemoteControlPairingAttemptId,
    },
    Expired {
        expired: RemoteControlControllerPairingAborted,
    },
    Rejected {
        attempt_id: RemoteControlPairingAttemptId,
        reason: RemoteControlPairingCompletedVerificationError,
    },
    NoActiveAttempt,
    OfferNotReceived,
    ApprovalRequired {
        attempt_id: RemoteControlPairingAttemptId,
    },
    AlreadyReceived {
        attempt_id: RemoteControlPairingAttemptId,
    },
}
impl From<prns_core::remote_control::ReceiveRemoteControlControllerPairingCompletedOutcome>
    for RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome
{
    fn from(
        value: prns_core::remote_control::ReceiveRemoteControlControllerPairingCompletedOutcome,
    ) -> Self {
        match value {
prns_core::remote_control::ReceiveRemoteControlControllerPairingCompletedOutcome::PersistenceOwed { attempt_id } => RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome::PersistenceOwed { attempt_id: attempt_id.into() },
prns_core::remote_control::ReceiveRemoteControlControllerPairingCompletedOutcome::Expired { expired } => RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome::Expired { expired: expired.into() },
prns_core::remote_control::ReceiveRemoteControlControllerPairingCompletedOutcome::Rejected { attempt_id, reason } => RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome::Rejected { attempt_id: attempt_id.into(), reason: reason.into() },
prns_core::remote_control::ReceiveRemoteControlControllerPairingCompletedOutcome::NoActiveAttempt => RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome::NoActiveAttempt,
prns_core::remote_control::ReceiveRemoteControlControllerPairingCompletedOutcome::OfferNotReceived => RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome::OfferNotReceived,
prns_core::remote_control::ReceiveRemoteControlControllerPairingCompletedOutcome::ApprovalRequired { attempt_id } => RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome::ApprovalRequired { attempt_id: attempt_id.into() },
prns_core::remote_control::ReceiveRemoteControlControllerPairingCompletedOutcome::AlreadyReceived { attempt_id } => RemoteControlReceiveRemoteControlControllerPairingCompletedOutcome::AlreadyReceived { attempt_id: attempt_id.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingCompletedVerificationError {
    ProtocolVersionMismatch {
        expected: RemoteControlPairingProtocolVersion,
        found: RemoteControlPairingProtocolVersion,
    },
    TranscriptMismatch {
        expected: RemoteControlPairingTranscriptDigest,
        found: RemoteControlPairingTranscriptDigest,
    },
    InvalidTargetSignature,
}
impl From<prns_core::remote_control::RemoteControlPairingCompletedVerificationError>
    for RemoteControlPairingCompletedVerificationError
{
    fn from(
        value: prns_core::remote_control::RemoteControlPairingCompletedVerificationError,
    ) -> Self {
        match value {
prns_core::remote_control::RemoteControlPairingCompletedVerificationError::ProtocolVersionMismatch { expected, found } => RemoteControlPairingCompletedVerificationError::ProtocolVersionMismatch { expected: expected.into(), found: found.into() },
prns_core::remote_control::RemoteControlPairingCompletedVerificationError::TranscriptMismatch { expected, found } => RemoteControlPairingCompletedVerificationError::TranscriptMismatch { expected: expected.into(), found: found.into() },
prns_core::remote_control::RemoteControlPairingCompletedVerificationError::InvalidTargetSignature => RemoteControlPairingCompletedVerificationError::InvalidTargetSignature,
}
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlPairingTranscriptDigest {
    pub value: Vec<u8>,
}
impl From<prns_core::remote_control::RemoteControlPairingTranscriptDigest>
    for RemoteControlPairingTranscriptDigest
{
    fn from(value: prns_core::remote_control::RemoteControlPairingTranscriptDigest) -> Self {
        Self {
            value: value.as_bytes().to_vec(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlControllerPairingResponseBridgeInvariantViolation {
    ConfirmationStateUnavailable {
        attempt_id: RemoteControlPairingAttemptId,
    },
    PersistenceStateUnavailable {
        attempt_id: RemoteControlPairingAttemptId,
    },
}
impl From<prns_core::engine::RemoteControlControllerPairingResponseBridgeInvariantViolation>
    for RemoteControlControllerPairingResponseBridgeInvariantViolation
{
    fn from(
        value: prns_core::engine::RemoteControlControllerPairingResponseBridgeInvariantViolation,
    ) -> Self {
        match value {
prns_core::engine::RemoteControlControllerPairingResponseBridgeInvariantViolation::ConfirmationStateUnavailable { attempt_id } => RemoteControlControllerPairingResponseBridgeInvariantViolation::ConfirmationStateUnavailable { attempt_id: attempt_id.into() },
prns_core::engine::RemoteControlControllerPairingResponseBridgeInvariantViolation::PersistenceStateUnavailable { attempt_id } => RemoteControlControllerPairingResponseBridgeInvariantViolation::PersistenceStateUnavailable { attempt_id: attempt_id.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlControllerPairingResponseEffect {
    Advanced,
    Expired {
        retired_link: RemoteControlLinkId,
    },
    NotAdvanced {
        value: RemoteControlFailRemoteControlControllerPairingRequestOutcome,
    },
}
impl From<prns_core::engine::RemoteControlControllerPairingResponseEffect>
    for RemoteControlControllerPairingResponseEffect
{
    fn from(value: prns_core::engine::RemoteControlControllerPairingResponseEffect) -> Self {
        match value {
            prns_core::engine::RemoteControlControllerPairingResponseEffect::Advanced => {
                RemoteControlControllerPairingResponseEffect::Advanced
            }
            prns_core::engine::RemoteControlControllerPairingResponseEffect::Expired {
                retired_link,
            } => RemoteControlControllerPairingResponseEffect::Expired {
                retired_link: retired_link.into(),
            },
            prns_core::engine::RemoteControlControllerPairingResponseEffect::NotAdvanced(value) => {
                RemoteControlControllerPairingResponseEffect::NotAdvanced {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlFailRemoteControlControllerPairingRequestOutcome {
    Aborted {
        aborted: RemoteControlControllerPairingAborted,
    },
    UnrelatedLink,
    NoActiveAttempt,
    PersistenceInProgress {
        attempt_id: RemoteControlPairingAttemptId,
    },
}
impl From<prns_core::remote_control::FailRemoteControlControllerPairingRequestOutcome>
    for RemoteControlFailRemoteControlControllerPairingRequestOutcome
{
    fn from(
        value: prns_core::remote_control::FailRemoteControlControllerPairingRequestOutcome,
    ) -> Self {
        match value {
prns_core::remote_control::FailRemoteControlControllerPairingRequestOutcome::Aborted { aborted } => RemoteControlFailRemoteControlControllerPairingRequestOutcome::Aborted { aborted: aborted.into() },
prns_core::remote_control::FailRemoteControlControllerPairingRequestOutcome::UnrelatedLink => RemoteControlFailRemoteControlControllerPairingRequestOutcome::UnrelatedLink,
prns_core::remote_control::FailRemoteControlControllerPairingRequestOutcome::NoActiveAttempt => RemoteControlFailRemoteControlControllerPairingRequestOutcome::NoActiveAttempt,
prns_core::remote_control::FailRemoteControlControllerPairingRequestOutcome::PersistenceInProgress { attempt_id } => RemoteControlFailRemoteControlControllerPairingRequestOutcome::PersistenceInProgress { attempt_id: attempt_id.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingControlErrorRemoteControlBeginRemoteControlControllerPairingControlFailure
{
    NodeStopped,
    Busy,
    Failed {
        value: RemoteControlBeginRemoteControlControllerPairingControlFailure,
    },
}
impl From<personal_rns::runtime::RemoteControlPairingControlError<personal_rns::runtime::BeginRemoteControlControllerPairingControlFailure>> for RemoteControlPairingControlErrorRemoteControlBeginRemoteControlControllerPairingControlFailure { fn from(value: personal_rns::runtime::RemoteControlPairingControlError<personal_rns::runtime::BeginRemoteControlControllerPairingControlFailure>) -> Self {
match value {
personal_rns::runtime::RemoteControlPairingControlError::<personal_rns::runtime::BeginRemoteControlControllerPairingControlFailure>::NodeStopped => RemoteControlPairingControlErrorRemoteControlBeginRemoteControlControllerPairingControlFailure::NodeStopped,
personal_rns::runtime::RemoteControlPairingControlError::<personal_rns::runtime::BeginRemoteControlControllerPairingControlFailure>::Busy => RemoteControlPairingControlErrorRemoteControlBeginRemoteControlControllerPairingControlFailure::Busy,
personal_rns::runtime::RemoteControlPairingControlError::<personal_rns::runtime::BeginRemoteControlControllerPairingControlFailure>::Failed(value) => RemoteControlPairingControlErrorRemoteControlBeginRemoteControlControllerPairingControlFailure::Failed { value: value.into() },
}
} }
#[derive(uniffi::Enum)]
pub enum RemoteControlBeginRemoteControlControllerPairingControlFailure {
    Begin {
        value: RemoteControlBeginRemoteControlControllerPairingFailure,
    },
    Identify {
        failure: RemoteControlSendErrorRemoteControlIdentifyFailure,
        cleanup: RemoteControlPairingLinkCleanupOutcome,
    },
    Request {
        value: RemoteControlControllerPairingRequestFailure,
    },
}
impl From<personal_rns::runtime::BeginRemoteControlControllerPairingControlFailure>
    for RemoteControlBeginRemoteControlControllerPairingControlFailure
{
    fn from(
        value: personal_rns::runtime::BeginRemoteControlControllerPairingControlFailure,
    ) -> Self {
        match value {
personal_rns::runtime::BeginRemoteControlControllerPairingControlFailure::Begin(value) => RemoteControlBeginRemoteControlControllerPairingControlFailure::Begin { value: value.into() },
personal_rns::runtime::BeginRemoteControlControllerPairingControlFailure::Identify { failure, cleanup } => RemoteControlBeginRemoteControlControllerPairingControlFailure::Identify { failure: failure.into(), cleanup: cleanup.into() },
personal_rns::runtime::BeginRemoteControlControllerPairingControlFailure::Request(value) => RemoteControlBeginRemoteControlControllerPairingControlFailure::Request { value: value.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlBeginRemoteControlControllerPairingFailure {
    ControllerIdentityUnavailable,
    Busy {
        active: RemoteControlControllerPairingActivity,
    },
    PairingUnavailable {
        reason: RemoteControlControllerPairingWindowError,
    },
    RequestBuild {
        failure: RemoteControlControllerPairingRequestBuildError,
        rollback: RemoteControlFailRemoteControlControllerPairingRequestOutcome,
    },
}
impl From<prns_core::engine::BeginRemoteControlControllerPairingFailure>
    for RemoteControlBeginRemoteControlControllerPairingFailure
{
    fn from(value: prns_core::engine::BeginRemoteControlControllerPairingFailure) -> Self {
        match value {
prns_core::engine::BeginRemoteControlControllerPairingFailure::ControllerIdentityUnavailable => RemoteControlBeginRemoteControlControllerPairingFailure::ControllerIdentityUnavailable,
prns_core::engine::BeginRemoteControlControllerPairingFailure::Busy { active } => RemoteControlBeginRemoteControlControllerPairingFailure::Busy { active: active.into() },
prns_core::engine::BeginRemoteControlControllerPairingFailure::PairingUnavailable { reason } => RemoteControlBeginRemoteControlControllerPairingFailure::PairingUnavailable { reason: reason.into() },
prns_core::engine::BeginRemoteControlControllerPairingFailure::RequestBuild { failure, rollback } => RemoteControlBeginRemoteControlControllerPairingFailure::RequestBuild { failure: failure.into(), rollback: rollback.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlControllerPairingWindowError {
    DeadlineNotFuture {
        started_at: RemoteControlInstantMillis,
        expires_at: RemoteControlInstantMillis,
    },
}
impl From<prns_core::remote_control::RemoteControlControllerPairingWindowError>
    for RemoteControlControllerPairingWindowError
{
    fn from(value: prns_core::remote_control::RemoteControlControllerPairingWindowError) -> Self {
        match value {
prns_core::remote_control::RemoteControlControllerPairingWindowError::DeadlineNotFuture { started_at, expires_at } => RemoteControlControllerPairingWindowError::DeadlineNotFuture { started_at: started_at.into(), expires_at: expires_at.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlControllerPairingRequestBuildError {
    Encode {
        value: RemoteControlPairingMessageWriteError,
    },
    Pack {
        value: RemoteControlPackBinaryError,
    },
    Capacity {
        required: u64,
        maximum: u64,
    },
}
impl From<prns_core::engine::RemoteControlControllerPairingRequestBuildError>
    for RemoteControlControllerPairingRequestBuildError
{
    fn from(value: prns_core::engine::RemoteControlControllerPairingRequestBuildError) -> Self {
        match value {
            prns_core::engine::RemoteControlControllerPairingRequestBuildError::Encode(value) => {
                RemoteControlControllerPairingRequestBuildError::Encode {
                    value: value.into(),
                }
            }
            prns_core::engine::RemoteControlControllerPairingRequestBuildError::Pack(value) => {
                RemoteControlControllerPairingRequestBuildError::Pack {
                    value: value.into(),
                }
            }
            prns_core::engine::RemoteControlControllerPairingRequestBuildError::Capacity {
                required,
                maximum,
            } => RemoteControlControllerPairingRequestBuildError::Capacity {
                required: required as u64,
                maximum: maximum as u64,
            },
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingMessageWriteError {
    BufferTooShort { required: u64, actual: u64 },
}
impl From<prns_core::remote_control::RemoteControlPairingMessageWriteError>
    for RemoteControlPairingMessageWriteError
{
    fn from(value: prns_core::remote_control::RemoteControlPairingMessageWriteError) -> Self {
        match value {
            prns_core::remote_control::RemoteControlPairingMessageWriteError::BufferTooShort {
                required,
                actual,
            } => RemoteControlPairingMessageWriteError::BufferTooShort {
                required: required as u64,
                actual: actual as u64,
            },
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPackBinaryError {
    BufferTooShort,
    LengthOutOfRange,
}
impl From<prns_core::routing::links::request::PackBinaryError> for RemoteControlPackBinaryError {
    fn from(value: prns_core::routing::links::request::PackBinaryError) -> Self {
        match value {
            prns_core::routing::links::request::PackBinaryError::BufferTooShort => {
                RemoteControlPackBinaryError::BufferTooShort
            }
            prns_core::routing::links::request::PackBinaryError::LengthOutOfRange => {
                RemoteControlPackBinaryError::LengthOutOfRange
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingLinkCleanupOutcome {
    Queued,
    NotQueued,
}
impl From<personal_rns::runtime::RemoteControlPairingLinkCleanupOutcome>
    for RemoteControlPairingLinkCleanupOutcome
{
    fn from(value: personal_rns::runtime::RemoteControlPairingLinkCleanupOutcome) -> Self {
        match value {
            personal_rns::runtime::RemoteControlPairingLinkCleanupOutcome::Queued => {
                RemoteControlPairingLinkCleanupOutcome::Queued
            }
            personal_rns::runtime::RemoteControlPairingLinkCleanupOutcome::NotQueued => {
                RemoteControlPairingLinkCleanupOutcome::NotQueued
            }
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlControllerPairingRequestFailure {
    pub cause: RemoteControlControllerPairingRequestFailureCause,
    pub exchange: RemoteControlFailRemoteControlControllerPairingRequestOutcome,
}
impl From<prns_core::engine::RemoteControlControllerPairingRequestFailure>
    for RemoteControlControllerPairingRequestFailure
{
    fn from(value: prns_core::engine::RemoteControlControllerPairingRequestFailure) -> Self {
        Self {
            cause: value.cause.into(),
            exchange: value.exchange.into(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlControllerPairingRequestFailureCause {
    Request {
        value: RemoteControlSendRequestFailure,
    },
    ResourceResponseUnsupported,
}
impl From<prns_core::engine::RemoteControlControllerPairingRequestFailureCause>
    for RemoteControlControllerPairingRequestFailureCause
{
    fn from(value: prns_core::engine::RemoteControlControllerPairingRequestFailureCause) -> Self {
        match value {
prns_core::engine::RemoteControlControllerPairingRequestFailureCause::Request(value) => RemoteControlControllerPairingRequestFailureCause::Request { value: value.into() },
prns_core::engine::RemoteControlControllerPairingRequestFailureCause::ResourceResponseUnsupported => RemoteControlControllerPairingRequestFailureCause::ResourceResponseUnsupported,
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlBeginRemoteControlControllerPairingControlError {
    NodeStopped,
    Busy,
    Failed {
        value: RemoteControlBeginRemoteControlControllerPairingControlFailure,
    },
}
impl From<personal_rns::runtime::BeginRemoteControlControllerPairingControlError>
    for RemoteControlBeginRemoteControlControllerPairingControlError
{
    fn from(value: personal_rns::runtime::BeginRemoteControlControllerPairingControlError) -> Self {
        match value {
            personal_rns::runtime::BeginRemoteControlControllerPairingControlError::NodeStopped => {
                RemoteControlBeginRemoteControlControllerPairingControlError::NodeStopped
            }
            personal_rns::runtime::BeginRemoteControlControllerPairingControlError::Busy => {
                RemoteControlBeginRemoteControlControllerPairingControlError::Busy
            }
            personal_rns::runtime::BeginRemoteControlControllerPairingControlError::Failed(
                value,
            ) => RemoteControlBeginRemoteControlControllerPairingControlError::Failed {
                value: value.into(),
            },
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlBeginRemoteControlControllerPairingSettlement {
    Completed {
        value: RemoteControlControllerPairingResponseReceived,
    },
    Failed {
        failure: RemoteControlBeginRemoteControlControllerPairingControlError,
    },
}
#[derive(uniffi::Record)]
pub struct RemoteControlApproveRemoteControlControllerPairing {
    pub attempt_id: RemoteControlPairingAttemptId,
}
impl TryFrom<RemoteControlApproveRemoteControlControllerPairing>
    for prns_core::engine::ApproveRemoteControlControllerPairing
{
    type Error = BindingError;
    fn try_from(
        value: RemoteControlApproveRemoteControlControllerPairing,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            attempt_id: value.attempt_id.try_into()?,
        })
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingControlErrorRemoteControlApproveRemoteControlControllerPairingControlFailure
{
    NodeStopped,
    Busy,
    Failed {
        value: RemoteControlApproveRemoteControlControllerPairingControlFailure,
    },
}
impl From<personal_rns::runtime::RemoteControlPairingControlError<personal_rns::runtime::ApproveRemoteControlControllerPairingControlFailure>> for RemoteControlPairingControlErrorRemoteControlApproveRemoteControlControllerPairingControlFailure { fn from(value: personal_rns::runtime::RemoteControlPairingControlError<personal_rns::runtime::ApproveRemoteControlControllerPairingControlFailure>) -> Self {
match value {
personal_rns::runtime::RemoteControlPairingControlError::<personal_rns::runtime::ApproveRemoteControlControllerPairingControlFailure>::NodeStopped => RemoteControlPairingControlErrorRemoteControlApproveRemoteControlControllerPairingControlFailure::NodeStopped,
personal_rns::runtime::RemoteControlPairingControlError::<personal_rns::runtime::ApproveRemoteControlControllerPairingControlFailure>::Busy => RemoteControlPairingControlErrorRemoteControlApproveRemoteControlControllerPairingControlFailure::Busy,
personal_rns::runtime::RemoteControlPairingControlError::<personal_rns::runtime::ApproveRemoteControlControllerPairingControlFailure>::Failed(value) => RemoteControlPairingControlErrorRemoteControlApproveRemoteControlControllerPairingControlFailure::Failed { value: value.into() },
}
} }
#[derive(uniffi::Enum)]
pub enum RemoteControlApproveRemoteControlControllerPairingControlFailure {
    Approve {
        value: RemoteControlApproveRemoteControlControllerPairingFailure,
    },
    Request {
        value: RemoteControlControllerPairingRequestFailure,
    },
}
impl From<personal_rns::runtime::ApproveRemoteControlControllerPairingControlFailure>
    for RemoteControlApproveRemoteControlControllerPairingControlFailure
{
    fn from(
        value: personal_rns::runtime::ApproveRemoteControlControllerPairingControlFailure,
    ) -> Self {
        match value {
            personal_rns::runtime::ApproveRemoteControlControllerPairingControlFailure::Approve(
                value,
            ) => RemoteControlApproveRemoteControlControllerPairingControlFailure::Approve {
                value: value.into(),
            },
            personal_rns::runtime::ApproveRemoteControlControllerPairingControlFailure::Request(
                value,
            ) => RemoteControlApproveRemoteControlControllerPairingControlFailure::Request {
                value: value.into(),
            },
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlApproveRemoteControlControllerPairingFailure {
    Expired {
        expired: RemoteControlControllerPairingAborted,
        retired_link: RemoteControlLinkId,
    },
    NoActiveAttempt,
    OfferNotReceived,
    AttemptMismatch {
        requested: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
    PersistenceInProgress {
        attempt_id: RemoteControlPairingAttemptId,
    },
    RequestBuild {
        failure: RemoteControlControllerPairingRequestBuildError,
        rollback: RemoteControlFailRemoteControlControllerPairingRequestOutcome,
    },
}
impl From<prns_core::engine::ApproveRemoteControlControllerPairingFailure>
    for RemoteControlApproveRemoteControlControllerPairingFailure
{
    fn from(value: prns_core::engine::ApproveRemoteControlControllerPairingFailure) -> Self {
        match value {
prns_core::engine::ApproveRemoteControlControllerPairingFailure::Expired { expired, retired_link } => RemoteControlApproveRemoteControlControllerPairingFailure::Expired { expired: expired.into(), retired_link: retired_link.into() },
prns_core::engine::ApproveRemoteControlControllerPairingFailure::NoActiveAttempt => RemoteControlApproveRemoteControlControllerPairingFailure::NoActiveAttempt,
prns_core::engine::ApproveRemoteControlControllerPairingFailure::OfferNotReceived => RemoteControlApproveRemoteControlControllerPairingFailure::OfferNotReceived,
prns_core::engine::ApproveRemoteControlControllerPairingFailure::AttemptMismatch { requested, active } => RemoteControlApproveRemoteControlControllerPairingFailure::AttemptMismatch { requested: requested.into(), active: active.into() },
prns_core::engine::ApproveRemoteControlControllerPairingFailure::PersistenceInProgress { attempt_id } => RemoteControlApproveRemoteControlControllerPairingFailure::PersistenceInProgress { attempt_id: attempt_id.into() },
prns_core::engine::ApproveRemoteControlControllerPairingFailure::RequestBuild { failure, rollback } => RemoteControlApproveRemoteControlControllerPairingFailure::RequestBuild { failure: failure.into(), rollback: rollback.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlApproveRemoteControlControllerPairingControlError {
    NodeStopped,
    Busy,
    Failed {
        value: RemoteControlApproveRemoteControlControllerPairingControlFailure,
    },
}
impl From<personal_rns::runtime::ApproveRemoteControlControllerPairingControlError>
    for RemoteControlApproveRemoteControlControllerPairingControlError
{
    fn from(
        value: personal_rns::runtime::ApproveRemoteControlControllerPairingControlError,
    ) -> Self {
        match value {
personal_rns::runtime::ApproveRemoteControlControllerPairingControlError::NodeStopped => RemoteControlApproveRemoteControlControllerPairingControlError::NodeStopped,
personal_rns::runtime::ApproveRemoteControlControllerPairingControlError::Busy => RemoteControlApproveRemoteControlControllerPairingControlError::Busy,
personal_rns::runtime::ApproveRemoteControlControllerPairingControlError::Failed(value) => RemoteControlApproveRemoteControlControllerPairingControlError::Failed { value: value.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlApproveRemoteControlControllerPairingSettlement {
    Completed {
        value: RemoteControlControllerPairingResponseReceived,
    },
    Failed {
        failure: RemoteControlApproveRemoteControlControllerPairingControlError,
    },
}
#[derive(uniffi::Record)]
pub struct RemoteControlRejectRemoteControlControllerPairing {
    pub attempt_id: RemoteControlPairingAttemptId,
}
impl TryFrom<RemoteControlRejectRemoteControlControllerPairing>
    for prns_core::engine::RejectRemoteControlControllerPairing
{
    type Error = BindingError;
    fn try_from(
        value: RemoteControlRejectRemoteControlControllerPairing,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            attempt_id: value.attempt_id.try_into()?,
        })
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlControllerPairingRejection {
    pub aborted: RemoteControlControllerPairingAborted,
    pub retired_link: RemoteControlLinkId,
}
impl From<prns_core::engine::RemoteControlControllerPairingRejection>
    for RemoteControlControllerPairingRejection
{
    fn from(value: prns_core::engine::RemoteControlControllerPairingRejection) -> Self {
        Self {
            aborted: value.aborted.into(),
            retired_link: value.retired_link.into(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingControlErrorRemoteControlRejectRemoteControlControllerPairingFailure {
    NodeStopped,
    Busy,
    Failed {
        value: RemoteControlRejectRemoteControlControllerPairingFailure,
    },
}
impl
    From<
        personal_rns::runtime::RemoteControlPairingControlError<
            prns_core::engine::RejectRemoteControlControllerPairingFailure,
        >,
    > for RemoteControlPairingControlErrorRemoteControlRejectRemoteControlControllerPairingFailure
{
    fn from(
        value: personal_rns::runtime::RemoteControlPairingControlError<
            prns_core::engine::RejectRemoteControlControllerPairingFailure,
        >,
    ) -> Self {
        match value {
personal_rns::runtime::RemoteControlPairingControlError::<prns_core::engine::RejectRemoteControlControllerPairingFailure>::NodeStopped => RemoteControlPairingControlErrorRemoteControlRejectRemoteControlControllerPairingFailure::NodeStopped,
personal_rns::runtime::RemoteControlPairingControlError::<prns_core::engine::RejectRemoteControlControllerPairingFailure>::Busy => RemoteControlPairingControlErrorRemoteControlRejectRemoteControlControllerPairingFailure::Busy,
personal_rns::runtime::RemoteControlPairingControlError::<prns_core::engine::RejectRemoteControlControllerPairingFailure>::Failed(value) => RemoteControlPairingControlErrorRemoteControlRejectRemoteControlControllerPairingFailure::Failed { value: value.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlRejectRemoteControlControllerPairingFailure {
    Expired {
        expired: RemoteControlControllerPairingAborted,
        retired_link: RemoteControlLinkId,
    },
    NoActiveAttempt,
    OfferNotReceived,
    AttemptMismatch {
        requested: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
    AlreadyApproved {
        attempt_id: RemoteControlPairingAttemptId,
    },
    PersistenceInProgress {
        attempt_id: RemoteControlPairingAttemptId,
    },
}
impl From<prns_core::engine::RejectRemoteControlControllerPairingFailure>
    for RemoteControlRejectRemoteControlControllerPairingFailure
{
    fn from(value: prns_core::engine::RejectRemoteControlControllerPairingFailure) -> Self {
        match value {
prns_core::engine::RejectRemoteControlControllerPairingFailure::Expired { expired, retired_link } => RemoteControlRejectRemoteControlControllerPairingFailure::Expired { expired: expired.into(), retired_link: retired_link.into() },
prns_core::engine::RejectRemoteControlControllerPairingFailure::NoActiveAttempt => RemoteControlRejectRemoteControlControllerPairingFailure::NoActiveAttempt,
prns_core::engine::RejectRemoteControlControllerPairingFailure::OfferNotReceived => RemoteControlRejectRemoteControlControllerPairingFailure::OfferNotReceived,
prns_core::engine::RejectRemoteControlControllerPairingFailure::AttemptMismatch { requested, active } => RemoteControlRejectRemoteControlControllerPairingFailure::AttemptMismatch { requested: requested.into(), active: active.into() },
prns_core::engine::RejectRemoteControlControllerPairingFailure::AlreadyApproved { attempt_id } => RemoteControlRejectRemoteControlControllerPairingFailure::AlreadyApproved { attempt_id: attempt_id.into() },
prns_core::engine::RejectRemoteControlControllerPairingFailure::PersistenceInProgress { attempt_id } => RemoteControlRejectRemoteControlControllerPairingFailure::PersistenceInProgress { attempt_id: attempt_id.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlRejectRemoteControlControllerPairingControlError {
    NodeStopped,
    Busy,
    Failed {
        value: RemoteControlRejectRemoteControlControllerPairingFailure,
    },
}
impl From<personal_rns::runtime::RejectRemoteControlControllerPairingControlError>
    for RemoteControlRejectRemoteControlControllerPairingControlError
{
    fn from(
        value: personal_rns::runtime::RejectRemoteControlControllerPairingControlError,
    ) -> Self {
        match value {
personal_rns::runtime::RejectRemoteControlControllerPairingControlError::NodeStopped => RemoteControlRejectRemoteControlControllerPairingControlError::NodeStopped,
personal_rns::runtime::RejectRemoteControlControllerPairingControlError::Busy => RemoteControlRejectRemoteControlControllerPairingControlError::Busy,
personal_rns::runtime::RejectRemoteControlControllerPairingControlError::Failed(value) => RemoteControlRejectRemoteControlControllerPairingControlError::Failed { value: value.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlRejectRemoteControlControllerPairingSettlement {
    Completed {
        value: RemoteControlControllerPairingRejection,
    },
    Failed {
        failure: RemoteControlRejectRemoteControlControllerPairingControlError,
    },
}
#[derive(uniffi::Record)]
pub struct RemoteControlApproveRemoteControlTargetPairing {
    pub attempt_id: RemoteControlPairingAttemptId,
}
impl TryFrom<RemoteControlApproveRemoteControlTargetPairing>
    for prns_core::engine::ApproveRemoteControlTargetPairing
{
    type Error = BindingError;
    fn try_from(
        value: RemoteControlApproveRemoteControlTargetPairing,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            attempt_id: value.attempt_id.try_into()?,
        })
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlTargetPairingApproval {
    AwaitingControllerCommit {
        attempt_id: RemoteControlPairingAttemptId,
    },
    AuthorizationOwed {
        attempt_id: RemoteControlPairingAttemptId,
        grant: RemoteControlControllerGrant,
    },
}
impl From<prns_core::engine::RemoteControlTargetPairingApproval>
    for RemoteControlTargetPairingApproval
{
    fn from(value: prns_core::engine::RemoteControlTargetPairingApproval) -> Self {
        match value {
            prns_core::engine::RemoteControlTargetPairingApproval::AwaitingControllerCommit {
                attempt_id,
            } => RemoteControlTargetPairingApproval::AwaitingControllerCommit {
                attempt_id: attempt_id.into(),
            },
            prns_core::engine::RemoteControlTargetPairingApproval::AuthorizationOwed {
                attempt_id,
                grant,
            } => RemoteControlTargetPairingApproval::AuthorizationOwed {
                attempt_id: attempt_id.into(),
                grant: grant.into(),
            },
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingControlErrorRemoteControlApproveRemoteControlTargetPairingFailure {
    NodeStopped,
    Busy,
    Failed {
        value: RemoteControlApproveRemoteControlTargetPairingFailure,
    },
}
impl
    From<
        personal_rns::runtime::RemoteControlPairingControlError<
            prns_core::engine::ApproveRemoteControlTargetPairingFailure,
        >,
    > for RemoteControlPairingControlErrorRemoteControlApproveRemoteControlTargetPairingFailure
{
    fn from(
        value: personal_rns::runtime::RemoteControlPairingControlError<
            prns_core::engine::ApproveRemoteControlTargetPairingFailure,
        >,
    ) -> Self {
        match value {
personal_rns::runtime::RemoteControlPairingControlError::<prns_core::engine::ApproveRemoteControlTargetPairingFailure>::NodeStopped => RemoteControlPairingControlErrorRemoteControlApproveRemoteControlTargetPairingFailure::NodeStopped,
personal_rns::runtime::RemoteControlPairingControlError::<prns_core::engine::ApproveRemoteControlTargetPairingFailure>::Busy => RemoteControlPairingControlErrorRemoteControlApproveRemoteControlTargetPairingFailure::Busy,
personal_rns::runtime::RemoteControlPairingControlError::<prns_core::engine::ApproveRemoteControlTargetPairingFailure>::Failed(value) => RemoteControlPairingControlErrorRemoteControlApproveRemoteControlTargetPairingFailure::Failed { value: value.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlApproveRemoteControlTargetPairingFailure {
    Expired {
        expired: RemoteControlTargetPairingAborted,
        retired_link: RemoteControlLinkId,
    },
    NoActiveAttempt,
    AttemptMismatch {
        requested: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
    OfferPendingDispatch {
        attempt_id: RemoteControlPairingAttemptId,
    },
    AlreadyApproved {
        attempt_id: RemoteControlPairingAttemptId,
    },
    FinalizationInProgress {
        attempt_id: RemoteControlPairingAttemptId,
    },
    CompletionRetentionExpired {
        attempt_id: RemoteControlPairingAttemptId,
        retired_link: RemoteControlLinkId,
    },
}
impl From<prns_core::engine::ApproveRemoteControlTargetPairingFailure>
    for RemoteControlApproveRemoteControlTargetPairingFailure
{
    fn from(value: prns_core::engine::ApproveRemoteControlTargetPairingFailure) -> Self {
        match value {
prns_core::engine::ApproveRemoteControlTargetPairingFailure::Expired { expired, retired_link } => RemoteControlApproveRemoteControlTargetPairingFailure::Expired { expired: expired.into(), retired_link: retired_link.into() },
prns_core::engine::ApproveRemoteControlTargetPairingFailure::NoActiveAttempt => RemoteControlApproveRemoteControlTargetPairingFailure::NoActiveAttempt,
prns_core::engine::ApproveRemoteControlTargetPairingFailure::AttemptMismatch { requested, active } => RemoteControlApproveRemoteControlTargetPairingFailure::AttemptMismatch { requested: requested.into(), active: active.into() },
prns_core::engine::ApproveRemoteControlTargetPairingFailure::OfferPendingDispatch { attempt_id } => RemoteControlApproveRemoteControlTargetPairingFailure::OfferPendingDispatch { attempt_id: attempt_id.into() },
prns_core::engine::ApproveRemoteControlTargetPairingFailure::AlreadyApproved { attempt_id } => RemoteControlApproveRemoteControlTargetPairingFailure::AlreadyApproved { attempt_id: attempt_id.into() },
prns_core::engine::ApproveRemoteControlTargetPairingFailure::FinalizationInProgress { attempt_id } => RemoteControlApproveRemoteControlTargetPairingFailure::FinalizationInProgress { attempt_id: attempt_id.into() },
prns_core::engine::ApproveRemoteControlTargetPairingFailure::CompletionRetentionExpired { attempt_id, retired_link } => RemoteControlApproveRemoteControlTargetPairingFailure::CompletionRetentionExpired { attempt_id: attempt_id.into(), retired_link: retired_link.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlApproveRemoteControlTargetPairingControlError {
    NodeStopped,
    Busy,
    Failed {
        value: RemoteControlApproveRemoteControlTargetPairingFailure,
    },
}
impl From<personal_rns::runtime::ApproveRemoteControlTargetPairingControlError>
    for RemoteControlApproveRemoteControlTargetPairingControlError
{
    fn from(value: personal_rns::runtime::ApproveRemoteControlTargetPairingControlError) -> Self {
        match value {
            personal_rns::runtime::ApproveRemoteControlTargetPairingControlError::NodeStopped => {
                RemoteControlApproveRemoteControlTargetPairingControlError::NodeStopped
            }
            personal_rns::runtime::ApproveRemoteControlTargetPairingControlError::Busy => {
                RemoteControlApproveRemoteControlTargetPairingControlError::Busy
            }
            personal_rns::runtime::ApproveRemoteControlTargetPairingControlError::Failed(value) => {
                RemoteControlApproveRemoteControlTargetPairingControlError::Failed {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlApproveRemoteControlTargetPairingSettlement {
    Completed {
        value: RemoteControlTargetPairingApproval,
    },
    Failed {
        failure: RemoteControlApproveRemoteControlTargetPairingControlError,
    },
}
#[derive(uniffi::Record)]
pub struct RemoteControlRejectRemoteControlTargetPairing {
    pub attempt_id: RemoteControlPairingAttemptId,
}
impl TryFrom<RemoteControlRejectRemoteControlTargetPairing>
    for prns_core::engine::RejectRemoteControlTargetPairing
{
    type Error = BindingError;
    fn try_from(value: RemoteControlRejectRemoteControlTargetPairing) -> Result<Self, Self::Error> {
        Ok(Self {
            attempt_id: value.attempt_id.try_into()?,
        })
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlTargetPairingRejection {
    pub aborted: RemoteControlTargetPairingAborted,
    pub retired_link: RemoteControlLinkId,
}
impl From<prns_core::engine::RemoteControlTargetPairingRejection>
    for RemoteControlTargetPairingRejection
{
    fn from(value: prns_core::engine::RemoteControlTargetPairingRejection) -> Self {
        Self {
            aborted: value.aborted.into(),
            retired_link: value.retired_link.into(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingControlErrorRemoteControlRejectRemoteControlTargetPairingFailure {
    NodeStopped,
    Busy,
    Failed {
        value: RemoteControlRejectRemoteControlTargetPairingFailure,
    },
}
impl
    From<
        personal_rns::runtime::RemoteControlPairingControlError<
            prns_core::engine::RejectRemoteControlTargetPairingFailure,
        >,
    > for RemoteControlPairingControlErrorRemoteControlRejectRemoteControlTargetPairingFailure
{
    fn from(
        value: personal_rns::runtime::RemoteControlPairingControlError<
            prns_core::engine::RejectRemoteControlTargetPairingFailure,
        >,
    ) -> Self {
        match value {
personal_rns::runtime::RemoteControlPairingControlError::<prns_core::engine::RejectRemoteControlTargetPairingFailure>::NodeStopped => RemoteControlPairingControlErrorRemoteControlRejectRemoteControlTargetPairingFailure::NodeStopped,
personal_rns::runtime::RemoteControlPairingControlError::<prns_core::engine::RejectRemoteControlTargetPairingFailure>::Busy => RemoteControlPairingControlErrorRemoteControlRejectRemoteControlTargetPairingFailure::Busy,
personal_rns::runtime::RemoteControlPairingControlError::<prns_core::engine::RejectRemoteControlTargetPairingFailure>::Failed(value) => RemoteControlPairingControlErrorRemoteControlRejectRemoteControlTargetPairingFailure::Failed { value: value.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlRejectRemoteControlTargetPairingFailure {
    Expired {
        expired: RemoteControlTargetPairingAborted,
        retired_link: RemoteControlLinkId,
    },
    NoActiveAttempt,
    AttemptMismatch {
        requested: RemoteControlPairingAttemptId,
        active: RemoteControlPairingAttemptId,
    },
    OfferPendingDispatch {
        attempt_id: RemoteControlPairingAttemptId,
    },
    AlreadyApproved {
        attempt_id: RemoteControlPairingAttemptId,
    },
    FinalizationInProgress {
        attempt_id: RemoteControlPairingAttemptId,
    },
    CompletionRetentionExpired {
        attempt_id: RemoteControlPairingAttemptId,
        retired_link: RemoteControlLinkId,
    },
}
impl From<prns_core::engine::RejectRemoteControlTargetPairingFailure>
    for RemoteControlRejectRemoteControlTargetPairingFailure
{
    fn from(value: prns_core::engine::RejectRemoteControlTargetPairingFailure) -> Self {
        match value {
prns_core::engine::RejectRemoteControlTargetPairingFailure::Expired { expired, retired_link } => RemoteControlRejectRemoteControlTargetPairingFailure::Expired { expired: expired.into(), retired_link: retired_link.into() },
prns_core::engine::RejectRemoteControlTargetPairingFailure::NoActiveAttempt => RemoteControlRejectRemoteControlTargetPairingFailure::NoActiveAttempt,
prns_core::engine::RejectRemoteControlTargetPairingFailure::AttemptMismatch { requested, active } => RemoteControlRejectRemoteControlTargetPairingFailure::AttemptMismatch { requested: requested.into(), active: active.into() },
prns_core::engine::RejectRemoteControlTargetPairingFailure::OfferPendingDispatch { attempt_id } => RemoteControlRejectRemoteControlTargetPairingFailure::OfferPendingDispatch { attempt_id: attempt_id.into() },
prns_core::engine::RejectRemoteControlTargetPairingFailure::AlreadyApproved { attempt_id } => RemoteControlRejectRemoteControlTargetPairingFailure::AlreadyApproved { attempt_id: attempt_id.into() },
prns_core::engine::RejectRemoteControlTargetPairingFailure::FinalizationInProgress { attempt_id } => RemoteControlRejectRemoteControlTargetPairingFailure::FinalizationInProgress { attempt_id: attempt_id.into() },
prns_core::engine::RejectRemoteControlTargetPairingFailure::CompletionRetentionExpired { attempt_id, retired_link } => RemoteControlRejectRemoteControlTargetPairingFailure::CompletionRetentionExpired { attempt_id: attempt_id.into(), retired_link: retired_link.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlRejectRemoteControlTargetPairingControlError {
    NodeStopped,
    Busy,
    Failed {
        value: RemoteControlRejectRemoteControlTargetPairingFailure,
    },
}
impl From<personal_rns::runtime::RejectRemoteControlTargetPairingControlError>
    for RemoteControlRejectRemoteControlTargetPairingControlError
{
    fn from(value: personal_rns::runtime::RejectRemoteControlTargetPairingControlError) -> Self {
        match value {
            personal_rns::runtime::RejectRemoteControlTargetPairingControlError::NodeStopped => {
                RemoteControlRejectRemoteControlTargetPairingControlError::NodeStopped
            }
            personal_rns::runtime::RejectRemoteControlTargetPairingControlError::Busy => {
                RemoteControlRejectRemoteControlTargetPairingControlError::Busy
            }
            personal_rns::runtime::RejectRemoteControlTargetPairingControlError::Failed(value) => {
                RemoteControlRejectRemoteControlTargetPairingControlError::Failed {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlRejectRemoteControlTargetPairingSettlement {
    Completed {
        value: RemoteControlTargetPairingRejection,
    },
    Failed {
        failure: RemoteControlRejectRemoteControlTargetPairingControlError,
    },
}
#[derive(uniffi::Record)]
pub struct RemoteControlTargetInventory {
    pub targets: Vec<RemoteControlIdentityHash>,
}
impl From<personal_rns::runtime::RemoteControlTargetInventory> for RemoteControlTargetInventory {
    fn from(value: personal_rns::runtime::RemoteControlTargetInventory) -> Self {
        Self {
            targets: value
                .targets()
                .iter()
                .map(|target| target.identity_hash().into())
                .collect(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlTargetInventoryControlError {
    NodeStopped,
    Busy,
    Unavailable,
    Inventory {
        value: RemoteControlTargetInventoryError,
    },
}
impl From<personal_rns::runtime::RemoteControlTargetInventoryControlError>
    for RemoteControlTargetInventoryControlError
{
    fn from(value: personal_rns::runtime::RemoteControlTargetInventoryControlError) -> Self {
        match value {
            personal_rns::runtime::RemoteControlTargetInventoryControlError::NodeStopped => {
                RemoteControlTargetInventoryControlError::NodeStopped
            }
            personal_rns::runtime::RemoteControlTargetInventoryControlError::Busy => {
                RemoteControlTargetInventoryControlError::Busy
            }
            personal_rns::runtime::RemoteControlTargetInventoryControlError::Unavailable => {
                RemoteControlTargetInventoryControlError::Unavailable
            }
            personal_rns::runtime::RemoteControlTargetInventoryControlError::Inventory(value) => {
                RemoteControlTargetInventoryControlError::Inventory {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlTargetInventoryError {
    CapacityInvariantViolation,
}
impl From<personal_rns::runtime::RemoteControlTargetInventoryError>
    for RemoteControlTargetInventoryError
{
    fn from(value: personal_rns::runtime::RemoteControlTargetInventoryError) -> Self {
        match value {
personal_rns::runtime::RemoteControlTargetInventoryError::CapacityInvariantViolation => RemoteControlTargetInventoryError::CapacityInvariantViolation,
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlTargetInventorySettlement {
    Completed {
        value: RemoteControlTargetInventory,
    },
    Failed {
        failure: RemoteControlTargetInventoryControlError,
    },
}
#[derive(uniffi::Record)]
pub struct RemoteControlResolvedRemoteControlTarget {
    pub target: RemoteControlIdentityHash,
    pub endpoint: RemoteControlEndpoint,
    pub controller: RemoteControlControllerIdentity,
    pub permitted_requests: RemoteControlRequestSet,
}
impl From<personal_rns::runtime::ResolvedRemoteControlTarget>
    for RemoteControlResolvedRemoteControlTarget
{
    fn from(value: personal_rns::runtime::ResolvedRemoteControlTarget) -> Self {
        Self {
            target: value.target().into(),
            endpoint: value.endpoint().into(),
            controller: (*value.controller()).into(),
            permitted_requests: (*value.permitted_requests()).into(),
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlEndpoint {
    pub destination_hash: RemoteControlDestinationHash,
}
impl From<prns_core::remote_control::RemoteControlEndpoint> for RemoteControlEndpoint {
    fn from(value: prns_core::remote_control::RemoteControlEndpoint) -> Self {
        Self {
            destination_hash: value.destination_hash().into(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlResolveRemoteControlTargetSettlement {
    Completed {
        value: RemoteControlResolvedRemoteControlTarget,
    },
    Failed {
        failure: RemoteControlResolveRemoteControlTargetControlError,
    },
}
#[derive(uniffi::Record)]
pub struct RemoteControlTargetAccess {
    pub target: RemoteControlTargetIdentity,
    pub authority: RemoteControlControllerAuthority,
    pub permitted_requests: RemoteControlRequestSet,
}
impl TryFrom<RemoteControlTargetAccess> for prns_core::remote_control::RemoteControlTargetAccess {
    type Error = BindingError;
    fn try_from(value: RemoteControlTargetAccess) -> Result<Self, Self::Error> {
        Ok(prns_core::remote_control::RemoteControlTargetAccess::new(
            value.target.try_into()?,
            value.authority.try_into()?,
            value.permitted_requests.try_into()?,
        )
        .map_err(|_| invalid("RemoteControlTargetAccess"))?)
    }
}
impl From<prns_core::remote_control::RemoteControlTargetAccess> for RemoteControlTargetAccess {
    fn from(value: prns_core::remote_control::RemoteControlTargetAccess) -> Self {
        Self {
            target: prns_core::remote_control::RemoteControlTargetIdentity::new(
                *value.target().public_keys(),
            )
            .into(),
            authority: value.authority().into(),
            permitted_requests: (*value.permitted_requests()).into(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlSetRemoteControlTargetAccessOutcome {
    Added,
    Unchanged,
    Updated { previous: RemoteControlTargetAccess },
}
impl From<prns_core::remote_control::SetRemoteControlTargetAccessOutcome>
    for RemoteControlSetRemoteControlTargetAccessOutcome
{
    fn from(value: prns_core::remote_control::SetRemoteControlTargetAccessOutcome) -> Self {
        match value {
            prns_core::remote_control::SetRemoteControlTargetAccessOutcome::Added => {
                RemoteControlSetRemoteControlTargetAccessOutcome::Added
            }
            prns_core::remote_control::SetRemoteControlTargetAccessOutcome::Unchanged => {
                RemoteControlSetRemoteControlTargetAccessOutcome::Unchanged
            }
            prns_core::remote_control::SetRemoteControlTargetAccessOutcome::Updated {
                previous,
            } => RemoteControlSetRemoteControlTargetAccessOutcome::Updated {
                previous: previous.into(),
            },
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlSetRemoteControlTargetAccessControlError {
    NodeStopped,
    Busy,
    Unavailable,
    CapacityExhausted,
}
impl From<personal_rns::runtime::SetRemoteControlTargetAccessControlError>
    for RemoteControlSetRemoteControlTargetAccessControlError
{
    fn from(value: personal_rns::runtime::SetRemoteControlTargetAccessControlError) -> Self {
        match value {
            personal_rns::runtime::SetRemoteControlTargetAccessControlError::NodeStopped => {
                RemoteControlSetRemoteControlTargetAccessControlError::NodeStopped
            }
            personal_rns::runtime::SetRemoteControlTargetAccessControlError::Busy => {
                RemoteControlSetRemoteControlTargetAccessControlError::Busy
            }
            personal_rns::runtime::SetRemoteControlTargetAccessControlError::Unavailable => {
                RemoteControlSetRemoteControlTargetAccessControlError::Unavailable
            }
            personal_rns::runtime::SetRemoteControlTargetAccessControlError::CapacityExhausted => {
                RemoteControlSetRemoteControlTargetAccessControlError::CapacityExhausted
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlSetRemoteControlTargetAccessSettlement {
    Completed {
        value: RemoteControlSetRemoteControlTargetAccessOutcome,
    },
    Failed {
        failure: RemoteControlSetRemoteControlTargetAccessControlError,
    },
}
#[derive(uniffi::Enum)]
pub enum RemoteControlForgetRemoteControlTargetOutcome {
    Forgotten { access: RemoteControlTargetAccess },
    NotFound,
}
impl From<prns_core::remote_control::ForgetRemoteControlTargetOutcome>
    for RemoteControlForgetRemoteControlTargetOutcome
{
    fn from(value: prns_core::remote_control::ForgetRemoteControlTargetOutcome) -> Self {
        match value {
            prns_core::remote_control::ForgetRemoteControlTargetOutcome::Forgotten { access } => {
                RemoteControlForgetRemoteControlTargetOutcome::Forgotten {
                    access: access.into(),
                }
            }
            prns_core::remote_control::ForgetRemoteControlTargetOutcome::NotFound => {
                RemoteControlForgetRemoteControlTargetOutcome::NotFound
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlForgetRemoteControlTargetControlError {
    NodeStopped,
    Busy,
    Unavailable,
}
impl From<personal_rns::runtime::ForgetRemoteControlTargetControlError>
    for RemoteControlForgetRemoteControlTargetControlError
{
    fn from(value: personal_rns::runtime::ForgetRemoteControlTargetControlError) -> Self {
        match value {
            personal_rns::runtime::ForgetRemoteControlTargetControlError::NodeStopped => {
                RemoteControlForgetRemoteControlTargetControlError::NodeStopped
            }
            personal_rns::runtime::ForgetRemoteControlTargetControlError::Busy => {
                RemoteControlForgetRemoteControlTargetControlError::Busy
            }
            personal_rns::runtime::ForgetRemoteControlTargetControlError::Unavailable => {
                RemoteControlForgetRemoteControlTargetControlError::Unavailable
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlForgetRemoteControlTargetSettlement {
    Completed {
        value: RemoteControlForgetRemoteControlTargetOutcome,
    },
    Failed {
        failure: RemoteControlForgetRemoteControlTargetControlError,
    },
}
#[derive(uniffi::Record)]
pub struct RemoteControlOpenRemoteControlPairing {
    pub target: RemoteControlEgressTarget,
    pub expires_after: RemoteControlPairingExpiresAfter,
    pub attempt_timeout: RemoteControlPairingAttemptTimeout,
    pub permissions: RemoteControlPairingPermissions,
    pub public_app_data: RemoteControlPairingPublicAppDataBytes,
}
impl TryFrom<RemoteControlOpenRemoteControlPairing>
    for prns_core::engine::OpenRemoteControlPairing
{
    type Error = BindingError;
    fn try_from(value: RemoteControlOpenRemoteControlPairing) -> Result<Self, Self::Error> {
        Ok(Self {
            target: value.target.try_into()?,
            expires_after: value.expires_after.try_into()?,
            attempt_timeout: value.attempt_timeout.try_into()?,
            permissions: value.permissions.try_into()?,
            public_app_data: value.public_app_data.try_into()?,
        })
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlEgressTarget {
    AllInterfaces,
    Interface { value: RemoteControlInterfaceId },
}
impl TryFrom<RemoteControlEgressTarget> for prns_core::engine::EgressTarget {
    type Error = BindingError;
    fn try_from(value: RemoteControlEgressTarget) -> Result<Self, Self::Error> {
        Ok(match value {
            RemoteControlEgressTarget::AllInterfaces => {
                prns_core::engine::EgressTarget::AllInterfaces
            }
            RemoteControlEgressTarget::Interface { value } => {
                prns_core::engine::EgressTarget::Interface(value.try_into()?)
            }
        })
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlPairingExpiresAfter {
    pub value: u64,
}
impl TryFrom<RemoteControlPairingExpiresAfter>
    for prns_core::remote_control::RemoteControlPairingExpiresAfter
{
    type Error = BindingError;
    fn try_from(value: RemoteControlPairingExpiresAfter) -> Result<Self, Self::Error> {
        Ok(
            prns_core::remote_control::RemoteControlPairingExpiresAfter::try_from(
                prns_core::units::DurationMillis(value.value),
            )
            .map_err(|_| invalid("RemoteControlPairingExpiresAfter"))?,
        )
    }
}
impl From<prns_core::remote_control::RemoteControlPairingExpiresAfter>
    for RemoteControlPairingExpiresAfter
{
    fn from(value: prns_core::remote_control::RemoteControlPairingExpiresAfter) -> Self {
        Self {
            value: value.duration().0,
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlPairingPublicAppDataBytes {
    pub value: Vec<u8>,
}
impl TryFrom<RemoteControlPairingPublicAppDataBytes>
    for prns_core::remote_control::RemoteControlPairingPublicAppDataBytes
{
    type Error = BindingError;
    fn try_from(value: RemoteControlPairingPublicAppDataBytes) -> Result<Self, Self::Error> {
        Ok(
            prns_core::remote_control::RemoteControlPairingPublicAppDataBytes::try_from(
                value.value.as_slice(),
            )
            .map_err(|_| invalid("RemoteControlPairingPublicAppDataBytes"))?,
        )
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlPairingOpened {
    pub endpoint: RemoteControlPairingEndpoint,
    pub expires_at: RemoteControlInstantMillis,
    pub invitation_code: RemoteControlPairingInvitationCode,
}
impl From<prns_core::engine::RemoteControlPairingOpened> for RemoteControlPairingOpened {
    fn from(value: prns_core::engine::RemoteControlPairingOpened) -> Self {
        Self {
            endpoint: value.endpoint.into(),
            expires_at: value.expires_at.into(),
            invitation_code: value.invitation_code.into(),
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingControlErrorRemoteControlOpenRemoteControlPairingFailure {
    NodeStopped,
    Busy,
    Failed {
        value: RemoteControlOpenRemoteControlPairingFailure,
    },
}
impl
    From<
        personal_rns::runtime::RemoteControlPairingControlError<
            prns_core::engine::OpenRemoteControlPairingFailure,
        >,
    > for RemoteControlPairingControlErrorRemoteControlOpenRemoteControlPairingFailure
{
    fn from(
        value: personal_rns::runtime::RemoteControlPairingControlError<
            prns_core::engine::OpenRemoteControlPairingFailure,
        >,
    ) -> Self {
        match value {
personal_rns::runtime::RemoteControlPairingControlError::<prns_core::engine::OpenRemoteControlPairingFailure>::NodeStopped => RemoteControlPairingControlErrorRemoteControlOpenRemoteControlPairingFailure::NodeStopped,
personal_rns::runtime::RemoteControlPairingControlError::<prns_core::engine::OpenRemoteControlPairingFailure>::Busy => RemoteControlPairingControlErrorRemoteControlOpenRemoteControlPairingFailure::Busy,
personal_rns::runtime::RemoteControlPairingControlError::<prns_core::engine::OpenRemoteControlPairingFailure>::Failed(value) => RemoteControlPairingControlErrorRemoteControlOpenRemoteControlPairingFailure::Failed { value: value.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlOpenRemoteControlPairingFailure {
    Rejected {
        value: RemoteControlOpenRemoteControlPairingRejection,
    },
    IdentityGenerationExhausted,
    HoldIdentity {
        value: RemoteControlHoldIdentityError,
    },
    RegisterEndpoint {
        value: RemoteControlRegisterDestinationError,
    },
    ConfigureRequestLimit,
    RegisterRequestEndpoint {
        value: RemoteControlTablePushError,
    },
    WriteAvailability {
        value: RemoteControlPairingAvailabilityWriteError,
    },
    PayloadCapacity,
    WritePacket {
        value: RemoteControlSendPlainPacketWriteError,
    },
}
impl From<prns_core::engine::OpenRemoteControlPairingFailure>
    for RemoteControlOpenRemoteControlPairingFailure
{
    fn from(value: prns_core::engine::OpenRemoteControlPairingFailure) -> Self {
        match value {
            prns_core::engine::OpenRemoteControlPairingFailure::Rejected(value) => {
                RemoteControlOpenRemoteControlPairingFailure::Rejected {
                    value: value.into(),
                }
            }
            prns_core::engine::OpenRemoteControlPairingFailure::IdentityGenerationExhausted => {
                RemoteControlOpenRemoteControlPairingFailure::IdentityGenerationExhausted
            }
            prns_core::engine::OpenRemoteControlPairingFailure::HoldIdentity(value) => {
                RemoteControlOpenRemoteControlPairingFailure::HoldIdentity {
                    value: value.into(),
                }
            }
            prns_core::engine::OpenRemoteControlPairingFailure::RegisterEndpoint(value) => {
                RemoteControlOpenRemoteControlPairingFailure::RegisterEndpoint {
                    value: value.into(),
                }
            }
            prns_core::engine::OpenRemoteControlPairingFailure::ConfigureRequestLimit => {
                RemoteControlOpenRemoteControlPairingFailure::ConfigureRequestLimit
            }
            prns_core::engine::OpenRemoteControlPairingFailure::RegisterRequestEndpoint(value) => {
                RemoteControlOpenRemoteControlPairingFailure::RegisterRequestEndpoint {
                    value: value.into(),
                }
            }
            prns_core::engine::OpenRemoteControlPairingFailure::WriteAvailability(value) => {
                RemoteControlOpenRemoteControlPairingFailure::WriteAvailability {
                    value: value.into(),
                }
            }
            prns_core::engine::OpenRemoteControlPairingFailure::PayloadCapacity => {
                RemoteControlOpenRemoteControlPairingFailure::PayloadCapacity
            }
            prns_core::engine::OpenRemoteControlPairingFailure::WritePacket(value) => {
                RemoteControlOpenRemoteControlPairingFailure::WritePacket {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlOpenRemoteControlPairingRejection {
    Unavailable,
    AlreadyOpen,
    NoTransmittingInterfaces,
    EgressTarget {
        value: RemoteControlEgressTargetRejection,
    },
    AttemptTimeoutExceedsWindow {
        attempt_timeout: RemoteControlPairingAttemptTimeout,
        pairing_expires_after: RemoteControlPairingExpiresAfter,
    },
    DeadlineOverflow,
}
impl From<prns_core::engine::OpenRemoteControlPairingRejection>
    for RemoteControlOpenRemoteControlPairingRejection
{
    fn from(value: prns_core::engine::OpenRemoteControlPairingRejection) -> Self {
        match value {
            prns_core::engine::OpenRemoteControlPairingRejection::Unavailable => {
                RemoteControlOpenRemoteControlPairingRejection::Unavailable
            }
            prns_core::engine::OpenRemoteControlPairingRejection::AlreadyOpen => {
                RemoteControlOpenRemoteControlPairingRejection::AlreadyOpen
            }
            prns_core::engine::OpenRemoteControlPairingRejection::NoTransmittingInterfaces => {
                RemoteControlOpenRemoteControlPairingRejection::NoTransmittingInterfaces
            }
            prns_core::engine::OpenRemoteControlPairingRejection::EgressTarget(value) => {
                RemoteControlOpenRemoteControlPairingRejection::EgressTarget {
                    value: value.into(),
                }
            }
            prns_core::engine::OpenRemoteControlPairingRejection::AttemptTimeoutExceedsWindow {
                attempt_timeout,
                pairing_expires_after,
            } => RemoteControlOpenRemoteControlPairingRejection::AttemptTimeoutExceedsWindow {
                attempt_timeout: attempt_timeout.into(),
                pairing_expires_after: pairing_expires_after.into(),
            },
            prns_core::engine::OpenRemoteControlPairingRejection::DeadlineOverflow => {
                RemoteControlOpenRemoteControlPairingRejection::DeadlineOverflow
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlEgressTargetRejection {
    UnknownInterface,
    InterfaceCannotTransmit,
}
impl From<prns_core::engine::EgressTargetRejection> for RemoteControlEgressTargetRejection {
    fn from(value: prns_core::engine::EgressTargetRejection) -> Self {
        match value {
            prns_core::engine::EgressTargetRejection::UnknownInterface => {
                RemoteControlEgressTargetRejection::UnknownInterface
            }
            prns_core::engine::EgressTargetRejection::InterfaceCannotTransmit => {
                RemoteControlEgressTargetRejection::InterfaceCannotTransmit
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlHoldIdentityError {
    StoreFull,
}
impl From<prns_core::identity::held::HoldIdentityError> for RemoteControlHoldIdentityError {
    fn from(value: prns_core::identity::held::HoldIdentityError) -> Self {
        match value {
            prns_core::identity::held::HoldIdentityError::StoreFull => {
                RemoteControlHoldIdentityError::StoreFull
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlRegisterDestinationError {
    Name { value: RemoteControlExpandNameError },
    RegistryFull,
    UnknownIdentity,
    RatchetTableFull,
    AppDataTooLong,
    InvalidGroupKey,
}
impl From<prns_core::routing::upstream_app_destinations::RegisterDestinationError>
    for RemoteControlRegisterDestinationError
{
    fn from(
        value: prns_core::routing::upstream_app_destinations::RegisterDestinationError,
    ) -> Self {
        match value {
prns_core::routing::upstream_app_destinations::RegisterDestinationError::Name(value) => RemoteControlRegisterDestinationError::Name { value: value.into() },
prns_core::routing::upstream_app_destinations::RegisterDestinationError::RegistryFull => RemoteControlRegisterDestinationError::RegistryFull,
prns_core::routing::upstream_app_destinations::RegisterDestinationError::UnknownIdentity => RemoteControlRegisterDestinationError::UnknownIdentity,
prns_core::routing::upstream_app_destinations::RegisterDestinationError::RatchetTableFull => RemoteControlRegisterDestinationError::RatchetTableFull,
prns_core::routing::upstream_app_destinations::RegisterDestinationError::AppDataTooLong => RemoteControlRegisterDestinationError::AppDataTooLong,
prns_core::routing::upstream_app_destinations::RegisterDestinationError::InvalidGroupKey => RemoteControlRegisterDestinationError::InvalidGroupKey,
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlExpandNameError {
    DotInComponent,
    NameTooLong,
}
impl From<prns_core::routing::announce::ExpandNameError> for RemoteControlExpandNameError {
    fn from(value: prns_core::routing::announce::ExpandNameError) -> Self {
        match value {
            prns_core::routing::announce::ExpandNameError::DotInComponent => {
                RemoteControlExpandNameError::DotInComponent
            }
            prns_core::routing::announce::ExpandNameError::NameTooLong => {
                RemoteControlExpandNameError::NameTooLong
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlTablePushError {
    TableFull,
}
impl From<prns_core::storage::TablePushError> for RemoteControlTablePushError {
    fn from(value: prns_core::storage::TablePushError) -> Self {
        match value {
            prns_core::storage::TablePushError::TableFull => RemoteControlTablePushError::TableFull,
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingAvailabilityWriteError {
    BufferTooShort {
        required: u64,
        actual: u64,
    },
    BuildAnnounce {
        value: RemoteControlAnnounceBuildError,
    },
    WriteAnnounce {
        value: RemoteControlWireError,
    },
}
impl From<prns_core::remote_control::RemoteControlPairingAvailabilityWriteError>
    for RemoteControlPairingAvailabilityWriteError
{
    fn from(value: prns_core::remote_control::RemoteControlPairingAvailabilityWriteError) -> Self {
        match value {
prns_core::remote_control::RemoteControlPairingAvailabilityWriteError::BufferTooShort { required, actual } => RemoteControlPairingAvailabilityWriteError::BufferTooShort { required: required as u64, actual: actual as u64 },
prns_core::remote_control::RemoteControlPairingAvailabilityWriteError::BuildAnnounce(value) => RemoteControlPairingAvailabilityWriteError::BuildAnnounce { value: value.into() },
prns_core::remote_control::RemoteControlPairingAvailabilityWriteError::WriteAnnounce(value) => RemoteControlPairingAvailabilityWriteError::WriteAnnounce { value: value.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlAnnounceBuildError {
    AnnounceTooLarge,
}
impl From<prns_core::routing::announce::AnnounceBuildError> for RemoteControlAnnounceBuildError {
    fn from(value: prns_core::routing::announce::AnnounceBuildError) -> Self {
        match value {
            prns_core::routing::announce::AnnounceBuildError::AnnounceTooLarge => {
                RemoteControlAnnounceBuildError::AnnounceTooLarge
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlWireError {
    BufferTooShort,
}
impl From<prns_core::wire::WireError> for RemoteControlWireError {
    fn from(value: prns_core::wire::WireError) -> Self {
        match value {
            prns_core::wire::WireError::BufferTooShort => RemoteControlWireError::BufferTooShort,
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlSendPlainPacketWriteError {
    Serialize,
}
impl From<prns_core::routing::delivery::send_plain::SendPlainPacketWriteError>
    for RemoteControlSendPlainPacketWriteError
{
    fn from(value: prns_core::routing::delivery::send_plain::SendPlainPacketWriteError) -> Self {
        match value {
            prns_core::routing::delivery::send_plain::SendPlainPacketWriteError::Serialize => {
                RemoteControlSendPlainPacketWriteError::Serialize
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlOpenRemoteControlPairingControlError {
    NodeStopped,
    Busy,
    Failed {
        value: RemoteControlOpenRemoteControlPairingFailure,
    },
}
impl From<personal_rns::runtime::OpenRemoteControlPairingControlError>
    for RemoteControlOpenRemoteControlPairingControlError
{
    fn from(value: personal_rns::runtime::OpenRemoteControlPairingControlError) -> Self {
        match value {
            personal_rns::runtime::OpenRemoteControlPairingControlError::NodeStopped => {
                RemoteControlOpenRemoteControlPairingControlError::NodeStopped
            }
            personal_rns::runtime::OpenRemoteControlPairingControlError::Busy => {
                RemoteControlOpenRemoteControlPairingControlError::Busy
            }
            personal_rns::runtime::OpenRemoteControlPairingControlError::Failed(value) => {
                RemoteControlOpenRemoteControlPairingControlError::Failed {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlOpenRemoteControlPairingSettlement {
    Completed {
        value: RemoteControlPairingOpened,
    },
    Failed {
        failure: RemoteControlOpenRemoteControlPairingControlError,
    },
}
#[derive(uniffi::Enum)]
pub enum RemoteControlCloseRemoteControlPairingOutcome {
    Closed {
        endpoint: RemoteControlPairingEndpoint,
    },
    AlreadyClosed,
}
impl From<prns_core::engine::CloseRemoteControlPairingOutcome>
    for RemoteControlCloseRemoteControlPairingOutcome
{
    fn from(value: prns_core::engine::CloseRemoteControlPairingOutcome) -> Self {
        match value {
            prns_core::engine::CloseRemoteControlPairingOutcome::Closed { endpoint } => {
                RemoteControlCloseRemoteControlPairingOutcome::Closed {
                    endpoint: endpoint.into(),
                }
            }
            prns_core::engine::CloseRemoteControlPairingOutcome::AlreadyClosed => {
                RemoteControlCloseRemoteControlPairingOutcome::AlreadyClosed
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlPairingControlErrorRemoteControlCloseRemoteControlPairingFailure {
    NodeStopped,
    Busy,
    Failed {
        value: RemoteControlCloseRemoteControlPairingFailure,
    },
}
impl
    From<
        personal_rns::runtime::RemoteControlPairingControlError<
            prns_core::engine::CloseRemoteControlPairingFailure,
        >,
    > for RemoteControlPairingControlErrorRemoteControlCloseRemoteControlPairingFailure
{
    fn from(
        value: personal_rns::runtime::RemoteControlPairingControlError<
            prns_core::engine::CloseRemoteControlPairingFailure,
        >,
    ) -> Self {
        match value {
personal_rns::runtime::RemoteControlPairingControlError::<prns_core::engine::CloseRemoteControlPairingFailure>::NodeStopped => RemoteControlPairingControlErrorRemoteControlCloseRemoteControlPairingFailure::NodeStopped,
personal_rns::runtime::RemoteControlPairingControlError::<prns_core::engine::CloseRemoteControlPairingFailure>::Busy => RemoteControlPairingControlErrorRemoteControlCloseRemoteControlPairingFailure::Busy,
personal_rns::runtime::RemoteControlPairingControlError::<prns_core::engine::CloseRemoteControlPairingFailure>::Failed(value) => RemoteControlPairingControlErrorRemoteControlCloseRemoteControlPairingFailure::Failed { value: value.into() },
}
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlCloseRemoteControlPairingFailure {
    Unavailable,
    RetirementIncomplete {
        first_remaining_link: RemoteControlLinkId,
        retired_links: RemoteControlLinkCount,
    },
    EndpointNotRegistered,
    IdentityNotHeld,
}
impl From<prns_core::engine::CloseRemoteControlPairingFailure>
    for RemoteControlCloseRemoteControlPairingFailure
{
    fn from(value: prns_core::engine::CloseRemoteControlPairingFailure) -> Self {
        match value {
            prns_core::engine::CloseRemoteControlPairingFailure::Unavailable => {
                RemoteControlCloseRemoteControlPairingFailure::Unavailable
            }
            prns_core::engine::CloseRemoteControlPairingFailure::RetirementIncomplete {
                first_remaining_link,
                retired_links,
            } => RemoteControlCloseRemoteControlPairingFailure::RetirementIncomplete {
                first_remaining_link: first_remaining_link.into(),
                retired_links: retired_links.into(),
            },
            prns_core::engine::CloseRemoteControlPairingFailure::EndpointNotRegistered => {
                RemoteControlCloseRemoteControlPairingFailure::EndpointNotRegistered
            }
            prns_core::engine::CloseRemoteControlPairingFailure::IdentityNotHeld => {
                RemoteControlCloseRemoteControlPairingFailure::IdentityNotHeld
            }
        }
    }
}
#[derive(uniffi::Record)]
pub struct RemoteControlLinkCount {
    pub value: u64,
}
impl From<prns_core::units::LinkCount> for RemoteControlLinkCount {
    fn from(value: prns_core::units::LinkCount) -> Self {
        Self {
            value: value.0 as u64,
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlCloseRemoteControlPairingControlError {
    NodeStopped,
    Busy,
    Failed {
        value: RemoteControlCloseRemoteControlPairingFailure,
    },
}
impl From<personal_rns::runtime::CloseRemoteControlPairingControlError>
    for RemoteControlCloseRemoteControlPairingControlError
{
    fn from(value: personal_rns::runtime::CloseRemoteControlPairingControlError) -> Self {
        match value {
            personal_rns::runtime::CloseRemoteControlPairingControlError::NodeStopped => {
                RemoteControlCloseRemoteControlPairingControlError::NodeStopped
            }
            personal_rns::runtime::CloseRemoteControlPairingControlError::Busy => {
                RemoteControlCloseRemoteControlPairingControlError::Busy
            }
            personal_rns::runtime::CloseRemoteControlPairingControlError::Failed(value) => {
                RemoteControlCloseRemoteControlPairingControlError::Failed {
                    value: value.into(),
                }
            }
        }
    }
}
#[derive(uniffi::Enum)]
pub enum RemoteControlCloseRemoteControlPairingSettlement {
    Completed {
        value: RemoteControlCloseRemoteControlPairingOutcome,
    },
    Failed {
        failure: RemoteControlCloseRemoteControlPairingControlError,
    },
}
use crate::facade::HostClientHandle;
#[uniffi::export]
impl HostClientHandle {
    pub async fn begin_remote_control_controller_pairing(
        &self,
        begin: RemoteControlBeginRemoteControlControllerPairing,
    ) -> Result<RemoteControlBeginRemoteControlControllerPairingSettlement, BindingError> {
        let begin: prns_core::engine::BeginRemoteControlControllerPairing = begin.try_into()?;
        let result = self.client.protocol_operation(move |handle| async move { personal_rns::runtime::RemoteControlPairingControl::begin_remote_control_controller_pairing(&handle, begin).await }).await.map_err(crate::facade::binding_error)?;
        Ok(match result {
            Ok(value) => RemoteControlBeginRemoteControlControllerPairingSettlement::Completed {
                value: value.into(),
            },
            Err(failure) => RemoteControlBeginRemoteControlControllerPairingSettlement::Failed {
                failure: failure.into(),
            },
        })
    }
    pub async fn approve_remote_control_controller_pairing(
        &self,
        approve: RemoteControlApproveRemoteControlControllerPairing,
    ) -> Result<RemoteControlApproveRemoteControlControllerPairingSettlement, BindingError> {
        let approve: prns_core::engine::ApproveRemoteControlControllerPairing =
            approve.try_into()?;
        let result = self.client.protocol_operation(move |handle| async move { personal_rns::runtime::RemoteControlPairingControl::approve_remote_control_controller_pairing(&handle, approve).await }).await.map_err(crate::facade::binding_error)?;
        Ok(match result {
            Ok(value) => RemoteControlApproveRemoteControlControllerPairingSettlement::Completed {
                value: value.into(),
            },
            Err(failure) => RemoteControlApproveRemoteControlControllerPairingSettlement::Failed {
                failure: failure.into(),
            },
        })
    }
    pub async fn reject_remote_control_controller_pairing(
        &self,
        reject: RemoteControlRejectRemoteControlControllerPairing,
    ) -> Result<RemoteControlRejectRemoteControlControllerPairingSettlement, BindingError> {
        let reject: prns_core::engine::RejectRemoteControlControllerPairing = reject.try_into()?;
        let result = self.client.protocol_operation(move |handle| async move { personal_rns::runtime::RemoteControlPairingControl::reject_remote_control_controller_pairing(&handle, reject).await }).await.map_err(crate::facade::binding_error)?;
        Ok(match result {
            Ok(value) => RemoteControlRejectRemoteControlControllerPairingSettlement::Completed {
                value: value.into(),
            },
            Err(failure) => RemoteControlRejectRemoteControlControllerPairingSettlement::Failed {
                failure: failure.into(),
            },
        })
    }
    pub async fn approve_remote_control_target_pairing(
        &self,
        approve: RemoteControlApproveRemoteControlTargetPairing,
    ) -> Result<RemoteControlApproveRemoteControlTargetPairingSettlement, BindingError> {
        let approve: prns_core::engine::ApproveRemoteControlTargetPairing = approve.try_into()?;
        let result = self.client.protocol_operation(move |handle| async move { personal_rns::runtime::RemoteControlPairingControl::approve_remote_control_target_pairing(&handle, approve).await }).await.map_err(crate::facade::binding_error)?;
        Ok(match result {
            Ok(value) => RemoteControlApproveRemoteControlTargetPairingSettlement::Completed {
                value: value.into(),
            },
            Err(failure) => RemoteControlApproveRemoteControlTargetPairingSettlement::Failed {
                failure: failure.into(),
            },
        })
    }
    pub async fn reject_remote_control_target_pairing(
        &self,
        reject: RemoteControlRejectRemoteControlTargetPairing,
    ) -> Result<RemoteControlRejectRemoteControlTargetPairingSettlement, BindingError> {
        let reject: prns_core::engine::RejectRemoteControlTargetPairing = reject.try_into()?;
        let result = self.client.protocol_operation(move |handle| async move { personal_rns::runtime::RemoteControlPairingControl::reject_remote_control_target_pairing(&handle, reject).await }).await.map_err(crate::facade::binding_error)?;
        Ok(match result {
            Ok(value) => RemoteControlRejectRemoteControlTargetPairingSettlement::Completed {
                value: value.into(),
            },
            Err(failure) => RemoteControlRejectRemoteControlTargetPairingSettlement::Failed {
                failure: failure.into(),
            },
        })
    }
    pub async fn remote_control_target_inventory(
        &self,
    ) -> Result<RemoteControlTargetInventorySettlement, BindingError> {
        let result = self.client.protocol_operation(move |handle| async move { personal_rns::runtime::RemoteControlTargetAccessControl::remote_control_target_inventory(&handle).await }).await.map_err(crate::facade::binding_error)?;
        Ok(match result {
            Ok(value) => RemoteControlTargetInventorySettlement::Completed {
                value: value.into(),
            },
            Err(failure) => RemoteControlTargetInventorySettlement::Failed {
                failure: failure.into(),
            },
        })
    }
    pub async fn resolve_remote_control_target(
        &self,
        target: RemoteControlIdentityHash,
    ) -> Result<RemoteControlResolveRemoteControlTargetSettlement, BindingError> {
        let target: prns_core::identity::IdentityHash = target.try_into()?;
        let result = self.client.protocol_operation(move |handle| async move { personal_rns::runtime::RemoteControlTargetAccessControl::resolve_remote_control_target(&handle, target).await }).await.map_err(crate::facade::binding_error)?;
        Ok(match result {
            Ok(value) => RemoteControlResolveRemoteControlTargetSettlement::Completed {
                value: value.into(),
            },
            Err(failure) => RemoteControlResolveRemoteControlTargetSettlement::Failed {
                failure: failure.into(),
            },
        })
    }
    pub async fn set_remote_control_target_access(
        &self,
        access: RemoteControlTargetAccess,
    ) -> Result<RemoteControlSetRemoteControlTargetAccessSettlement, BindingError> {
        let access: prns_core::remote_control::RemoteControlTargetAccess = access.try_into()?;
        let result = self.client.protocol_operation(move |handle| async move { personal_rns::runtime::RemoteControlTargetAccessControl::set_remote_control_target_access(&handle, access).await }).await.map_err(crate::facade::binding_error)?;
        Ok(match result {
            Ok(value) => RemoteControlSetRemoteControlTargetAccessSettlement::Completed {
                value: value.into(),
            },
            Err(failure) => RemoteControlSetRemoteControlTargetAccessSettlement::Failed {
                failure: failure.into(),
            },
        })
    }
    pub async fn forget_remote_control_target(
        &self,
        target: RemoteControlIdentityHash,
    ) -> Result<RemoteControlForgetRemoteControlTargetSettlement, BindingError> {
        let target: prns_core::identity::IdentityHash = target.try_into()?;
        let result = self.client.protocol_operation(move |handle| async move { personal_rns::runtime::RemoteControlTargetAccessControl::forget_remote_control_target(&handle, target).await }).await.map_err(crate::facade::binding_error)?;
        Ok(match result {
            Ok(value) => RemoteControlForgetRemoteControlTargetSettlement::Completed {
                value: value.into(),
            },
            Err(failure) => RemoteControlForgetRemoteControlTargetSettlement::Failed {
                failure: failure.into(),
            },
        })
    }
    pub async fn open_remote_control_pairing(
        &self,
        open: RemoteControlOpenRemoteControlPairing,
    ) -> Result<RemoteControlOpenRemoteControlPairingSettlement, BindingError> {
        let open: prns_core::engine::OpenRemoteControlPairing = open.try_into()?;
        let result = self
            .client
            .protocol_operation(move |handle| async move {
                personal_rns::runtime::PrnsNodeApi::open_remote_control_pairing(&handle, open).await
            })
            .await
            .map_err(crate::facade::binding_error)?;
        Ok(match result {
            Ok(value) => RemoteControlOpenRemoteControlPairingSettlement::Completed {
                value: value.into(),
            },
            Err(failure) => RemoteControlOpenRemoteControlPairingSettlement::Failed {
                failure: failure.into(),
            },
        })
    }
    pub async fn close_remote_control_pairing(
        &self,
    ) -> Result<RemoteControlCloseRemoteControlPairingSettlement, BindingError> {
        let result = self
            .client
            .protocol_operation(move |handle| async move {
                personal_rns::runtime::PrnsNodeApi::close_remote_control_pairing(&handle).await
            })
            .await
            .map_err(crate::facade::binding_error)?;
        Ok(match result {
            Ok(value) => RemoteControlCloseRemoteControlPairingSettlement::Completed {
                value: value.into(),
            },
            Err(failure) => RemoteControlCloseRemoteControlPairingSettlement::Failed {
                failure: failure.into(),
            },
        })
    }
}
