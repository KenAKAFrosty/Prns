use super::*;

const TRACE_CAPACITY: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Operation {
    Read { offset: usize, len: usize },
    Write { offset: usize, len: usize },
    Erase { offset: usize, len: usize },
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Cut {
    pub operation: usize,
    pub completed_bytes: usize,
}

enum Power {
    On,
    LoseCommitAcknowledgement,
    CutAt(Cut),
    RemoveAt(Cut),
    Removed,
    Off,
}

pub(super) struct Control {
    power: Power,
    pub trace: Vec<Operation>,
}

impl Control {
    pub fn lose_commit_acknowledgement(&mut self) {
        self.trace.clear();
        self.power = Power::LoseCommitAcknowledgement;
    }
    pub fn new() -> Self {
        Self {
            power: Power::On,
            trace: Vec::new(),
        }
    }

    pub fn arm(&mut self, cut: Option<Cut>) {
        self.trace.clear();
        self.power = cut.map_or(Power::On, Power::CutAt);
    }

    pub fn remove_power_at(&mut self, cut: Cut) {
        self.trace.clear();
        self.power = Power::RemoveAt(cut);
    }

    pub fn power_removed(&self) -> bool {
        matches!(self.power, Power::Removed)
    }

    fn begin(&mut self, operation: Operation, len: usize) -> Result<usize, TestFlashError> {
        if matches!(self.power, Power::Off | Power::Removed) {
            return Err(TestFlashError);
        }
        let (offset, alignment) = match operation {
            Operation::Read { offset, .. } => (offset, Flash::READ_SIZE),
            Operation::Write { offset, .. } => (offset, Flash::WRITE_SIZE),
            Operation::Erase { offset, .. } => (offset, Flash::ERASE_SIZE),
        };
        assert!(offset.is_multiple_of(alignment) && len.is_multiple_of(alignment));
        assert!(offset.checked_add(len).is_some_and(|end| end <= CAPACITY));
        assert!(
            self.trace.len() < TRACE_CAPACITY,
            "bounded owner flash trace"
        );
        let ordinal = self.trace.len();
        self.trace.push(operation);
        if matches!(self.power, Power::LoseCommitAcknowledgement)
            && matches!(operation, Operation::Write { len: 4, .. })
        {
            self.power = Power::Off;
            return Ok(len);
        }
        if let Power::CutAt(cut) = self.power {
            if ordinal == cut.operation {
                assert!(cut.completed_bytes <= len);
                self.power = Power::Off;
                return Ok(cut.completed_bytes);
            }
        }
        if let Power::RemoveAt(cut) = self.power {
            if ordinal == cut.operation {
                assert!(cut.completed_bytes <= len);
                self.power = Power::Removed;
                return Ok(cut.completed_bytes);
            }
        }
        Ok(len)
    }

    fn finish(&self) -> Result<(), TestFlashError> {
        if matches!(self.power, Power::Off) {
            Err(TestFlashError)
        } else {
            Ok(())
        }
    }
}

#[test]
fn tears_change_only_the_selected_prefix_and_power_loss_is_sticky() {
    embassy_futures::block_on(async {
        let control = Rc::new(RefCell::new(Control::new()));
        let mut flash = Flash::boot([0xff; CAPACITY], control.clone());
        control.borrow_mut().arm(Some(Cut {
            operation: 0,
            completed_bytes: 2,
        }));
        assert!(flash.write(4, &[1, 2, 3, 4]).await.is_err());
        let mut expected = [0xff; CAPACITY];
        expected[4..6].copy_from_slice(&[1, 2]);
        assert_eq!(flash.inner.bytes, expected);
        assert!(flash.erase(0, ERASE as u32).await.is_err());
        assert!(flash.read(0, &mut [0; 4]).await.is_err());
        assert_eq!(flash.into_image(), expected);
        assert_eq!(
            control.borrow().trace,
            std::vec![Operation::Write { offset: 4, len: 4 }]
        );

        let control = Rc::new(RefCell::new(Control::new()));
        let mut flash = Flash::boot([0; CAPACITY], control.clone());
        control.borrow_mut().arm(Some(Cut {
            operation: 0,
            completed_bytes: ERASE - 1,
        }));
        assert!(flash.erase(ERASE as u32, (ERASE * 2) as u32).await.is_err());
        let mut expected = [0; CAPACITY];
        expected[ERASE..ERASE * 2 - 1].fill(0xff);
        assert_eq!(flash.into_image(), expected);

        let control = Rc::new(RefCell::new(Control::new()));
        let mut flash = Flash::boot([0xaa; CAPACITY], control.clone());
        control.borrow_mut().arm(Some(Cut {
            operation: 0,
            completed_bytes: 2,
        }));
        let mut output = [0; 4];
        assert!(flash.read(0, &mut output).await.is_err());
        assert_eq!(output, [0xaa, 0xaa, 0, 0]);
        assert_eq!(flash.into_image(), [0xaa; CAPACITY]);
    });
}

pub(super) struct Flash {
    inner: TestFlash,
    control: Rc<RefCell<Control>>,
}

#[test]
fn power_removal_leaves_io_pending_until_dropped_without_repeating_the_operation() {
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    for completed_bytes in 0..=4 {
        let control = Rc::new(RefCell::new(Control::new()));
        let mut flash = Flash::boot([0xff; CAPACITY], control.clone());
        control.borrow_mut().remove_power_at(Cut {
            operation: 0,
            completed_bytes,
        });
        {
            let mut write = core::pin::pin!(flash.write(4, &[1, 2, 3, 4]));
            for _ in 0..2 {
                assert!(core::future::Future::poll(write.as_mut(), &mut context).is_pending());
                assert!(control.borrow().power_removed());
                assert_eq!(
                    control.borrow().trace,
                    std::vec![Operation::Write { offset: 4, len: 4 }]
                );
            }
        }
        let mut expected = [0xff; CAPACITY];
        expected[4..4 + completed_bytes].copy_from_slice(&[1, 2, 3, 4][..completed_bytes]);
        assert_eq!(flash.into_image(), expected);
    }
    let control = Rc::new(RefCell::new(Control::new()));
    let mut flash = Flash::boot([0; CAPACITY], control.clone());
    control.borrow_mut().remove_power_at(Cut {
        operation: 0,
        completed_bytes: ERASE - 1,
    });
    {
        let mut erase = core::pin::pin!(flash.erase(0, ERASE as u32));
        for _ in 0..2 {
            assert!(core::future::Future::poll(erase.as_mut(), &mut context).is_pending());
        }
    }
    let mut expected = [0; CAPACITY];
    expected[..ERASE - 1].fill(0xff);
    assert_eq!(flash.into_image(), expected);
    assert_eq!(
        control.borrow().trace,
        std::vec![Operation::Erase {
            offset: 0,
            len: ERASE
        }]
    );
}

impl Flash {
    pub fn boot(image: [u8; CAPACITY], control: Rc<RefCell<Control>>) -> Self {
        let mut inner = TestFlash::new();
        inner.bytes = image;
        Self { inner, control }
    }
    pub fn into_image(self) -> [u8; CAPACITY] {
        self.inner.bytes
    }

    async fn finish(&self) -> Result<(), TestFlashError> {
        let power_removed = self.control.borrow().power_removed();
        if power_removed {
            core::future::pending::<()>().await;
        }
        self.control.borrow().finish()
    }
}

impl ErrorType for Flash {
    type Error = TestFlashError;
}

impl ReadNorFlash for Flash {
    const READ_SIZE: usize = 4;
    async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        let len = self.control.borrow_mut().begin(
            Operation::Read {
                offset: offset as usize,
                len: bytes.len(),
            },
            bytes.len(),
        )?;
        self.inner.read(offset, &mut bytes[..len]).await?;
        self.finish().await
    }
    fn capacity(&self) -> usize {
        CAPACITY
    }
}

impl NorFlash for Flash {
    const WRITE_SIZE: usize = 4;
    const ERASE_SIZE: usize = ERASE;
    async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        assert!(
            self.inner.bytes[offset as usize..offset as usize + bytes.len()]
                .iter()
                .zip(bytes)
                .all(|(old, new)| old & new == *new)
        );
        let len = self.control.borrow_mut().begin(
            Operation::Write {
                offset: offset as usize,
                len: bytes.len(),
            },
            bytes.len(),
        )?;
        self.inner.write(offset, &bytes[..len]).await?;
        self.finish().await
    }
    async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        let len = (to - from) as usize;
        let completed = self.control.borrow_mut().begin(
            Operation::Erase {
                offset: from as usize,
                len,
            },
            len,
        )?;
        self.inner.bytes[from as usize..from as usize + completed].fill(0xff);
        self.finish().await
    }
}
