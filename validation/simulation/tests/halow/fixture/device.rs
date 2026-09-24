use super::*;
use personal_rns::wifi_halow::{Destination, HaLowDatagrams, HaLowRadioSource, ReceivedDatagram};
use std::{
    io,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Present,
    Absent,
}
#[derive(Clone)]
struct DeviceState {
    presence: Presence,
    generation: u64,
}
struct Binding {
    radio: Option<RadioId>,
    neighbor: Option<RadioId>,
    attempts: u64,
}
#[derive(Clone)]
pub struct DeviceControl {
    state: watch::Sender<DeviceState>,
    binding: Arc<Mutex<Binding>>,
}
impl DeviceControl {
    pub fn new() -> Self {
        Self {
            state: watch::channel(DeviceState {
                presence: Presence::Present,
                generation: 0,
            })
            .0,
            binding: Arc::new(Mutex::new(Binding {
                radio: None,
                neighbor: None,
                attempts: 0,
            })),
        }
    }
    pub fn set_presence(&self, presence: Presence) {
        self.state.send_modify(|state| {
            state.presence = presence;
            state.generation = state
                .generation
                .checked_add(1)
                .expect("binding generation budget");
        });
    }
    pub fn radio(&self) -> Option<RadioId> {
        self.binding.lock().expect("binding state").radio
    }
    pub fn attempts(&self) -> u64 {
        self.binding.lock().expect("binding state").attempts
    }
    pub fn neighbor(&self, radio: RadioId) {
        self.binding.lock().expect("binding state").neighbor = Some(radio);
    }
}
#[derive(Clone)]
pub enum Bindings {
    Explicit,
    ManagedTarget(DeviceControl),
}

pub struct Source {
    initial: Option<VirtualHaLowRadio>,
    medium: VirtualHaLowMedium,
    control: DeviceControl,
}
impl Source {
    pub fn new(
        initial: VirtualHaLowRadio,
        medium: VirtualHaLowMedium,
        control: DeviceControl,
    ) -> Self {
        Self {
            initial: Some(initial),
            medium,
            control,
        }
    }
}
pub struct BoundRadio {
    radio: VirtualHaLowRadio,
    control: DeviceControl,
    generation: u64,
}
impl HaLowRadioSource for Source {
    type Datagrams = BoundRadio;
    async fn open(&mut self) -> io::Result<BoundRadio> {
        {
            let mut binding = self.control.binding.lock().expect("binding state");
            binding.attempts = binding
                .attempts
                .checked_add(1)
                .expect("open attempt budget");
        }
        let state = self.control.state.borrow().clone();
        if state.presence == Presence::Absent {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        let radio = match self.initial.take() {
            Some(radio) => radio,
            None => self
                .medium
                .attach(mac(TARGET))
                .expect("bounded replacement binding"),
        };
        let mut binding = self.control.binding.lock().expect("binding state");
        binding.radio = Some(radio.id());
        if let Some(neighbor) = binding.neighbor {
            self.medium
                .set_path(radio.id(), neighbor, PathState::Reachable);
            self.medium
                .set_path(neighbor, radio.id(), PathState::Reachable);
        }
        Ok(BoundRadio {
            radio,
            control: self.control.clone(),
            generation: state.generation,
        })
    }
}
impl BoundRadio {
    async fn retired(&self) {
        let mut state = self.control.state.subscribe();
        loop {
            if state.borrow_and_update().generation != self.generation {
                return;
            }
            state.changed().await.expect("live device control");
        }
    }
}
impl HaLowDatagrams for BoundRadio {
    async fn send(&self, destination: Destination, payload: &[u8]) -> io::Result<()> {
        if self.control.state.borrow().generation != self.generation {
            return Err(io::Error::from(io::ErrorKind::NetworkDown));
        }
        self.radio.send(destination, payload).await
    }
    async fn receive(&self, buffer: &mut [u8]) -> io::Result<ReceivedDatagram> {
        tokio::select! {
            biased;
            () = self.retired() => Err(io::Error::from(io::ErrorKind::NetworkDown)),
            received = self.radio.receive(buffer) => received,
        }
    }
}
impl Drop for BoundRadio {
    fn drop(&mut self) {
        let mut binding = self.control.binding.lock().expect("binding state");
        assert_eq!(binding.radio, Some(self.radio.id()), "one binding owner");
        binding.radio = None;
    }
}
