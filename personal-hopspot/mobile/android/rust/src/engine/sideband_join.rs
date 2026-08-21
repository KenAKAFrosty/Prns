use personal_rns::shared_instance::rns_rpc::RpcAuthenticationKey;

use super::{hex, EnginePorts};

pub(super) fn render(rpc_key: &RpcAuthenticationKey, ports: EnginePorts) -> String {
    let rpc_key_hex = hex(rpc_key.as_bytes());
    let local_port = ports.local;
    let control_port = ports.rpc;
    format!(
        "# Sideband uses this template to generate its internal RNS configuration.\n\
         # Keep the transport placeholder so Sideband's transport setting remains authoritative.\n\
         \n\
         [reticulum]\n\
           enable_transport = TRANSPORT_IS_ENABLED\n\
         \n\
         # Sideband must share its RNS instance to join the local Hopspot transport.\n\
           share_instance = Yes\n\
           shared_instance_type = tcp\n\
           shared_instance_port = {local_port}\n\
           instance_control_port = {control_port}\n\
           rpc_key = {rpc_key_hex}\n\
           panic_on_interface_error = No\n\
         \n\
         [logging]\n\
           loglevel = 3\n\
         \n\
         [interfaces]\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_complete_sideband_tcp_template() {
        let key = RpcAuthenticationKey::new(vec![0x5a; 32]);
        assert_eq!(
            render(
                &key,
                EnginePorts {
                    local: 37_428,
                    rpc: 37_429,
                },
            ),
            "# Sideband uses this template to generate its internal RNS configuration.\n\
             # Keep the transport placeholder so Sideband's transport setting remains authoritative.\n\
             \n\
             [reticulum]\n\
               enable_transport = TRANSPORT_IS_ENABLED\n\
             \n\
             # Sideband must share its RNS instance to join the local Hopspot transport.\n\
               share_instance = Yes\n\
               shared_instance_type = tcp\n\
               shared_instance_port = 37428\n\
               instance_control_port = 37429\n\
               rpc_key = 5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a\n\
               panic_on_interface_error = No\n\
             \n\
             [logging]\n\
               loglevel = 3\n\
             \n\
             [interfaces]\n"
        );
    }
}
