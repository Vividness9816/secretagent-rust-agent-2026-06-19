//! Windows Service Control Manager backend. Installs an auto-start service whose binPath is
//! `<exe> service-run`; the `service-run` entry hands control to the SCM dispatcher so the
//! process runs AS a real service (responds to Stop) rather than being killed for not
//! reporting Running. ADR-20260621: in-binary dispatcher, never `sc.exe`.

use super::{SERVICE_DISPLAY, SERVICE_NAME};
use anyhow::{Context, Result};
use std::ffi::OsString;
use std::time::Duration;

pub fn install() -> Result<()> {
    use windows_service::service::{
        ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceType,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let exe = std::env::current_exe().context("resolving the running binary path")?;
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .context("opening the service manager (run from an elevated/admin shell)")?;

    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_DISPLAY),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart, // survives reboot
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec![OsString::from("service-run")],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let service = manager
        .create_service(&info, ServiceAccess::CHANGE_CONFIG)
        .context("creating the service (needs admin)")?;
    let _ = service.set_description("SecretAgent autonomous agent gateway");
    println!("installed {SERVICE_NAME} (Windows Service, auto-start). It will start on boot.");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    use windows_service::service::ServiceAccess;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("opening the service manager")?;
    let service = manager
        .open_service(SERVICE_NAME, ServiceAccess::DELETE | ServiceAccess::STOP)
        .context("opening the service")?;
    let _ = service.stop();
    service.delete().context("deleting the service")?;
    println!("uninstalled {SERVICE_NAME} (Windows Service).");
    Ok(())
}

/// Never fails — reports the SCM state for doctor.
pub fn status() -> String {
    use windows_service::service::ServiceAccess;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let q = (|| -> Result<String> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)?;
        let st = service.query_status()?;
        Ok(format!("{:?}", st.current_state))
    })();
    q.unwrap_or_else(|_| "not installed".to_string())
}

// ---- the SCM dispatcher: makes `<exe> service-run` run AS a service ----

windows_service::define_windows_service!(ffi_service_main, service_main);

fn service_main(_args: Vec<OsString>) {
    // Errors here can't surface to a console; best-effort. A failure leaves the service in a
    // start-pending→failed state visible via `sc query` (+ the daemon.log in 4c).
    let _ = run_service();
}

fn run_service() -> Result<()> {
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};

    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();
    let handler = move |control| match control {
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        // Stop = `sc stop` / uninstall; Shutdown = OS reboot/poweroff (the boot-survival path
        // acceptance #1 cares about — SCM sends SHUTDOWN, not Stop). Both drain to clean exit.
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = shutdown_tx.send(());
            ServiceControlHandlerResult::NoError
        }
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let status_handle = service_control_handler::register(SERVICE_NAME, handler)?;

    let running = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    };
    status_handle.set_service_status(running)?;

    // Run the async gateway on a tokio runtime; the shutdown future resolves when SCM signals
    // Stop (a std mpsc rx awaited off the reactor via spawn_blocking).
    let rt = tokio::runtime::Runtime::new()?;
    let result = rt.block_on(async move {
        let shutdown = async move {
            let _ = tokio::task::spawn_blocking(move || {
                let _ = shutdown_rx.recv();
            })
            .await;
        };
        crate::gateway::run_until(shutdown).await
    });

    let stopped = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    };
    status_handle.set_service_status(stopped)?;
    result
}

/// Entry for the `service-run` subcommand on Windows: hand control to the SCM dispatcher.
pub fn run_service_dispatch() -> Result<()> {
    windows_service::service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .context("starting the SCM dispatcher (only valid when launched by the service manager)")?;
    Ok(())
}
