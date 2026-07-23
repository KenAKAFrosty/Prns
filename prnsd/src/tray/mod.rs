mod icon;

#[cfg(target_os = "linux")]
mod platform {
    use std::sync::OnceLock;

    use ksni::blocking::TrayMethods;
    use ksni::menu::StandardItem;

    use crate::shutdown::{self, ShutdownRequest, ShutdownSignal};

    use super::icon;

    pub(crate) struct RunningTray {
        _handle: ksni::blocking::Handle<LinuxTray>,
    }

    struct LinuxTray {
        shutdown: ShutdownRequest,
    }

    impl ksni::Tray for LinuxTray {
        const MENU_ON_ACTIVATE: bool = true;

        fn id(&self) -> String {
            "prnsd".into()
        }

        fn title(&self) -> String {
            "Personal RNS Daemon".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            static ICONS: OnceLock<Vec<ksni::Icon>> = OnceLock::new();

            ICONS
                .get_or_init(|| [32, 64].into_iter().map(status_notifier_icon).collect())
                .clone()
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                icon_name: String::new(),
                icon_pixmap: self.icon_pixmap(),
                title: "Personal RNS Daemon".into(),
                description: if self.shutdown.was_requested() {
                    "Stopping prnsd…"
                } else {
                    "prnsd is running"
                }
                .into(),
            }
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            vec![
                StandardItem {
                    label: format!("Personal RNS Daemon · v{}", env!("CARGO_PKG_VERSION")),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
                ksni::MenuItem::Separator,
                StandardItem {
                    label: if self.shutdown.was_requested() {
                        "Stopping prnsd…"
                    } else {
                        "Stop prnsd"
                    }
                    .into(),
                    enabled: !self.shutdown.was_requested(),
                    activate: Box::new(|tray: &mut LinuxTray| {
                        tray.shutdown.request();
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    fn status_notifier_icon(size: u32) -> ksni::Icon {
        let icon::TrayIcon { rgba, size } = icon::render(size);
        let mut argb = Vec::with_capacity(rgba.len());
        for pixel in rgba.chunks_exact(4) {
            argb.extend_from_slice(&[pixel[3], pixel[0], pixel[1], pixel[2]]);
        }
        ksni::Icon {
            width: size as i32,
            height: size as i32,
            data: argb,
        }
    }

    pub(crate) fn start() -> Result<(RunningTray, ShutdownSignal), String> {
        let (shutdown, signal) = shutdown::channel();
        let handle = LinuxTray { shutdown }
            .spawn()
            .map_err(|error| format!("StatusNotifier tray start failed: {error}"))?;
        Ok((RunningTray { _handle: handle }, signal))
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod platform {
    use std::process::ExitCode;
    use std::time::Duration;

    use prnsd_control::ManagedProcess;
    use tao::event::Event;
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    use crate::shutdown::{self, ShutdownRequest};
    use crate::{cli, daemon};

    use super::icon;

    enum TrayEvent {
        DaemonReady(tokio::sync::oneshot::Sender<Result<(), String>>),
        StopRequested,
    }

    struct DesktopTray {
        icon: TrayIcon,
        stop_item: MenuItem,
    }

    impl DesktopTray {
        fn new() -> Result<Self, String> {
            let heading = MenuItem::new(
                format!("Personal RNS Daemon · v{}", env!("CARGO_PKG_VERSION")),
                false,
                None,
            );
            let stop_item = MenuItem::with_id("prnsd-stop", "Stop prnsd", true, None);
            let separator = PredefinedMenuItem::separator();
            let menu = Menu::with_items(&[&heading, &separator, &stop_item])
                .map_err(|error| format!("tray menu build failed: {error}"))?;
            let rendered = icon::render(64);
            let tray_icon = Icon::from_rgba(rendered.rgba, rendered.size, rendered.size)
                .map_err(|error| format!("tray icon pixels invalid: {error}"))?;
            let icon = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("Personal RNS Daemon is running")
                .with_icon(tray_icon)
                .with_menu_on_left_click(true)
                .build()
                .map_err(|error| format!("tray icon build failed: {error}"))?;
            Ok(Self { icon, stop_item })
        }

        fn show_stopping(&self) {
            self.stop_item.set_text("Stopping prnsd…");
            self.stop_item.set_enabled(false);
            let _ = self
                .icon
                .set_tooltip(Some("Personal RNS Daemon is stopping"));
        }
    }

    pub(crate) fn run(args: cli::DaemonArgs, managed: Option<ManagedProcess>) -> ! {
        #[cfg(target_os = "macos")]
        let event_loop = {
            use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};

            let mut event_loop = EventLoopBuilder::<TrayEvent>::with_user_event().build();
            event_loop.set_activation_policy(ActivationPolicy::Accessory);
            event_loop.set_dock_visibility(false);
            event_loop
        };
        #[cfg(target_os = "windows")]
        let event_loop = EventLoopBuilder::<TrayEvent>::with_user_event().build();

        let menu_proxy = event_loop.create_proxy();
        let daemon_proxy = event_loop.create_proxy();
        let (shutdown, signal) = shutdown::channel();
        let mut shutdown = Some(shutdown);
        let mut tray = None;

        let ready_proxy = daemon_proxy.clone();
        let spawned = std::thread::Builder::new()
            .name("prnsd-runtime".into())
            .spawn(move || {
                let exit_code = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => {
                        let (ready, ready_signal) = tokio::sync::oneshot::channel();
                        runtime.spawn(async move {
                            if ready_signal.await.is_ok() {
                                let (started, started_signal) = tokio::sync::oneshot::channel();
                                if ready_proxy
                                    .send_event(TrayEvent::DaemonReady(started))
                                    .is_err()
                                {
                                    tracing::warn!(
                                        event = "tray_unavailable",
                                        error = "desktop event loop is closed",
                                    );
                                    return;
                                }
                                match tokio::time::timeout(Duration::from_secs(5), started_signal)
                                    .await
                                {
                                    Ok(Ok(Ok(()))) => {
                                        tracing::info!(event = "tray_started");
                                    }
                                    Ok(Ok(Err(error))) => {
                                        tracing::warn!(event = "tray_unavailable", error = %error);
                                    }
                                    Ok(Err(_)) => {
                                        tracing::warn!(
                                            event = "tray_unavailable",
                                            error = "desktop event loop dropped tray startup",
                                        );
                                    }
                                    Err(_) => {
                                        tracing::warn!(
                                            event = "tray_unavailable",
                                            error = "desktop event loop did not respond",
                                        );
                                    }
                                }
                            }
                        });
                        runtime.block_on(daemon::run(args, managed, Some(signal), Some(ready)));
                        0
                    }
                    Err(error) => {
                        eprintln!("prnsd: async runtime initialization failed: {error}");
                        1
                    }
                };

                // `EventLoop::run` owns the main thread on these platforms. The
                // daemon has already completed its persistence and observability
                // shutdown here, so terminating the process is both safe and
                // robust when no interactive desktop session can service a
                // final user event.
                std::process::exit(exit_code);
            });
        if let Err(error) = spawned {
            eprintln!("prnsd: daemon thread initialization failed: {error}");
            std::process::exit(1);
        }

        event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::Wait;
            match event {
                Event::UserEvent(TrayEvent::DaemonReady(started)) => {
                    let outcome = match DesktopTray::new() {
                        Ok(created) => {
                            let stop_id = created.stop_item.id().clone();
                            let proxy = menu_proxy.clone();
                            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                                if event.id() == &stop_id {
                                    let _ = proxy.send_event(TrayEvent::StopRequested);
                                }
                            }));
                            tray = Some(created);
                            Ok(())
                        }
                        Err(error) => Err(error),
                    };
                    let _ = started.send(outcome);
                }
                Event::UserEvent(TrayEvent::StopRequested)
                    if shutdown.as_mut().is_some_and(ShutdownRequest::request) =>
                {
                    if let Some(tray) = tray.as_ref() {
                        tray.show_stopping();
                    }
                }
                _ => {}
            }
        })
    }

    pub(crate) fn managed_process() -> Result<Option<ManagedProcess>, ExitCode> {
        ManagedProcess::from_environment().map_err(|error| {
            eprintln!("prnsd: {error}");
            ExitCode::FAILURE
        })
    }
}

pub(crate) use platform::*;
