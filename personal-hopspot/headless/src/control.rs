//! Explicit Remote Control provisioning; no autonomous application announcements.
use personal_rns::identity::{PublicIdentityMaterial, IDENTITY_PUBLIC_KEY_LEN};
use personal_rns::prelude::*;
use personal_rns::remote_control::RemoteControlControllerAuthority;

#[derive(Debug, clap::Args)]
pub struct ControlOptions {
    /// Authorized controller public identity (128 hex characters); repeat for more controllers.
    /// Grants read-only inspection and AnnounceSelf. No value supplies no initial grants.
    #[arg(long, value_parser = controller)]
    pub controller_public_key: Vec<RemoteControlControllerIdentity>,
    /// Grants read-only inspection, AnnounceSelf and the bounded AppMessage probe.
    #[arg(long, value_parser = controller)]
    pub app_controller_public_key: Vec<RemoteControlControllerIdentity>,
    /// Grants read-only inspection plus a bounded interface-change stream.
    #[arg(long, value_parser = controller)]
    pub watch_controller_public_key: Vec<RemoteControlControllerIdentity>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("controller grants are invalid: {0:?}")]
    Grants(RemoteControlControllerGrantsError),
    #[error("controller permissions are invalid: {0:?}")]
    Permissions(RemoteControlControllerGrantError),
}

pub fn operator_requests() -> RemoteControlRequestSet {
    let mut requests = RemoteControlRequestSet::only(RemoteControlRequestKind::Describe);
    requests.insert(RemoteControlRequestKind::AnnounceSelf);
    requests.insert(RemoteControlRequestKind::DescribeBuild);
    requests.insert(RemoteControlRequestKind::InventoryInterfaces);
    requests.insert(RemoteControlRequestKind::InventoryInterfaceConfig);
    requests.insert(RemoteControlRequestKind::InventoryInterfacePeers);
    requests
}

pub fn app_operator_requests() -> RemoteControlRequestSet {
    let mut requests = operator_requests();
    requests.insert(RemoteControlRequestKind::AppMessage);
    requests
}

pub fn watch_operator_requests() -> RemoteControlRequestSet {
    let mut requests = operator_requests();
    requests.insert(RemoteControlRequestKind::WatchInterfaces);
    requests
}

pub fn public_identity(value: &str) -> Result<PublicIdentityMaterial, hex::FromHexError> {
    let mut bytes = [0; IDENTITY_PUBLIC_KEY_LEN];
    hex::decode_to_slice(value, &mut bytes)?;
    Ok(PublicIdentityMaterial::from_bytes(bytes))
}

fn controller(value: &str) -> Result<RemoteControlControllerIdentity, hex::FromHexError> {
    public_identity(value)
        .map(|material| RemoteControlControllerIdentity::new(material.public_keys()))
}

impl ControlOptions {
    pub fn grants(&self) -> Result<Vec<RemoteControlControllerGrant>, Error> {
        let mut requested = std::collections::BTreeMap::new();
        for (controllers, permissions) in [
            (&self.controller_public_key, operator_requests()),
            (&self.app_controller_public_key, app_operator_requests()),
            (&self.watch_controller_public_key, watch_operator_requests()),
        ] {
            for controller in controllers {
                let entry = requested
                    .entry(*controller.identity_hash().as_bytes())
                    .or_insert((*controller, operator_requests()));
                for permission in permissions.iter() {
                    entry.1.insert(permission);
                }
            }
        }
        let grants = requested
            .into_values()
            .map(|(controller, requests)| {
                RemoteControlControllerGrant::new(
                    controller,
                    RemoteControlControllerAuthority::Operator,
                    requests,
                )
                .map_err(Error::Permissions)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !grants.is_empty() {
            RemoteControlControllerGrants::try_from(grants.as_slice()).map_err(Error::Grants)?;
        }
        Ok(grants)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        control: ControlOptions,
    }

    #[test]
    fn grants_are_explicit_bounded_and_nonadministrative() {
        assert!(Cli::try_parse_from(["host"])
            .unwrap()
            .control
            .grants()
            .unwrap()
            .is_empty());
        assert!(Cli::try_parse_from(["host", "--controller-public-key", "12"]).is_err());
        let key = "21".repeat(64);
        let options = Cli::try_parse_from(["host", "--controller-public-key", &key]).unwrap();
        let grants = options.control.grants().unwrap();
        assert_eq!(
            grants[0].authority(),
            RemoteControlControllerAuthority::Operator
        );
        assert_eq!(grants[0].effective_requests(), operator_requests());
        assert!(!grants[0]
            .effective_requests()
            .supports(RemoteControlRequestKind::AppMessage));
        let app = Cli::try_parse_from(["host", "--app-controller-public-key", &key]).unwrap();
        let app_grants = app.control.grants().unwrap();
        assert_eq!(app_grants[0].effective_requests(), app_operator_requests());
        assert!(app_grants[0]
            .effective_requests()
            .supports(RemoteControlRequestKind::AppMessage));
        let watch = Cli::try_parse_from(["host", "--watch-controller-public-key", &key]).unwrap();
        let watch_grants = watch.control.grants().unwrap();
        assert_eq!(
            watch_grants[0].effective_requests(),
            watch_operator_requests()
        );
        assert!(!watch_grants[0]
            .effective_requests()
            .supports(RemoteControlRequestKind::AppMessage));
        let duplicate = Cli::try_parse_from([
            "host",
            "--controller-public-key",
            &key,
            "--controller-public-key",
            &key,
        ])
        .unwrap();
        assert_eq!(duplicate.control.grants().unwrap().len(), 1);
        let combined = Cli::try_parse_from([
            "host",
            "--app-controller-public-key",
            &key,
            "--watch-controller-public-key",
            &key,
        ])
        .unwrap()
        .control
        .grants()
        .unwrap();
        assert_eq!(combined.len(), 1);
        assert!(combined[0]
            .effective_requests()
            .supports(RemoteControlRequestKind::AppMessage));
        assert!(combined[0]
            .effective_requests()
            .supports(RemoteControlRequestKind::WatchInterfaces));
    }
}
