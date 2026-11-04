//! A byte-addressed NOR image, with a power cut that stops *all* subsequent I/O.
//! Reopening creates a new controller from bytes; faults, traces and RAM do not
//! survive. This models digital NOR behavior, not physical erase remanence.

use embedded_storage::nor_flash::{
    ErrorType, NorFlash, NorFlashError, NorFlashErrorKind, ReadNorFlash,
};
use std::sync::{Arc, Mutex};

pub const ERASE: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Write { offset: u32, length: usize },
    Erase { offset: u32, length: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timing {
    Before,
    During,
    After,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cut {
    /// Zero-based mutating I/O call, relative to `arm`.
    pub action: usize,
    pub timing: Timing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerLoss;
impl NorFlashError for PowerLoss {
    fn kind(&self) -> NorFlashErrorKind {
        NorFlashErrorKind::Other
    }
}

struct Image {
    bytes: Vec<u8>,
    cut: Option<Cut>,
    fired: bool,
    powered: bool,
    actions: Vec<Action>,
}

#[derive(Clone)]
pub struct Controller(Arc<Mutex<Image>>);
impl Controller {
    pub fn erased(capacity: usize) -> Self {
        Self::from_bytes(vec![0xff; capacity])
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(Arc::new(Mutex::new(Image {
            bytes,
            cut: None,
            fired: false,
            powered: true,
            actions: Vec::new(),
        })))
    }

    pub fn flash(&self) -> Flash {
        Flash(self.clone())
    }

    pub fn arm(&self, cut: Cut) {
        let mut image = self.0.lock().unwrap();
        assert!(image.powered);
        image.actions.clear();
        image.cut = Some(cut);
        image.fired = false;
    }

    pub fn clear_trace(&self) {
        self.0.lock().unwrap().actions.clear();
    }

    pub fn actions(&self) -> Vec<Action> {
        self.0.lock().unwrap().actions.clone()
    }

    pub fn fired(&self) -> bool {
        self.0.lock().unwrap().fired
    }

    /// Restore device I/O while retaining an uncertain coordinator, to exercise
    /// its explicit recovery gate after a transient flash failure. CPU-restart
    /// tests instead create a new controller from bytes and never call this.
    pub fn restore_power(&self) {
        let mut image = self.0.lock().unwrap();
        image.powered = true;
        image.cut = None;
    }

    /// Evidence copy only; production recovery sees the fresh NOR instance.
    pub fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().bytes.clone()
    }

    fn mutate(&self, action: Action, data: Option<&[u8]>) -> Result<(), PowerLoss> {
        let mut image = self.0.lock().unwrap();
        if !image.powered {
            return Err(PowerLoss);
        }
        let ordinal = image.actions.len();
        image.actions.push(action);
        let timing = image
            .cut
            .filter(|cut| cut.action == ordinal)
            .map(|c| c.timing);
        let (offset, length) = match action {
            Action::Write { offset, length } | Action::Erase { offset, length } => {
                (offset as usize, length)
            }
        };
        assert!(offset + length <= image.bytes.len());
        let completed = match timing {
            Some(Timing::Before) => 0,
            Some(Timing::During) => length / 2,
            Some(Timing::After) | None => length,
        };
        let target = &mut image.bytes[offset..offset + completed];
        match action {
            Action::Write { .. } => {
                for (destination, source) in target.iter_mut().zip(data.unwrap()) {
                    assert_eq!(
                        *destination & source,
                        *source,
                        "NOR cannot program zero to one"
                    );
                    *destination &= source;
                }
            }
            Action::Erase { .. } => target.fill(0xff),
        }
        if timing.is_some() {
            image.fired = true;
            image.powered = false;
            return Err(PowerLoss);
        }
        Ok(())
    }
}

pub struct Flash(Controller);
impl ErrorType for Flash {
    type Error = PowerLoss;
}
impl ReadNorFlash for Flash {
    const READ_SIZE: usize = 4;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), PowerLoss> {
        assert!(offset.is_multiple_of(Self::READ_SIZE as u32));
        assert!(bytes.len().is_multiple_of(Self::READ_SIZE));
        let image = self.0 .0.lock().unwrap();
        if !image.powered {
            return Err(PowerLoss);
        }
        bytes.copy_from_slice(&image.bytes[offset as usize..offset as usize + bytes.len()]);
        Ok(())
    }

    fn capacity(&self) -> usize {
        self.0 .0.lock().unwrap().bytes.len()
    }
}
impl NorFlash for Flash {
    const WRITE_SIZE: usize = 4;
    const ERASE_SIZE: usize = ERASE;

    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), PowerLoss> {
        assert!(offset.is_multiple_of(Self::WRITE_SIZE as u32));
        assert!(bytes.len().is_multiple_of(Self::WRITE_SIZE));
        self.0.mutate(
            Action::Write {
                offset,
                length: bytes.len(),
            },
            Some(bytes),
        )
    }

    fn erase(&mut self, from: u32, to: u32) -> Result<(), PowerLoss> {
        assert!(from.is_multiple_of(Self::ERASE_SIZE as u32));
        assert!(to.is_multiple_of(Self::ERASE_SIZE as u32));
        assert!(to >= from);
        self.0.mutate(
            Action::Erase {
                offset: from,
                length: (to - from) as usize,
            },
            None,
        )
    }
}
