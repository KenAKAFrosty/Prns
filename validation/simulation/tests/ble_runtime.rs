use personal_rns::interfaces::bluetooth_auto::{
    AppleHost, BleAddress, BleIdentity, BleRoleCapabilities, BlueZHost, Endpoint, LinkCapabilities,
};
use personal_rns::interfaces::{ConnectionState, InterfaceStatus};
use personal_rns::runtime::InterfaceSupervisor;
use personal_rns::Fleet;
use prns_interfaces_tokio::bluetooth_auto::BluetoothAuto;
use prns_simulation::ble::{
    BleMediumConfig, VirtualBleBackendConfig, VirtualBleLab, VirtualBleLinkConfig,
};
use prns_simulation::{SimulationDurationInTicks, SimulationTick};

const MAX_PEERS: usize = 4;

fn backend_config(address: u8, rssi: i8) -> VirtualBleBackendConfig {
    let link = VirtualBleLinkConfig::new(4, 4, 2_048)
        .unwrap_or_else(|error| unreachable!("test link configuration is valid: {error}"));
    VirtualBleBackendConfig::new(
        BleAddress::new([address; 6]),
        rssi,
        BleRoleCapabilities::DualRole,
        SimulationDurationInTicks::from_ticks(1),
        2,
        link,
    )
    .unwrap_or_else(|error| unreachable!("test backend configuration is valid: {error}"))
}

#[tokio::test]
async fn production_supervisors_discover_handshake_and_attach_members() {
    let medium = BleMediumConfig::new(2, 8, 16, 256)
        .unwrap_or_else(|error| unreachable!("test medium configuration is valid: {error}"));
    let lab = VirtualBleLab::new(medium);
    let first_backend = lab
        .attach_backend(backend_config(1, -41))
        .unwrap_or_else(|error| unreachable!("first backend attaches: {error}"));
    let second_backend = lab
        .attach_backend(backend_config(2, -52))
        .unwrap_or_else(|error| unreachable!("second backend attaches: {error}"));
    let capabilities = LinkCapabilities {
        l2cap: None,
        link_mtu: 500,
    };
    let first = BluetoothAuto::<_, MAX_PEERS>::new(
        first_backend,
        BleIdentity::new([1; 16]),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        capabilities,
    );
    let second = BluetoothAuto::<_, MAX_PEERS>::new(
        second_backend,
        BleIdentity::new([2; 16]),
        Endpoint::BlueZ(BlueZHost::Linux),
        capabilities,
    );
    let first_status = first.status();
    let second_status = second.status();
    let (first_fleet, _first_tail) = Fleet::detached(first_status.id());
    let (second_fleet, _second_tail) = Fleet::detached(second_status.id());
    let drive_lab = async {
        for tick in 0..64 {
            tokio::task::yield_now().await;
            lab.advance_to(SimulationTick::from_ticks(tick))
                .unwrap_or_else(|error| unreachable!("bounded test advance succeeds: {error}"));
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            if first_status.connection() == ConnectionState::Connected
                && second_status.connection() == ConnectionState::Connected
            {
                return;
            }
        }
        unreachable!("both production supervisors must attach a member")
    };

    tokio::select! {
        () = first.run(first_fleet) => unreachable!("first supervisor runs until cancelled"),
        () = second.run(second_fleet) => unreachable!("second supervisor runs until cancelled"),
        () = drive_lab => {}
    }
}
