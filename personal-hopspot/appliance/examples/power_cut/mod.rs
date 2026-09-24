use std::io::Write;

use super::*;

const OPERATOR_WINDOW_SECONDS: u64 = 90;

pub(super) struct AwaitElectricalCut;

impl ObserveRadioWrites for AwaitElectricalCut {
    fn reached(&mut self, checkpoint: RadioCheckpoint) -> std::io::Result<()> {
        if checkpoint != RadioCheckpoint::VendorFileReplaced(RadioFile::Wireless) {
            return Ok(());
        }
        println!(
            "{}",
            serde_json::json!({
                "event": "qualification_power_cut_ready",
                "checkpoint": "wireless_replaced_before_mesh11sd",
                "operator_window_seconds": OPERATOR_WINDOW_SECONDS,
            })
        );
        std::io::stdout().flush()?;
        std::thread::sleep(std::time::Duration::from_secs(OPERATOR_WINDOW_SECONDS));
        Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "electrical power cut did not occur inside the qualification window",
        ))
    }
}
