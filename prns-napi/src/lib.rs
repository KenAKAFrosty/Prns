mod errors;
mod events;
mod marshal;
mod node;

pub use node::start_node;

use napi::bindgen_prelude::Buffer;
use napi::Result;
use napi_derive::napi;

use crate::errors::{code_err, ErrorCode};

#[napi(object)]
pub struct BackendInfo {
    pub backend: String,
    pub capabilities: Vec<String>,
    pub interface_kinds: Vec<String>,
}

#[napi]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[napi]
pub fn host_contract_abi() -> u32 {
    prns_host::HOST_CONTRACT_ABI
}

#[napi]
pub fn host_schema_version() -> u32 {
    prns_host::HOST_SCHEMA_VERSION
}

#[napi]
pub fn backend_info() -> BackendInfo {
    let info = prns_host_native::native_backend_info();
    BackendInfo {
        backend: info.backend().contract_name().to_string(),
        capabilities: info
            .capabilities()
            .map(|capability| capability.contract_name().to_string())
            .collect(),
        interface_kinds: info
            .interface_kinds()
            .map(|kind| kind.contract_name().to_string())
            .collect(),
    }
}

#[napi]
pub fn generate_identity_secret() -> Result<Buffer, ErrorCode> {
    personal_rns::try_generate_identity_secret()
        .map(|secret| Buffer::from(secret.to_vec()))
        .map_err(|error| {
            code_err(
                ErrorCode::Internal,
                format!("entropy unavailable: {error:?}"),
            )
        })
}

#[napi]
pub fn request_path_hash(path: String) -> Buffer {
    Buffer::from(
        personal_rns::routing::request_handlers::RequestPathHash::of(&path)
            .as_bytes()
            .to_vec(),
    )
}

#[cfg(test)]
mod tests {
    use super::backend_info;

    #[test]
    fn backend_info_uses_exact_contract_names() {
        let info = backend_info();
        assert_eq!(info.backend, "Native");
        assert_eq!(
            info.capabilities,
            [
                "TcpClient",
                "TcpServer",
                "Udp",
                "Serial",
                "Usb",
                "Bluetooth",
                "Wifi",
                "WebSocket",
                "I2p",
                "Weave",
            ]
        );
        assert_eq!(
            info.interface_kinds,
            [
                "AutoLan",
                "TcpClient",
                "TcpServer",
                "Udp",
                "Serial",
                "Kiss",
                "Ax25Kiss",
                "RNode",
                "MultiRNode",
                "Pipe",
                "BackboneClient",
                "BackboneServer",
                "I2p",
                "Weave",
                "AutomaticUsb",
                "AutomaticBluetoothLe",
                "WebSocketClient",
                "WebSocketServer",
            ]
        );
    }
}
