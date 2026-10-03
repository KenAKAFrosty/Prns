use super::flash::*;
use embedded_storage_async::nor_flash::{NorFlash, ReadNorFlash};

#[tokio::test]
async fn invalid_operations_leave_the_whole_image_and_trace_unchanged() {
    let image = Image([0; CAPACITY]);
    let mut flash = Flash::boot(image.clone());
    assert_eq!(
        flash.write(0, &[0, 0, 0, 1]).await,
        Err(Error::RequiresErase)
    );
    assert_eq!(flash.write(1, &[0; 4]).await, Err(Error::Misaligned));
    assert_eq!(
        flash.read(CAPACITY as u32, &mut [0; 4]).await,
        Err(Error::OutOfBounds)
    );
    assert_eq!(flash.erase(0, 1).await, Err(Error::Misaligned));
    assert_eq!(flash.erase(PAGE as u32, 0).await, Err(Error::OutOfBounds));
    assert_eq!(flash.trace(), &[]);
    assert_eq!(flash.into_image(), image);
}

#[tokio::test]
async fn torn_program_is_durable_and_power_loss_is_sticky_until_reboot() {
    let mut flash = Flash::boot(Image([0xff; CAPACITY]));
    flash.arm(Cut {
        operation: 0,
        completed_bytes: 2,
    });
    assert_eq!(flash.write(0, &[1, 2, 3, 4]).await, Err(Error::PowerLost));
    assert_eq!(flash.erase(0, PAGE as u32).await, Err(Error::PowerLost));
    assert_eq!(flash.read(0, &mut [0; 4]).await, Err(Error::PowerLost));
    assert_eq!(flash.trace(), &[Operation::Write { offset: 0, len: 4 }]);
    let mut expected = Image([0xff; CAPACITY]);
    expected.0[..2].copy_from_slice(&[1, 2]);
    let actual = flash.into_image();
    assert_eq!(actual, expected);
    let mut rebooted = Flash::boot(actual);
    rebooted.erase(0, PAGE as u32).await.unwrap();
    assert_eq!(rebooted.into_image(), Image([0xff; CAPACITY]));
}

#[tokio::test]
async fn torn_erase_changes_only_the_selected_prefix() {
    let mut flash = Flash::boot(Image([0; CAPACITY]));
    flash.arm(Cut {
        operation: 0,
        completed_bytes: 7,
    });
    assert_eq!(
        flash.erase(PAGE as u32, (PAGE * 2) as u32).await,
        Err(Error::PowerLost)
    );
    let mut expected = Image([0; CAPACITY]);
    expected.0[PAGE..PAGE + 7].fill(0xff);
    assert_eq!(flash.into_image(), expected);
}
