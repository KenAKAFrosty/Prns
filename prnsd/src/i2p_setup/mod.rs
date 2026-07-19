mod platform;
mod stanza;

use std::fmt;

use personal_rns::i2p::{DuplicateI2pPeer, I2pPeerAddress, SamBridgeAddress};

use crate::i2p_doctor::{
    self, I2pDoctorError, I2pDoctorReady, I2pDoctorRemediation, I2pDoctorRequest, RemoteSamAccess,
};
use platform::{
    BrowserOpenError, GuidancePage, SetupHost, LOCAL_SAM_CONFIGURATION_URL,
    OFFICIAL_SAM_GUIDE_URL,
};
use stanza::I2pInterfaceStanza;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SetupReachability {
    OutboundOnly,
    Connectable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BrowserPreference {
    PrintOnly,
    OpenApplicablePage,
}

#[derive(Debug)]
pub(super) struct I2pSetupRequest {
    doctor: I2pDoctorRequest,
    stanza: I2pInterfaceStanza,
    browser: BrowserPreference,
}

impl I2pSetupRequest {
    pub(super) fn new(
        address: SamBridgeAddress,
        remote_access: RemoteSamAccess,
        peers: impl IntoIterator<Item = I2pPeerAddress>,
        reachability: SetupReachability,
        browser: BrowserPreference,
    ) -> Result<Self, DuplicateI2pPeer> {
        Ok(Self {
            doctor: I2pDoctorRequest::new(address, remote_access),
            stanza: I2pInterfaceStanza::new(peers, reachability)?,
            browser,
        })
    }
}

#[derive(Debug)]
enum SetupReadiness {
    Ready(I2pDoctorReady),
    NeedsAction(I2pDoctorError),
}

impl SetupReadiness {
    const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    const fn remediation(&self) -> Option<I2pDoctorRemediation> {
        match self {
            Self::Ready(_) => None,
            Self::NeedsAction(error) => Some(error.remediation()),
        }
    }
}

#[derive(Debug)]
enum BrowserLaunch {
    NotRequested,
    NotApplicable,
    Opened(GuidancePage),
    Failed {
        page: GuidancePage,
        source: BrowserOpenError,
    },
}

#[derive(Debug)]
pub(super) struct I2pSetupReport {
    host: SetupHost,
    readiness: SetupReadiness,
    stanza: I2pInterfaceStanza,
    browser: BrowserLaunch,
}

impl I2pSetupReport {
    pub(super) const fn is_ready(&self) -> bool {
        self.readiness.is_ready()
    }
}

impl fmt::Display for I2pSetupReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "I2P setup for {}", self.host)?;
        match &self.readiness {
            SetupReadiness::Ready(ready) => {
                writeln!(formatter, "\n{ready}")?;
            }
            SetupReadiness::NeedsAction(error) => {
                writeln!(formatter, "\nI2P is not ready yet:\n  {error}")?;
                self.render_remediation(formatter, error.remediation())?;
            }
        }
        match &self.browser {
            BrowserLaunch::Opened(page) => {
                writeln!(formatter, "\nOpened {}", page.url())?;
            }
            BrowserLaunch::Failed { page, source } => {
                writeln!(
                    formatter,
                    "\nCould not open {}: {source}\nOpen that URL manually instead.",
                    page.url()
                )?;
            }
            BrowserLaunch::NotRequested | BrowserLaunch::NotApplicable => {}
        }
        writeln!(
            formatter,
            "\nPrns interface stanza (place this beneath `[interfaces]`):"
        )?;
        write!(formatter, "{}", self.stanza)?;
        if self.stanza.is_idle() {
            writeln!(
                formatter,
                "This starter stanza is valid but idle; add `--peer NAME_OR_DESTINATION` or `--connectable` when you are ready to exchange traffic."
            )?;
        }
        if self.stanza.is_connectable() {
            writeln!(
                formatter,
                "Connectable mode creates persistent I2P destination credentials in Prns storage; keep that storage private and backed up."
            )?;
        }
        formatter.write_str(
            "\nPrnsd did not download, install, elevate, enable services, or change router/firewall settings.\nI2P consumes local CPU, memory, storage, and bandwidth and may keep running independently. It protects I2P routing, but does not anonymize application content or conceal I2P use from every observer.",
        )
    }
}

impl I2pSetupReport {
    fn render_remediation(
        &self,
        formatter: &mut fmt::Formatter<'_>,
        remediation: I2pDoctorRemediation,
    ) -> fmt::Result {
        match remediation {
            I2pDoctorRemediation::InstallRouter => {
                let guidance = self.host.installation_guidance();
                write!(
                    formatter,
                    "\nInstall Java I2P:\n  {}\n  Official guidance: {}\n  Verify the current artifact, signature, and platform instructions on that page before installing.",
                    guidance.summary,
                    guidance.page.url()
                )
            }
            I2pDoctorRemediation::EnableSam => write!(
                formatter,
                "\nEnable SAM:\n  Open {LOCAL_SAM_CONFIGURATION_URL}\n  Start the SAM application bridge and configure it to start automatically, then rerun the doctor."
            ),
            I2pDoctorRemediation::InspectRouter => write!(
                formatter,
                "\nInspect the router and its SAM logs, then compare its settings with the official SAM guidance: {OFFICIAL_SAM_GUIDE_URL}"
            ),
            I2pDoctorRemediation::SecureSamPath => formatter.write_str(
                "\nKeep SAM on loopback or place it behind a trusted encrypted tunnel. The setup helper will not weaken this boundary.",
            ),
            I2pDoctorRemediation::RestoreHostEntropy => formatter.write_str(
                "\nRestore the operating system random source before creating any I2P destination credentials.",
            ),
        }
    }
}

pub(super) async fn run(request: I2pSetupRequest) -> I2pSetupReport {
    let host = SetupHost::detect();
    let readiness = match i2p_doctor::run(request.doctor).await {
        Ok(ready) => SetupReadiness::Ready(ready),
        Err(error) => SetupReadiness::NeedsAction(error),
    };
    let browser = launch_browser(host, request.browser, readiness.remediation());
    I2pSetupReport {
        host,
        readiness,
        stanza: request.stanza,
        browser,
    }
}

fn launch_browser(
    host: SetupHost,
    preference: BrowserPreference,
    remediation: Option<I2pDoctorRemediation>,
) -> BrowserLaunch {
    if preference == BrowserPreference::PrintOnly {
        return BrowserLaunch::NotRequested;
    }
    let page = match remediation {
        Some(I2pDoctorRemediation::InstallRouter) => host.installation_guidance().page,
        Some(I2pDoctorRemediation::EnableSam) => GuidancePage::SamConfiguration,
        _ => return BrowserLaunch::NotApplicable,
    };
    match host.open(page) {
        Ok(()) => BrowserLaunch::Opened(page),
        Err(source) => BrowserLaunch::Failed { page, source },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_selection_only_opens_actionable_fixed_pages() {
        let host = SetupHost::new(
            platform::HostOperatingSystem::MacOs,
            platform::HostArchitecture::AppleSilicon,
        );
        assert!(matches!(
            launch_browser(
                host,
                BrowserPreference::PrintOnly,
                Some(I2pDoctorRemediation::InstallRouter)
            ),
            BrowserLaunch::NotRequested
        ));
        assert!(matches!(
            launch_browser(
                host,
                BrowserPreference::OpenApplicablePage,
                Some(I2pDoctorRemediation::InspectRouter)
            ),
            BrowserLaunch::NotApplicable
        ));
    }

    #[test]
    fn incomplete_setup_reports_policy_stanza_and_non_mutation_together() {
        let report = I2pSetupReport {
            host: SetupHost::new(
                platform::HostOperatingSystem::MacOs,
                platform::HostArchitecture::AppleSilicon,
            ),
            readiness: SetupReadiness::NeedsAction(I2pDoctorError::UnsafeEndpoint(
                SamBridgeAddress::new("router.internal:7656").unwrap(),
            )),
            stanza: I2pInterfaceStanza::new(Vec::new(), SetupReachability::OutboundOnly)
                .unwrap(),
            browser: BrowserLaunch::NotApplicable,
        };

        let rendered = report.to_string();
        assert!(!report.is_ready());
        assert!(rendered.contains("Keep SAM on loopback"));
        assert!(rendered.contains("connectable = No"));
        assert!(rendered.contains("valid but idle"));
        assert!(rendered.contains("did not download, install, elevate"));
    }
}
