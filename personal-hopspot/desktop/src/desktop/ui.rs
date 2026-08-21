use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "linux")]
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics_simulator::{
    BinaryColorTheme, OutputSettings, OutputSettingsBuilder, SimulatorDisplay, SimulatorEvent,
};
use heapless::Vec as HVec;
use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand};
use personal_rns::interfaces::lora::{RadioProfile, DEFAULT_915_PROFILE};
use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus};
use personal_rns::storage::{GrowableHeap, StorageLayout};
use sdl2::event::{Event, WindowEvent};
use sdl2::keyboard::Keycode;
use sdl2::pixels::PixelFormatEnum;
#[cfg(not(target_os = "linux"))]
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
#[cfg(not(target_os = "linux"))]
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use personal_hopspot_core::{
    self as screen, AccessPointState, Card, DisplayPowerControl, InputEvent, ScreenContent,
    UiAction, UiConfiguration, UiState,
};

use super::runtime::{classify, WindowHandles, USB_INTERFACE_ID};

const PANEL: Size = Size::new(64, 128);
const FRAME: Duration = Duration::from_millis(screen::COALESCE_MS);
const LIVE_REFRESH: Duration = Duration::from_millis(250);
const STATUS_LOG_THROTTLE: Duration = Duration::from_millis(1000);
const NOTICE_TIMEOUT: Duration = Duration::from_millis(900);
const LONG_PRESS_THRESHOLD: Duration = Duration::from_millis(500);

fn ui_state() -> UiState {
    UiState::new(UiConfiguration {
        storage_limits: <GrowableHeap as StorageLayout>::LIMITS,
        display_power_control: DisplayPowerControl::Unavailable,
        access_point: AccessPointState::Unsupported,
        shared_instance_config_export: screen::SharedInstanceConfigExport::Unavailable,
        gnss: screen::GnssAvailability::Unavailable,
    })
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PressSource {
    Key,
    Mouse,
}

#[derive(Clone, Copy)]
struct PressStart {
    source: PressSource,
    started_at: Instant,
    long_press_sent: bool,
}

fn press_start(source: PressSource) -> PressStart {
    PressStart {
        source,
        started_at: Instant::now(),
        long_press_sent: false,
    }
}

fn dispatch_long_press_if_ready(
    active_press: &mut Option<PressStart>,
    now: Instant,
    content: ScreenContent<'_, '_>,
    ui_state: &mut UiState,
) -> UiAction {
    let Some(press) = active_press.as_mut() else {
        return UiAction::None;
    };
    if content.cards.is_empty()
        || press.long_press_sent
        || now.duration_since(press.started_at) < LONG_PRESS_THRESHOLD
    {
        return UiAction::None;
    }

    press.long_press_sent = true;
    ui_state.handle_input(InputEvent::LongPress, content)
}

fn finish_press(
    active_press: &mut Option<PressStart>,
    source: PressSource,
    released_at: Instant,
    content: ScreenContent<'_, '_>,
    ui_state: &mut UiState,
) -> UiAction {
    let Some(press) = active_press.take() else {
        return UiAction::None;
    };
    if press.source != source {
        *active_press = Some(press);
        return UiAction::None;
    }

    if press.long_press_sent {
        return UiAction::None;
    }

    let event = if released_at.duration_since(press.started_at) >= LONG_PRESS_THRESHOLD {
        InputEvent::LongPress
    } else {
        InputEvent::ShortPress
    };
    ui_state.handle_input(event, content)
}

fn selected_card_id(ui_state: &UiState, content: ScreenContent<'_, '_>) -> Option<InterfaceId> {
    ui_state.selected_card(content.cards).map(|card| card.id())
}

struct LoggedStatus {
    connection: ConnectionState,
    rx_bytes: u64,
    tx_bytes: u64,
    last_emit: Instant,
}

enum DesktopControl {
    ShowWindow,
    HideWindow,
    Announce,
    Quit,
}

#[cfg(not(target_os = "linux"))]
struct TrayController {
    icon: TrayIcon,
    window_item: MenuItem,
    announce_item: MenuItem,
    quit_item: MenuItem,
    window_open: bool,
}

#[cfg(not(target_os = "linux"))]
impl TrayController {
    fn new(window_open: bool) -> Result<Self, String> {
        let window_item = MenuItem::with_id(
            "hopspot-window",
            if window_open {
                "Hide Hopspot"
            } else {
                "Open Hopspot"
            },
            true,
            None,
        );
        let announce_item = MenuItem::with_id("hopspot-announce", "Announce Now", true, None);
        let quit_item = MenuItem::with_id("hopspot-quit", "Quit Hopspot", true, None);
        let separator = PredefinedMenuItem::separator();
        let menu = Menu::with_items(&[&window_item, &announce_item, &separator, &quit_item])
            .map_err(|error| format!("tray menu build failed: {error}"))?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Personal Hopspot is running")
            .with_icon(hopspot_tray_icon()?)
            .with_menu_on_left_click(true)
            .build()
            .map_err(|error| format!("tray icon build failed: {error}"))?;

        Ok(Self {
            icon,
            window_item,
            announce_item,
            quit_item,
            window_open,
        })
    }

    fn set_window_open(&mut self, open: bool) {
        if self.window_open == open {
            return;
        }
        self.window_open = open;
        self.window_item
            .set_text(if open { "Hide Hopspot" } else { "Open Hopspot" });
    }

    fn drain_controls(&mut self, window_open: bool) -> Vec<DesktopControl> {
        self.set_window_open(window_open);

        let mut controls = Vec::new();
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = event.id();
            if id == self.window_item.id() {
                controls.push(if self.window_open {
                    DesktopControl::HideWindow
                } else {
                    DesktopControl::ShowWindow
                });
            } else if id == self.announce_item.id() {
                controls.push(DesktopControl::Announce);
            } else if id == self.quit_item.id() {
                controls.push(DesktopControl::Quit);
            }
        }
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            match event {
                TrayIconEvent::Click {
                    id,
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } if id == *self.icon.id() => controls.push(DesktopControl::ShowWindow),
                TrayIconEvent::DoubleClick {
                    id,
                    button: MouseButton::Left,
                    ..
                } if id == *self.icon.id() => controls.push(DesktopControl::ShowWindow),
                _ => {}
            }
        }
        controls
    }
}

#[cfg(target_os = "linux")]
struct LinuxTray {
    controls: mpsc::Sender<DesktopControl>,
    window_open: Arc<AtomicBool>,
}

#[cfg(target_os = "linux")]
impl ksni::Tray for LinuxTray {
    fn id(&self) -> String {
        "personal-hopspot".into()
    }

    fn title(&self) -> String {
        "Personal Hopspot".into()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.controls.send(DesktopControl::ShowWindow);
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let (rgba, size) = hopspot_tray_rgba();
        let mut argb = Vec::with_capacity(rgba.len());
        for pixel in rgba.as_chunks::<4>().0 {
            argb.extend_from_slice(&[pixel[3], pixel[0], pixel[1], pixel[2]]);
        }
        vec![ksni::Icon {
            width: size as i32,
            height: size as i32,
            data: argb,
        }]
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;

        vec![
            StandardItem {
                label: if self.window_open.load(Ordering::Relaxed) {
                    "Hide Hopspot".into()
                } else {
                    "Open Hopspot".into()
                },
                activate: Box::new(|tray: &mut LinuxTray| {
                    let control = if tray.window_open.load(Ordering::Relaxed) {
                        DesktopControl::HideWindow
                    } else {
                        DesktopControl::ShowWindow
                    };
                    let _ = tray.controls.send(control);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Announce Now".into(),
                activate: Box::new(|tray: &mut LinuxTray| {
                    let _ = tray.controls.send(DesktopControl::Announce);
                }),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit Hopspot".into(),
                activate: Box::new(|tray: &mut LinuxTray| {
                    let _ = tray.controls.send(DesktopControl::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

#[cfg(target_os = "linux")]
struct TrayController {
    _handle: ksni::blocking::Handle<LinuxTray>,
    controls: mpsc::Receiver<DesktopControl>,
    window_open: Arc<AtomicBool>,
}

#[cfg(target_os = "linux")]
impl TrayController {
    fn new(window_open: bool) -> Result<Self, String> {
        use ksni::blocking::TrayMethods;

        let (sender, controls) = mpsc::channel();
        let window_open = Arc::new(AtomicBool::new(window_open));
        let handle = LinuxTray {
            controls: sender,
            window_open: window_open.clone(),
        }
        .spawn()
        .map_err(|error| format!("StatusNotifier tray start failed: {error}"))?;
        Ok(Self {
            _handle: handle,
            controls,
            window_open,
        })
    }

    fn drain_controls(&mut self, window_open: bool) -> Vec<DesktopControl> {
        self.window_open.store(window_open, Ordering::Relaxed);
        self.controls.try_iter().collect()
    }
}

#[cfg(not(target_os = "linux"))]
fn hopspot_tray_icon() -> Result<Icon, String> {
    let (rgba, size) = hopspot_tray_rgba();
    Icon::from_rgba(rgba, size, size).map_err(|error| format!("tray icon pixels invalid: {error}"))
}

fn hopspot_tray_rgba() -> (Vec<u8>, u32) {
    const SIZE: u32 = 32;
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    let center = (SIZE as f32 - 1.0) / 2.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let distance = (dx * dx + dy * dy).sqrt();
            let idx = ((y * SIZE + x) * 4) as usize;
            let pixel = &mut rgba[idx..idx + 4];
            let h_left = (10..=13).contains(&x) && (9..=23).contains(&y);
            let h_right = (19..=22).contains(&x) && (9..=23).contains(&y);
            let h_crossbar = (10..=22).contains(&x) && (15..=17).contains(&y);
            if (11.5..=14.0).contains(&distance) {
                pixel.copy_from_slice(&[44, 232, 178, 255]);
            } else if distance < 10.0 && (h_left || h_right || h_crossbar) {
                pixel.copy_from_slice(&[230, 255, 248, 255]);
            } else if distance < 12.0 {
                pixel.copy_from_slice(&[7, 20, 28, 255]);
            }
        }
    }
    (rgba, SIZE)
}

struct HopspotWindow {
    output: OutputSettings,
    canvas: sdl2::render::Canvas<sdl2::video::Window>,
    event_pump: sdl2::EventPump,
    visible: bool,
    _sdl: sdl2::Sdl,
}

impl HopspotWindow {
    fn new(title: &str, output: &OutputSettings, display: &SimulatorDisplay<BinaryColor>) -> Self {
        let sdl = sdl2::init().expect("SDL initializes");
        let video = sdl.video().expect("SDL video subsystem initializes");
        let size = display.output_size(output);
        let window = video
            .window(title, size.width, size.height)
            .position_centered()
            .build()
            .expect("SDL creates the Hopspot window");
        let canvas = window
            .into_canvas()
            .build()
            .expect("SDL creates the Hopspot canvas");
        let event_pump = sdl.event_pump().expect("SDL event pump initializes");
        let mut window = Self {
            output: *output,
            canvas,
            event_pump,
            visible: true,
            _sdl: sdl,
        };
        window.update(display);
        window
    }

    fn is_visible(&self) -> bool {
        self.visible
    }

    fn show(&mut self) {
        if self.visible {
            self.canvas.window_mut().raise();
            return;
        }
        self.visible = true;
        self.canvas.window_mut().show();
        self.canvas.window_mut().restore();
        self.canvas.window_mut().raise();
    }

    fn hide(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.canvas.window_mut().hide();
    }

    fn update(&mut self, display: &SimulatorDisplay<BinaryColor>) {
        if !self.visible {
            return;
        }
        let output = display.to_rgb_output_image(&self.output);
        let image = output.as_image_buffer();
        let size = display.output_size(&self.output);
        let creator = self.canvas.texture_creator();
        let mut texture = creator
            .create_texture_streaming(PixelFormatEnum::RGB24, size.width, size.height)
            .expect("SDL creates the Hopspot texture");
        texture
            .update(None, image.as_raw(), size.width as usize * 3)
            .expect("SDL updates the Hopspot texture");
        self.canvas
            .copy(&texture, None, None)
            .expect("SDL copies the Hopspot texture");
        self.canvas.present();
    }

    fn events(&mut self) -> Vec<SimulatorEvent> {
        let mut events = Vec::new();
        let output = self.output;
        let output_to_display = |x, y| {
            let pitch = output.scale.saturating_add(output.pixel_spacing) as i32;
            Point::new(x / pitch.max(1), y / pitch.max(1))
        };
        while let Some(event) = self.event_pump.poll_event() {
            match event {
                Event::Quit { .. }
                | Event::Window {
                    win_event: WindowEvent::Close,
                    ..
                }
                | Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => events.push(SimulatorEvent::Quit),
                Event::KeyDown {
                    keycode: Some(keycode),
                    keymod,
                    repeat,
                    ..
                } if self.visible => events.push(SimulatorEvent::KeyDown {
                    keycode,
                    keymod,
                    repeat,
                }),
                Event::KeyUp {
                    keycode: Some(keycode),
                    keymod,
                    repeat,
                    ..
                } if self.visible => events.push(SimulatorEvent::KeyUp {
                    keycode,
                    keymod,
                    repeat,
                }),
                Event::MouseButtonDown {
                    x, y, mouse_btn, ..
                } if self.visible => {
                    let point = output_to_display(x, y);
                    events.push(SimulatorEvent::MouseButtonDown { mouse_btn, point });
                }
                Event::MouseButtonUp {
                    x, y, mouse_btn, ..
                } if self.visible => {
                    let point = output_to_display(x, y);
                    events.push(SimulatorEvent::MouseButtonUp { mouse_btn, point });
                }
                Event::MouseWheel {
                    x, y, direction, ..
                } if self.visible => events.push(SimulatorEvent::MouseWheel {
                    scroll_delta: Point::new(x, y),
                    direction,
                }),
                Event::MouseMotion { x, y, .. } if self.visible => {
                    let point = output_to_display(x, y);
                    events.push(SimulatorEvent::MouseMove { point });
                }
                _ => {}
            }
        }
        events
    }
}

pub(super) fn run_window(handles: WindowHandles) {
    let handle = handles.handle;
    let usb_status = handles.usb_status;
    let wifi_status = handles.wifi_status;
    let ble_status = handles.ble_status;
    let tcp_status = handles.tcp_status;
    let tcp_id = handles.tcp_id;
    let tcp_target = handles.tcp_target;
    let node_page_destination = handles.node_page_destination;

    let output = OutputSettingsBuilder::new()
        .theme(BinaryColorTheme::OledBlue)
        .scale(4)
        .build();
    let mut display = SimulatorDisplay::<BinaryColor>::new(PANEL);
    let mut window = HopspotWindow::new("Personal Hopspot", &output, &display);
    let mut tray = match TrayController::new(true) {
        Ok(tray) => {
            tracing::info!(event = "tray_started");
            Some(tray)
        }
        Err(error) => {
            tracing::warn!(event = "tray_disabled", error = %error);
            None
        }
    };

    let wifi_id = wifi_status.id();
    let tcp_target = tcp_target.as_deref();
    let classify = move |id: InterfaceId| classify(id, wifi_id, tcp_id, tcp_target);

    let query_handle = handle.clone();

    let toggle_usb = usb_status.clone();
    let toggle_wifi = wifi_status.clone();
    let toggle_ble = ble_status.clone();
    let toggle_tcp = tcp_status.clone();
    let apply_action = move |action: UiAction,
                             selected_id: Option<InterfaceId>,
                             ui_state: &mut UiState,
                             working_lora_profile: &mut RadioProfile,
                             notice_until: &mut Option<Instant>| match action
    {
        UiAction::None => {}
        UiAction::OledOff => {
            ui_state.show_notice(screen::UiNotice::OledOff);
            *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
        }
        UiAction::ToggleOledAutoOff => {}
        UiAction::ControlGnss(_) => {}
        UiAction::Sleep => {
            ui_state.show_notice(screen::UiNotice::Sleeping);
            *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
            toggle_usb.disable();
            toggle_wifi.disable();
            if let Some(ble) = &toggle_ble {
                ble.disable();
            }
            if let Some(tcp) = &toggle_tcp {
                tcp.disable();
            }
        }
        UiAction::Wake => {
            ui_state.show_notice(screen::UiNotice::Awake);
            *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
            toggle_usb.enable();
            toggle_wifi.enable();
            if let Some(ble) = &toggle_ble {
                ble.enable();
            }
            if let Some(tcp) = &toggle_tcp {
                tcp.enable();
            }
        }
        UiAction::Announce => {
            ui_state.show_notice(screen::UiNotice::Announcing);
            *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
            if let Some(id) = handle.issue(PrnsCommand::AnnounceNow(AnnounceNow {
                destination: node_page_destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            })) {
                tracing::info!(event = "manual_announce_issued");
                tracing::debug!(
                    event = "manual_announce_issued_detail",
                    command_id = id.0,
                    destination = ?node_page_destination.as_bytes(),
                );
            }
        }
        UiAction::ToggleSelectedInterface => match selected_id {
            Some(id) if id == USB_INTERFACE_ID => {
                ui_state.show_notice(if toggle_usb.is_enabled() {
                    screen::UiNotice::TurningOff
                } else {
                    screen::UiNotice::TurningOn
                });
                *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
                toggle_usb.toggle_enabled();
            }
            Some(id) if id == toggle_wifi.id() => {
                ui_state.show_notice(if toggle_wifi.is_enabled() {
                    screen::UiNotice::TurningOff
                } else {
                    screen::UiNotice::TurningOn
                });
                *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
                toggle_wifi.toggle_enabled();
            }
            Some(id) if id.kind() == Some(InterfaceKind::BluetoothAuto) => {
                if let Some(ble) = &toggle_ble {
                    ui_state.show_notice(if ble.is_enabled() {
                        screen::UiNotice::TurningOff
                    } else {
                        screen::UiNotice::TurningOn
                    });
                    *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
                    ble.toggle_enabled();
                }
            }
            Some(id) if Some(id) == tcp_id => {
                if let Some(tcp) = &toggle_tcp {
                    ui_state.show_notice(if tcp.is_enabled() {
                        screen::UiNotice::TurningOff
                    } else {
                        screen::UiNotice::TurningOn
                    });
                    *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
                    tcp.toggle_enabled();
                }
            }
            _ => {}
        },
        UiAction::ToggleStationUplink => {}
        UiAction::OpenLoRaEditor => ui_state.open_lora_editor(*working_lora_profile),
        UiAction::SetLoRaProfile(profile) => {
            ui_state.show_notice(screen::UiNotice::Saved);
            *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
            *working_lora_profile = profile;
        }
        UiAction::ResetLoRaProfile => {
            ui_state.show_notice(screen::UiNotice::Saved);
            *notice_until = Some(Instant::now() + NOTICE_TIMEOUT);
            *working_lora_profile = DEFAULT_915_PROFILE;
        }
        UiAction::SwapRadioMode => {}
        UiAction::OpenDocs => {}
        UiAction::CopySharedInstanceConfig => {}
    };

    let mut ui_state = ui_state();
    let mut working_lora_profile = DEFAULT_915_PROFILE;
    let mut notice_until: Option<Instant> = None;
    let mut active_press: Option<PressStart> = None;
    let mut last_logged: HashMap<InterfaceId, LoggedStatus> = HashMap::new();
    let mut interface_changes = query_handle.interface_store().subscribe();
    let mut cards: HVec<Card, 16> = HVec::new();
    let mut activity = screen::CardActivityTracker::<16>::new();
    let activity_started = Instant::now();
    let mut needs_redraw = true;
    let mut last_redraw = Instant::now();
    loop {
        let content = ScreenContent {
            cards: &cards,
            local_docs: None,
        };
        if let Some(tray) = tray.as_mut() {
            for control in tray.drain_controls(window.is_visible()) {
                match control {
                    DesktopControl::ShowWindow => {
                        window.show();
                        active_press = None;
                        needs_redraw = true;
                    }
                    DesktopControl::HideWindow => {
                        window.hide();
                        active_press = None;
                    }
                    DesktopControl::Announce => {
                        apply_action(
                            UiAction::Announce,
                            None,
                            &mut ui_state,
                            &mut working_lora_profile,
                            &mut notice_until,
                        );
                        needs_redraw = true;
                    }
                    DesktopControl::Quit => return,
                }
            }
        }

        for event in window.events() {
            match event {
                SimulatorEvent::Quit => {
                    if tray.is_some() {
                        window.hide();
                        active_press = None;
                        continue;
                    }
                    return;
                }
                SimulatorEvent::KeyDown { repeat: false, .. } => {
                    active_press.get_or_insert(press_start(PressSource::Key));
                    needs_redraw = true;
                }
                SimulatorEvent::KeyUp { .. } => {
                    let released = finish_press(
                        &mut active_press,
                        PressSource::Key,
                        Instant::now(),
                        content,
                        &mut ui_state,
                    );
                    let selected = selected_card_id(&ui_state, content);
                    apply_action(
                        released,
                        selected,
                        &mut ui_state,
                        &mut working_lora_profile,
                        &mut notice_until,
                    );
                    needs_redraw = true;
                }
                SimulatorEvent::MouseButtonDown { .. } => {
                    active_press.get_or_insert(press_start(PressSource::Mouse));
                    needs_redraw = true;
                }
                SimulatorEvent::MouseButtonUp { .. } => {
                    let released = finish_press(
                        &mut active_press,
                        PressSource::Mouse,
                        Instant::now(),
                        content,
                        &mut ui_state,
                    );
                    let selected = selected_card_id(&ui_state, content);
                    apply_action(
                        released,
                        selected,
                        &mut ui_state,
                        &mut working_lora_profile,
                        &mut notice_until,
                    );
                    needs_redraw = true;
                }
                SimulatorEvent::KeyDown { repeat: true, .. }
                | SimulatorEvent::MouseWheel { .. }
                | SimulatorEvent::MouseMove { .. } => {}
            }
        }

        let holding = active_press.is_some();
        let long_press =
            dispatch_long_press_if_ready(&mut active_press, Instant::now(), content, &mut ui_state);
        let selected = selected_card_id(&ui_state, content);
        apply_action(
            long_press,
            selected,
            &mut ui_state,
            &mut working_lora_profile,
            &mut notice_until,
        );

        let interfaces_changed = interface_changes.drain_changed();
        if window.is_visible()
            && (holding || interfaces_changed || last_redraw.elapsed() >= LIVE_REFRESH)
        {
            needs_redraw = true;
        }

        if notice_until.is_some_and(|until| Instant::now() >= until) {
            ui_state.clear_notice();
            notice_until = None;
            needs_redraw = true;
        }

        if needs_redraw && window.is_visible() {
            let snapshots = query_handle.interfaces();
            let now = Instant::now();
            for status in &snapshots {
                let connection = status.connection;
                let prev = last_logged.get(&status.id);
                let state_changed = prev.is_none_or(|p| p.connection != connection);
                let bytes_changed = prev
                    .map_or(status.rx_bytes != 0 || status.tx_bytes != 0, |p| {
                        p.rx_bytes != status.rx_bytes || p.tx_bytes != status.tx_bytes
                    });
                let throttle_ok =
                    prev.is_none_or(|p| now.duration_since(p.last_emit) >= STATUS_LOG_THROTTLE);
                if state_changed || (bytes_changed && throttle_ok) {
                    tracing::debug!(
                        event = "interface_status",
                        interface = classify(status.id)
                            .as_ref()
                            .map_or("?", |(_, label)| label.as_str()),
                        state = ?connection,
                        rx_bytes = status.rx_bytes,
                        tx_bytes = status.tx_bytes,
                        links = status.links,
                        destinations = status.destinations,
                    );
                    last_logged.insert(
                        status.id,
                        LoggedStatus {
                            connection,
                            rx_bytes: status.rx_bytes,
                            tx_bytes: status.tx_bytes,
                            last_emit: now,
                        },
                    );
                }
            }
            cards = screen::snapshots_to_cards(&snapshots, classify);
            let activity_secs = activity_started
                .elapsed()
                .as_secs()
                .min(u64::from(u32::MAX)) as u32;
            activity.update(&mut cards, activity_secs);
            let content = ScreenContent {
                cards: &cards,
                local_docs: None,
            };
            ui_state.sync(content);
            let interface_menu_details = screen::snapshots_to_interface_menu_details(
                ui_state.selected_card(content.cards),
                &snapshots,
            );
            let battery = screen::BatteryGauge::lipo().sample(&mut screen::NoBattery);
            let animation_ms = activity_started
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            screen::render(
                &mut display,
                screen::RenderFrame {
                    content,
                    battery,
                    gnss: None,
                    state: &ui_state,
                    interface_menu_details: &interface_menu_details,
                    animation_ms,
                },
            );
            window.update(&display);
            needs_redraw = false;
            last_redraw = Instant::now();
        }

        std::thread::sleep(FRAME);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::interfaces::{InterfaceSnapshot, Membership};

    fn test_cards() -> HVec<Card, 1> {
        screen::snapshots_to_cards(
            &[InterfaceSnapshot {
                id: InterfaceId::new([0; 8]),
                mode: personal_rns::interfaces::InterfaceMode::Full,
                gravity: personal_rns::interfaces::InterfaceGravity::ZERO,
                connection: ConnectionState::Connected,
                failure_reason: None,
                rx_bytes: 0,
                tx_bytes: 0,
                transfer_rates: None,
                destinations: 0,
                links: 0,
                transported_links: 0,
                membership: Membership::Independent,
            }],
            |_| Some((screen::CardKind::Usb, screen::card_label("USB"))),
        )
    }

    fn test_content(cards: &[Card]) -> ScreenContent<'_, 'static> {
        ScreenContent {
            cards,
            local_docs: None,
        }
    }

    #[test]
    fn release_before_threshold_is_short_press() {
        let started_at = Instant::now();
        let mut active_press = Some(PressStart {
            source: PressSource::Key,
            started_at,
            long_press_sent: false,
        });
        let mut ui_state = ui_state();
        let cards = test_cards();
        let content = test_content(&cards);

        finish_press(
            &mut active_press,
            PressSource::Key,
            started_at + LONG_PRESS_THRESHOLD - Duration::from_millis(1),
            content,
            &mut ui_state,
        );

        assert!(active_press.is_none());
        assert!(ui_state.selected_card(content.cards).is_some());
    }

    #[test]
    fn hold_dispatches_long_press_at_threshold() {
        let started_at = Instant::now();
        let mut active_press = Some(PressStart {
            source: PressSource::Mouse,
            started_at,
            long_press_sent: false,
        });
        let mut ui_state = ui_state();
        let cards = test_cards();
        let content = test_content(&cards);

        dispatch_long_press_if_ready(
            &mut active_press,
            started_at + LONG_PRESS_THRESHOLD,
            content,
            &mut ui_state,
        );

        assert!(active_press.expect("press remains active").long_press_sent);
        assert_eq!(
            ui_state.handle_input(InputEvent::LongPress, content),
            UiAction::Announce
        );
    }

    #[test]
    fn release_after_dispatched_long_press_is_noop() {
        let started_at = Instant::now();
        let mut active_press = Some(PressStart {
            source: PressSource::Key,
            started_at,
            long_press_sent: false,
        });
        let mut ui_state = ui_state();
        let cards = test_cards();
        let content = test_content(&cards);

        dispatch_long_press_if_ready(
            &mut active_press,
            started_at + LONG_PRESS_THRESHOLD,
            content,
            &mut ui_state,
        );
        finish_press(
            &mut active_press,
            PressSource::Key,
            started_at + LONG_PRESS_THRESHOLD + Duration::from_millis(1),
            content,
            &mut ui_state,
        );

        assert!(active_press.is_none());
        assert_eq!(
            ui_state.handle_input(InputEvent::LongPress, content),
            UiAction::Announce
        );
    }

    #[test]
    fn release_at_threshold_is_long_press_fallback() {
        let started_at = Instant::now();
        let mut active_press = Some(PressStart {
            source: PressSource::Key,
            started_at,
            long_press_sent: false,
        });
        let mut ui_state = ui_state();
        let cards = test_cards();
        let content = test_content(&cards);

        finish_press(
            &mut active_press,
            PressSource::Key,
            started_at + LONG_PRESS_THRESHOLD,
            content,
            &mut ui_state,
        );

        assert!(active_press.is_none());
        assert_eq!(
            ui_state.handle_input(InputEvent::LongPress, content),
            UiAction::Announce
        );
    }
}
