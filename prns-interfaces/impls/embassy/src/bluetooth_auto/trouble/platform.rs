use cfg_if::cfg_if;

use super::backend::BleHub;
use super::*;

cfg_if! {
    if #[cfg(target_arch = "riscv32")] {
        pub const PEER_CAPACITY: usize = 8;

        pub(super) const GATT_FRAGMENT_PAYLOAD: usize = 120;

        /// A wide connect scan catches dual-role peers that advertise only in short windows.
        pub(super) const CONNECT_SCAN_INTERVAL: Duration = Duration::from_millis(200);
        pub(super) const CONNECT_SCAN_WINDOW: Duration = Duration::from_millis(60);
        pub(super) const IDLE_SCAN_INTERVAL: Duration = Duration::from_millis(1500);
        pub(super) const IDLE_SCAN_WINDOW: Duration = Duration::from_millis(60);
        pub(super) const SCAN_WINDOW: Duration = Duration::from_millis(300);

        pub(super) const L2CAP_MPS: u16 = 185;
        pub(super) const CONN_PARAM_UPDATE_TIMEOUT: Duration = Duration::from_secs(2);

        pub(super) fn preferred_conn_params() -> RequestedConnParams {
            RequestedConnParams {
                min_connection_interval: Duration::from_millis(120),
                max_connection_interval: Duration::from_millis(120),
                max_latency: 0,
                min_event_length: Duration::from_millis(1),
                max_event_length: Duration::from_millis(4),
                supervision_timeout: Duration::from_secs(8),
            }
        }

        pub(super) async fn prepare_accepted_connection<T: TroubleTransport>(
            stack: &TroubleStack<T>,
            connection: &GattConnection<'_, '_, DefaultPacketPool>,
        ) {
            let _ = with_timeout(
                CONN_PARAM_UPDATE_TIMEOUT,
                connection
                    .raw()
                    .update_connection_params(stack, &preferred_conn_params()),
            )
            .await;
        }

        pub(super) async fn tune_accepted_connection<T: TroubleTransport>(
            hub: &BleHub,
            stack: &TroubleStack<T>,
            connection: &Connection<'_, DefaultPacketPool>,
        ) {
            let _ = (hub, stack, connection);
        }

        pub(super) async fn tune_dialed_connection<T: TroubleTransport>(
            hub: &BleHub,
            stack: &TroubleStack<T>,
            connection: &Connection<'_, DefaultPacketPool>,
        ) {
            let _ = hub;
            let phy_2m =
                with_timeout(PHY_UPDATE_TIMEOUT, connection.set_phy(stack, PhyKind::Le2M)).await;
            crate::diagnostic_log::debug!(
                "ble: 2M PHY request ok={}",
                matches!(phy_2m, Ok(Ok(())))
            );
        }
    } else {
        pub const PEER_CAPACITY: usize = 4;

        pub(super) const GATT_FRAGMENT_PAYLOAD: usize = 180;

        /// A wide connect scan catches dual-role peers that advertise only in short windows.
        pub(super) const CONNECT_SCAN_INTERVAL: Duration = Duration::from_millis(100);
        pub(super) const CONNECT_SCAN_WINDOW: Duration = Duration::from_millis(80);
        pub(super) const IDLE_SCAN_INTERVAL: Duration = Duration::from_secs(1);
        pub(super) const IDLE_SCAN_WINDOW: Duration = Duration::from_millis(200);
        pub(super) const SCAN_WINDOW: Duration = Duration::from_millis(600);

        pub(super) const CONNECTED_DISCOVERY_QUIET_MS: u64 = 750;
        pub(super) const CONNECTED_DISCOVERY_REST_MS: u64 = 500;
        pub(super) const CONNECTED_DISCOVERY_MAX_REST_MS: u64 = 5_000;
        pub(super) const CONNECTED_DISCOVERY_WINDOW: Duration = Duration::from_millis(60);
        pub(super) const CONNECTED_ADV_INTERVAL_MIN: Duration = Duration::from_millis(30);
        pub(super) const CONNECTED_ADV_INTERVAL_MAX: Duration = Duration::from_millis(40);
        pub(super) const CONNECTED_SCAN_INTERVAL: Duration = Duration::from_millis(60);
        pub(super) const CONNECTED_SCAN_WINDOW: Duration = Duration::from_millis(60);
        pub(super) const CONNECTED_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

        pub(super) const L2CAP_MPS: u16 = 247;
        pub(super) const DATA_LENGTH_OCTETS: u16 = 251;
        // Maximum on-air time for a 251-octet LE data PDU on the 1M PHY. The negotiated value
        // shrinks naturally when both peers move to 2M, while remaining valid if the peer keeps
        // the 1M PHY.
        pub(super) const DATA_LENGTH_TIME_US: u16 = 2_120;

        pub(super) fn preferred_conn_params() -> RequestedConnParams {
            RequestedConnParams::default()
        }

        pub(super) async fn prepare_accepted_connection<T: TroubleTransport>(
            stack: &TroubleStack<T>,
            connection: &GattConnection<'_, '_, DefaultPacketPool>,
        ) {
            let _ = (stack, connection);
        }

        async fn tune_connection<T: TroubleTransport>(
            hub: &BleHub,
            stack: &TroubleStack<T>,
            connection: &Connection<'_, DefaultPacketPool>,
            origin: Origin,
        ) {
            let activity = hub.begin_busy_operation();
            let data_length = with_timeout(
                PHY_UPDATE_TIMEOUT,
                connection.update_data_length(stack, DATA_LENGTH_OCTETS, DATA_LENGTH_TIME_US),
            )
            .await;
            let phy =
                with_timeout(PHY_UPDATE_TIMEOUT, connection.set_phy(stack, PhyKind::Le2M)).await;
            drop(activity);
            crate::diagnostic_log::info!(
                "ble: {origin:?} link tuning dle_251={} phy_2m={}",
                matches!(data_length, Ok(Ok(()))),
                matches!(phy, Ok(Ok(()))),
            );
        }

        pub(super) async fn tune_accepted_connection<T: TroubleTransport>(
            hub: &BleHub,
            stack: &TroubleStack<T>,
            connection: &Connection<'_, DefaultPacketPool>,
        ) {
            tune_connection(hub, stack, connection, Origin::Accepted).await;
        }

        pub(super) async fn tune_dialed_connection<T: TroubleTransport>(
            hub: &BleHub,
            stack: &TroubleStack<T>,
            connection: &Connection<'_, DefaultPacketPool>,
        ) {
            tune_connection(hub, stack, connection, Origin::Dialed).await;
        }
    }
}
