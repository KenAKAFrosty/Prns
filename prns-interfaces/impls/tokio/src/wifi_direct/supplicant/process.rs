use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, Command};

use super::ctrl::WpaCtrlError;
use prns_core::interfaces::wifi_direct::DEVICE_NAME_MARKER;

const SOCKET_POLL: Duration = Duration::from_millis(200);
const SOCKET_ROUNDS: u32 = 40;

#[derive(Debug)]
pub enum SupplicantLaunchError {
    Config(std::io::Error),
    Spawn(std::io::Error),
    Exited,
    SocketTimeout,
    Attach(WpaCtrlError),
}

pub struct SupplicantProcess {
    child: Child,
    conf_path: PathBuf,
}

impl SupplicantProcess {
    pub async fn spawn(interface: &str) -> Result<(Self, PathBuf), SupplicantLaunchError> {
        let ctrl_dir = PathBuf::from(format!("/run/prns_wpa_{interface}"));
        let _ = std::fs::create_dir_all(&ctrl_dir);
        let socket = ctrl_dir.join(interface);
        let _ = std::fs::remove_file(&socket);
        let conf_path = std::env::temp_dir().join(format!("prns-wpa-{interface}.conf"));
        let config = format!(
            "ctrl_interface=DIR={dir} GROUP=netdev\n\
             update_config=0\n\
             device_name={name}\n\
             device_type=1-0050F204-1\n\
             p2p_go_intent=15\n",
            dir = ctrl_dir.display(),
            name = device_name(interface),
        );
        std::fs::write(&conf_path, config).map_err(SupplicantLaunchError::Config)?;
        let mut child = Command::new("wpa_supplicant")
            .arg("-Dnl80211")
            .arg("-i")
            .arg(interface)
            .arg("-c")
            .arg(&conf_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(SupplicantLaunchError::Spawn)?;
        for _ in 0..SOCKET_ROUNDS {
            if socket.exists() {
                return Ok((Self { child, conf_path }, ctrl_dir));
            }
            if matches!(child.try_wait(), Ok(Some(_))) {
                return Err(SupplicantLaunchError::Exited);
            }
            tokio::time::sleep(SOCKET_POLL).await;
        }
        Err(SupplicantLaunchError::SocketTimeout)
    }
}

impl Drop for SupplicantProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        let _ = std::fs::remove_file(&self.conf_path);
    }
}

fn device_name(interface: &str) -> String {
    let raw = std::fs::read_to_string(format!("/sys/class/net/{interface}/address"));
    let octets: Option<Vec<String>> = raw.ok().map(|line| {
        line.trim()
            .split(':')
            .map(str::to_owned)
            .collect::<Vec<_>>()
    });
    match octets {
        Some(parts) if parts.len() == 6 => {
            format!("{DEVICE_NAME_MARKER}-{}{}{}", parts[3], parts[4], parts[5])
        }
        _ => format!("{DEVICE_NAME_MARKER}-node"),
    }
}
