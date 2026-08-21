mod actions;
mod icon;

use crate::daemon::DaemonStatus;

const DAEMON_DISPLAY_NAME: &str = "Prns Daemon";

fn status_label(status: DaemonStatus, managed: bool, stopping: bool) -> String {
    if stopping {
        return "Stopping prnsd…".into();
    }
    let prefix = if managed {
        if status.unavailable != 0 || status.retrying != 0 || status.impaired != 0 {
            "Degraded"
        } else {
            "Running"
        }
    } else {
        "Foreground session"
    };
    if status.unavailable != 0 {
        return format!(
            "{prefix} · {} {} unavailable",
            status.unavailable,
            interface_noun(status.unavailable),
        );
    }
    if status.retrying != 0 {
        return format!(
            "{prefix} · {} {} retrying",
            status.retrying,
            interface_noun(status.retrying),
        );
    }
    if status.impaired != 0 {
        return format!(
            "{prefix} · {} {} impaired",
            status.impaired,
            interface_noun(status.impaired),
        );
    }
    format!(
        "{prefix} · {} {}",
        status.interface_count,
        interface_noun(status.interface_count),
    )
}

const fn interface_noun(count: u32) -> &'static str {
    if count == 1 {
        "interface"
    } else {
        "interfaces"
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::path::PathBuf;
    use std::sync::OnceLock;

    use ksni::menu::StandardItem;
    use ksni::TrayMethods;

    use crate::daemon::{DaemonStatus, DaemonStatusPublisher};
    use crate::shutdown::{self, ShutdownRequest, ShutdownSignal};

    use super::actions::{TrayAction, TrayActionContext, TrayActionError};
    use super::{icon, status_label, DAEMON_DISPLAY_NAME};

    pub(crate) struct RunningTray {
        handle: ksni::Handle<LinuxTray>,
    }

    #[derive(Debug)]
    pub(crate) enum TrayStartError {
        Actions(TrayActionError),
        Platform(ksni::Error),
    }

    impl std::fmt::Display for TrayStartError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Actions(error) => write!(formatter, "tray actions unavailable: {error}"),
                Self::Platform(error) => {
                    write!(formatter, "StatusNotifier tray unavailable: {error}")
                }
            }
        }
    }

    struct LinuxTray {
        actions: TrayActionContext,
        status: DaemonStatus,
        shutdown: ShutdownRequest,
    }

    impl ksni::Tray for LinuxTray {
        const MENU_ON_ACTIVATE: bool = true;

        fn id(&self) -> String {
            "prnsd".into()
        }

        fn title(&self) -> String {
            DAEMON_DISPLAY_NAME.into()
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
                title: DAEMON_DISPLAY_NAME.into(),
                description: status_label(
                    self.status,
                    self.actions.can_attach_terminal(),
                    self.shutdown.was_requested(),
                ),
            }
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            vec![
                StandardItem {
                    label: format!("{DAEMON_DISPLAY_NAME} · v{}", env!("CARGO_PKG_VERSION")),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: status_label(
                        self.status,
                        self.actions.can_attach_terminal(),
                        self.shutdown.was_requested(),
                    ),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
                ksni::MenuItem::Separator,
                StandardItem {
                    label: "Open Prns Terminal".into(),
                    enabled: self.actions.can_attach_terminal(),
                    activate: Box::new(|tray: &mut LinuxTray| {
                        tray.perform(TrayAction::OpenTerminal);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Show Network Status".into(),
                    activate: Box::new(|tray: &mut LinuxTray| {
                        tray.perform(TrayAction::ShowStatus);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Announce NNPages Now".into(),
                    activate: Box::new(|tray: &mut LinuxTray| {
                        tray.perform(TrayAction::AnnounceNnPages);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Manage Interfaces…".into(),
                    activate: Box::new(|tray: &mut LinuxTray| {
                        tray.perform(TrayAction::ManageInterfaces);
                    }),
                    ..Default::default()
                }
                .into(),
                ksni::MenuItem::Separator,
                StandardItem {
                    label: "Open Configuration Folder".into(),
                    activate: Box::new(|tray: &mut LinuxTray| {
                        tray.perform(TrayAction::OpenConfigDirectory);
                    }),
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

    impl LinuxTray {
        fn perform(&self, action: TrayAction) {
            if let Err(error) = self.actions.perform(action) {
                tracing::warn!(
                    event = "tray_action_failed",
                    action = action.event_name(),
                    error = %error,
                );
            }
        }
    }

    fn status_notifier_icon(size: u32) -> ksni::Icon {
        let icon::TrayIcon { rgba, size } = icon::render(size);
        let mut argb = Vec::with_capacity(rgba.len());
        for pixel in rgba.as_chunks::<4>().0 {
            argb.extend_from_slice(&[pixel[3], pixel[0], pixel[1], pixel[2]]);
        }
        ksni::Icon {
            width: size as i32,
            height: size as i32,
            data: argb,
        }
    }

    pub(crate) async fn start(
        config_dir: PathBuf,
        managed_state_dir: Option<PathBuf>,
        status: DaemonStatus,
    ) -> Result<(RunningTray, ShutdownSignal, DaemonStatusPublisher), TrayStartError> {
        let actions = TrayActionContext::discover(config_dir, managed_state_dir)
            .map_err(TrayStartError::Actions)?;
        let (shutdown, signal) = shutdown::channel();
        let handle = LinuxTray {
            actions,
            status,
            shutdown,
        }
        .spawn()
        .await
        .map_err(TrayStartError::Platform)?;
        let update_handle = handle.clone();
        let publisher = DaemonStatusPublisher::new(move |status| {
            let handle = update_handle.clone();
            tokio::spawn(async move {
                let _ = handle.update(|tray| tray.status = status).await;
            });
        });
        Ok((RunningTray { handle }, signal, publisher))
    }

    impl Drop for RunningTray {
        fn drop(&mut self) {
            drop(self.handle.shutdown());
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod platform {
    use std::time::Duration;

    use prnsd_control::ManagedProcess;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
    use winit::window::WindowId;

    use crate::daemon::{DaemonPresentation, DaemonReady, DaemonStatus, DaemonStatusPublisher};
    use crate::shutdown::{self, ShutdownRequest};
    use crate::{cli, daemon};

    use super::actions::{TrayAction, TrayActionContext};
    use super::{icon, status_label, DAEMON_DISPLAY_NAME};

    enum TrayEvent {
        DaemonReady {
            ready: DaemonReady,
            started: tokio::sync::oneshot::Sender<Result<(), String>>,
        },
        StatusChanged(DaemonStatus),
        MenuSelected(MenuEvent),
        StopRequested,
    }

    struct DesktopTray {
        actions: TrayActionContext,
        icon: TrayIcon,
        status_item: MenuItem,
        open_terminal_item: MenuItem,
        show_status_item: MenuItem,
        announce_nnpages_item: MenuItem,
        manage_interfaces_item: MenuItem,
        open_config_item: MenuItem,
        stop_item: MenuItem,
    }

    impl DesktopTray {
        fn new(ready: DaemonReady) -> Result<Self, String> {
            let actions = TrayActionContext::discover(ready.config_dir, ready.managed_state_dir)
                .map_err(|error| format!("tray actions unavailable: {error}"))?;
            let managed = actions.can_attach_terminal();
            let heading = MenuItem::new(
                format!("{DAEMON_DISPLAY_NAME} · v{}", env!("CARGO_PKG_VERSION")),
                false,
                None,
            );
            let status_item =
                MenuItem::new(status_label(ready.status, managed, false), false, None);
            let open_terminal_item =
                MenuItem::with_id("prnsd-open-terminal", "Open Prns Terminal", managed, None);
            let show_status_item =
                MenuItem::with_id("prnsd-show-status", "Show Network Status", true, None);
            let announce_nnpages_item =
                MenuItem::with_id("prnsd-announce-nnpages", "Announce NNPages Now", true, None);
            let manage_interfaces_item =
                MenuItem::with_id("prnsd-manage-interfaces", "Manage Interfaces…", true, None);
            let open_config_item =
                MenuItem::with_id("prnsd-open-config", "Open Configuration Folder", true, None);
            let stop_item = MenuItem::with_id("prnsd-stop", "Stop prnsd", true, None);
            let status_separator = PredefinedMenuItem::separator();
            let tools_separator = PredefinedMenuItem::separator();
            let stop_separator = PredefinedMenuItem::separator();
            let menu = Menu::with_items(&[
                &heading,
                &status_item,
                &status_separator,
                &open_terminal_item,
                &show_status_item,
                &announce_nnpages_item,
                &manage_interfaces_item,
                &tools_separator,
                &open_config_item,
                &stop_separator,
                &stop_item,
            ])
            .map_err(|error| format!("tray menu build failed: {error}"))?;
            let rendered = icon::render(64);
            let tray_icon = Icon::from_rgba(rendered.rgba, rendered.size, rendered.size)
                .map_err(|error| format!("tray icon pixels invalid: {error}"))?;
            let icon = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip(format!("{DAEMON_DISPLAY_NAME} is running"))
                .with_icon(tray_icon)
                .with_menu_on_left_click(true)
                .build()
                .map_err(|error| format!("tray icon build failed: {error}"))?;
            Ok(Self {
                actions,
                icon,
                status_item,
                open_terminal_item,
                show_status_item,
                announce_nnpages_item,
                manage_interfaces_item,
                open_config_item,
                stop_item,
            })
        }

        fn action_for(&self, event: &MenuEvent) -> Option<TrayAction> {
            let id = event.id();
            if id == self.open_terminal_item.id() {
                Some(TrayAction::OpenTerminal)
            } else if id == self.show_status_item.id() {
                Some(TrayAction::ShowStatus)
            } else if id == self.announce_nnpages_item.id() {
                Some(TrayAction::AnnounceNnPages)
            } else if id == self.manage_interfaces_item.id() {
                Some(TrayAction::ManageInterfaces)
            } else if id == self.open_config_item.id() {
                Some(TrayAction::OpenConfigDirectory)
            } else {
                None
            }
        }

        fn update_status(&self, status: DaemonStatus) {
            let label = status_label(status, self.actions.can_attach_terminal(), false);
            self.status_item.set_text(&label);
            let _ = self
                .icon
                .set_tooltip(Some(format!("{DAEMON_DISPLAY_NAME} · {label}")));
        }

        fn perform(&self, action: TrayAction) {
            if let Err(error) = self.actions.perform(action) {
                tracing::warn!(
                    event = "tray_action_failed",
                    action = action.event_name(),
                    error = %error,
                );
            }
        }

        fn show_stopping(&self) {
            self.status_item.set_text("Stopping prnsd…");
            self.stop_item.set_text("Stopping prnsd…");
            self.stop_item.set_enabled(false);
            let _ = self
                .icon
                .set_tooltip(Some(format!("{DAEMON_DISPLAY_NAME} is stopping")));
        }
    }

    struct TrayApplication {
        menu_proxy: winit::event_loop::EventLoopProxy<TrayEvent>,
        shutdown: Option<ShutdownRequest>,
        tray: Option<DesktopTray>,
    }

    impl ApplicationHandler<TrayEvent> for TrayApplication {
        fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

        fn window_event(
            &mut self,
            _event_loop: &ActiveEventLoop,
            _window_id: WindowId,
            _event: WindowEvent,
        ) {
        }

        fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: TrayEvent) {
            match event {
                TrayEvent::DaemonReady { ready, started } => {
                    let outcome = match DesktopTray::new(ready) {
                        Ok(created) => {
                            let proxy = self.menu_proxy.clone();
                            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                                let _ = proxy.send_event(TrayEvent::MenuSelected(event));
                            }));
                            self.tray = Some(created);
                            Ok(())
                        }
                        Err(error) => Err(error),
                    };
                    let _ = started.send(outcome);
                }
                TrayEvent::StatusChanged(status) => {
                    if let Some(tray) = self.tray.as_ref() {
                        tray.update_status(status);
                    }
                }
                TrayEvent::MenuSelected(event)
                    if self
                        .tray
                        .as_ref()
                        .is_some_and(|tray| event.id() == tray.stop_item.id()) =>
                {
                    let _ = self.menu_proxy.send_event(TrayEvent::StopRequested);
                }
                TrayEvent::MenuSelected(event) => {
                    if let Some((tray, action)) = self
                        .tray
                        .as_ref()
                        .and_then(|tray| tray.action_for(&event).map(|action| (tray, action)))
                    {
                        tray.perform(action);
                    }
                }
                TrayEvent::StopRequested
                    if self.shutdown.as_mut().is_some_and(ShutdownRequest::request) =>
                {
                    if let Some(tray) = self.tray.as_ref() {
                        tray.show_stopping();
                    }
                }
                TrayEvent::StopRequested => {}
            }
        }
    }

    pub(crate) fn run(args: cli::DaemonArgs, managed: Option<ManagedProcess>) -> ! {
        let report_panic = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            report_panic(panic_info);
            std::process::exit(101);
        }));
        let mut event_loop_builder = EventLoop::<TrayEvent>::with_user_event();
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};

            event_loop_builder
                .with_activation_policy(ActivationPolicy::Accessory)
                .with_default_menu(false);
        }
        let event_loop = match event_loop_builder.build() {
            Ok(event_loop) => event_loop,
            Err(error) => {
                eprintln!(
                    "prnsd: desktop event loop unavailable ({error}); running without a tray"
                );
                run_without_tray(args, managed);
            }
        };
        event_loop.set_control_flow(ControlFlow::Wait);

        let menu_proxy = event_loop.create_proxy();
        let daemon_proxy = event_loop.create_proxy();
        let (shutdown, signal) = shutdown::channel();

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
                        let status_proxy = ready_proxy.clone();
                        let presentation = DaemonPresentation {
                            ready,
                            status: DaemonStatusPublisher::new(move |status| {
                                let _ = status_proxy.send_event(TrayEvent::StatusChanged(status));
                            }),
                        };
                        runtime.spawn(async move {
                            if let Ok(ready) = ready_signal.await {
                                let (started, started_signal) = tokio::sync::oneshot::channel();
                                if ready_proxy
                                    .send_event(TrayEvent::DaemonReady { ready, started })
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
                        match runtime.block_on(daemon::run(
                            args,
                            managed,
                            Some(signal),
                            Some(presentation),
                        )) {
                            Ok(()) => 0,
                            Err(error) => {
                                eprintln!("prnsd: {error}");
                                1
                            }
                        }
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

        let mut application = TrayApplication {
            menu_proxy,
            shutdown: Some(shutdown),
            tray: None,
        };
        if let Err(error) = event_loop.run_app(&mut application) {
            eprintln!("prnsd: desktop event loop failed: {error}");
        }
        loop {
            std::thread::park();
        }
    }

    fn run_without_tray(args: cli::DaemonArgs, managed: Option<ManagedProcess>) -> ! {
        let exit_code = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => match runtime.block_on(daemon::run(args, managed, None, None)) {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("prnsd: {error}");
                    1
                }
            },
            Err(error) => {
                eprintln!("prnsd: async runtime initialization failed: {error}");
                1
            }
        };
        std::process::exit(exit_code)
    }
}

pub(crate) use platform::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_labels_are_live_specific_and_grammatical() {
        assert_eq!(
            status_label(
                DaemonStatus {
                    interface_count: 4,
                    retrying: 0,
                    impaired: 0,
                    unavailable: 0,
                },
                true,
                false,
            ),
            "Running · 4 interfaces"
        );
        assert_eq!(
            status_label(
                DaemonStatus {
                    interface_count: 4,
                    retrying: 1,
                    impaired: 0,
                    unavailable: 0,
                },
                true,
                false,
            ),
            "Degraded · 1 interface retrying"
        );
        assert_eq!(
            status_label(
                DaemonStatus {
                    interface_count: 4,
                    retrying: 0,
                    impaired: 0,
                    unavailable: 2,
                },
                true,
                false,
            ),
            "Degraded · 2 interfaces unavailable"
        );
        assert_eq!(
            status_label(
                DaemonStatus {
                    interface_count: 4,
                    retrying: 0,
                    impaired: 2,
                    unavailable: 0,
                },
                true,
                false,
            ),
            "Degraded · 2 interfaces impaired"
        );
    }

    #[test]
    fn foreground_and_stopping_states_do_not_claim_managed_attachment() {
        let status = DaemonStatus {
            interface_count: 1,
            retrying: 0,
            impaired: 0,
            unavailable: 0,
        };
        assert_eq!(
            status_label(status, false, false),
            "Foreground session · 1 interface"
        );
        assert_eq!(status_label(status, true, true), "Stopping prnsd…");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_tray_start_failures_preserve_platform_and_action_causes() {
        let platform = TrayStartError::Platform(ksni::Error::WontShow);
        let actions = TrayStartError::Actions(actions::TrayActionError::ForegroundSession);

        assert!(matches!(platform, TrayStartError::Platform(_)));
        assert!(matches!(actions, TrayStartError::Actions(_)));
    }
}
