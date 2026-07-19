use super::MacosBleBackend;

#[tokio::test]
#[ignore = "needs a real Bluetooth radio + Bluetooth permission; run with `--ignored` on a Mac"]
async fn the_node_powers_on_advertises_and_scans() {
    let _backend = MacosBleBackend::new()
        .await
        .expect("bluetooth should power on, advertise, and scan");
}
