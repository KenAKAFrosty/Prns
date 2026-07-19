use std::fmt;
use std::fs;
use std::io;
use std::process::{Command, Stdio};

const OS_RELEASE_ID: &str = "ID";
const OS_RELEASE_ID_LIKE: &str = "ID_LIKE";

pub(super) const OFFICIAL_DOWNLOADS_URL: &str = "https://i2p.net/en/downloads/";
pub(super) const OFFICIAL_WINDOWS_GUIDE_URL: &str =
    "https://i2p.net/en/docs/guides/installing-i2p-on-windows/";
pub(super) const OFFICIAL_DEBIAN_GUIDE_URL: &str =
    "https://i2p.net/en/docs/guides/installing-i2p-on-debian-and-ubuntu/";
pub(super) const OFFICIAL_SAM_GUIDE_URL: &str = "https://i2p.net/en/docs/api/samv3/";
pub(super) const LOCAL_SAM_CONFIGURATION_URL: &str = "http://127.0.0.1:7657/configclients";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HostArchitecture {
    AppleSilicon,
    X86_64,
    X86,
    Arm,
    Other(&'static str),
}

impl HostArchitecture {
    fn detect() -> Self {
        match std::env::consts::ARCH {
            "aarch64" if cfg!(target_os = "macos") => Self::AppleSilicon,
            "x86_64" => Self::X86_64,
            "x86" => Self::X86,
            "aarch64" | "arm" => Self::Arm,
            architecture => Self::Other(architecture),
        }
    }
}

impl fmt::Display for HostArchitecture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AppleSilicon => "Apple Silicon",
            Self::X86_64 => "x86-64",
            Self::X86 => "x86",
            Self::Arm => "ARM",
            Self::Other(architecture) => architecture,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LinuxFamily {
    Debian,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HostOperatingSystem {
    MacOs,
    Windows,
    Linux(LinuxFamily),
    Other(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SetupHost {
    operating_system: HostOperatingSystem,
    architecture: HostArchitecture,
}

impl SetupHost {
    pub(super) fn detect() -> Self {
        let operating_system = match std::env::consts::OS {
            "macos" => HostOperatingSystem::MacOs,
            "windows" => HostOperatingSystem::Windows,
            "linux" => HostOperatingSystem::Linux(detect_linux_family()),
            operating_system => HostOperatingSystem::Other(operating_system),
        };
        Self {
            operating_system,
            architecture: HostArchitecture::detect(),
        }
    }

    #[cfg(test)]
    pub(super) const fn new(
        operating_system: HostOperatingSystem,
        architecture: HostArchitecture,
    ) -> Self {
        Self {
            operating_system,
            architecture,
        }
    }

    pub(super) fn installation_guidance(self) -> InstallationGuidance {
        match (self.operating_system, self.architecture) {
            (HostOperatingSystem::Windows, _) => InstallationGuidance {
                summary: "The official Windows guide recommends the Easy Install Bundle for most users; it includes its own Java runtime.",
                page: GuidancePage::WindowsInstallation,
            },
            (HostOperatingSystem::MacOs, HostArchitecture::AppleSilicon) => InstallationGuidance {
                summary: "The official downloads page offers an Apple Silicon Easy Installer that includes its own Java runtime.",
                page: GuidancePage::OfficialDownloads,
            },
            (HostOperatingSystem::MacOs, _) => InstallationGuidance {
                summary: "Use the official Java installer path for macOS; the current router requires Java 8 or newer.",
                page: GuidancePage::OfficialDownloads,
            },
            (HostOperatingSystem::Linux(LinuxFamily::Debian), _) => InstallationGuidance {
                summary: "Follow the official Debian or Ubuntu repository guide and review each repository, key, and elevation step before running it.",
                page: GuidancePage::DebianInstallation,
            },
            (HostOperatingSystem::Linux(LinuxFamily::Other), _) => InstallationGuidance {
                summary: "Check your distribution packages first, or use the official Java installer path for Linux with Java 8 or newer.",
                page: GuidancePage::OfficialDownloads,
            },
            (HostOperatingSystem::Other(_), _) => InstallationGuidance {
                summary: "Consult the official I2P downloads page for a supported Java I2P installation on this platform.",
                page: GuidancePage::OfficialDownloads,
            },
        }
    }

    pub(super) fn open(self, page: GuidancePage) -> Result<(), BrowserOpenError> {
        let command = self
            .browser_command(page)
            .ok_or(BrowserOpenError::UnsupportedPlatform(self))?;
        Command::new(command.program)
            .args(command.arguments.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|source| BrowserOpenError::Spawn { command, source })
    }

    fn browser_command(self, page: GuidancePage) -> Option<BrowserCommand> {
        let url = page.url();
        match self.operating_system {
            HostOperatingSystem::MacOs => Some(BrowserCommand {
                program: "open",
                arguments: vec![url],
            }),
            HostOperatingSystem::Windows => Some(BrowserCommand {
                program: "cmd",
                arguments: vec!["/C", "start", "", url],
            }),
            HostOperatingSystem::Linux(_) => Some(BrowserCommand {
                program: "xdg-open",
                arguments: vec![url],
            }),
            HostOperatingSystem::Other(_) => None,
        }
    }
}

impl fmt::Display for SetupHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.operating_system {
            HostOperatingSystem::MacOs => write!(formatter, "macOS ({})", self.architecture),
            HostOperatingSystem::Windows => {
                write!(formatter, "Windows ({})", self.architecture)
            }
            HostOperatingSystem::Linux(LinuxFamily::Debian) => {
                write!(formatter, "Debian-family Linux ({})", self.architecture)
            }
            HostOperatingSystem::Linux(LinuxFamily::Other) => {
                write!(formatter, "Linux ({})", self.architecture)
            }
            HostOperatingSystem::Other(operating_system) => {
                write!(formatter, "{operating_system} ({})", self.architecture)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct InstallationGuidance {
    pub(super) summary: &'static str,
    pub(super) page: GuidancePage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GuidancePage {
    OfficialDownloads,
    WindowsInstallation,
    DebianInstallation,
    SamConfiguration,
}

impl GuidancePage {
    pub(super) const fn url(self) -> &'static str {
        match self {
            Self::OfficialDownloads => OFFICIAL_DOWNLOADS_URL,
            Self::WindowsInstallation => OFFICIAL_WINDOWS_GUIDE_URL,
            Self::DebianInstallation => OFFICIAL_DEBIAN_GUIDE_URL,
            Self::SamConfiguration => LOCAL_SAM_CONFIGURATION_URL,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BrowserCommand {
    program: &'static str,
    arguments: Vec<&'static str>,
}

#[derive(Debug)]
pub(super) enum BrowserOpenError {
    UnsupportedPlatform(SetupHost),
    Spawn {
        command: BrowserCommand,
        source: io::Error,
    },
}

impl fmt::Display for BrowserOpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform(host) => {
                write!(formatter, "no browser launcher is known for {host}")
            }
            Self::Spawn { command, source } => {
                write!(formatter, "could not run {}: {source}", command.program)
            }
        }
    }
}

fn detect_linux_family() -> LinuxFamily {
    fs::read_to_string("/etc/os-release")
        .ok()
        .map_or(LinuxFamily::Other, |source| linux_family(&source))
}

fn linux_family(source: &str) -> LinuxFamily {
    let id = os_release_value(source, OS_RELEASE_ID);
    let id_like = os_release_value(source, OS_RELEASE_ID_LIKE);
    if id.is_some_and(is_debian_family_name)
        || id_like.is_some_and(|families| families.split_whitespace().any(is_debian_family_name))
    {
        LinuxFamily::Debian
    } else {
        LinuxFamily::Other
    }
}

fn os_release_value<'a>(source: &'a str, key: &str) -> Option<&'a str> {
    source.lines().find_map(|line| {
        let (candidate, value) = line.split_once('=')?;
        (candidate == key).then(|| value.trim_matches(['\'', '"']))
    })
}

fn is_debian_family_name(value: &str) -> bool {
    matches!(value, "debian" | "ubuntu")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_distribution_detection_uses_id_and_id_like() {
        assert_eq!(linux_family("ID=debian\n"), LinuxFamily::Debian);
        assert_eq!(
            linux_family("ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n"),
            LinuxFamily::Debian
        );
        assert_eq!(linux_family("ID=fedora\n"), LinuxFamily::Other);
    }

    #[test]
    fn platform_guidance_tracks_current_official_install_paths() {
        let apple = SetupHost::new(HostOperatingSystem::MacOs, HostArchitecture::AppleSilicon)
            .installation_guidance();
        assert!(apple.summary.contains("Apple Silicon Easy Installer"));
        assert_eq!(apple.page.url(), OFFICIAL_DOWNLOADS_URL);

        let debian = SetupHost::new(
            HostOperatingSystem::Linux(LinuxFamily::Debian),
            HostArchitecture::X86_64,
        )
        .installation_guidance();
        assert_eq!(debian.page.url(), OFFICIAL_DEBIAN_GUIDE_URL);
    }

    #[test]
    fn browser_commands_are_platform_owned_and_use_only_fixed_urls() {
        let mac = SetupHost::new(HostOperatingSystem::MacOs, HostArchitecture::X86_64)
            .browser_command(GuidancePage::SamConfiguration)
            .unwrap();
        assert_eq!(mac.program, "open");
        assert_eq!(mac.arguments, vec![LOCAL_SAM_CONFIGURATION_URL]);

        let windows = SetupHost::new(HostOperatingSystem::Windows, HostArchitecture::X86_64)
            .browser_command(GuidancePage::WindowsInstallation)
            .unwrap();
        assert_eq!(windows.program, "cmd");
        assert_eq!(
            windows.arguments,
            vec!["/C", "start", "", OFFICIAL_WINDOWS_GUIDE_URL]
        );
    }
}
