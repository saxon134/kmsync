#![allow(unsafe_code)]

use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::{
    atomic::{AtomicBool, AtomicPtr, Ordering},
    OnceLock,
};
use std::thread;
use std::time::Duration;

use windows_sys::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW,
    SERVICE_ACCEPT_STOP, SERVICE_CONTROL_STOP, SERVICE_RUNNING, SERVICE_START_PENDING,
    SERVICE_STATUS, SERVICE_STATUS_HANDLE, SERVICE_STOPPED, SERVICE_STOP_PENDING,
    SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
};

static SERVICE_NAME: OnceLock<String> = OnceLock::new();
static SERVICE_CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();
static SERVICE_STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static SERVICE_STATUS_HANDLE_PTR: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(null_mut());

pub(crate) fn run(service_name: &str, config_path: &Path) -> Result<(), String> {
    let _ = SERVICE_NAME.set(service_name.to_string());
    let _ = SERVICE_CONFIG_PATH.set(config_path.to_path_buf());
    SERVICE_STOP_REQUESTED.store(false, Ordering::SeqCst);

    let mut service_name_wide = wide_string(service_name);
    let service_table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: service_name_wide.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: null_mut(),
            lpServiceProc: None,
        },
    ];

    let started = unsafe { StartServiceCtrlDispatcherW(service_table.as_ptr()) };
    if started == 0 {
        return Err(format!(
            "failed to start Windows service dispatcher: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut windows_sys::core::PWSTR) {
    let service_name = SERVICE_NAME
        .get()
        .map_or("KMSyncCoreService", String::as_str);
    let service_name_wide = wide_string(service_name);
    let handle = unsafe {
        RegisterServiceCtrlHandlerExW(
            service_name_wide.as_ptr(),
            Some(service_control_handler),
            null(),
        )
    };
    if handle.is_null() {
        return;
    }
    SERVICE_STATUS_HANDLE_PTR.store(handle.cast(), Ordering::SeqCst);

    set_status(handle, SERVICE_START_PENDING, 0);
    set_status(handle, SERVICE_RUNNING, SERVICE_ACCEPT_STOP);

    while !SERVICE_STOP_REQUESTED.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(500));
    }

    set_status(handle, SERVICE_STOP_PENDING, 0);
    set_status(handle, SERVICE_STOPPED, 0);
}

unsafe extern "system" fn service_control_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut std::ffi::c_void,
    _context: *mut std::ffi::c_void,
) -> u32 {
    if control == SERVICE_CONTROL_STOP {
        SERVICE_STOP_REQUESTED.store(true, Ordering::SeqCst);
        let handle = SERVICE_STATUS_HANDLE_PTR.load(Ordering::SeqCst);
        if !handle.is_null() {
            set_status(handle.cast(), SERVICE_STOP_PENDING, 0);
        }
    }
    0
}

fn set_status(handle: SERVICE_STATUS_HANDLE, state: u32, controls_accepted: u32) {
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: controls_accepted,
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: 0,
    };
    unsafe {
        SetServiceStatus(handle, &status);
    }
}

fn wide_string(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
