#[cfg(target_os = "macos")]
#[tokio::main]
async fn main() {
    use prns_core::interfaces::bluetooth_auto::{default_group_tag, 
        AdvertisingMode, BleBackend, BleIdentity, ScanningMode,
    };
    use prns_ffi::bluetooth_auto::macos::MacosBleBackend;

    let mut backend = match MacosBleBackend::new(BleIdentity::new([0; 16]), default_group_tag()).await {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("bluetooth did not power on: {error:?}");
            eprintln!("grant Bluetooth access in System Settings > Privacy & Security > Bluetooth");
            return;
        }
    };
    if let Err(error) =
        <MacosBleBackend as BleBackend<{ MacosBleBackend::MAX_PEERS }>>::set_advertising(
            &mut backend,
            AdvertisingMode::On,
        )
        .await
    {
        eprintln!("advertising did not start after publication: {error:?}");
        return;
    }
    if let Err(error) =
        <MacosBleBackend as BleBackend<{ MacosBleBackend::MAX_PEERS }>>::set_scanning(
            &mut backend,
            ScanningMode::On,
        )
        .await
    {
        eprintln!("scanning did not start after power-on: {error:?}");
        return;
    }
    println!("powered on and published — explicitly advertising and scanning. Ctrl-C to stop.");
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
