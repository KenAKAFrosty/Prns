//! The AX.25-KISS interface (RNS `AX25KISSInterface`): Reticulum packets wrapped in an AX.25 UI
//! frame and carried as KISS over a serial TNC. The wire framing and the startup TNC config are the
//! same KISS the plain [`kiss`](super::kiss) interface speaks — reused, not duplicated. What is
//! distinct lives in [`core`]: the fixed 16-byte AX.25 UI header (`APZRNS-0` destination, the
//! configured source callsign/SSID, control + PID) prepended to every packet and stripped on receive.

pub mod core;
