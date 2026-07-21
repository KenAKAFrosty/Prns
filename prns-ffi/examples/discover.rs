#[cfg(target_os = "macos")]
#[tokio::main]
async fn main() {
    use prns_core::interfaces::bluetooth_auto::BleIdentity;
    use prns_ffi::bluetooth_auto::macos::MacosBleBackend;

    let mut backend = match MacosBleBackend::new(BleIdentity::new([0; 16])).await {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("bluetooth did not power on: {error:?}");
            eprintln!("grant Bluetooth access in System Settings > Privacy & Security > Bluetooth");
            return;
        }
    };
    println!("powered on — advertising the Prns service and scanning for peers. Ctrl-C to stop.");
    loop {
        match backend.next_sighting().await {
            Some(address) => println!("sighting: {:02x?}", address.octets()),
            None => {
                eprintln!("backend closed");
                break;
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("the `discover` example is macOS-only (it drives the CoreBluetooth backend)");
}
