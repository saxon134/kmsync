#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

mod client;
mod desktop_config;
mod desktop_state;
#[allow(dead_code)]
mod local_config;
mod native_desktop;
mod platform;
mod transport;
#[cfg(windows)]
mod windows_service;

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError},
    Arc, Mutex, OnceLock,
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use kmsync_core::{
    file_content_hash, local_ipc, ClipboardFileMetadata, ClipboardFiles, ClipboardText,
    CompiledProfile, ControlMessage, DeviceId, FileTransferChunk, InputEvent, InputEventEnvelope,
    Key, KeyEvent, KeyState, Modifiers, Profile, ProtocolEvent, ProtocolFrame, ProtocolPayload,
    RemoteInputState, TransportLane,
};
use platform::{
    CaptureDecision, CapturedInput, ClipboardBackend, DisplayLayout, InputCaptureBackend,
    InputInjector, PlatformAdapter, PointerPosition,
};
use transport::{QuicEventReceiver, QuicEventSender};

const CAPTURE_CONNECT_RETRY_DELAY: Duration = Duration::from_millis(500);
const METRICS_REPORT_INTERVAL: Duration = Duration::from_secs(5);
const DESKTOP_CAPTURE_PLAN_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const DESKTOP_DIRECT_CONNECT_TIMEOUT: Duration = Duration::from_millis(150);
const DESKTOP_TRANSMIT_FAILURE_BACKOFF: Duration = Duration::from_millis(500);
const DESKTOP_STALE_MOUSE_MOVE_DROP_AFTER: Duration = Duration::from_millis(120);
const DESKTOP_RELIABLE_QUEUE_SEND_GRACE: Duration = Duration::from_millis(25);
const DESKTOP_RELIABLE_QUEUE_SEND_RETRY_DELAY: Duration = Duration::from_millis(1);
const RELIABLE_INPUT_GAP_RECOVERY_TIMEOUT: Duration = Duration::from_millis(20);
const DEFAULT_FILE_TRANSFER_CHUNK_BYTES: usize = 1024;
const MAX_FILE_TRANSFER_CHUNK_BYTES: usize = 1024;
const DEFAULT_DAEMON_CONFIG_FILE: &str = "daemon.example.json";
const DEFAULT_DAEMON_CONFIG_TEMPLATE: &str = include_str!("../../../configs/daemon.example.json");
const WINDOWS_SERVICE_NAME: &str = "KMSyncCoreService";
const WINDOWS_SERVICE_DISPLAY_NAME: &str = "KMSync Core Service";
const MACOS_LAUNCH_AGENT_PATH: &str = "/Library/LaunchAgents/com.kmsync.mvp.plist";
const MACOS_INSTALLED_APP_BINARY_PATH: &str = "/Applications/KMSync.app/Contents/MacOS/kmsync";
const MACOS_CORE_SERVICE_LAUNCH_SERVICES_CONTEXT: &str = "launch_services_app";
const MACOS_CORE_SERVICE_DIRECT_APP_BINARY_CONTEXT: &str = "direct_app_binary";

fn main() {
    install_crash_report_hook();
    if let Err(error) = run() {
        eprintln!("{}", format_user_diagnostic(&error));
        std::process::exit(1);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiagnosticKind {
    PermissionMissing,
    CoreServiceUnavailable,
    ConnectionFailed,
    InjectionFailed,
    Unknown,
}

#[derive(Debug, Eq, PartialEq)]
struct UserDiagnostic {
    kind: DiagnosticKind,
    title: &'static str,
    next_steps: &'static [&'static str],
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct DaemonError {
    kind: DiagnosticKind,
    message: String,
}

impl DaemonError {
    fn new(kind: DiagnosticKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn from_message(message: impl Into<String>) -> Self {
        let message = message.into();
        Self::new(classify_diagnostic_kind(&message), message)
    }

    const fn kind(&self) -> DiagnosticKind {
        self.kind
    }

    fn diagnostic(&self) -> UserDiagnostic {
        diagnostic_for_kind(self.kind())
    }
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DaemonError {}

impl From<String> for DaemonError {
    fn from(message: String) -> Self {
        Self::from_message(message)
    }
}

impl From<&str> for DaemonError {
    fn from(message: &str) -> Self {
        Self::from_message(message)
    }
}

const PERMISSION_NEXT_STEPS: &[&str] = &[
    "Grant Accessibility permission to KMSync so it can inject input.",
    "Grant Input Monitoring permission so KMSync can capture keyboard and mouse events.",
    "Restart kmsync after changing macOS privacy permissions.",
];

const CONNECTION_NEXT_STEPS: &[&str] = &[
    "Check that both devices are online and on the same network or reachable through relay.",
    "Verify the target address, backend session, firewall, and VPN settings.",
    "Retry after network changes settle, then refresh device presence.",
];

const CORE_SERVICE_NEXT_STEPS: &[&str] = &[
    "Restart KMSync so the desktop core-service can start.",
    "If KMSync was just updated, install the latest macOS package and reopen KMSync.app.",
    "On macOS, open /Applications/KMSync.app so the desktop app can start its core-service.",
    "Run `kmsync status` again to confirm the local core-service is reachable.",
];

const INJECTION_NEXT_STEPS: &[&str] = &[
    "Run as the interactive desktop user on Windows so SendInput can reach the active session.",
    "Grant Accessibility permission on macOS before injecting keyboard or mouse events.",
    "Review the keyboard mapping when the error names an unsupported key.",
];

const UNKNOWN_NEXT_STEPS: &[&str] = &[
    "Review the error details above and rerun the command with the same arguments.",
    "Run `kmsync info` to inspect platform capabilities and permission hints.",
];

fn diagnostic_for_error(error: &str) -> UserDiagnostic {
    diagnostic_for_kind(classify_diagnostic_kind(error))
}

fn classify_diagnostic_kind(error: &str) -> DiagnosticKind {
    if contains_any_case_insensitive(error, &["local ipc"]) {
        return DiagnosticKind::CoreServiceUnavailable;
    }

    if contains_any_case_insensitive(
        error,
        &[
            "permission",
            "accessibility",
            "input monitoring",
            "event tap",
            "interactive desktop",
        ],
    ) {
        return DiagnosticKind::PermissionMissing;
    }

    if contains_any_case_insensitive(
        error,
        &[
            "connection refused",
            "failed to connect",
            "request failed",
            "timed out",
            "timeout",
            "network is unreachable",
            "no route to host",
            "host unreachable",
            "connection reset",
            "connection aborted",
            "all direct lan candidates failed",
            "no direct lan candidates available",
            "relay route failed",
            "targetoffline",
        ],
    ) {
        return DiagnosticKind::ConnectionFailed;
    }

    if contains_any_case_insensitive(
        error,
        &[
            "sendinput",
            "input injection",
            "injection failed",
            "unsupported windows key",
            "unsupported macos key",
            "failed to create keyboard event",
            "failed to create mouse",
            "failed to create scroll event",
        ],
    ) {
        return DiagnosticKind::InjectionFailed;
    }

    DiagnosticKind::Unknown
}

const fn diagnostic_for_kind(kind: DiagnosticKind) -> UserDiagnostic {
    match kind {
        DiagnosticKind::PermissionMissing => UserDiagnostic {
            kind,
            title: "Permission required",
            next_steps: PERMISSION_NEXT_STEPS,
        },
        DiagnosticKind::CoreServiceUnavailable => UserDiagnostic {
            kind,
            title: "Core service unavailable",
            next_steps: CORE_SERVICE_NEXT_STEPS,
        },
        DiagnosticKind::ConnectionFailed => UserDiagnostic {
            kind,
            title: "Connection failed",
            next_steps: CONNECTION_NEXT_STEPS,
        },
        DiagnosticKind::InjectionFailed => UserDiagnostic {
            kind,
            title: "Input injection failed",
            next_steps: INJECTION_NEXT_STEPS,
        },
        DiagnosticKind::Unknown => UserDiagnostic {
            kind,
            title: "Unexpected error",
            next_steps: UNKNOWN_NEXT_STEPS,
        },
    }
}

fn format_user_diagnostic(error: &DaemonError) -> String {
    let diagnostic = error.diagnostic();
    let mut output = format!("kmsync: {}\n", diagnostic.title);
    let _ = writeln!(output, "  details: {error}");
    output.push_str("  next steps:");
    for (index, step) in diagnostic.next_steps.iter().enumerate() {
        let _ = write!(output, "\n    {}. {step}", index + 1);
    }
    output
}

fn contains_any_case_insensitive(haystack: &str, needles: &[&str]) -> bool {
    needles
        .iter()
        .any(|needle| contains_case_insensitive(haystack, needle))
}

fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

fn run() -> Result<(), DaemonError> {
    run_with_args(env::args().skip(1))
}

fn run_with_args(args: impl Iterator<Item = String>) -> Result<(), DaemonError> {
    let args = Args::parse(args).map_err(DaemonError::from)?;

    let result = match args.command {
        Command::Desktop {
            config_path,
            output_path,
        } => run_desktop(&config_path, output_path.as_deref()),
        Command::Info => print_info(),
        Command::RequestPermissions => request_platform_permissions(),
        Command::SelfTest { profile } => run_self_test(profile),
        Command::Listen { bind } => run_listener(bind),
        Command::SendDemo { target, profile } => run_send_demo(target, profile),
        Command::TargetProbe {
            config_path,
            target_device_id,
        } => run_target_probe(&config_path, &target_device_id),
        Command::TargetInputTest {
            config_path,
            target_device_id,
        } => run_target_input_test(&config_path, &target_device_id),
        Command::CaptureSend {
            target,
            profile,
            mode,
            application_exceptions,
        } => run_capture_send(target, profile, mode, application_exceptions, 0),
        Command::CaptureConnect {
            config_path,
            target_device_id,
            profile,
            mode,
            application_exceptions,
        } => run_capture_connect(
            config_path,
            target_device_id,
            profile,
            mode,
            application_exceptions,
        ),
        Command::CoreService { config_path } => run_core_service(&config_path),
        Command::Heartbeat { config_path } => {
            let config = client::ClientConfig::load(&config_path)?;
            client::run_heartbeat_loop(config)
        }
        Command::ClipGet => run_clip_get(),
        Command::ClipSet { text } => run_clip_set(&text),
        Command::ClipSend { target } => run_clip_send(target),
        Command::ClipWatch {
            target,
            interval,
            policy,
        } => run_clip_watch(target, interval, policy),
        Command::FileSend {
            target,
            file_path,
            chunk_bytes,
        } => run_file_send(target, &file_path, chunk_bytes),
        Command::Devices { config_path } => {
            let config = client::ClientConfig::load(&config_path)?;
            client::print_devices(config)
        }
        Command::ConnectionDiagnostics {
            config_path,
            target_device_id,
        } => {
            let config = client::ClientConfig::load(&config_path)?;
            client::print_connection_diagnostics(config, &target_device_id)
        }
        Command::DesktopDiagnostics {
            config_path,
            probe_targets,
        } => run_desktop_diagnostics(&config_path, probe_targets),
        Command::Profiles { config_path } => {
            let config = client::ClientConfig::load(&config_path)?;
            client::print_profiles(config)
        }
        Command::ProfileSet {
            config_path,
            source_device_id,
            target_device_id,
            profile_path,
        } => {
            let config = client::ClientConfig::load(&config_path)?;
            client::upsert_profile_from_file(
                config,
                source_device_id,
                target_device_id,
                &profile_path,
            )
        }
        Command::UpdateCheck {
            config_path,
            device_id,
            platform,
            version,
            channel,
        } => {
            let config = client::ClientConfig::load(&config_path)?;
            client::print_update_check(config, device_id, platform, version, channel)
        }
        Command::WindowsService { config_path } => run_windows_service(&config_path),
        Command::LocalIpcEndpoint => print_local_ipc_endpoint(),
        Command::LocalIpcServeOnce { endpoint } => run_local_ipc_serve_once(&endpoint),
        Command::LocalIpcPing { endpoint } => run_local_ipc_ping(&endpoint),
        Command::Ui { args } => kmsync_ui::run_with_args(args.into_iter()),
        Command::Help => {
            print_help();
            Ok(())
        }
    };
    result.map_err(DaemonError::from)
}

fn run_desktop(config_path: &Path, output_path: Option<&Path>) -> Result<(), String> {
    ensure_daemon_config_file(config_path)?;
    match desktop_launch_mode(output_path) {
        DesktopLaunchMode::NativeWindow => {
            if let Err(error) = ensure_core_service_for_desktop(config_path) {
                eprintln!("desktop core-service autostart failed: {error}");
            }
            spawn_foreground_desktop_capture_for_native_window(config_path);
            return native_desktop::run_native_desktop(config_path);
        }
        DesktopLaunchMode::HtmlExport(output_path) => {
            let mut state = build_local_desktop_state(config_path)?;
            attach_current_desktop_render_core_service_health_status(&mut state);
            let html = kmsync_ui::desktop_panel::render_desktop_panel(&state)?;
            write_desktop_page(&output_path, &html)?;
            println!("desktop_page={}", output_path.display());
            return Ok(());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DesktopLaunchMode {
    NativeWindow,
    HtmlExport(PathBuf),
}

fn desktop_launch_mode(output_path: Option<&Path>) -> DesktopLaunchMode {
    let Some(output_path) = output_path else {
        return DesktopLaunchMode::NativeWindow;
    };
    DesktopLaunchMode::HtmlExport(output_path.to_path_buf())
}

fn spawn_foreground_desktop_capture_for_native_window(config_path: &Path) {
    if !desktop_should_spawn_foreground_capture(env::consts::OS) {
        return;
    }
    let config_path = config_path.to_path_buf();
    thread::spawn(move || loop {
        if let Err(error) = run_desktop_capture_supervisor(&config_path) {
            eprintln!("foreground desktop capture failed: {error}; retrying");
        }
        thread::sleep(Duration::from_secs(2));
    });
}

fn desktop_should_spawn_foreground_capture(os: &str) -> bool {
    os == "macos"
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CoreServiceAutostartCommand {
    program: PathBuf,
    args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CoreServiceAutostartAction {
    AlreadyRunning,
    Start(CoreServiceAutostartCommand),
    Restart {
        stop: CoreServiceAutostartCommand,
        start: CoreServiceAutostartCommand,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CoreServiceHealth {
    version: String,
    input_hot_path: String,
    config_path: Option<PathBuf>,
    device_id: Option<String>,
    launch_context: Option<String>,
}

fn ensure_core_service_for_desktop(config_path: &Path) -> Result<(), String> {
    let endpoint = local_ipc::default_local_ipc_endpoint();
    let health_result = read_core_service_health(&endpoint);
    let exe_path =
        env::current_exe().map_err(|error| format!("failed to locate kmsync: {error}"))?;
    match desktop_core_service_autostart_action(
        health_result,
        &exe_path,
        config_path,
        env::consts::OS,
    ) {
        CoreServiceAutostartAction::AlreadyRunning => Ok(()),
        CoreServiceAutostartAction::Start(command) => spawn_core_service_autostart(command),
        CoreServiceAutostartAction::Restart { stop, start } => {
            run_core_service_stop_command(stop)?;
            thread::sleep(Duration::from_millis(200));
            spawn_core_service_autostart(start)
        }
    }
}

fn desktop_core_service_autostart_action(
    core_service_health: Result<CoreServiceHealth, String>,
    exe_path: &Path,
    config_path: &Path,
    os: &str,
) -> CoreServiceAutostartAction {
    if core_service_health.as_ref().is_ok_and(|health| {
        core_service_health_is_compatible_for_exe(health, config_path, exe_path, os)
    }) {
        return CoreServiceAutostartAction::AlreadyRunning;
    }

    if os == "macos" {
        if let Some(app_bundle) = macos_app_bundle_path_from_executable(exe_path) {
            let start = macos_app_core_service_start_command(&app_bundle, config_path);
            if core_service_health.as_ref().is_ok() {
                return CoreServiceAutostartAction::Restart {
                    stop: macos_app_core_service_stop_command(&app_bundle),
                    start,
                };
            }
            return CoreServiceAutostartAction::Start(start);
        }
    }

    if os == "windows" && is_windows_installed_package_executable(exe_path) {
        let start = portable_core_service_start_command(exe_path, config_path);
        if core_service_health.as_ref().is_ok() {
            return CoreServiceAutostartAction::Restart {
                stop: windows_installed_core_service_stop_command(exe_path),
                start,
            };
        }
        return CoreServiceAutostartAction::Start(start);
    }

    let start = portable_core_service_start_command(exe_path, config_path);
    if os == "windows" && core_service_health.as_ref().is_ok() {
        return CoreServiceAutostartAction::Restart {
            stop: windows_installed_core_service_stop_command(exe_path),
            start,
        };
    }

    CoreServiceAutostartAction::Start(start)
}

fn portable_core_service_start_command(
    exe_path: &Path,
    config_path: &Path,
) -> CoreServiceAutostartCommand {
    CoreServiceAutostartCommand {
        program: exe_path.to_path_buf(),
        args: vec![
            "core-service".to_string(),
            config_path.display().to_string(),
        ],
    }
}

fn core_service_health_is_compatible(health: &CoreServiceHealth, config_path: &Path) -> bool {
    let current_exe = env::current_exe().ok();
    core_service_health_is_compatible_for_exe(
        health,
        config_path,
        current_exe.as_deref().unwrap_or_else(|| Path::new("")),
        env::consts::OS,
    )
}

fn core_service_health_is_compatible_for_exe(
    health: &CoreServiceHealth,
    config_path: &Path,
    exe_path: &Path,
    os: &str,
) -> bool {
    health.version == env!("CARGO_PKG_VERSION")
        && health.input_hot_path == "daemon_data_plane"
        && health
            .config_path
            .as_deref()
            .is_some_and(|health_config_path| same_config_path(health_config_path, config_path))
        && core_service_launch_context_is_compatible(health, exe_path, os)
}

fn core_service_launch_context_is_compatible(
    health: &CoreServiceHealth,
    exe_path: &Path,
    os: &str,
) -> bool {
    if os == "macos" && macos_app_bundle_path_from_executable(exe_path).is_some() {
        return health.launch_context.as_deref()
            == Some(MACOS_CORE_SERVICE_LAUNCH_SERVICES_CONTEXT);
    }
    true
}

fn same_config_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn macos_app_core_service_start_command(
    app_bundle: &Path,
    config_path: &Path,
) -> CoreServiceAutostartCommand {
    CoreServiceAutostartCommand {
        program: PathBuf::from("/usr/bin/open"),
        args: vec![
            "-n".to_string(),
            app_bundle.display().to_string(),
            "--args".to_string(),
            "core-service".to_string(),
            config_path.display().to_string(),
        ],
    }
}

fn macos_app_core_service_stop_command(app_bundle: &Path) -> CoreServiceAutostartCommand {
    let binary_pattern = pkill_regex_escape(
        &app_bundle
            .join("Contents")
            .join("MacOS")
            .join("kmsync")
            .display()
            .to_string(),
    );
    CoreServiceAutostartCommand {
        program: PathBuf::from("/usr/bin/pkill"),
        args: vec!["-f".to_string(), format!("{binary_pattern} .*core-service")],
    }
}

fn pkill_regex_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn windows_installed_core_service_stop_command(exe_path: &Path) -> CoreServiceAutostartCommand {
    let exe_path = powershell_single_quoted(&exe_path.display().to_string());
    let script = format!(
        "Get-CimInstance Win32_Process | Where-Object {{ $_.ExecutablePath -eq {exe_path} -and $_.CommandLine -match '(^|\\s)core-service(\\s|$)' }} | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force }}"
    );
    CoreServiceAutostartCommand {
        program: PathBuf::from("powershell.exe"),
        args: vec![
            "-NoProfile".to_string(),
            "-ExecutionPolicy".to_string(),
            "Bypass".to_string(),
            "-Command".to_string(),
            script,
        ],
    }
}

fn powershell_single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn read_core_service_health(
    endpoint: &local_ipc::LocalIpcEndpoint,
) -> Result<CoreServiceHealth, String> {
    let mut client =
        local_ipc::LocalIpcClient::connect(endpoint).map_err(|error| error.to_string())?;
    match client
        .request(&local_ipc::LocalIpcRequest::Status)
        .map_err(|error| error.to_string())?
    {
        local_ipc::LocalIpcResponse::Status {
            service,
            version,
            input_hot_path,
            launch_context,
            config_path,
            device_id,
            ..
        } if service == "kmsync" => Ok(CoreServiceHealth {
            version,
            input_hot_path,
            config_path: config_path.map(PathBuf::from),
            device_id,
            launch_context,
        }),
        local_ipc::LocalIpcResponse::Status { service, .. } => {
            Err(format!("unexpected local IPC service '{service}'"))
        }
        local_ipc::LocalIpcResponse::Error { code, message } => {
            Err(format!("local IPC error {code}: {message}"))
        }
        response => Err(format!("unexpected local IPC response: {response:?}")),
    }
}

fn spawn_core_service_autostart(command: CoreServiceAutostartCommand) -> Result<(), String> {
    std::process::Command::new(&command.program)
        .args(&command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            format!(
                "failed to start {} {}: {error}",
                command.program.display(),
                command.args.join(" ")
            )
        })
}

fn run_core_service_stop_command(command: CoreServiceAutostartCommand) -> Result<(), String> {
    std::process::Command::new(&command.program)
        .args(&command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|_| ())
        .map_err(|error| {
            format!(
                "failed to stop existing core-service with {} {}: {error}",
                command.program.display(),
                command.args.join(" ")
            )
        })
}

fn core_service_launch_context() -> Option<String> {
    if env::consts::OS != "macos" {
        return None;
    }
    let exe_path = env::current_exe().ok()?;
    macos_core_service_launch_context_from_args_executable_and_parent(
        env::args(),
        &exe_path,
        current_parent_process_id(),
    )
}

fn macos_core_service_launch_context_from_args_executable_and_parent<I, S>(
    args: I,
    exe_path: &Path,
    parent_pid: Option<u32>,
) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    macos_app_bundle_path_from_executable(exe_path)?;
    if args
        .into_iter()
        .any(|arg| arg.as_ref().starts_with("-psn_"))
        || parent_pid == Some(1)
    {
        Some(MACOS_CORE_SERVICE_LAUNCH_SERVICES_CONTEXT.to_string())
    } else {
        Some(MACOS_CORE_SERVICE_DIRECT_APP_BINARY_CONTEXT.to_string())
    }
}

#[allow(unsafe_code)]
fn current_parent_process_id() -> Option<u32> {
    #[cfg(unix)]
    {
        let parent = unsafe { libc::getppid() };
        return u32::try_from(parent).ok();
    }

    #[cfg(not(unix))]
    {
        None
    }
}

fn build_local_desktop_state(config_path: &Path) -> Result<kmsync_core::DesktopState, String> {
    ensure_daemon_config_file(config_path)?;
    let client_config = client::ClientConfig::load(config_path)?;
    let mut desktop_config = desktop_config::DesktopConfig::load(config_path).unwrap_or_default();
    let (server_state, mut server_error) = desktop_server_probe_result_to_state(
        client::probe_server_reachable(&client_config.server_url),
    );
    let local_lan_ips = client::discover_local_lan_ips()
        .into_iter()
        .map(|ip| ip.to_string())
        .collect::<Vec<_>>();
    let mut current_device_id = None;
    let mut devices = Vec::new();
    let mut master_error = None;
    if server_state == kmsync_core::DesktopConnectionState::Connected {
        match client::load_desktop_device_inventory(&client_config, &local_lan_ips) {
            Ok(inventory) => {
                current_device_id = Some(inventory.current_device_id);
                devices = inventory.devices;
                match load_and_cache_server_topology(
                    config_path,
                    &client_config,
                    current_device_id.as_deref(),
                    desktop_config.profile_path.clone(),
                ) {
                    Ok(config) => {
                        desktop_config = config;
                    }
                    Err(error) => {
                        master_error = Some(format!("拓扑刷新失败：{error}"));
                    }
                }
            }
            Err(error) => {
                master_error = Some(format!("设备列表刷新失败：{error}"));
            }
        }
    } else if server_error.is_none() {
        server_error = Some("服务器未连接，无法刷新设备列表".to_string());
    }
    let permissions = platform::current_platform().permission_checks();
    let mut state = desktop_state::build_desktop_state(desktop_state::DesktopStateBuildInput {
        config_path,
        device_name: &client_config.device_name,
        server_url: &client_config.server_url,
        listen_port: client_config.listen_port,
        current_device_id: current_device_id.as_deref(),
        local_lan_ips,
        desktop_config: &desktop_config,
        devices: &devices,
        permissions: &permissions,
        server_state,
        server_error,
        master_error,
    });
    attach_desktop_sync_runtime_status(config_path, &mut state)?;
    Ok(state)
}

fn attach_desktop_sync_runtime_status(
    config_path: &Path,
    state: &mut kmsync_core::DesktopState,
) -> Result<(), String> {
    let runtime = read_desktop_sync_runtime_status(config_path)?;
    let runtime = if stale_macos_background_service_runtime_failure(state, &runtime) {
        kmsync_core::DesktopSyncRuntimeState::default()
    } else {
        runtime
    };
    if runtime.state == kmsync_core::DesktopSyncRuntimeKind::Failed {
        let error = runtime
            .error
            .clone()
            .unwrap_or_else(|| "同步通道运行失败".to_string());
        if state.master_error.is_none() {
            state.master_error = Some(format!("同步通道失败：{error}"));
        }
    }
    state.sync_runtime = runtime;
    Ok(())
}

fn stale_macos_background_service_runtime_failure(
    state: &kmsync_core::DesktopState,
    runtime: &kmsync_core::DesktopSyncRuntimeState,
) -> bool {
    state.device.os == "macos"
        && state.device.role == kmsync_core::DesktopRole::Master
        && runtime.state == kmsync_core::DesktopSyncRuntimeKind::Failed
        && runtime
            .error
            .as_deref()
            .is_some_and(|error| contains_case_insensitive(error, "background service"))
}

fn run_desktop_diagnostics(config_path: &Path, probe_targets: bool) -> Result<(), String> {
    let mut state = build_local_desktop_state(config_path)?;
    attach_current_core_service_health_status(&mut state);
    let target_probes = if probe_targets {
        collect_desktop_target_probe_diagnostics(config_path, &state)
    } else {
        Vec::new()
    };
    let launch_agent = current_macos_launch_agent_diagnostic();
    let installed_app = current_macos_installed_app_diagnostic();
    print!(
        "{}",
        render_desktop_diagnostic_report_with_macos_context(
            &state,
            launch_agent.as_ref(),
            installed_app.as_ref(),
            &target_probes,
        )
    );
    Ok(())
}

fn collect_desktop_target_probe_diagnostics(
    config_path: &Path,
    state: &kmsync_core::DesktopState,
) -> Vec<DesktopTargetProbeDiagnostic> {
    desktop_layout_edge_targets(&state.layout)
        .into_iter()
        .map(|(edge, target_device_id)| {
            match send_target_probe_frame(config_path, target_device_id) {
                Ok(transport) => DesktopTargetProbeDiagnostic {
                    edge,
                    target_device_id: target_device_id.to_string(),
                    status: DesktopTargetProbeStatus::Ok,
                    transport: Some(transport),
                    error: None,
                },
                Err(error) => DesktopTargetProbeDiagnostic {
                    edge,
                    target_device_id: target_device_id.to_string(),
                    status: DesktopTargetProbeStatus::Failed,
                    transport: error.transport,
                    error: Some(error.message),
                },
            }
        })
        .collect()
}

#[cfg(test)]
fn render_desktop_diagnostic_report(state: &kmsync_core::DesktopState) -> String {
    render_desktop_diagnostic_report_with_macos_context(state, None, None, &[])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MacosLaunchAgentState {
    DirectAppBinary,
    LaunchServicesApp,
    LegacyOpen,
    Missing,
    ReadFailed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MacosLaunchAgentDiagnostic {
    path: PathBuf,
    state: MacosLaunchAgentState,
    program: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MacosInstalledAppState {
    Current,
    Outdated,
    Missing,
    ReadFailed,
    CurrentHashUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MacosInstalledAppDiagnostic {
    path: PathBuf,
    state: MacosInstalledAppState,
    installed_hash: Option<String>,
    current_hash: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DesktopTargetProbeStatus {
    Ok,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DesktopTargetProbeDiagnostic {
    edge: Edge,
    target_device_id: String,
    status: DesktopTargetProbeStatus,
    transport: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MacosInstalledAppHashError {
    Missing,
    ReadFailed(String),
}

fn current_macos_launch_agent_diagnostic() -> Option<MacosLaunchAgentDiagnostic> {
    if env::consts::OS != "macos" {
        return None;
    }
    Some(read_macos_launch_agent_diagnostic(Path::new(
        MACOS_LAUNCH_AGENT_PATH,
    )))
}

fn read_macos_launch_agent_diagnostic(path: &Path) -> MacosLaunchAgentDiagnostic {
    match fs::read_to_string(path) {
        Ok(plist) => macos_launch_agent_diagnostic_from_plist(path.to_path_buf(), &plist),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => MacosLaunchAgentDiagnostic {
            path: path.to_path_buf(),
            state: MacosLaunchAgentState::Missing,
            program: None,
            error: None,
        },
        Err(error) => MacosLaunchAgentDiagnostic {
            path: path.to_path_buf(),
            state: MacosLaunchAgentState::ReadFailed,
            program: None,
            error: Some(error.to_string()),
        },
    }
}

fn current_macos_installed_app_diagnostic() -> Option<MacosInstalledAppDiagnostic> {
    if env::consts::OS != "macos" {
        return None;
    }
    Some(macos_installed_app_diagnostic_from_hashes(
        PathBuf::from(MACOS_INSTALLED_APP_BINARY_PATH),
        diagnostic_file_hash(Path::new(MACOS_INSTALLED_APP_BINARY_PATH)),
        current_exe_hash_for_macos_app_diagnostic(),
    ))
}

fn macos_installed_app_diagnostic_from_hashes(
    path: PathBuf,
    installed_hash: Result<String, MacosInstalledAppHashError>,
    current_hash: Result<String, String>,
) -> MacosInstalledAppDiagnostic {
    let (current_hash, current_error) = match current_hash {
        Ok(hash) => (Some(hash), None),
        Err(error) => (None, Some(error)),
    };

    match installed_hash {
        Ok(installed_hash) => {
            let state = match current_hash.as_deref() {
                Some(current_hash) if current_hash == installed_hash => {
                    MacosInstalledAppState::Current
                }
                Some(_) => MacosInstalledAppState::Outdated,
                None => MacosInstalledAppState::CurrentHashUnavailable,
            };
            MacosInstalledAppDiagnostic {
                path,
                state,
                installed_hash: Some(installed_hash),
                current_hash,
                error: current_error,
            }
        }
        Err(MacosInstalledAppHashError::Missing) => MacosInstalledAppDiagnostic {
            path,
            state: MacosInstalledAppState::Missing,
            installed_hash: None,
            current_hash,
            error: current_error,
        },
        Err(MacosInstalledAppHashError::ReadFailed(error)) => MacosInstalledAppDiagnostic {
            path,
            state: MacosInstalledAppState::ReadFailed,
            installed_hash: None,
            current_hash,
            error: Some(error),
        },
    }
}

fn diagnostic_file_hash(path: &Path) -> Result<String, MacosInstalledAppHashError> {
    let bytes = fs::read(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            MacosInstalledAppHashError::Missing
        } else {
            MacosInstalledAppHashError::ReadFailed(error.to_string())
        }
    })?;
    Ok(format!("{:016x}", file_content_hash(&bytes)))
}

fn current_exe_hash_for_macos_app_diagnostic() -> Result<String, String> {
    let current_exe = env::current_exe()
        .map_err(|error| format!("failed to locate current executable: {error}"))?;
    diagnostic_file_hash(&current_exe).map_err(|error| match error {
        MacosInstalledAppHashError::Missing => {
            format!("current executable is missing: {}", current_exe.display())
        }
        MacosInstalledAppHashError::ReadFailed(error) => error,
    })
}

fn macos_launch_agent_diagnostic_from_plist(
    path: PathBuf,
    plist: &str,
) -> MacosLaunchAgentDiagnostic {
    let program_arguments = plist_program_argument_values(plist);
    let program = program_arguments.first().cloned();
    let state = match program.as_deref() {
        Some("/usr/bin/open")
            if macos_launch_agent_uses_launch_services_app(&program_arguments) =>
        {
            MacosLaunchAgentState::LaunchServicesApp
        }
        Some("/usr/bin/open") => MacosLaunchAgentState::LegacyOpen,
        Some(program)
            if program.ends_with("/KMSync.app/Contents/MacOS/kmsync")
                && program_arguments
                    .get(1)
                    .is_some_and(|arg| arg == "core-service") =>
        {
            MacosLaunchAgentState::DirectAppBinary
        }
        _ => MacosLaunchAgentState::Unknown,
    };

    MacosLaunchAgentDiagnostic {
        path,
        state,
        program,
        error: None,
    }
}

fn macos_launch_agent_uses_launch_services_app(program_arguments: &[String]) -> bool {
    let Some(args_index) = program_arguments.iter().position(|arg| arg == "--args") else {
        return false;
    };
    program_arguments
        .iter()
        .take(args_index)
        .any(|arg| arg.ends_with("/KMSync.app"))
        && program_arguments[args_index + 1..]
            .iter()
            .any(|arg| arg == "core-service")
}

fn plist_program_argument_values(plist: &str) -> Vec<String> {
    let Some(key_index) = plist.find("<key>ProgramArguments</key>") else {
        return Vec::new();
    };
    let after_key = &plist[key_index + "<key>ProgramArguments</key>".len()..];
    let Some(array_start_index) = after_key.find("<array>") else {
        return Vec::new();
    };
    let after_array_start = &after_key[array_start_index + "<array>".len()..];
    let Some(array_end_index) = after_array_start.find("</array>") else {
        return Vec::new();
    };
    plist_string_values(&after_array_start[..array_end_index])
}

fn plist_string_values(plist: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut rest = plist;
    while let Some(start_index) = rest.find("<string>") {
        let value_start = start_index + "<string>".len();
        let after_start = &rest[value_start..];
        let Some(end_index) = after_start.find("</string>") else {
            break;
        };
        values.push(xml_unescape(after_start[..end_index].trim()));
        rest = &after_start[end_index + "</string>".len()..];
    }
    values
}

fn xml_unescape(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[cfg(test)]
fn render_desktop_diagnostic_report_with_launch_agent(
    state: &kmsync_core::DesktopState,
    launch_agent: Option<&MacosLaunchAgentDiagnostic>,
) -> String {
    render_desktop_diagnostic_report_with_macos_context(state, launch_agent, None, &[])
}

#[cfg(test)]
fn render_desktop_diagnostic_report_with_target_probes(
    state: &kmsync_core::DesktopState,
    target_probes: &[DesktopTargetProbeDiagnostic],
) -> String {
    render_desktop_diagnostic_report_with_macos_context(state, None, None, target_probes)
}

fn render_desktop_diagnostic_report_with_macos_context(
    state: &kmsync_core::DesktopState,
    launch_agent: Option<&MacosLaunchAgentDiagnostic>,
    installed_app: Option<&MacosInstalledAppDiagnostic>,
    target_probes: &[DesktopTargetProbeDiagnostic],
) -> String {
    let mut output = String::from("desktop diagnostic report\n");
    let _ = writeln!(
        output,
        "config_path={}",
        state.config_path.as_deref().unwrap_or("-")
    );
    let _ = writeln!(
        output,
        "device_id={} name={} os={} role={}",
        state.device.id.as_deref().unwrap_or("-"),
        state.device.name,
        state.device.os,
        desktop_role_diagnostic_label(&state.device.role)
    );
    let _ = writeln!(
        output,
        "server_state={} server_url={} server_error={}",
        desktop_connection_diagnostic_label(&state.server_state),
        state.network.server_url.as_deref().unwrap_or("-"),
        state.server_error.as_deref().unwrap_or("-")
    );
    let _ = writeln!(
        output,
        "master_state={} master_device_id={} master_error={}",
        desktop_connection_diagnostic_label(&state.master_state),
        state.master_device_id.as_deref().unwrap_or("-"),
        state.master_error.as_deref().unwrap_or("-")
    );
    let _ = writeln!(
        output,
        "core_service={}",
        desktop_core_service_diagnostic_label(state)
    );
    let _ = writeln!(
        output,
        "layout_left={} layout_right={} layout_top={} layout_bottom={}",
        desktop_layout_target_diagnostic_label(state.layout.left.as_deref()),
        desktop_layout_target_diagnostic_label(state.layout.right.as_deref()),
        desktop_layout_target_diagnostic_label(state.layout.top.as_deref()),
        desktop_layout_target_diagnostic_label(state.layout.bottom.as_deref())
    );
    if let Some(launch_agent) = launch_agent {
        let _ = writeln!(output, "launch_agent_path={}", launch_agent.path.display());
        let _ = writeln!(
            output,
            "launch_agent_state={}",
            macos_launch_agent_state_label(launch_agent.state)
        );
        let _ = writeln!(
            output,
            "launch_agent_program={}",
            diagnostic_empty_dash(launch_agent.program.as_deref().unwrap_or(""))
        );
        if let Some(error) = launch_agent.error.as_deref() {
            let _ = writeln!(output, "launch_agent_error={error}");
        }
    }
    if let Some(installed_app) = installed_app {
        let _ = writeln!(
            output,
            "installed_app_path={}",
            installed_app.path.display()
        );
        let _ = writeln!(
            output,
            "installed_app_state={}",
            macos_installed_app_state_label(installed_app.state)
        );
        let _ = writeln!(
            output,
            "installed_app_hash={}",
            diagnostic_empty_dash(installed_app.installed_hash.as_deref().unwrap_or(""))
        );
        let _ = writeln!(
            output,
            "current_exe_hash={}",
            diagnostic_empty_dash(installed_app.current_hash.as_deref().unwrap_or(""))
        );
        if let Some(error) = installed_app.error.as_deref() {
            let _ = writeln!(output, "installed_app_error={error}");
        }
    }
    let _ = writeln!(
        output,
        "sync_runtime={} relay_connected={}",
        desktop_sync_runtime_diagnostic_label(&state.sync_runtime.state),
        state.sync_runtime.relay_connected
    );
    let _ = writeln!(
        output,
        "sync_targets={}",
        desktop_runtime_targets_diagnostic_label(&state.sync_runtime.targets)
    );
    let _ = writeln!(
        output,
        "captured_events={} routed_events={} sent_events={}",
        state.sync_runtime.captured_events,
        state.sync_runtime.routed_events,
        state.sync_runtime.sent_events
    );
    let _ = writeln!(
        output,
        "last_capture pointer={} routed={} edge={} target={}",
        desktop_capture_pointer_diagnostic_label(
            state.sync_runtime.last_capture_pointer_x,
            state.sync_runtime.last_capture_pointer_y
        ),
        state.sync_runtime.last_capture_routed,
        diagnostic_empty_dash(
            state
                .sync_runtime
                .last_capture_edge
                .as_deref()
                .unwrap_or("")
        ),
        diagnostic_empty_dash(
            state
                .sync_runtime
                .last_capture_target
                .as_deref()
                .unwrap_or("")
        )
    );
    let _ = writeln!(
        output,
        "received_events={} injected_events={}",
        state.sync_runtime.received_events, state.sync_runtime.injected_events
    );
    if let Some(error) = state.sync_runtime.error.as_deref() {
        let _ = writeln!(output, "sync_runtime_error={error}");
    }
    for permission in &state.permissions {
        let _ = writeln!(
            output,
            "permission_check={} status={} label=\"{}\"",
            permission.key,
            permission.status,
            diagnostic_quote(&permission.label)
        );
    }

    for (edge, target_id) in desktop_layout_edge_targets(&state.layout) {
        if let Some(device) = state.devices.iter().find(|device| device.id == target_id) {
            let _ = writeln!(
                output,
                "target edge={} id={} name={} online={} relay_rx_online={} lan_ips={}",
                edge.as_str(),
                device.id,
                device.name,
                device.online,
                desktop_peer_relay_diagnostic_label(device),
                diagnostic_empty_dash(&device.lan_ips.join(","))
            );
        } else {
            let _ = writeln!(
                output,
                "target edge={} id={} name=- online=unknown relay_rx_online=unknown lan_ips=-",
                edge.as_str(),
                target_id
            );
        }
    }

    for probe in target_probes {
        let _ = writeln!(
            output,
            "target_probe edge={} id={} status={} transport={} error=\"{}\"",
            probe.edge.as_str(),
            probe.target_device_id,
            desktop_target_probe_status_label(probe.status),
            diagnostic_empty_dash(probe.transport.as_deref().unwrap_or("")),
            diagnostic_quote(probe.error.as_deref().unwrap_or("-"))
        );
    }

    for warning in desktop_diagnostic_warnings(state) {
        let _ = writeln!(output, "warning={warning}");
    }
    if let Some(launch_agent) = launch_agent {
        if let Some(warning) = macos_launch_agent_warning(launch_agent.state) {
            let _ = writeln!(output, "warning={warning}");
        }
    }
    if let Some(installed_app) = installed_app {
        if let Some(warning) = macos_installed_app_warning(installed_app.state) {
            let _ = writeln!(output, "warning={warning}");
        }
    }
    output.push_str("privacy=connection_metadata_only\n");
    output
}

fn macos_launch_agent_state_label(state: MacosLaunchAgentState) -> &'static str {
    match state {
        MacosLaunchAgentState::DirectAppBinary => "direct_app_binary",
        MacosLaunchAgentState::LaunchServicesApp => "launch_services_app",
        MacosLaunchAgentState::LegacyOpen => "legacy_open",
        MacosLaunchAgentState::Missing => "missing",
        MacosLaunchAgentState::ReadFailed => "read_failed",
        MacosLaunchAgentState::Unknown => "unknown",
    }
}

fn macos_launch_agent_warning(state: MacosLaunchAgentState) -> Option<&'static str> {
    match state {
        MacosLaunchAgentState::DirectAppBinary => Some("direct_macos_launch_agent"),
        MacosLaunchAgentState::LegacyOpen => Some("legacy_macos_launch_agent"),
        MacosLaunchAgentState::Missing => Some("macos_launch_agent_missing"),
        MacosLaunchAgentState::ReadFailed => Some("macos_launch_agent_unreadable"),
        MacosLaunchAgentState::Unknown => Some("macos_launch_agent_unknown"),
        MacosLaunchAgentState::LaunchServicesApp => None,
    }
}

fn macos_installed_app_state_label(state: MacosInstalledAppState) -> &'static str {
    match state {
        MacosInstalledAppState::Current => "current",
        MacosInstalledAppState::Outdated => "outdated",
        MacosInstalledAppState::Missing => "missing",
        MacosInstalledAppState::ReadFailed => "read_failed",
        MacosInstalledAppState::CurrentHashUnavailable => "current_hash_unavailable",
    }
}

fn macos_installed_app_warning(state: MacosInstalledAppState) -> Option<&'static str> {
    match state {
        MacosInstalledAppState::Outdated => Some("installed_macos_app_outdated"),
        MacosInstalledAppState::Missing => Some("installed_macos_app_missing"),
        MacosInstalledAppState::ReadFailed => Some("installed_macos_app_unreadable"),
        MacosInstalledAppState::CurrentHashUnavailable => Some("current_exe_hash_unavailable"),
        MacosInstalledAppState::Current => None,
    }
}

fn desktop_target_probe_status_label(status: DesktopTargetProbeStatus) -> &'static str {
    match status {
        DesktopTargetProbeStatus::Ok => "ok",
        DesktopTargetProbeStatus::Failed => "failed",
    }
}

fn desktop_diagnostic_warnings(state: &kmsync_core::DesktopState) -> Vec<&'static str> {
    let mut warnings = Vec::new();
    if desktop_core_service_diagnostic_label(state) == "unavailable" {
        warnings.push("core_service_unavailable");
    } else if desktop_core_service_diagnostic_label(state) == "incompatible" {
        warnings.push("core_service_incompatible");
    }
    if state.sync_runtime.state == kmsync_core::DesktopSyncRuntimeKind::Failed {
        warnings.push("sync_runtime_failed");
    }
    if state.device.role == kmsync_core::DesktopRole::Master
        && state
            .layout
            .target_device_ids()
            .into_iter()
            .any(|target_id| {
                state
                    .devices
                    .iter()
                    .find(|device| device.id == target_id)
                    .is_some_and(|device| device.online && !device.sync_relay_status_known)
            })
    {
        warnings.push("server_relay_status_unavailable");
    }
    if state.device.role == kmsync_core::DesktopRole::Client
        && state.master_state == kmsync_core::DesktopConnectionState::Disconnected
        && state.sync_runtime.relay_connected
    {
        warnings.push("waiting_for_master");
    }
    if desktop_targets_not_on_local_subnet(state) {
        warnings.push("target_lan_not_on_local_subnet");
    }
    if desktop_background_permission_scope_mismatch(state) {
        warnings.push("background_permission_scope_mismatch");
    }
    warnings
}

fn desktop_targets_not_on_local_subnet(state: &kmsync_core::DesktopState) -> bool {
    if state.device.role != kmsync_core::DesktopRole::Master || state.network.lan_ips.is_empty() {
        return false;
    }
    state
        .layout
        .target_device_ids()
        .into_iter()
        .filter_map(|target_id| state.devices.iter().find(|device| device.id == target_id))
        .filter(|device| device.online && !device.lan_ips.is_empty())
        .any(|device| {
            device.lan_ips.iter().all(|target_ip| {
                state
                    .network
                    .lan_ips
                    .iter()
                    .all(|local_ip| !desktop_same_lan_subnet(local_ip, target_ip))
            })
        })
}

fn desktop_same_lan_subnet(left: &str, right: &str) -> bool {
    let Ok(left) = left.parse::<std::net::IpAddr>() else {
        return false;
    };
    let Ok(right) = right.parse::<std::net::IpAddr>() else {
        return false;
    };
    match (left, right) {
        (std::net::IpAddr::V4(left), std::net::IpAddr::V4(right)) => {
            let left = left.octets();
            let right = right.octets();
            left[..3] == right[..3]
        }
        (std::net::IpAddr::V6(left), std::net::IpAddr::V6(right)) => {
            let left = left.segments();
            let right = right.segments();
            left[..4] == right[..4]
        }
        _ => false,
    }
}

fn desktop_background_permission_scope_mismatch(state: &kmsync_core::DesktopState) -> bool {
    let Some(error) = state.sync_runtime.error.as_deref() else {
        return false;
    };
    contains_case_insensitive(error, "Input Monitoring")
        && state.permissions.iter().any(|permission| {
            permission.status == "granted"
                && contains_case_insensitive(&permission.key, "input_monitoring")
        })
}

fn diagnostic_quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn desktop_core_service_diagnostic_label(state: &kmsync_core::DesktopState) -> &'static str {
    match state.master_error.as_deref() {
        Some(error) if error.contains("后台服务未运行") => "unavailable",
        Some(error) if error.contains("后台服务") => "incompatible",
        _ => "ok",
    }
}

fn diagnostic_empty_dash(value: &str) -> String {
    if value.trim().is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    }
}

fn desktop_layout_target_diagnostic_label(target: Option<&str>) -> String {
    diagnostic_empty_dash(target.unwrap_or(""))
}

fn desktop_runtime_targets_diagnostic_label(targets: &[String]) -> String {
    diagnostic_empty_dash(&targets.join(","))
}

fn desktop_capture_pointer_diagnostic_label(x: Option<i64>, y: Option<i64>) -> String {
    match (x, y) {
        (Some(x), Some(y)) => format!("{x},{y}"),
        _ => "-".to_string(),
    }
}

fn desktop_peer_relay_diagnostic_label(device: &kmsync_core::DesktopPeerState) -> &'static str {
    if !device.sync_relay_status_known {
        "unknown"
    } else if device.sync_relay_online {
        "true"
    } else {
        "false"
    }
}

fn desktop_role_diagnostic_label(role: &kmsync_core::DesktopRole) -> &'static str {
    match role {
        kmsync_core::DesktopRole::Master => "master",
        kmsync_core::DesktopRole::Client => "client",
    }
}

fn desktop_connection_diagnostic_label(
    state: &kmsync_core::DesktopConnectionState,
) -> &'static str {
    match state {
        kmsync_core::DesktopConnectionState::Connecting => "connecting",
        kmsync_core::DesktopConnectionState::Connected => "connected",
        kmsync_core::DesktopConnectionState::Disconnected => "disconnected",
        kmsync_core::DesktopConnectionState::Retrying => "retrying",
        kmsync_core::DesktopConnectionState::SelfDevice => "self_device",
    }
}

fn desktop_sync_runtime_diagnostic_label(
    state: &kmsync_core::DesktopSyncRuntimeKind,
) -> &'static str {
    match state {
        kmsync_core::DesktopSyncRuntimeKind::Unknown => "unknown",
        kmsync_core::DesktopSyncRuntimeKind::Idle => "idle",
        kmsync_core::DesktopSyncRuntimeKind::Listening => "listening",
        kmsync_core::DesktopSyncRuntimeKind::Armed => "armed",
        kmsync_core::DesktopSyncRuntimeKind::Failed => "failed",
    }
}

pub(crate) fn attach_current_core_service_health_status(state: &mut kmsync_core::DesktopState) {
    let endpoint = local_ipc::default_local_ipc_endpoint();
    attach_core_service_health_status(state, read_core_service_health(&endpoint));
}

fn attach_current_desktop_render_core_service_health_status(state: &mut kmsync_core::DesktopState) {
    let endpoint = local_ipc::default_local_ipc_endpoint();
    attach_desktop_render_core_service_health_status(state, read_core_service_health(&endpoint));
}

fn attach_desktop_render_core_service_health_status(
    state: &mut kmsync_core::DesktopState,
    health: Result<CoreServiceHealth, String>,
) {
    attach_core_service_health_status(state, health);
}

fn attach_core_service_health_status(
    state: &mut kmsync_core::DesktopState,
    health: Result<CoreServiceHealth, String>,
) {
    let health = match health {
        Ok(health) => health,
        Err(error) => {
            let message = format!(
                "后台服务未运行，可能仍在启动或启动失败，同步通道无法工作，请重启 KMSync。details={error}"
            );
            state.master_error = Some(message.clone());
            state.sync_runtime = desktop_core_service_failed_runtime_status(&message);
            return;
        }
    };
    let config_path = state.config_path.as_deref().map(Path::new);
    if config_path
        .is_some_and(|config_path| core_service_health_is_compatible(&health, config_path))
    {
        return;
    }
    let message = core_service_health_issue_message(&health);
    state.master_error = Some(message.clone());
    state.sync_runtime = desktop_core_service_failed_runtime_status(&message);
}

fn core_service_health_issue_message(health: &CoreServiceHealth) -> String {
    let current_exe = env::current_exe().ok();
    core_service_health_issue_message_for_exe(
        health,
        current_exe.as_deref().unwrap_or_else(|| Path::new("")),
        env::consts::OS,
    )
}

fn core_service_health_issue_message_for_exe(
    health: &CoreServiceHealth,
    exe_path: &Path,
    os: &str,
) -> String {
    let issue = if health.version != env!("CARGO_PKG_VERSION") {
        "后台服务版本不兼容"
    } else if health.input_hot_path != "daemon_data_plane" {
        "检测到旧后台服务或非同步热路径"
    } else if health.config_path.is_none() {
        "后台服务未上报配置路径"
    } else if !core_service_launch_context_is_compatible(health, exe_path, os) {
        "后台服务启动方式不兼容"
    } else {
        "后台服务配置不一致"
    };
    let config_path = health
        .config_path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "-".to_string());
    let device_id = health.device_id.as_deref().unwrap_or("-");
    let launch_context = health.launch_context.as_deref().unwrap_or("-");
    format!(
        "{issue}，同步通道无法工作，请重新安装或重启 KMSync。version={} input_hot_path={} config_path={} device_id={} launch_context={}",
        health.version, health.input_hot_path, config_path, device_id, launch_context
    )
}

fn desktop_core_service_failed_runtime_status(error: &str) -> kmsync_core::DesktopSyncRuntimeState {
    kmsync_core::DesktopSyncRuntimeState {
        state: kmsync_core::DesktopSyncRuntimeKind::Failed,
        error: Some(error.to_string()),
        ..kmsync_core::DesktopSyncRuntimeState::default()
    }
}

fn load_and_cache_server_topology(
    config_path: &Path,
    client_config: &client::ClientConfig,
    current_device_id: Option<&str>,
    profile_path: Option<PathBuf>,
) -> Result<desktop_config::DesktopConfig, String> {
    let topology = client::ControlClient::new(client_config.server_url.clone()).get_topology()?;
    let role =
        desktop_config::role_for_topology(current_device_id, topology.master_device_id.as_deref());
    desktop_config::set_topology_in_config_file(
        config_path,
        role.clone(),
        topology.master_device_id.as_deref(),
        &topology.layout,
    )?;
    Ok(desktop_config::DesktopConfig {
        role,
        master_device_id: topology.master_device_id,
        layout: topology.layout,
        profile_path,
    })
}

fn desktop_server_probe_result_to_state(
    result: Result<(), String>,
) -> (kmsync_core::DesktopConnectionState, Option<String>) {
    match result {
        Ok(()) => (kmsync_core::DesktopConnectionState::Connected, None),
        Err(error) => (
            kmsync_core::DesktopConnectionState::Disconnected,
            Some(error),
        ),
    }
}

fn ensure_daemon_config_file(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(path, DEFAULT_DAEMON_CONFIG_TEMPLATE).map_err(|error| {
        format!(
            "failed to create default config {}: {error}",
            path.display()
        )
    })
}

fn write_desktop_page(path: &Path, html: &str) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(path, html).map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn print_info() -> Result<(), String> {
    let adapter = platform::current_platform();
    println!("KMSync");
    println!("  os: {:?}", adapter.os_kind());
    println!("  input capture: {}", adapter.capabilities().input_capture);
    println!(
        "  input injection: {}",
        adapter.capabilities().input_injection
    );
    println!(
        "  clipboard text: {}",
        adapter.capabilities().clipboard_text
    );
    for check in adapter.permission_checks() {
        println!(
            "  permission check: {} status={} label=\"{}\" guidance=\"{}\"",
            check.id,
            check.status.as_str(),
            check.label,
            check.guidance
        );
    }

    for hint in adapter.permission_hints() {
        println!("  permission: {hint}");
    }

    Ok(())
}

fn request_platform_permissions() -> Result<(), String> {
    platform::request_platform_permissions();
    println!("requested platform permissions");
    print_info()
}

fn print_local_ipc_endpoint() -> Result<(), String> {
    let endpoint = local_ipc::default_local_ipc_endpoint();
    println!("local_ipc_transport={}", endpoint.transport.as_str());
    println!("local_ipc_address={}", endpoint.address);
    Ok(())
}

fn run_local_ipc_serve_once(endpoint: &local_ipc::LocalIpcEndpoint) -> Result<(), String> {
    println!(
        "local_ipc_listening transport={} address={}",
        endpoint.transport.as_str(),
        endpoint.address
    );
    local_ipc::LocalIpcServer::bind(endpoint)
        .map_err(|error| error.to_string())?
        .serve_one(handle_local_ipc_request)
        .map_err(|error| error.to_string())
}

fn run_local_ipc_ping(endpoint: &local_ipc::LocalIpcEndpoint) -> Result<(), String> {
    ping_local_ipc_endpoint(endpoint)?;
    println!(
        "local_ipc=ok transport={} address={}",
        endpoint.transport.as_str(),
        endpoint.address
    );
    Ok(())
}

fn ping_local_ipc_endpoint(endpoint: &local_ipc::LocalIpcEndpoint) -> Result<(), String> {
    let nonce = u64::try_from(unix_timestamp_millis()).unwrap_or(u64::MAX);
    let mut client =
        local_ipc::LocalIpcClient::connect(endpoint).map_err(|error| error.to_string())?;
    match client
        .request(&local_ipc::LocalIpcRequest::Ping { nonce })
        .map_err(|error| error.to_string())?
    {
        local_ipc::LocalIpcResponse::Pong {
            nonce: response_nonce,
        } if response_nonce == nonce => Ok(()),
        local_ipc::LocalIpcResponse::Error { code, message } => {
            Err(format!("local IPC error {code}: {message}"))
        }
        response => Err(format!("unexpected local IPC response: {response:?}")),
    }
}

fn handle_local_ipc_request(request: local_ipc::LocalIpcRequest) -> local_ipc::LocalIpcResponse {
    handle_local_ipc_request_with_config_path(request, None)
}

fn handle_local_ipc_request_with_config_path(
    request: local_ipc::LocalIpcRequest,
    config_path: Option<&Path>,
) -> local_ipc::LocalIpcResponse {
    match request {
        local_ipc::LocalIpcRequest::Ping { nonce } => local_ipc::LocalIpcResponse::Pong { nonce },
        local_ipc::LocalIpcRequest::Status => {
            let endpoint = local_ipc::default_local_ipc_endpoint();
            let input_hot_path = if config_path.is_some() {
                "daemon_data_plane"
            } else {
                "not_on_local_ipc"
            };
            local_ipc::LocalIpcResponse::Status {
                service: "kmsync".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                input_hot_path: input_hot_path.to_string(),
                platform_transport: endpoint.transport.as_str().to_string(),
                launch_context: core_service_launch_context(),
                config_path: config_path.map(|path| path.display().to_string()),
                device_id: config_path.and_then(core_service_status_device_id),
            }
        }
        local_ipc::LocalIpcRequest::GetDesktopState => desktop_state_response(config_path, false),
        local_ipc::LocalIpcRequest::SetDeviceRole {
            role,
            master_device_id,
        } => {
            let Some(config_path) = config_path else {
                return desktop_state_unavailable();
            };
            if let Err(error) = desktop_config::set_role_in_config_file(
                config_path,
                role,
                master_device_id.as_deref(),
            ) {
                return local_ipc::LocalIpcResponse::Error {
                    code: "desktop_config_write_failed".to_string(),
                    message: error,
                };
            }
            desktop_state_response(Some(config_path), true)
        }
        local_ipc::LocalIpcRequest::SetLayout { layout } => {
            let Some(config_path) = config_path else {
                return desktop_state_unavailable();
            };
            if let Err(error) = layout.validate(None) {
                return local_ipc::LocalIpcResponse::Error {
                    code: "invalid_desktop_layout".to_string(),
                    message: format!("{error:?}"),
                };
            }
            if let Err(error) = desktop_config::set_layout_in_config_file(config_path, &layout) {
                return local_ipc::LocalIpcResponse::Error {
                    code: "desktop_config_write_failed".to_string(),
                    message: error,
                };
            }
            desktop_state_response(Some(config_path), true)
        }
        local_ipc::LocalIpcRequest::SetServerEndpoint { host, port } => {
            let Some(config_path) = config_path else {
                return desktop_state_unavailable();
            };
            if let Err(error) =
                desktop_config::set_server_endpoint_in_config_file(config_path, &host, port)
            {
                return local_ipc::LocalIpcResponse::Error {
                    code: "desktop_config_write_failed".to_string(),
                    message: error,
                };
            }
            desktop_state_response(Some(config_path), true)
        }
    }
}

fn core_service_status_device_id(config_path: &Path) -> Option<String> {
    let config = client::ClientConfig::load(config_path).ok()?;
    let text = fs::read_to_string(config.identity_path).ok()?;
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get("device_id")?
        .as_str()
        .map(str::to_string)
}

fn desktop_state_unavailable() -> local_ipc::LocalIpcResponse {
    local_ipc::LocalIpcResponse::Error {
        code: "desktop_state_unavailable".to_string(),
        message: "desktop state is only available from the configured core service".to_string(),
    }
}

fn desktop_state_response(
    config_path: Option<&Path>,
    applied: bool,
) -> local_ipc::LocalIpcResponse {
    let Some(config_path) = config_path else {
        return desktop_state_unavailable();
    };
    match build_local_desktop_state(config_path) {
        Ok(state) if applied => local_ipc::LocalIpcResponse::ConfigApplied { state },
        Ok(state) => local_ipc::LocalIpcResponse::DesktopState { state },
        Err(error) => local_ipc::LocalIpcResponse::Error {
            code: "desktop_state_failed".to_string(),
            message: error,
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CoreServicePlan {
    config_path: PathBuf,
    bind: SocketAddr,
    ipc_endpoint: local_ipc::LocalIpcEndpoint,
    input_hot_path: &'static str,
    control_plane: &'static str,
}

impl CoreServicePlan {
    fn from_config(config_path: PathBuf, config: &client::ClientConfig) -> Self {
        Self {
            config_path,
            bind: SocketAddr::from(([0, 0, 0, 0], config.listen_port)),
            ipc_endpoint: local_ipc::default_local_ipc_endpoint(),
            input_hot_path: "daemon_data_plane",
            control_plane: "local_ipc_and_heartbeat",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct DesktopCapturePlan {
    targets: Vec<DesktopCaptureTarget>,
}

#[derive(Default)]
struct DesktopCaptureRuntimeCounters {
    captured_events: AtomicU64,
    routed_events: AtomicU64,
    sent_events: AtomicU64,
    transmit_failed: AtomicBool,
    last_capture: Mutex<Option<DesktopCaptureObservation>>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
struct DesktopCaptureRuntimeSnapshot {
    captured_events: u64,
    routed_events: u64,
    sent_events: u64,
    last_capture: Option<DesktopCaptureObservation>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct DesktopCaptureObservation {
    pointer_x: Option<i64>,
    pointer_y: Option<i64>,
    edge: Option<String>,
    target_device_id: Option<String>,
    routed: bool,
}

impl DesktopCaptureRuntimeCounters {
    fn record_captured(&self) -> u64 {
        self.captured_events.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn record_routed(&self) -> u64 {
        self.routed_events.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn record_capture_observation(
        &self,
        pointer: Option<PointerPosition>,
        edge: Option<Edge>,
        target_device_id: Option<&str>,
        routed: bool,
    ) {
        let observation = DesktopCaptureObservation {
            pointer_x: pointer.map(|pointer| rounded_pointer_coordinate(pointer.x)),
            pointer_y: pointer.map(|pointer| rounded_pointer_coordinate(pointer.y)),
            edge: edge.map(|edge| edge.as_str().to_string()),
            target_device_id: target_device_id.map(str::to_string),
            routed,
        };
        match self.last_capture.lock() {
            Ok(mut last_capture) => *last_capture = Some(observation),
            Err(error) => eprintln!("desktop capture observation lock poisoned: {error}"),
        }
    }

    fn record_sent(&self) -> u64 {
        self.transmit_failed.store(false, Ordering::Relaxed);
        self.sent_events.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn record_transmit_failed(&self) {
        self.transmit_failed.store(true, Ordering::Relaxed);
    }

    fn transmit_failed(&self) -> bool {
        self.transmit_failed.load(Ordering::Relaxed)
    }

    fn snapshot(&self) -> DesktopCaptureRuntimeSnapshot {
        DesktopCaptureRuntimeSnapshot {
            captured_events: self.captured_events.load(Ordering::Relaxed),
            routed_events: self.routed_events.load(Ordering::Relaxed),
            sent_events: self.sent_events.load(Ordering::Relaxed),
            last_capture: self
                .last_capture
                .lock()
                .ok()
                .and_then(|last_capture| last_capture.clone()),
        }
    }
}

fn rounded_pointer_coordinate(value: f64) -> i64 {
    if !value.is_finite() {
        return 0;
    }
    value.round().clamp(i64::MIN as f64, i64::MAX as f64) as i64
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DesktopCaptureTarget {
    edge: Edge,
    target_device_id: String,
    profile_name: ProfileName,
    display: Option<kmsync_core::DesktopDisplayState>,
}

struct DesktopCaptureRouteResult<'a> {
    edge: Option<Edge>,
    target_device_id: Option<&'a str>,
    profile_name: Option<ProfileName>,
    route: RouteResult,
    remote_event: Option<InputEvent>,
}

struct DesktopCaptureRouter {
    plan: DesktopCapturePlan,
    display_layout: DisplayLayout,
    active: Option<DesktopCaptureTarget>,
    remote_pointer: Option<PointerEntryPosition>,
    cooldown_until: Option<Instant>,
    local_restore_position: Option<PointerPosition>,
}

enum DesktopRemotePointerAction {
    Event(InputEvent),
    Release,
    None,
}

impl DesktopCaptureRouter {
    fn with_display_layout(plan: DesktopCapturePlan, display_layout: DisplayLayout) -> Self {
        Self {
            plan,
            display_layout,
            active: None,
            remote_pointer: None,
            cooldown_until: None,
            local_restore_position: None,
        }
    }

    fn replace_plan(&mut self, plan: DesktopCapturePlan) {
        if self.plan == plan {
            return;
        }
        self.plan = plan;
        self.active = None;
        self.remote_pointer = None;
        self.cooldown_until = None;
        self.local_restore_position = None;
    }

    #[cfg(test)]
    fn route(&mut self, captured: CapturedInput) -> DesktopCaptureRouteResult<'_> {
        self.route_at(captured, Instant::now())
    }

    #[cfg(test)]
    fn route_at(&mut self, captured: CapturedInput, now: Instant) -> DesktopCaptureRouteResult<'_> {
        self.route_at_with_application(captured, None, now)
    }

    fn route_at_with_application(
        &mut self,
        captured: CapturedInput,
        application_id: Option<&str>,
        now: Instant,
    ) -> DesktopCaptureRouteResult<'_> {
        if is_system_reserved_shortcut(captured.event)
            || ApplicationExceptionRules::default().matches(application_id)
        {
            return DesktopCaptureRouteResult::local(CaptureDecision::Continue);
        }

        if self.is_release_hotkey(captured, Hotkey::default_release()) {
            let local_pointer_action =
                self.active
                    .is_some()
                    .then_some(LocalPointerAction::Restore {
                        position: self.local_restore_position,
                    });
            self.active = None;
            self.remote_pointer = None;
            self.local_restore_position = None;
            self.cooldown_until = cooldown_deadline(now, default_edge_cooldown());
            return DesktopCaptureRouteResult::local(CaptureDecision::Continue)
                .with_pointer_action(local_pointer_action);
        }

        let mut entry_position = None;
        let mut local_pointer_action = None;
        if self.active.is_none() && !self.cooldown_active(now) {
            if let Some(target) = self
                .plan
                .targets
                .iter()
                .find(|target| self.should_activate_at_edge(captured, target.edge))
                .cloned()
            {
                self.local_restore_position = captured.pointer;
                entry_position = self.entry_position(captured.pointer, target.edge);
                self.remote_pointer = entry_position;
                local_pointer_action = Some(LocalPointerAction::Hide);
                self.active = Some(target);
            }
        }

        if self.active.is_some() {
            let active_edge = self.active.as_ref().expect("active target").edge;
            let active_display = self
                .active
                .as_ref()
                .and_then(|active| active.display.clone());
            let remote_event = if entry_position.is_some() {
                match captured.event {
                    InputEvent::Mouse(kmsync_core::MouseEvent::Move { .. }) => {
                        match self.remote_event_for_active_capture(
                            captured.event,
                            active_edge,
                            active_display.as_ref(),
                        ) {
                            DesktopRemotePointerAction::Event(event) => Some(event),
                            DesktopRemotePointerAction::Release
                            | DesktopRemotePointerAction::None => None,
                        }
                    }
                    _ => None,
                }
            } else {
                match self.remote_event_for_active_capture(
                    captured.event,
                    active_edge,
                    active_display.as_ref(),
                ) {
                    DesktopRemotePointerAction::Event(event) => Some(event),
                    DesktopRemotePointerAction::Release => {
                        let local_pointer_action = Some(LocalPointerAction::Restore {
                            position: self.local_restore_position,
                        });
                        self.active = None;
                        self.remote_pointer = None;
                        self.local_restore_position = None;
                        self.cooldown_until = cooldown_deadline(now, default_edge_cooldown());
                        return DesktopCaptureRouteResult::local(CaptureDecision::Continue)
                            .with_pointer_action(local_pointer_action);
                    }
                    DesktopRemotePointerAction::None => None,
                }
            };
            let entry_position = entry_position.or_else(|| {
                desktop_remote_event_needs_pointer_sync(remote_event)
                    .then_some(self.remote_pointer)?
            });
            let active = self.active.as_ref().expect("active target");
            DesktopCaptureRouteResult::remote(
                active,
                CaptureDecision::Suppress,
                entry_position,
                local_pointer_action,
                remote_event,
            )
        } else {
            DesktopCaptureRouteResult::local(CaptureDecision::Continue)
        }
    }

    fn should_activate_at_edge(&self, captured: CapturedInput, edge: Edge) -> bool {
        self.at_edge(captured.pointer, edge) && edge_activation_motion_matches(captured.event, edge)
    }

    fn remote_event_for_active_capture(
        &mut self,
        event: InputEvent,
        active_edge: Edge,
        active_display: Option<&kmsync_core::DesktopDisplayState>,
    ) -> DesktopRemotePointerAction {
        let InputEvent::Mouse(kmsync_core::MouseEvent::Move { dx, dy }) = event else {
            return DesktopRemotePointerAction::Event(event);
        };
        self.remote_pointer_action_for_move(dx, dy, active_edge, active_display)
    }

    fn remote_pointer_action_for_move(
        &mut self,
        dx: f32,
        dy: f32,
        active_edge: Edge,
        active_display: Option<&kmsync_core::DesktopDisplayState>,
    ) -> DesktopRemotePointerAction {
        if dx == 0.0 && dy == 0.0 {
            return DesktopRemotePointerAction::None;
        }
        let Some((width, height)) = self.remote_pointer_reference_size(active_display) else {
            return DesktopRemotePointerAction::None;
        };
        let mut position = self.remote_pointer.unwrap_or(PointerEntryPosition {
            x_ratio: 0.5,
            y_ratio: 0.5,
        });
        let next_x = position.x_ratio + (f64::from(dx) / width) as f32;
        let next_y = position.y_ratio + (f64::from(dy) / height) as f32;
        if remote_pointer_crossed_return_edge(active_edge, next_x, next_y, dx, dy) {
            return DesktopRemotePointerAction::Release;
        }
        position.x_ratio = next_x.clamp(0.0, 1.0);
        position.y_ratio = next_y.clamp(0.0, 1.0);
        self.remote_pointer = Some(position);
        DesktopRemotePointerAction::Event(InputEvent::Mouse(kmsync_core::MouseEvent::Move {
            dx,
            dy,
        }))
    }

    fn remote_pointer_reference_size(
        &self,
        display: Option<&kmsync_core::DesktopDisplayState>,
    ) -> Option<(f64, f64)> {
        if let Some(display) = display {
            if display.width_px > 0 && display.height_px > 0 {
                return Some((f64::from(display.width_px), f64::from(display.height_px)));
            }
        }
        let bounds = self.display_layout.virtual_bounds()?;
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return None;
        }
        Some((bounds.width, bounds.height))
    }

    fn cooldown_active(&mut self, now: Instant) -> bool {
        let Some(deadline) = self.cooldown_until else {
            return false;
        };
        if now < deadline {
            true
        } else {
            self.cooldown_until = None;
            false
        }
    }

    fn at_edge(&self, pointer: Option<PointerPosition>, edge: Edge) -> bool {
        let (Some(pointer), Some(bounds)) = (pointer, self.display_layout.virtual_bounds()) else {
            return false;
        };
        let threshold = 8.0;
        match edge {
            Edge::Left => pointer.x <= bounds.x + threshold,
            Edge::Right => pointer.x >= bounds.x + bounds.width - threshold,
            Edge::Top => pointer.y <= bounds.y + threshold,
            Edge::Bottom => pointer.y >= bounds.y + bounds.height - threshold,
            Edge::TopLeft => pointer.x <= bounds.x + threshold && pointer.y <= bounds.y + threshold,
            Edge::TopRight => {
                pointer.x >= bounds.x + bounds.width - threshold
                    && pointer.y <= bounds.y + threshold
            }
            Edge::BottomLeft => {
                pointer.x <= bounds.x + threshold
                    && pointer.y >= bounds.y + bounds.height - threshold
            }
            Edge::BottomRight => {
                pointer.x >= bounds.x + bounds.width - threshold
                    && pointer.y >= bounds.y + bounds.height - threshold
            }
        }
    }

    fn entry_position(
        &self,
        pointer: Option<PointerPosition>,
        edge: Edge,
    ) -> Option<PointerEntryPosition> {
        let (Some(pointer), Some(bounds)) = (pointer, self.display_layout.virtual_bounds()) else {
            return None;
        };
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return None;
        }
        let x_ratio = ((pointer.x - bounds.x) / bounds.width).clamp(0.0, 1.0) as f32;
        let y_ratio = ((pointer.y - bounds.y) / bounds.height).clamp(0.0, 1.0) as f32;
        Some(match edge {
            Edge::Left => PointerEntryPosition {
                x_ratio: 1.0,
                y_ratio,
            },
            Edge::Right => PointerEntryPosition {
                x_ratio: 0.0,
                y_ratio,
            },
            Edge::Top => PointerEntryPosition {
                x_ratio,
                y_ratio: 1.0,
            },
            Edge::Bottom => PointerEntryPosition {
                x_ratio,
                y_ratio: 0.0,
            },
            Edge::TopLeft => PointerEntryPosition {
                x_ratio: 1.0,
                y_ratio: 1.0,
            },
            Edge::TopRight => PointerEntryPosition {
                x_ratio: 0.0,
                y_ratio: 1.0,
            },
            Edge::BottomLeft => PointerEntryPosition {
                x_ratio: 1.0,
                y_ratio: 0.0,
            },
            Edge::BottomRight => PointerEntryPosition {
                x_ratio: 0.0,
                y_ratio: 0.0,
            },
        })
    }

    fn is_release_hotkey(&self, captured: CapturedInput, release_hotkey: Hotkey) -> bool {
        let InputEvent::Key(event) = captured.event else {
            return false;
        };
        release_hotkey.matches(event)
    }
}

const fn edge_activation_motion_matches(event: InputEvent, edge: Edge) -> bool {
    let InputEvent::Mouse(kmsync_core::MouseEvent::Move { dx, dy }) = event else {
        return false;
    };
    match edge {
        Edge::Left => dx < 0.0,
        Edge::Right => dx > 0.0,
        Edge::Top => dy < 0.0,
        Edge::Bottom => dy > 0.0,
        Edge::TopLeft => dx < 0.0 || dy < 0.0,
        Edge::TopRight => dx > 0.0 || dy < 0.0,
        Edge::BottomLeft => dx < 0.0 || dy > 0.0,
        Edge::BottomRight => dx > 0.0 || dy > 0.0,
    }
}

const fn desktop_remote_event_needs_pointer_sync(event: Option<InputEvent>) -> bool {
    matches!(
        event,
        Some(InputEvent::Mouse(kmsync_core::MouseEvent::Button { .. }))
    )
}

impl<'a> DesktopCaptureRouteResult<'a> {
    const fn local(decision: CaptureDecision) -> Self {
        Self {
            edge: None,
            target_device_id: None,
            profile_name: None,
            route: RouteResult::local(decision),
            remote_event: None,
        }
    }

    fn remote(
        target: &'a DesktopCaptureTarget,
        decision: CaptureDecision,
        entry_position: Option<PointerEntryPosition>,
        local_pointer_action: Option<LocalPointerAction>,
        remote_event: Option<InputEvent>,
    ) -> Self {
        Self {
            edge: Some(target.edge),
            target_device_id: Some(target.target_device_id.as_str()),
            profile_name: Some(target.profile_name),
            route: RouteResult::remote_with_entry_and_pointer_action(
                decision,
                entry_position,
                local_pointer_action,
            ),
            remote_event,
        }
    }

    const fn with_pointer_action(mut self, action: Option<LocalPointerAction>) -> Self {
        self.route = self.route.with_pointer_action(action);
        self
    }
}

fn remote_pointer_crossed_return_edge(
    active_edge: Edge,
    x_ratio: f32,
    y_ratio: f32,
    dx: f32,
    dy: f32,
) -> bool {
    match active_edge {
        Edge::Left => x_ratio >= 1.0 && dx > 0.0,
        Edge::Right => x_ratio <= 0.0 && dx < 0.0,
        Edge::Top => y_ratio >= 1.0 && dy > 0.0,
        Edge::Bottom => y_ratio <= 0.0 && dy < 0.0,
        Edge::TopLeft => (x_ratio >= 1.0 && dx > 0.0) || (y_ratio >= 1.0 && dy > 0.0),
        Edge::TopRight => (x_ratio <= 0.0 && dx < 0.0) || (y_ratio >= 1.0 && dy > 0.0),
        Edge::BottomLeft => (x_ratio >= 1.0 && dx > 0.0) || (y_ratio <= 0.0 && dy < 0.0),
        Edge::BottomRight => (x_ratio <= 0.0 && dx < 0.0) || (y_ratio <= 0.0 && dy < 0.0),
    }
}

fn desktop_capture_plan_from_state(state: &kmsync_core::DesktopState) -> DesktopCapturePlan {
    if state.device.role != kmsync_core::DesktopRole::Master {
        return DesktopCapturePlan::default();
    }

    let mut targets = Vec::new();
    for (edge, target_device_id) in desktop_layout_edge_targets(&state.layout) {
        let Some(peer) = state
            .devices
            .iter()
            .find(|device| device.id == target_device_id && device.online)
        else {
            continue;
        };
        targets.push(DesktopCaptureTarget {
            edge,
            target_device_id: target_device_id.to_string(),
            profile_name: profile_name_for_desktop_pair(&state.device.os, &peer.os),
            display: peer.display.clone(),
        });
    }
    DesktopCapturePlan { targets }
}

#[cfg(test)]
fn refresh_desktop_capture_router_plan_from_state(
    router: &mut DesktopCaptureRouter,
    runtime_plan: &mut DesktopCapturePlan,
    state: &kmsync_core::DesktopState,
) -> bool {
    let latest_plan = desktop_capture_plan_from_state(state);
    if latest_plan == *runtime_plan {
        return false;
    }
    router.replace_plan(latest_plan.clone());
    *runtime_plan = latest_plan;
    true
}

fn apply_pending_desktop_capture_plan_updates(
    rx: &Receiver<DesktopCapturePlan>,
    router: &mut DesktopCaptureRouter,
    runtime_plan: &mut DesktopCapturePlan,
) -> bool {
    let mut changed = false;
    loop {
        match rx.try_recv() {
            Ok(latest_plan) if latest_plan != *runtime_plan => {
                router.replace_plan(latest_plan.clone());
                *runtime_plan = latest_plan;
                changed = true;
            }
            Ok(_) => {}
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return changed,
        }
    }
}

fn spawn_desktop_capture_plan_refresh_worker(
    config_path: PathBuf,
    tx: SyncSender<DesktopCapturePlan>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || loop {
        thread::sleep(DESKTOP_CAPTURE_PLAN_REFRESH_INTERVAL);
        let plan = match build_local_desktop_state(&config_path) {
            Ok(state) => desktop_capture_plan_from_state(&state),
            Err(error) => {
                eprintln!("desktop capture plan refresh failed: {error}");
                continue;
            }
        };
        match tx.try_send(plan) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => break,
        }
    })
}

fn spawn_desktop_capture_startup_probe_status_writer(
    config_path: PathBuf,
    plan: DesktopCapturePlan,
) {
    let config_path_for_write = config_path.clone();
    let _ = spawn_desktop_capture_startup_probe_status_writer_with(
        plan,
        move |target_device_id| send_target_probe_frame(&config_path, target_device_id),
        move |runtime| {
            if let Err(error) = write_desktop_sync_runtime_status(&config_path_for_write, &runtime)
            {
                eprintln!("desktop capture runtime status write failed: {error}");
            }
        },
    );
}

fn spawn_desktop_capture_startup_probe_status_writer_with<Probe, Write>(
    plan: DesktopCapturePlan,
    mut probe: Probe,
    mut write_runtime: Write,
) -> thread::JoinHandle<()>
where
    Probe: FnMut(&str) -> Result<String, TargetProbeFailure> + Send + 'static,
    Write: FnMut(kmsync_core::DesktopSyncRuntimeState) + Send + 'static,
{
    thread::spawn(move || {
        if let Some(runtime) = desktop_capture_startup_probe_runtime_status_with(&plan, &mut probe)
        {
            write_runtime(runtime);
        }
    })
}

fn desktop_capture_startup_probe_runtime_status_with<Probe>(
    plan: &DesktopCapturePlan,
    probe: &mut Probe,
) -> Option<kmsync_core::DesktopSyncRuntimeState>
where
    Probe: FnMut(&str) -> Result<String, TargetProbeFailure>,
{
    for target in &plan.targets {
        if let Err(failure) = probe(&target.target_device_id) {
            eprintln!(
                "desktop capture target probe failed for {}: {}",
                target.target_device_id, failure.message
            );
            return Some(desktop_capture_probe_failed_runtime_status(
                plan,
                &target.target_device_id,
                &failure,
            ));
        }
    }
    None
}

fn desktop_capture_idle_log_line() -> &'static str {
    "desktop_capture=idle reason=no_online_layout_targets"
}

fn desktop_capture_plan_log_line(plan: &DesktopCapturePlan) -> String {
    let targets = plan
        .targets
        .iter()
        .map(|target| {
            format!(
                "target={} edge={} profile={}",
                target.target_device_id,
                target.edge.as_str(),
                target.profile_name.as_str()
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "desktop_capture=armed targets={} {}",
        plan.targets.len(),
        targets
    )
}

fn desktop_capture_idle_runtime_status() -> kmsync_core::DesktopSyncRuntimeState {
    kmsync_core::DesktopSyncRuntimeState {
        state: kmsync_core::DesktopSyncRuntimeKind::Idle,
        error: None,
        targets: Vec::new(),
        updated_at: None,
        ..kmsync_core::DesktopSyncRuntimeState::default()
    }
}

fn desktop_capture_plan_runtime_status(
    plan: &DesktopCapturePlan,
) -> kmsync_core::DesktopSyncRuntimeState {
    kmsync_core::DesktopSyncRuntimeState {
        state: kmsync_core::DesktopSyncRuntimeKind::Armed,
        error: None,
        targets: desktop_capture_plan_target_ids(plan),
        updated_at: None,
        ..kmsync_core::DesktopSyncRuntimeState::default()
    }
}

fn desktop_capture_observed_runtime_status(
    plan: &DesktopCapturePlan,
    counters: &DesktopCaptureRuntimeCounters,
) -> Option<kmsync_core::DesktopSyncRuntimeState> {
    if counters.transmit_failed() {
        return None;
    }
    let snapshot = counters.snapshot();
    let mut runtime = kmsync_core::DesktopSyncRuntimeState {
        state: kmsync_core::DesktopSyncRuntimeKind::Armed,
        error: None,
        targets: desktop_capture_plan_target_ids(plan),
        updated_at: None,
        captured_events: snapshot.captured_events,
        routed_events: snapshot.routed_events,
        sent_events: snapshot.sent_events,
        ..kmsync_core::DesktopSyncRuntimeState::default()
    };
    apply_capture_observation_to_runtime(&mut runtime, snapshot.last_capture.as_ref());
    Some(runtime)
}

fn desktop_capture_failed_runtime_status(
    plan: &DesktopCapturePlan,
    error: &str,
) -> kmsync_core::DesktopSyncRuntimeState {
    kmsync_core::DesktopSyncRuntimeState {
        state: kmsync_core::DesktopSyncRuntimeKind::Failed,
        error: Some(error.to_string()),
        targets: desktop_capture_plan_target_ids(plan),
        updated_at: None,
        ..kmsync_core::DesktopSyncRuntimeState::default()
    }
}

fn desktop_capture_probe_failed_runtime_status(
    plan: &DesktopCapturePlan,
    target_device_id: &str,
    failure: &TargetProbeFailure,
) -> kmsync_core::DesktopSyncRuntimeState {
    let transport = failure.transport.as_deref().unwrap_or("-");
    desktop_capture_failed_runtime_status(
        plan,
        &format!(
            "目标 {target_device_id} 同步探测失败：transport={transport} error={}",
            failure.message
        ),
    )
}

fn desktop_capture_plan_target_ids(plan: &DesktopCapturePlan) -> Vec<String> {
    plan.targets
        .iter()
        .map(|target| target.target_device_id.clone())
        .collect()
}

fn desktop_input_listener_runtime_status() -> kmsync_core::DesktopSyncRuntimeState {
    kmsync_core::DesktopSyncRuntimeState {
        state: kmsync_core::DesktopSyncRuntimeKind::Listening,
        error: None,
        targets: Vec::new(),
        updated_at: None,
        relay_connected: false,
        ..kmsync_core::DesktopSyncRuntimeState::default()
    }
}

fn desktop_input_listener_relay_runtime_status(
    relay_connected: bool,
    error: Option<String>,
) -> kmsync_core::DesktopSyncRuntimeState {
    kmsync_core::DesktopSyncRuntimeState {
        state: kmsync_core::DesktopSyncRuntimeKind::Listening,
        error,
        targets: Vec::new(),
        updated_at: Some(unix_timestamp_seconds()),
        relay_connected,
        ..kmsync_core::DesktopSyncRuntimeState::default()
    }
}

fn desktop_input_listener_progress_runtime_status(
    received_events: u64,
    last_received_at: u64,
) -> kmsync_core::DesktopSyncRuntimeState {
    kmsync_core::DesktopSyncRuntimeState {
        state: kmsync_core::DesktopSyncRuntimeKind::Listening,
        error: None,
        targets: Vec::new(),
        updated_at: None,
        relay_connected: true,
        received_events,
        last_received_at: Some(last_received_at),
        ..kmsync_core::DesktopSyncRuntimeState::default()
    }
}

fn desktop_input_injection_progress_runtime_status(
    injected_events: u64,
    last_injected_at: u64,
) -> kmsync_core::DesktopSyncRuntimeState {
    kmsync_core::DesktopSyncRuntimeState {
        state: kmsync_core::DesktopSyncRuntimeKind::Listening,
        error: None,
        targets: Vec::new(),
        updated_at: None,
        relay_connected: true,
        injected_events,
        last_injected_at: Some(last_injected_at),
        ..kmsync_core::DesktopSyncRuntimeState::default()
    }
}

fn desktop_input_listener_failed_runtime_status(
    error: &str,
) -> kmsync_core::DesktopSyncRuntimeState {
    kmsync_core::DesktopSyncRuntimeState {
        state: kmsync_core::DesktopSyncRuntimeKind::Failed,
        error: Some(error.to_string()),
        targets: Vec::new(),
        updated_at: None,
        ..kmsync_core::DesktopSyncRuntimeState::default()
    }
}

fn desktop_capture_transmit_failed_runtime_status(
    target_device_id: &str,
    error: &str,
    counters: &DesktopCaptureRuntimeCounters,
) -> kmsync_core::DesktopSyncRuntimeState {
    let snapshot = counters.snapshot();
    let mut runtime = kmsync_core::DesktopSyncRuntimeState {
        state: kmsync_core::DesktopSyncRuntimeKind::Failed,
        error: Some(format!("发送到 {target_device_id} 失败：{error}")),
        targets: vec![target_device_id.to_string()],
        updated_at: None,
        captured_events: snapshot.captured_events,
        routed_events: snapshot.routed_events,
        sent_events: snapshot.sent_events,
        ..kmsync_core::DesktopSyncRuntimeState::default()
    };
    apply_capture_observation_to_runtime(&mut runtime, snapshot.last_capture.as_ref());
    runtime
}

#[cfg(test)]
fn desktop_capture_transmit_recovered_runtime_status<'a>(
    target_device_ids: impl IntoIterator<Item = &'a str>,
) -> kmsync_core::DesktopSyncRuntimeState {
    kmsync_core::DesktopSyncRuntimeState {
        state: kmsync_core::DesktopSyncRuntimeKind::Armed,
        error: None,
        targets: target_device_ids.into_iter().map(str::to_string).collect(),
        updated_at: None,
        ..kmsync_core::DesktopSyncRuntimeState::default()
    }
}

fn desktop_capture_transmit_progress_runtime_status<'a>(
    target_device_ids: impl IntoIterator<Item = &'a str>,
    counters: &DesktopCaptureRuntimeCounters,
    last_sent_at: u64,
) -> kmsync_core::DesktopSyncRuntimeState {
    let snapshot = counters.snapshot();
    let mut runtime = kmsync_core::DesktopSyncRuntimeState {
        state: kmsync_core::DesktopSyncRuntimeKind::Armed,
        error: None,
        targets: target_device_ids.into_iter().map(str::to_string).collect(),
        updated_at: None,
        captured_events: snapshot.captured_events,
        routed_events: snapshot.routed_events,
        sent_events: snapshot.sent_events,
        last_sent_at: Some(last_sent_at),
        ..kmsync_core::DesktopSyncRuntimeState::default()
    };
    apply_capture_observation_to_runtime(&mut runtime, snapshot.last_capture.as_ref());
    runtime
}

fn apply_capture_observation_to_runtime(
    runtime: &mut kmsync_core::DesktopSyncRuntimeState,
    observation: Option<&DesktopCaptureObservation>,
) {
    let Some(observation) = observation else {
        return;
    };
    runtime.last_capture_pointer_x = observation.pointer_x;
    runtime.last_capture_pointer_y = observation.pointer_y;
    runtime.last_capture_edge = observation.edge.clone();
    runtime.last_capture_target = observation.target_device_id.clone();
    runtime.last_capture_routed = observation.routed;
}

fn desktop_sync_runtime_status_path(config_path: &Path) -> PathBuf {
    config_path.with_file_name("desktop-runtime.json")
}

fn read_desktop_sync_runtime_status(
    config_path: &Path,
) -> Result<kmsync_core::DesktopSyncRuntimeState, String> {
    let path = desktop_sync_runtime_status_path(config_path);
    if !path.exists() {
        return Ok(kmsync_core::DesktopSyncRuntimeState::default());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

fn write_desktop_sync_runtime_status(
    config_path: &Path,
    runtime: &kmsync_core::DesktopSyncRuntimeState,
) -> Result<(), String> {
    let mut runtime = runtime.clone();
    runtime.updated_at = Some(unix_timestamp_seconds());
    let text = serde_json::to_string_pretty(&runtime)
        .map(|mut text| {
            text.push('\n');
            text
        })
        .map_err(|error| format!("failed to encode desktop runtime status: {error}"))?;
    local_config::write_text_atomic(&desktop_sync_runtime_status_path(config_path), &text)
}

fn desktop_layout_edge_targets(layout: &kmsync_core::DesktopLayout) -> Vec<(Edge, &str)> {
    [
        (Edge::Left, layout.left.as_deref()),
        (Edge::Right, layout.right.as_deref()),
        (Edge::Top, layout.top.as_deref()),
        (Edge::Bottom, layout.bottom.as_deref()),
    ]
    .into_iter()
    .filter_map(|(edge, device_id)| device_id.map(|device_id| (edge, device_id)))
    .collect()
}

fn profile_name_for_desktop_pair(source_os: &str, target_os: &str) -> ProfileName {
    match (source_os, target_os) {
        ("macos", "windows" | "linux") => ProfileName::MacToWindows,
        ("windows" | "linux", "macos") => ProfileName::WindowsToMac,
        ("macos", _) => ProfileName::MacToWindows,
        _ => ProfileName::WindowsToMac,
    }
}

enum CoreServiceThreadResult {
    DataPlane(Result<(), String>),
    Heartbeat(Result<(), String>),
    LocalIpc(Result<(), String>),
    DesktopCapture(Result<(), String>),
}

impl CoreServiceThreadResult {
    const fn component(&self) -> &'static str {
        match self {
            Self::DataPlane(_) => "data_plane",
            Self::Heartbeat(_) => "heartbeat",
            Self::LocalIpc(_) => "local_ipc",
            Self::DesktopCapture(_) => "desktop_capture",
        }
    }

    fn into_result(self) -> Result<(), String> {
        match self {
            Self::DataPlane(result)
            | Self::Heartbeat(result)
            | Self::LocalIpc(result)
            | Self::DesktopCapture(result) => result,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CoreServiceWorkerAction {
    Continue,
    Stop(Result<(), String>),
}

fn core_service_action_for_worker_result(
    result: CoreServiceThreadResult,
) -> CoreServiceWorkerAction {
    let component = result.component();
    match result.into_result() {
        Ok(()) if component == "heartbeat" => {
            eprintln!("core service heartbeat stopped unexpectedly; retrying");
            CoreServiceWorkerAction::Continue
        }
        Ok(()) if component == "desktop_capture" => {
            eprintln!("core service desktop capture stopped unexpectedly; retrying");
            CoreServiceWorkerAction::Continue
        }
        Ok(()) => CoreServiceWorkerAction::Stop(Err(format!(
            "core service {component} stopped unexpectedly"
        ))),
        Err(error) if component == "heartbeat" => {
            eprintln!("core service heartbeat failed: {error}; retrying");
            CoreServiceWorkerAction::Continue
        }
        Err(error) if component == "desktop_capture" => {
            eprintln!("core service desktop capture failed: {error}; retrying");
            CoreServiceWorkerAction::Continue
        }
        Err(error) => {
            CoreServiceWorkerAction::Stop(Err(format!("core service {component} failed: {error}")))
        }
    }
}

fn run_core_service(config_path: &Path) -> Result<(), String> {
    ensure_daemon_config_file(config_path)?;
    let config = client::ClientConfig::load(config_path)?;
    let plan = CoreServicePlan::from_config(config_path.to_path_buf(), &config);
    println!(
        "core_service=starting bind={} local_ipc_transport={} local_ipc_address={} input_hot_path={} control_plane={}",
        plan.bind,
        plan.ipc_endpoint.transport.as_str(),
        plan.ipc_endpoint.address,
        plan.input_hot_path,
        plan.control_plane
    );

    let (result_tx, result_rx) = std::sync::mpsc::channel();

    let bind = plan.bind;
    let data_plane_config = config.clone();
    let data_plane_runtime_config_path = plan.config_path.clone();
    let data_plane_result_tx = result_tx.clone();
    thread::spawn(move || {
        let _ =
            data_plane_result_tx.send(CoreServiceThreadResult::DataPlane(run_listener_with_relay(
                bind,
                Some(data_plane_config),
                Some(data_plane_runtime_config_path),
            )));
    });

    let heartbeat_result_tx = result_tx.clone();
    thread::spawn(move || loop {
        let result = client::run_heartbeat_loop(config.clone());
        let _ = heartbeat_result_tx.send(CoreServiceThreadResult::Heartbeat(result));
        thread::sleep(Duration::from_secs(
            config.heartbeat_interval_seconds.max(5),
        ));
    });

    let ipc_endpoint = plan.ipc_endpoint.clone();
    let ipc_config_path = plan.config_path.clone();
    let ipc_result_tx = result_tx.clone();
    thread::spawn(move || {
        let _ = ipc_result_tx.send(CoreServiceThreadResult::LocalIpc(
            run_local_ipc_serve_forever(&ipc_endpoint, &ipc_config_path),
        ));
    });

    if core_service_should_spawn_desktop_capture(env::consts::OS) {
        let capture_config_path = plan.config_path.clone();
        let capture_result_tx = result_tx.clone();
        thread::spawn(move || loop {
            let result = run_desktop_capture_supervisor(&capture_config_path);
            let _ = capture_result_tx.send(CoreServiceThreadResult::DesktopCapture(result));
            thread::sleep(Duration::from_secs(2));
        });
    }

    wait_core_service_results(result_rx)
}

fn core_service_should_spawn_desktop_capture(os: &str) -> bool {
    os != "macos"
}

fn run_desktop_capture_supervisor(config_path: &Path) -> Result<(), String> {
    let mut logged_idle = false;
    loop {
        let state = build_local_desktop_state(config_path)?;
        if !desktop_capture_supervisor_should_manage_runtime(&state) {
            thread::sleep(Duration::from_secs(2));
            continue;
        }
        let plan = desktop_capture_plan_from_state(&state);
        if plan.targets.is_empty() {
            if let Err(error) = write_desktop_sync_runtime_status(
                config_path,
                &desktop_capture_idle_runtime_status(),
            ) {
                eprintln!("desktop capture runtime status write failed: {error}");
            }
            if !logged_idle {
                eprintln!("{}", desktop_capture_idle_log_line());
                logged_idle = true;
            }
            thread::sleep(Duration::from_secs(2));
            continue;
        }
        return run_desktop_capture_plan(config_path, plan);
    }
}

fn desktop_capture_supervisor_should_manage_runtime(state: &kmsync_core::DesktopState) -> bool {
    state.device.role == kmsync_core::DesktopRole::Master
}

fn run_desktop_capture_plan(config_path: &Path, plan: DesktopCapturePlan) -> Result<(), String> {
    eprintln!("{}", desktop_capture_plan_log_line(&plan));
    if let Err(error) =
        write_desktop_sync_runtime_status(config_path, &desktop_capture_plan_runtime_status(&plan))
    {
        eprintln!("desktop capture runtime status write failed: {error}");
    }
    spawn_desktop_capture_startup_probe_status_writer(config_path.to_path_buf(), plan.clone());
    let runtime_plan = plan.clone();
    let mut platform = platform::current_platform();
    let display_layout = platform.display_layout();
    let mut router = DesktopCaptureRouter::with_display_layout(plan, display_layout);
    let (tx, rx) = sync_channel(1024);
    let queue_stats = CaptureQueueStats::default();
    let runtime_counters = Arc::new(DesktopCaptureRuntimeCounters::default());
    let local_pointer_hidden = Arc::new(AtomicBool::new(false));
    let tx_queue_stats = queue_stats.clone();
    let tx_config_path = config_path.to_path_buf();
    let tx_runtime_counters = Arc::clone(&runtime_counters);
    thread::spawn(move || {
        transmit_desktop_capture_events(rx, tx_config_path, tx_queue_stats, tx_runtime_counters);
    });
    let (plan_tx, plan_rx) = sync_channel(1);
    spawn_desktop_capture_plan_refresh_worker(config_path.to_path_buf(), plan_tx);

    let capture_local_pointer_hidden = Arc::clone(&local_pointer_hidden);
    let capture_runtime_counters = Arc::clone(&runtime_counters);
    let mut capture_runtime_plan = runtime_plan.clone();
    let capture_runtime_config_path = config_path.to_path_buf();
    let mut last_capture_progress_write = None;
    let capture_result = platform.capture_loop(move |captured| {
        let now = Instant::now();
        if apply_pending_desktop_capture_plan_updates(
            &plan_rx,
            &mut router,
            &mut capture_runtime_plan,
        ) {
            let runtime = if capture_runtime_plan.targets.is_empty() {
                desktop_capture_idle_runtime_status()
            } else {
                desktop_capture_plan_runtime_status(&capture_runtime_plan)
            };
            if let Err(write_error) =
                write_desktop_sync_runtime_status(&capture_runtime_config_path, &runtime)
            {
                eprintln!("desktop capture runtime status write failed: {write_error}");
            }
        }
        capture_runtime_counters.record_captured();
        let route = router.route_at_with_application(captured, None, now);
        let routed = route.route.send_remote;
        capture_runtime_counters.record_capture_observation(
            captured.pointer,
            route.edge,
            route.target_device_id.as_deref(),
            routed,
        );
        if routed {
            capture_runtime_counters.record_routed();
        }
        enqueue_desktop_capture(&tx, &queue_stats, &route);
        apply_local_pointer_action(
            route.route.local_pointer_action,
            &capture_local_pointer_hidden,
        );
        if desktop_runtime_progress_write_due(last_capture_progress_write, now) {
            last_capture_progress_write = Some(now);
            if let Some(runtime) = desktop_capture_observed_runtime_status(
                &capture_runtime_plan,
                &capture_runtime_counters,
            ) {
                if let Err(write_error) =
                    write_desktop_sync_runtime_status(&capture_runtime_config_path, &runtime)
                {
                    eprintln!("desktop capture runtime status write failed: {write_error}");
                }
            }
        }
        route.route.decision
    });
    restore_local_pointer_if_hidden(&local_pointer_hidden, None);
    if let Err(error) = &capture_result {
        if let Err(write_error) = write_desktop_sync_runtime_status(
            config_path,
            &desktop_capture_failed_runtime_status(&runtime_plan, error),
        ) {
            eprintln!("desktop capture runtime status write failed: {write_error}");
        }
    }
    capture_result
}

fn run_windows_service(config_path: &Path) -> Result<(), String> {
    println!(
        "windows_service=starting name={} display_name={} config={}",
        WINDOWS_SERVICE_NAME,
        WINDOWS_SERVICE_DISPLAY_NAME,
        config_path.display()
    );
    #[cfg(windows)]
    {
        return windows_service::run(WINDOWS_SERVICE_NAME, config_path);
    }

    #[cfg(not(windows))]
    {
        let _ = config_path;
        Err("windows-service is only available on Windows".to_string())
    }
}

#[cfg(test)]
fn windows_service_command_line(binary: &Path) -> String {
    format!("{} windows-service", quote_command_path(binary))
}

#[cfg(test)]
fn windows_companion_command_line(binary: &Path) -> String {
    format!("{} core-service", quote_command_path(binary))
}

#[cfg(test)]
fn quote_command_path(path: &Path) -> String {
    let text = path.display().to_string().replace('"', r#"\""#);
    format!(r#""{text}""#)
}

fn wait_core_service_results(rx: Receiver<CoreServiceThreadResult>) -> Result<(), String> {
    loop {
        let result = rx
            .recv()
            .map_err(|error| format!("core service worker result channel closed: {error}"))?;
        match core_service_action_for_worker_result(result) {
            CoreServiceWorkerAction::Continue => continue,
            CoreServiceWorkerAction::Stop(result) => return result,
        }
    }
}

fn run_local_ipc_serve_forever(
    endpoint: &local_ipc::LocalIpcEndpoint,
    config_path: &Path,
) -> Result<(), String> {
    println!(
        "local_ipc_listening transport={} address={}",
        endpoint.transport.as_str(),
        endpoint.address
    );
    loop {
        local_ipc::LocalIpcServer::bind(endpoint)
            .map_err(|error| error.to_string())?
            .serve_one(|request| {
                handle_local_ipc_request_with_config_path(request, Some(config_path))
            })
            .map_err(|error| error.to_string())?;
    }
}

fn run_self_test(profile_name: ProfileName) -> Result<(), String> {
    let profile = profile_name.profile();
    let compiled = CompiledProfile::compile(&profile).map_err(|error| format!("{error:?}"))?;
    let adapter = platform::current_platform();

    let event = InputEvent::Key(KeyEvent {
        key: Key::C,
        state: KeyState::Pressed,
        modifiers: match profile_name {
            ProfileName::MacToWindows => Modifiers::META,
            ProfileName::WindowsToMac => Modifiers::CONTROL,
        },
    });

    let mapped = compiled.transform(event);
    print!(
        "{}",
        render_self_test_report(SelfTestReport {
            profile_name,
            input_event_type: input_event_log_type(&event),
            mapped_event_type: input_event_log_type(&mapped),
            capabilities: adapter.capabilities(),
            permission_checks: adapter.permission_checks(),
            permission_hints: adapter.permission_hints(),
            network_quic: check_quic_network(),
        })
    );

    Ok(())
}

struct SelfTestReport<'a> {
    profile_name: ProfileName,
    input_event_type: &'static str,
    mapped_event_type: &'static str,
    capabilities: platform::PlatformCapabilities,
    permission_checks: Vec<platform::PlatformPermissionCheck>,
    permission_hints: &'a [&'a str],
    network_quic: Result<(), String>,
}

fn render_self_test_report(report: SelfTestReport<'_>) -> String {
    let mut output = String::from("self-test\n");
    let _ = writeln!(
        output,
        "profile={:?} profile_mapping=ok input_event={} mapped_event={}",
        report.profile_name, report.input_event_type, report.mapped_event_type
    );
    let _ = writeln!(
        output,
        "input_capture={}",
        capability_status(report.capabilities.input_capture)
    );
    let _ = writeln!(
        output,
        "input_injection={}",
        capability_status(report.capabilities.input_injection)
    );
    let _ = writeln!(
        output,
        "clipboard_text={}",
        capability_status(report.capabilities.clipboard_text)
    );
    match report.network_quic {
        Ok(()) => output.push_str("network_quic=ok\n"),
        Err(error) => {
            let _ = writeln!(output, "network_quic=failed error={error}");
        }
    }
    for check in report.permission_checks {
        let _ = writeln!(
            output,
            "permission_check={} status={} label=\"{}\" guidance=\"{}\"",
            check.id,
            check.status.as_str(),
            check.label,
            check.guidance
        );
    }
    for hint in report.permission_hints {
        let _ = writeln!(output, "permission_hint={hint}");
    }
    output
}

const fn capability_status(available: bool) -> &'static str {
    if available {
        "ok"
    } else {
        "unavailable"
    }
}

fn check_quic_network() -> Result<(), String> {
    let receiver = QuicEventReceiver::bind("127.0.0.1:0".parse().expect("valid loopback bind"))?;
    let _sender = QuicEventSender::connect(receiver.local_addr()?)?;
    Ok(())
}

fn input_event_log_type(event: &InputEvent) -> &'static str {
    match event {
        InputEvent::Key(_) => "key",
        InputEvent::Mouse(kmsync_core::MouseEvent::Move { .. }) => "mouse_move",
        InputEvent::Mouse(kmsync_core::MouseEvent::Position { .. }) => "mouse_position",
        InputEvent::Mouse(kmsync_core::MouseEvent::Button { .. }) => "mouse_button",
        InputEvent::Scroll(_) => "scroll",
    }
}

fn run_listener(bind: SocketAddr) -> Result<(), String> {
    run_listener_with_relay(bind, None, None)
}

fn run_listener_with_relay(
    bind: SocketAddr,
    relay_config: Option<client::ClientConfig>,
    runtime_config_path: Option<PathBuf>,
) -> Result<(), String> {
    let receiver = match QuicEventReceiver::bind(bind) {
        Ok(receiver) => receiver,
        Err(error) => {
            write_input_listener_failed_runtime_status_if_configured(
                runtime_config_path.as_deref(),
                &error,
            );
            return Err(error);
        }
    };
    write_input_listener_runtime_status_if_configured(runtime_config_path.as_deref());
    let (input_tx, input_rx) = sync_channel(1024);
    let (clipboard_tx, clipboard_rx) = sync_channel(16);
    let (control_tx, control_rx) = sync_channel(32);
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let latency_stats = ListenerLatencyStats::default();
    println!("listening on {bind}");

    let receive_input_tx = input_tx.clone();
    let receive_clipboard_tx = clipboard_tx.clone();
    let receive_control_tx = control_tx.clone();
    let receive_result_tx = result_tx.clone();
    let receive_latency_stats = latency_stats.clone();
    let receive_runtime_config_path = runtime_config_path.clone();
    thread::spawn(move || {
        let mut receiver = receiver;
        let result = receive_remote_frames(
            &mut receiver,
            receive_input_tx,
            receive_clipboard_tx,
            receive_control_tx,
            receive_latency_stats,
            receive_runtime_config_path,
        );
        let _ = receive_result_tx.send(ListenerThreadResult::Receive(result));
    });

    if let Some(config) = relay_config {
        let relay_input_tx = input_tx.clone();
        let relay_clipboard_tx = clipboard_tx.clone();
        let relay_control_tx = control_tx.clone();
        let relay_latency_stats = latency_stats.clone();
        let relay_runtime_config_path = runtime_config_path.clone();
        thread::spawn(move || {
            run_relay_receive_loop(
                config,
                relay_input_tx,
                relay_clipboard_tx,
                relay_control_tx,
                relay_latency_stats,
                relay_runtime_config_path,
            );
        });
    }

    let injection_result_tx = result_tx.clone();
    let injection_latency_stats = latency_stats.clone();
    let injection_runtime_config_path = runtime_config_path.clone();
    thread::spawn(move || {
        let mut adapter = platform::current_platform();
        let result = inject_received_frames(
            input_rx,
            &mut adapter,
            injection_latency_stats,
            injection_runtime_config_path,
        );
        let _ = injection_result_tx.send(ListenerThreadResult::Injection(result));
    });

    let clipboard_result_tx = result_tx.clone();
    thread::spawn(move || {
        let mut adapter = platform::current_platform();
        let result = apply_clipboard_frames(clipboard_rx, &mut adapter);
        let _ = clipboard_result_tx.send(ListenerThreadResult::Clipboard(result));
    });

    let control_result_tx = result_tx.clone();
    thread::spawn(move || {
        let result = handle_control_frames(control_rx);
        let _ = control_result_tx.send(ListenerThreadResult::Control(result));
    });

    let result = wait_listener_results(result_rx);
    if let Err(error) = &result {
        write_input_listener_failed_runtime_status_if_configured(
            runtime_config_path.as_deref(),
            error,
        );
    }
    result
}

fn write_input_listener_runtime_status_if_configured(config_path: Option<&Path>) {
    write_input_listener_desktop_runtime_status_if_configured(
        config_path,
        &desktop_input_listener_runtime_status(),
        "status",
    );
}

fn write_input_listener_failed_runtime_status_if_configured(
    config_path: Option<&Path>,
    error: &str,
) {
    write_input_listener_desktop_runtime_status_if_configured(
        config_path,
        &desktop_input_listener_failed_runtime_status(error),
        "failure",
    );
}

fn write_input_listener_relay_runtime_status_if_configured(
    config_path: Option<&Path>,
    relay_connected: bool,
    error: Option<String>,
) {
    write_input_listener_desktop_runtime_status_if_configured(
        config_path,
        &desktop_input_listener_relay_runtime_status(relay_connected, error),
        "relay",
    );
}

fn write_input_listener_desktop_runtime_status_if_configured(
    config_path: Option<&Path>,
    runtime: &kmsync_core::DesktopSyncRuntimeState,
    context: &str,
) {
    let Some(config_path) = config_path else {
        return;
    };
    if !input_listener_runtime_status_should_write(config_path) {
        return;
    }
    if let Err(write_error) = write_desktop_sync_runtime_status(config_path, runtime) {
        eprintln!("desktop input listener {context} runtime status write failed: {write_error}");
    }
}

fn input_listener_runtime_status_should_write(config_path: &Path) -> bool {
    match desktop_config::DesktopConfig::load(config_path) {
        Ok(config) => config.role == kmsync_core::DesktopRole::Client,
        Err(error) => {
            eprintln!(
                "desktop input listener runtime status skipped; failed to read role from {}: {error}",
                config_path.display()
            );
            false
        }
    }
}

enum ListenerThreadResult {
    Receive(Result<(), String>),
    Injection(Result<(), String>),
    Clipboard(Result<(), String>),
    Control(Result<(), String>),
}

fn wait_listener_results(result_rx: Receiver<ListenerThreadResult>) -> Result<(), String> {
    let mut receive_result = None;
    let mut injection_result = None;
    let mut clipboard_result = None;
    let mut control_result = None;

    loop {
        match result_rx
            .recv()
            .map_err(|error| format!("listener worker result channel closed: {error}"))?
        {
            ListenerThreadResult::Receive(result) => {
                receive_result = Some(result);
            }
            ListenerThreadResult::Injection(result) => {
                if result.is_err() {
                    return result;
                }
                injection_result = Some(result);
            }
            ListenerThreadResult::Clipboard(result) => {
                if result.is_err() {
                    return result;
                }
                clipboard_result = Some(result);
            }
            ListenerThreadResult::Control(result) => {
                if result.is_err() {
                    return result;
                }
                control_result = Some(result);
            }
        }

        if receive_result.is_some()
            && injection_result.is_some()
            && clipboard_result.is_some()
            && control_result.is_some()
        {
            return combine_listener_results([
                receive_result.expect("checked receive result"),
                injection_result.expect("checked injection result"),
                clipboard_result.expect("checked clipboard result"),
                control_result.expect("checked control result"),
            ]);
        }
    }
}

fn combine_listener_results<const N: usize>(
    results: [Result<(), String>; N],
) -> Result<(), String> {
    let mut combined_error = None;
    for result in results {
        if let Err(error) = result {
            combined_error = Some(match combined_error {
                Some(existing) => format!("{existing}; {error}"),
                None => error,
            });
        }
    }
    combined_error.map_or(Ok(()), Err)
}

trait ProtocolFrameReceiver {
    fn recv_frame(&mut self) -> Result<ProtocolFrame, String>;
}

impl ProtocolFrameReceiver for QuicEventReceiver {
    fn recv_frame(&mut self) -> Result<ProtocolFrame, String> {
        QuicEventReceiver::recv_frame(self)
    }
}

impl ProtocolFrameReceiver for client::RelayFrameReceiver {
    fn recv_frame(&mut self) -> Result<ProtocolFrame, String> {
        client::RelayFrameReceiver::recv_frame(self)
    }
}

fn run_relay_receive_loop(
    config: client::ClientConfig,
    input_tx: SyncSender<ReceivedInputFrame>,
    clipboard_tx: SyncSender<ReceivedClipboardFrame>,
    control_tx: SyncSender<ReceivedControlFrame>,
    stats: ListenerLatencyStats,
    runtime_config_path: Option<PathBuf>,
) {
    loop {
        let identity = match client::DeviceIdentity::load_or_generate(&config.identity_path) {
            Ok(identity) => identity,
            Err(error) => {
                eprintln!("relay receive unavailable; identity failed: {error}");
                write_input_listener_relay_runtime_status_if_configured(
                    runtime_config_path.as_deref(),
                    false,
                    Some(error),
                );
                thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        let mut receiver =
            match client::RelayFrameReceiver::connect(&config.server_url, &identity.device_id) {
                Ok(receiver) => receiver,
                Err(error) => {
                    eprintln!("relay receive unavailable; reconnecting: {error}");
                    write_input_listener_relay_runtime_status_if_configured(
                        runtime_config_path.as_deref(),
                        false,
                        Some(error),
                    );
                    thread::sleep(Duration::from_secs(2));
                    continue;
                }
            };
        println!("relay receive connected for {}", identity.device_id);
        write_input_listener_relay_runtime_status_if_configured(
            runtime_config_path.as_deref(),
            true,
            None,
        );
        if let Err(error) = receive_remote_frames(
            &mut receiver,
            input_tx.clone(),
            clipboard_tx.clone(),
            control_tx.clone(),
            stats.clone(),
            runtime_config_path.clone(),
        ) {
            eprintln!("relay receive disconnected; reconnecting: {error}");
            write_input_listener_relay_runtime_status_if_configured(
                runtime_config_path.as_deref(),
                false,
                Some(error),
            );
            thread::sleep(Duration::from_secs(2));
        }
    }
}

#[derive(Debug, Clone)]
struct ReceivedInputFrame {
    frame: ProtocolFrame,
    received_at: Instant,
}

const RELIABLE_INPUT_REORDER_WINDOW: u64 = 64;

#[derive(Default)]
struct ReliableInputSequencer {
    next_sequence: u64,
    pending: BTreeMap<u64, ReceivedInputFrame>,
}

enum ReliableInputDecision {
    Inject(ReceivedInputFrame),
    Buffer,
    Drop,
    Recover(ReceivedInputFrame),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputGapRecoveryAction {
    Block,
    Wait(Duration),
    Recover,
}

#[derive(Default)]
struct InputGapRecovery {
    deadline: Option<Instant>,
}

impl InputGapRecovery {
    fn next_action(&mut self, has_gap: bool, now: Instant) -> InputGapRecoveryAction {
        if !has_gap {
            self.deadline = None;
            return InputGapRecoveryAction::Block;
        }

        let deadline = *self
            .deadline
            .get_or_insert(now + RELIABLE_INPUT_GAP_RECOVERY_TIMEOUT);
        if now >= deadline {
            self.deadline = None;
            InputGapRecoveryAction::Recover
        } else {
            InputGapRecoveryAction::Wait(deadline.duration_since(now))
        }
    }

    fn reset(&mut self) {
        self.deadline = None;
    }
}

impl ReliableInputSequencer {
    fn new() -> Self {
        Self {
            next_sequence: 1,
            pending: BTreeMap::new(),
        }
    }

    fn accept(&mut self, received: ReceivedInputFrame) -> ReliableInputDecision {
        let sequence = received.frame.sequence;
        if sequence < self.next_sequence {
            return ReliableInputDecision::Drop;
        }

        if sequence == self.next_sequence {
            self.next_sequence = self.next_sequence.saturating_add(1);
            return ReliableInputDecision::Inject(received);
        }

        if sequence.saturating_sub(self.next_sequence) > RELIABLE_INPUT_REORDER_WINDOW {
            self.pending.clear();
            self.next_sequence = sequence.saturating_add(1);
            return ReliableInputDecision::Recover(received);
        }

        self.pending.entry(sequence).or_insert(received);
        ReliableInputDecision::Buffer
    }

    fn pop_ready(&mut self) -> Option<ReceivedInputFrame> {
        let ready = self.pending.remove(&self.next_sequence)?;
        self.next_sequence = self.next_sequence.saturating_add(1);
        Some(ready)
    }

    fn skip_to_sequence(&mut self, sequence: u64) {
        if sequence > self.next_sequence {
            self.next_sequence = sequence;
        }
    }

    fn next_pending_sequence(&self) -> Option<u64> {
        self.pending
            .first_key_value()
            .map(|(&sequence, _)| sequence)
    }

    const fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    fn observe_unreliable_sequence(&mut self, sequence: u64) {
        if sequence == self.next_sequence {
            self.next_sequence = sequence.saturating_add(1);
        }
    }
}

#[derive(Debug, Clone)]
struct ReceivedClipboardFrame {
    clipboard: ClipboardText,
    received_at: Instant,
}

#[derive(Debug, Clone)]
struct ReceivedControlFrame {
    message: ControlMessage,
    received_at: Instant,
}

#[derive(Clone, Default)]
struct ListenerLatencyStats {
    inner: Arc<ListenerLatencyStatsInner>,
}

#[derive(Default)]
struct ListenerLatencyStatsInner {
    last_send_to_receive_micros: AtomicUsize,
    max_send_to_receive_micros: AtomicUsize,
    last_receive_to_inject_micros: AtomicUsize,
    max_receive_to_inject_micros: AtomicUsize,
    last_end_to_end_input_micros: AtomicUsize,
    max_end_to_end_input_micros: AtomicUsize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
struct ListenerLatencyStatsSnapshot {
    last_send_to_receive_micros: usize,
    max_send_to_receive_micros: usize,
    last_receive_to_inject_micros: usize,
    max_receive_to_inject_micros: usize,
    last_end_to_end_input_micros: usize,
    max_end_to_end_input_micros: usize,
}

impl ListenerLatencyStats {
    fn record_send_to_receive_micros(&self, micros: u64) {
        let micros = usize::try_from(micros).unwrap_or(usize::MAX);
        self.inner
            .last_send_to_receive_micros
            .store(micros, Ordering::Relaxed);
        self.inner
            .max_send_to_receive_micros
            .fetch_max(micros, Ordering::Relaxed);
    }

    fn record_receive_to_inject_latency(&self, elapsed: Duration) {
        let micros = usize::try_from(elapsed.as_micros()).unwrap_or(usize::MAX);
        self.inner
            .last_receive_to_inject_micros
            .store(micros, Ordering::Relaxed);
        self.inner
            .max_receive_to_inject_micros
            .fetch_max(micros, Ordering::Relaxed);
    }

    fn record_end_to_end_input_micros(&self, micros: u64) {
        let micros = usize::try_from(micros).unwrap_or(usize::MAX);
        self.inner
            .last_end_to_end_input_micros
            .store(micros, Ordering::Relaxed);
        self.inner
            .max_end_to_end_input_micros
            .fetch_max(micros, Ordering::Relaxed);
    }

    #[cfg(test)]
    fn snapshot(&self) -> ListenerLatencyStatsSnapshot {
        ListenerLatencyStatsSnapshot {
            last_send_to_receive_micros: self
                .inner
                .last_send_to_receive_micros
                .load(Ordering::Relaxed),
            max_send_to_receive_micros: self
                .inner
                .max_send_to_receive_micros
                .load(Ordering::Relaxed),
            last_receive_to_inject_micros: self
                .inner
                .last_receive_to_inject_micros
                .load(Ordering::Relaxed),
            max_receive_to_inject_micros: self
                .inner
                .max_receive_to_inject_micros
                .load(Ordering::Relaxed),
            last_end_to_end_input_micros: self
                .inner
                .last_end_to_end_input_micros
                .load(Ordering::Relaxed),
            max_end_to_end_input_micros: self
                .inner
                .max_end_to_end_input_micros
                .load(Ordering::Relaxed),
        }
    }
}

fn receive_remote_frames(
    receiver: &mut impl ProtocolFrameReceiver,
    input_tx: SyncSender<ReceivedInputFrame>,
    clipboard_tx: SyncSender<ReceivedClipboardFrame>,
    control_tx: SyncSender<ReceivedControlFrame>,
    stats: ListenerLatencyStats,
    runtime_config_path: Option<PathBuf>,
) -> Result<(), String> {
    let mut received_events = 0_u64;
    let mut last_progress_write = None;
    loop {
        let frame = receiver.recv_frame()?;
        let received_at = Instant::now();
        match frame.payload {
            ProtocolPayload::Input(_) => {
                let received_wall_micros = now_micros()?;
                stats.record_send_to_receive_micros(
                    received_wall_micros.saturating_sub(frame.timestamp_micros),
                );
                let _queued = queue_received_input_frame(&input_tx, frame, received_at)?;
                received_events = received_events.saturating_add(1);
                if desktop_runtime_progress_write_due(last_progress_write, Instant::now()) {
                    last_progress_write = Some(Instant::now());
                    let timestamp = unix_timestamp_seconds();
                    write_input_listener_desktop_runtime_status_if_configured(
                        runtime_config_path.as_deref(),
                        &desktop_input_listener_progress_runtime_status(received_events, timestamp),
                        "receive progress",
                    );
                }
            }
            ProtocolPayload::ClipboardText(clipboard) => {
                let bytes = clipboard.text.len();
                let clipboard = ReceivedClipboardFrame {
                    clipboard,
                    received_at,
                };
                match clipboard_tx.try_send(clipboard) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        eprintln!("error=clipboard_queue_full event=clipboard_text bytes={bytes}")
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        return Err("clipboard thread is disconnected".to_string());
                    }
                }
            }
            ProtocolPayload::ClipboardFiles(files) => {
                println!("{}", clipboard_files_log_line(&files));
            }
            ProtocolPayload::FileChunk(chunk) => {
                println!("{}", file_chunk_log_line(&chunk));
            }
            ProtocolPayload::Control(message) => {
                let kind = control_message_kind(&message);
                match control_tx.try_send(ReceivedControlFrame {
                    message,
                    received_at,
                }) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        eprintln!("error=control_queue_full event=control kind={kind}");
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        return Err("control thread is disconnected".to_string());
                    }
                }
            }
        }
    }
}

fn queue_received_input_frame(
    input_tx: &SyncSender<ReceivedInputFrame>,
    frame: ProtocolFrame,
    received_at: Instant,
) -> Result<bool, String> {
    let frame = ReceivedInputFrame { frame, received_at };
    if received_frame_is_unreliable_mouse_move(&frame) {
        return match input_tx.try_send(frame) {
            Ok(()) => Ok(true),
            Err(TrySendError::Full(_)) => Ok(false),
            Err(TrySendError::Disconnected(_)) => {
                Err("input injection thread is disconnected".to_string())
            }
        };
    }
    input_tx
        .send(frame)
        .map(|()| true)
        .map_err(|_| "input injection thread is disconnected".to_string())
}

fn received_frame_is_unreliable_mouse_move(received: &ReceivedInputFrame) -> bool {
    let ProtocolPayload::Input(input) = &received.frame.payload else {
        return false;
    };
    input.channel == kmsync_core::InputChannel::InputUnreliable
        && matches!(
            input.event,
            InputEvent::Mouse(kmsync_core::MouseEvent::Move { .. })
        )
}

fn inject_received_frames(
    rx: Receiver<ReceivedInputFrame>,
    adapter: &mut impl InputInjector,
    stats: ListenerLatencyStats,
    runtime_config_path: Option<PathBuf>,
) -> Result<(), String> {
    let mut remote_input = RemoteInputState::default();
    let mut current_target_device_id = None;
    let mut reliable_input = ReliableInputSequencer::new();
    let mut pending_unreliable_input = BTreeMap::new();
    let mut gap_recovery = InputGapRecovery::default();
    let mut injected_events = 0_u64;
    let mut last_progress_write = None;
    loop {
        let received = match gap_recovery.next_action(
            has_pending_input_gap(&pending_unreliable_input, &reliable_input),
            Instant::now(),
        ) {
            InputGapRecoveryAction::Block => match rx.recv() {
                Ok(received) => received,
                Err(_) => break,
            },
            InputGapRecoveryAction::Wait(timeout) => match rx.recv_timeout(timeout) {
                Ok(received) => received,
                Err(RecvTimeoutError::Timeout) => {
                    gap_recovery.reset();
                    recover_pending_input_gap(
                        &mut pending_unreliable_input,
                        &mut reliable_input,
                        adapter,
                        &mut remote_input,
                        &mut current_target_device_id,
                        &stats,
                        runtime_config_path.as_deref(),
                        &mut injected_events,
                        &mut last_progress_write,
                    )?;
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            },
            InputGapRecoveryAction::Recover => {
                recover_pending_input_gap(
                    &mut pending_unreliable_input,
                    &mut reliable_input,
                    adapter,
                    &mut remote_input,
                    &mut current_target_device_id,
                    &stats,
                    runtime_config_path.as_deref(),
                    &mut injected_events,
                    &mut last_progress_write,
                )?;
                continue;
            }
        };

        if is_reliable_input_frame(&received) {
            match reliable_input.accept(received) {
                ReliableInputDecision::Inject(ready) => {
                    inject_received_input_frame_and_record(
                        ready,
                        adapter,
                        &mut remote_input,
                        &mut current_target_device_id,
                        &stats,
                        runtime_config_path.as_deref(),
                        &mut injected_events,
                        &mut last_progress_write,
                    )?;
                    inject_ready_pending_input_frames(
                        &mut pending_unreliable_input,
                        &mut reliable_input,
                        adapter,
                        &mut remote_input,
                        &mut current_target_device_id,
                        &stats,
                        runtime_config_path.as_deref(),
                        &mut injected_events,
                        &mut last_progress_write,
                    )?;
                }
                ReliableInputDecision::Recover(ready) => {
                    release_tracked_input(adapter, &mut remote_input)?;
                    inject_received_input_frame_and_record(
                        ready,
                        adapter,
                        &mut remote_input,
                        &mut current_target_device_id,
                        &stats,
                        runtime_config_path.as_deref(),
                        &mut injected_events,
                        &mut last_progress_write,
                    )?;
                    inject_ready_pending_input_frames(
                        &mut pending_unreliable_input,
                        &mut reliable_input,
                        adapter,
                        &mut remote_input,
                        &mut current_target_device_id,
                        &stats,
                        runtime_config_path.as_deref(),
                        &mut injected_events,
                        &mut last_progress_write,
                    )?;
                }
                ReliableInputDecision::Buffer | ReliableInputDecision::Drop => {}
            }
        } else {
            let sequence = received.frame.sequence;
            match sequence.cmp(&reliable_input.next_sequence()) {
                std::cmp::Ordering::Less => continue,
                std::cmp::Ordering::Greater => {
                    pending_unreliable_input.entry(sequence).or_insert(received);
                    continue;
                }
                std::cmp::Ordering::Equal => {}
            }
            inject_unreliable_input_frame_and_record(
                received,
                adapter,
                &mut remote_input,
                &mut current_target_device_id,
                &stats,
                runtime_config_path.as_deref(),
                &mut reliable_input,
                &mut injected_events,
                &mut last_progress_write,
            )?;
            inject_ready_pending_input_frames(
                &mut pending_unreliable_input,
                &mut reliable_input,
                adapter,
                &mut remote_input,
                &mut current_target_device_id,
                &stats,
                runtime_config_path.as_deref(),
                &mut injected_events,
                &mut last_progress_write,
            )?;
        }
    }
    while recover_pending_input_gap(
        &mut pending_unreliable_input,
        &mut reliable_input,
        adapter,
        &mut remote_input,
        &mut current_target_device_id,
        &stats,
        runtime_config_path.as_deref(),
        &mut injected_events,
        &mut last_progress_write,
    )? {}
    release_tracked_input(adapter, &mut remote_input)
}

fn is_reliable_input_frame(received: &ReceivedInputFrame) -> bool {
    received.frame.payload.transport_lane() == TransportLane::InputReliable
}

fn has_pending_input_gap(
    pending_unreliable_input: &BTreeMap<u64, ReceivedInputFrame>,
    reliable_input: &ReliableInputSequencer,
) -> bool {
    next_pending_input_sequence(pending_unreliable_input, reliable_input)
        .is_some_and(|sequence| sequence > reliable_input.next_sequence())
}

fn next_pending_input_sequence(
    pending_unreliable_input: &BTreeMap<u64, ReceivedInputFrame>,
    reliable_input: &ReliableInputSequencer,
) -> Option<u64> {
    match (
        pending_unreliable_input
            .first_key_value()
            .map(|(&sequence, _)| sequence),
        reliable_input.next_pending_sequence(),
    ) {
        (Some(unreliable), Some(reliable)) => Some(unreliable.min(reliable)),
        (Some(unreliable), None) => Some(unreliable),
        (None, Some(reliable)) => Some(reliable),
        (None, None) => None,
    }
}

fn recover_pending_input_gap(
    pending_unreliable_input: &mut BTreeMap<u64, ReceivedInputFrame>,
    reliable_input: &mut ReliableInputSequencer,
    adapter: &mut impl InputInjector,
    remote_input: &mut RemoteInputState,
    current_target_device_id: &mut Option<DeviceId>,
    stats: &ListenerLatencyStats,
    runtime_config_path: Option<&Path>,
    injected_events: &mut u64,
    last_progress_write: &mut Option<Instant>,
) -> Result<bool, String> {
    let Some(sequence) = next_pending_input_sequence(pending_unreliable_input, reliable_input)
    else {
        return Ok(false);
    };
    reliable_input.skip_to_sequence(sequence);
    inject_ready_pending_input_frames(
        pending_unreliable_input,
        reliable_input,
        adapter,
        remote_input,
        current_target_device_id,
        stats,
        runtime_config_path,
        injected_events,
        last_progress_write,
    )?;
    Ok(true)
}

fn inject_ready_pending_input_frames(
    pending_unreliable_input: &mut BTreeMap<u64, ReceivedInputFrame>,
    reliable_input: &mut ReliableInputSequencer,
    adapter: &mut impl InputInjector,
    remote_input: &mut RemoteInputState,
    current_target_device_id: &mut Option<DeviceId>,
    stats: &ListenerLatencyStats,
    runtime_config_path: Option<&Path>,
    injected_events: &mut u64,
    last_progress_write: &mut Option<Instant>,
) -> Result<(), String> {
    loop {
        if let Some(ready) = pending_unreliable_input.remove(&reliable_input.next_sequence()) {
            inject_unreliable_input_frame_and_record(
                ready,
                adapter,
                remote_input,
                current_target_device_id,
                stats,
                runtime_config_path,
                reliable_input,
                injected_events,
                last_progress_write,
            )?;
            continue;
        }
        if let Some(ready) = reliable_input.pop_ready() {
            inject_received_input_frame_and_record(
                ready,
                adapter,
                remote_input,
                current_target_device_id,
                stats,
                runtime_config_path,
                injected_events,
                last_progress_write,
            )?;
            continue;
        }
        break;
    }
    Ok(())
}

fn inject_unreliable_input_frame_and_record(
    received: ReceivedInputFrame,
    adapter: &mut impl InputInjector,
    remote_input: &mut RemoteInputState,
    current_target_device_id: &mut Option<DeviceId>,
    stats: &ListenerLatencyStats,
    runtime_config_path: Option<&Path>,
    reliable_input: &mut ReliableInputSequencer,
    injected_events: &mut u64,
    last_progress_write: &mut Option<Instant>,
) -> Result<(), String> {
    let sequence = received.frame.sequence;
    inject_received_input_frame_and_record(
        received,
        adapter,
        remote_input,
        current_target_device_id,
        stats,
        runtime_config_path,
        injected_events,
        last_progress_write,
    )?;
    reliable_input.observe_unreliable_sequence(sequence);
    Ok(())
}

fn inject_received_input_frame_and_record(
    received: ReceivedInputFrame,
    adapter: &mut impl InputInjector,
    remote_input: &mut RemoteInputState,
    current_target_device_id: &mut Option<DeviceId>,
    stats: &ListenerLatencyStats,
    runtime_config_path: Option<&Path>,
    injected_events: &mut u64,
    last_progress_write: &mut Option<Instant>,
) -> Result<(), String> {
    if stale_received_unreliable_mouse_move(&received, Instant::now()) {
        return Ok(());
    }
    inject_received_input_frame(
        received,
        adapter,
        remote_input,
        current_target_device_id,
        stats,
    )?;
    *injected_events = injected_events.saturating_add(1);
    if desktop_runtime_progress_write_due(*last_progress_write, Instant::now()) {
        *last_progress_write = Some(Instant::now());
        let timestamp = unix_timestamp_seconds();
        write_input_listener_desktop_runtime_status_if_configured(
            runtime_config_path,
            &desktop_input_injection_progress_runtime_status(*injected_events, timestamp),
            "injection progress",
        );
    }
    Ok(())
}

fn stale_received_unreliable_mouse_move(received: &ReceivedInputFrame, now: Instant) -> bool {
    let ProtocolPayload::Input(input) = &received.frame.payload else {
        return false;
    };
    input.channel == kmsync_core::InputChannel::InputUnreliable
        && matches!(
            input.event,
            InputEvent::Mouse(kmsync_core::MouseEvent::Move { .. })
        )
        && now.duration_since(received.received_at) >= DESKTOP_STALE_MOUSE_MOVE_DROP_AFTER
}

fn inject_received_input_frame(
    received: ReceivedInputFrame,
    adapter: &mut impl InputInjector,
    remote_input: &mut RemoteInputState,
    current_target_device_id: &mut Option<DeviceId>,
    stats: &ListenerLatencyStats,
) -> Result<(), String> {
    stats.record_receive_to_inject_latency(received.received_at.elapsed());
    match received.frame.payload {
        ProtocolPayload::Input(input) => {
            let now = now_micros()?;
            stats.record_end_to_end_input_micros(
                now.saturating_sub(received.frame.timestamp_micros),
            );
            release_on_target_change(
                adapter,
                remote_input,
                current_target_device_id,
                input.target_device_id,
            )?;
            inject_or_release_on_error(adapter, remote_input, input.event)
        }
        ProtocolPayload::ClipboardText(_)
        | ProtocolPayload::ClipboardFiles(_)
        | ProtocolPayload::FileChunk(_)
        | ProtocolPayload::Control(_) => Ok(()),
    }
}

fn handle_control_frames(rx: Receiver<ReceivedControlFrame>) -> Result<(), String> {
    for frame in rx {
        let _receive_to_control = frame.received_at.elapsed();
        println!("{}", control_log_line(&frame.message));
    }
    Ok(())
}

fn apply_clipboard_frames(
    rx: Receiver<ReceivedClipboardFrame>,
    adapter: &mut impl ClipboardBackend,
) -> Result<(), String> {
    let mut state = ClipboardSyncState::new(local_clipboard_source_id());
    let policy = ClipboardSyncPolicy::default();
    apply_clipboard_frames_with_state(rx, adapter, &mut state, &policy)
}

fn apply_clipboard_frames_with_state(
    rx: Receiver<ReceivedClipboardFrame>,
    adapter: &mut impl ClipboardBackend,
    state: &mut ClipboardSyncState,
    policy: &ClipboardSyncPolicy,
) -> Result<(), String> {
    for frame in rx {
        let _receive_to_clipboard = frame.received_at.elapsed();
        println!("{}", clipboard_log_line(&frame));
        if let Err(reason) =
            policy.check_remote(&frame.clipboard, frame.received_at, Instant::now())
        {
            println!(
                "skipped event=clipboard_text reason={} bytes={}",
                reason.as_str(),
                clipboard_content_bytes(&frame.clipboard)
            );
            continue;
        }
        if state.should_apply_remote(&frame.clipboard) {
            adapter.set_clipboard_content(&frame.clipboard)?;
            state.mark_applied_remote(&frame.clipboard);
        }
    }
    Ok(())
}

fn clipboard_log_line(frame: &ReceivedClipboardFrame) -> String {
    format!(
        "received event=clipboard_text bytes={} source={} version={} hash={}",
        clipboard_content_bytes(&frame.clipboard),
        frame.clipboard.source_id,
        frame.clipboard.version,
        frame.clipboard.content_hash
    )
}

fn clipboard_files_log_line(files: &ClipboardFiles) -> String {
    format!(
        "received event=clipboard_files files={} bytes={} source={} version={} hash={}",
        files.files.len(),
        clipboard_files_total_bytes(files),
        files.source_id,
        files.version,
        files.content_hash
    )
}

fn file_chunk_log_line(chunk: &FileTransferChunk) -> String {
    format!(
        "received event=file_chunk transfer={} file_index={} chunk={} offset={} total_bytes={} bytes={} final={}",
        chunk.transfer_id,
        chunk.file_index,
        chunk.chunk_index,
        chunk.offset,
        chunk.total_size,
        chunk.data.len(),
        chunk.is_final
    )
}

fn control_log_line(message: &ControlMessage) -> String {
    match message {
        ControlMessage::Heartbeat {
            source_device_id,
            session_id,
            sequence,
        } => format!(
            "received event=control kind=heartbeat source={source_device_id} session={session_id} sequence={sequence}"
        ),
        ControlMessage::Capabilities {
            source_device_id,
            protocol,
            channels,
        } => format!(
            "received event=control kind=capabilities source={source_device_id} protocol_min={} protocol_max={} input_unreliable={} input_reliable={} clipboard={} control={}",
            protocol.min,
            protocol.max,
            channels.input_unreliable,
            channels.input_reliable,
            channels.clipboard,
            channels.control
        ),
        ControlMessage::ConfigVersion {
            source_device_id,
            version,
        } => format!(
            "received event=control kind=config_version source={source_device_id} version={version}"
        ),
        ControlMessage::SessionState {
            source_device_id,
            session_id,
            state,
        } => format!(
            "received event=control kind=session_state source={source_device_id} session={session_id} state={state:?}"
        ),
    }
}

const fn control_message_kind(message: &ControlMessage) -> &'static str {
    match message {
        ControlMessage::Heartbeat { .. } => "heartbeat",
        ControlMessage::Capabilities { .. } => "capabilities",
        ControlMessage::ConfigVersion { .. } => "config_version",
        ControlMessage::SessionState { .. } => "session_state",
    }
}

#[cfg(test)]
fn listener_log_line(frame: &ProtocolFrame) -> Option<String> {
    match &frame.payload {
        ProtocolPayload::Input(_) => None,
        ProtocolPayload::ClipboardText(clipboard) => Some(format!(
            "received event=clipboard_text bytes={} source={} version={} hash={}",
            clipboard_content_bytes(clipboard),
            clipboard.source_id,
            clipboard.version,
            clipboard.content_hash
        )),
        ProtocolPayload::ClipboardFiles(files) => Some(clipboard_files_log_line(files)),
        ProtocolPayload::FileChunk(chunk) => Some(file_chunk_log_line(chunk)),
        ProtocolPayload::Control(message) => Some(control_log_line(message)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClipboardSignature {
    source_id: DeviceId,
    version: u64,
    content_hash: u64,
}

impl ClipboardSignature {
    fn new(clipboard: &ClipboardText) -> Self {
        Self {
            source_id: clipboard.source_id,
            version: clipboard.version,
            content_hash: clipboard.content_hash,
        }
    }
}

#[derive(Debug, Clone)]
struct ClipboardSyncState {
    local_source_id: DeviceId,
    next_version: u64,
    last_applied_remote: Option<ClipboardSignature>,
}

impl ClipboardSyncState {
    const fn new(local_source_id: DeviceId) -> Self {
        Self {
            local_source_id,
            next_version: 1,
            last_applied_remote: None,
        }
    }

    fn next_local_content(&mut self, clipboard: ClipboardText) -> ClipboardText {
        let version = self.next_version;
        self.next_version = self.next_version.saturating_add(1);
        clipboard.with_source_version(self.local_source_id, version)
    }

    #[cfg(test)]
    fn should_send_local_text(&self, text: &str) -> bool {
        self.should_send_local_content(&ClipboardText::from_local_text(0, 0, text.to_string()))
    }

    fn should_send_local_content(&self, clipboard: &ClipboardText) -> bool {
        let content_hash = clipboard.content_hash;
        !matches!(
            self.last_applied_remote,
            Some(signature) if signature.content_hash == content_hash
        )
    }

    fn should_apply_remote(&self, clipboard: &ClipboardText) -> bool {
        clipboard.source_id != self.local_source_id
            && self.last_applied_remote != Some(ClipboardSignature::new(clipboard))
    }

    fn mark_applied_remote(&mut self, clipboard: &ClipboardText) {
        self.last_applied_remote = Some(ClipboardSignature::new(clipboard));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClipboardSyncPolicy {
    enabled: bool,
    max_bytes: usize,
    ttl: Duration,
    sensitive_app_blacklist: Vec<String>,
}

const DEFAULT_SENSITIVE_CLIPBOARD_APPS: &[&str] = &[
    "1password",
    "bitwarden",
    "keepass",
    "keepassxc",
    "lastpass",
    "dashlane",
    "keeper",
    "enpass",
    "proton pass",
    "protonpass",
];

impl Default for ClipboardSyncPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            max_bytes: 1024 * 1024,
            ttl: Duration::from_secs(5 * 60),
            sensitive_app_blacklist: DEFAULT_SENSITIVE_CLIPBOARD_APPS
                .iter()
                .map(|app| (*app).to_string())
                .collect(),
        }
    }
}

impl ClipboardSyncPolicy {
    fn check_local(
        &self,
        clipboard: &ClipboardText,
        source_app: Option<&str>,
        captured_at: Instant,
        now: Instant,
    ) -> Result<(), ClipboardPolicyBlock> {
        self.check_content(clipboard, captured_at, now)?;
        if let Some(source_app) = source_app {
            if self
                .sensitive_app_blacklist
                .iter()
                .any(|blocked| contains_case_insensitive(source_app, blocked))
            {
                return Err(ClipboardPolicyBlock::SensitiveApp);
            }
        }
        Ok(())
    }

    fn check_remote(
        &self,
        clipboard: &ClipboardText,
        received_at: Instant,
        now: Instant,
    ) -> Result<(), ClipboardPolicyBlock> {
        self.check_content(clipboard, received_at, now)
    }

    fn check_content(
        &self,
        clipboard: &ClipboardText,
        captured_at: Instant,
        now: Instant,
    ) -> Result<(), ClipboardPolicyBlock> {
        if !self.enabled {
            return Err(ClipboardPolicyBlock::SyncDisabled);
        }

        let bytes = clipboard_content_bytes(clipboard);
        if bytes > self.max_bytes {
            return Err(ClipboardPolicyBlock::TooLarge {
                bytes,
                max_bytes: self.max_bytes,
            });
        }

        if now.saturating_duration_since(captured_at) > self.ttl {
            return Err(ClipboardPolicyBlock::Expired);
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipboardPolicyBlock {
    SyncDisabled,
    TooLarge { bytes: usize, max_bytes: usize },
    Expired,
    SensitiveApp,
}

impl ClipboardPolicyBlock {
    const fn as_str(self) -> &'static str {
        match self {
            Self::SyncDisabled => "sync_disabled",
            Self::TooLarge { .. } => "too_large",
            Self::Expired => "expired",
            Self::SensitiveApp => "sensitive_app",
        }
    }
}

fn clipboard_content_bytes(clipboard: &ClipboardText) -> usize {
    clipboard
        .text
        .len()
        .saturating_add(clipboard.html.as_ref().map_or(0, String::len))
        .saturating_add(clipboard.image.as_ref().map_or(0, |image| image.rgba.len()))
}

fn clipboard_files_total_bytes(files: &ClipboardFiles) -> u64 {
    files
        .files
        .iter()
        .fold(0_u64, |total, file| total.saturating_add(file.byte_len))
}

fn local_clipboard_source_id() -> DeviceId {
    static SOURCE_ID: OnceLock<DeviceId> = OnceLock::new();
    *SOURCE_ID.get_or_init(|| {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        timestamp ^ u128::from(std::process::id())
    })
}

fn inject_or_release_on_error(
    adapter: &mut impl InputInjector,
    remote_input: &mut RemoteInputState,
    event: InputEvent,
) -> Result<(), String> {
    match adapter.inject(event) {
        Ok(()) => {
            remote_input.apply(event);
            Ok(())
        }
        Err(error) => release_error_or(adapter, remote_input, error),
    }
}

fn release_error_or(
    adapter: &mut impl InputInjector,
    remote_input: &mut RemoteInputState,
    error: String,
) -> Result<(), String> {
    let release_error = release_tracked_input(adapter, remote_input).err();
    if let Some(release_error) = release_error {
        Err(format!(
            "{error}; failed to release remote input: {release_error}"
        ))
    } else {
        Err(error)
    }
}

fn release_on_target_change(
    adapter: &mut impl InputInjector,
    remote_input: &mut RemoteInputState,
    current_target_device_id: &mut Option<DeviceId>,
    next_target_device_id: DeviceId,
) -> Result<(), String> {
    if matches!(*current_target_device_id, Some(current) if current != next_target_device_id) {
        release_tracked_input(adapter, remote_input)?;
    }
    *current_target_device_id = Some(next_target_device_id);
    Ok(())
}

fn release_tracked_input(
    adapter: &mut impl InputInjector,
    remote_input: &mut RemoteInputState,
) -> Result<(), String> {
    let mut first_error = None;
    for event in remote_input.release_all() {
        if let Err(error) = adapter.inject(event) {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn run_send_demo(target: SocketAddr, profile_name: ProfileName) -> Result<(), String> {
    let profile = profile_name.profile();
    let compiled = CompiledProfile::compile(&profile).map_err(|error| format!("{error:?}"))?;
    let mut sender = QuicEventSender::connect(target)?;

    let events = [
        InputEvent::Key(KeyEvent {
            key: Key::C,
            state: KeyState::Pressed,
            modifiers: match profile_name {
                ProfileName::MacToWindows => Modifiers::META,
                ProfileName::WindowsToMac => Modifiers::CONTROL,
            },
        }),
        InputEvent::Key(KeyEvent {
            key: Key::C,
            state: KeyState::Released,
            modifiers: Modifiers::NONE,
        }),
        InputEvent::Scroll(kmsync_core::ScrollEvent { dx: 0.0, dy: 4.0 }),
    ];

    for (index, event) in events.into_iter().enumerate() {
        let mapped = compiled.transform(event);
        sender.send(ProtocolEvent {
            sequence: u64::try_from(index + 1).map_err(|_| "sequence overflow".to_string())?,
            timestamp_micros: now_micros()?,
            event: mapped,
        })?;
    }

    println!("sent demo events to {target}");
    Ok(())
}

fn run_target_probe(config_path: &Path, target_device_id: &str) -> Result<(), String> {
    match send_target_probe_frame(config_path, target_device_id) {
        Ok(transport_label) => {
            println!(
                "target_probe=ok target_device_id={} transport={} payload=control",
                target_device_id, transport_label
            );
            Ok(())
        }
        Err(error) => Err(error.message),
    }
}

fn run_target_input_test(config_path: &Path, target_device_id: &str) -> Result<(), String> {
    match send_target_input_test_frame(config_path, target_device_id) {
        Ok(transport_label) => {
            println!(
                "target_input_test=ok target_device_id={} transport={} payload=input_reliable_noop_scroll",
                target_device_id, transport_label
            );
            Ok(())
        }
        Err(error) => Err(error.message),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetProbeFailure {
    transport: Option<String>,
    message: String,
}

fn send_target_probe_frame(
    config_path: &Path,
    target_device_id: &str,
) -> Result<String, TargetProbeFailure> {
    let config = client::ClientConfig::load(config_path).map_err(target_probe_setup_failure)?;
    let identity = client::DeviceIdentity::load_or_generate(&config.identity_path)
        .map_err(target_probe_setup_failure)?;
    let source_device_id =
        protocol_device_id_from_uuid(&identity.device_id).map_err(target_probe_setup_failure)?;
    let _target_device_id =
        protocol_device_id_from_uuid(target_device_id).map_err(target_probe_setup_failure)?;
    let mut transport =
        connect_desktop_target_transport(config, &identity.device_id, target_device_id).map_err(
            |error| TargetProbeFailure {
                transport: None,
                message: format!("target probe connect failed: {error}"),
            },
        )?;
    let transport_label = transport.label().to_string();
    let frame = target_probe_control_frame(
        source_device_id,
        1,
        now_micros().map_err(target_probe_setup_failure)?,
    );
    transport
        .send_frame(target_device_id, &frame)
        .map_err(|error| TargetProbeFailure {
            transport: Some(transport_label.clone()),
            message: format!("target probe send failed via {transport_label}: {error}"),
        })?;
    Ok(transport_label)
}

fn send_target_input_test_frame(
    config_path: &Path,
    target_device_id: &str,
) -> Result<String, TargetProbeFailure> {
    let config = client::ClientConfig::load(config_path).map_err(target_probe_setup_failure)?;
    let identity = client::DeviceIdentity::load_or_generate(&config.identity_path)
        .map_err(target_probe_setup_failure)?;
    let source_device_id =
        protocol_device_id_from_uuid(&identity.device_id).map_err(target_probe_setup_failure)?;
    let target_protocol_device_id =
        protocol_device_id_from_uuid(target_device_id).map_err(target_probe_setup_failure)?;
    let mut transport =
        connect_desktop_target_transport(config, &identity.device_id, target_device_id).map_err(
            |error| TargetProbeFailure {
                transport: None,
                message: format!("target input test connect failed: {error}"),
            },
        )?;
    let transport_label = transport.label().to_string();
    let frame = target_input_test_frame(
        source_device_id,
        target_protocol_device_id,
        1,
        now_micros().map_err(target_probe_setup_failure)?,
    );
    transport
        .send_frame(target_device_id, &frame)
        .map_err(|error| TargetProbeFailure {
            transport: Some(transport_label.clone()),
            message: format!("target input test send failed via {transport_label}: {error}"),
        })?;
    Ok(transport_label)
}

fn target_probe_setup_failure(error: String) -> TargetProbeFailure {
    TargetProbeFailure {
        transport: None,
        message: error,
    }
}

fn target_probe_control_frame(
    source_device_id: DeviceId,
    sequence: u64,
    timestamp_micros: u64,
) -> ProtocolFrame {
    ProtocolFrame {
        sequence,
        timestamp_micros,
        payload: ProtocolPayload::Control(ControlMessage::heartbeat(
            source_device_id,
            target_probe_session_id(source_device_id),
            sequence,
        )),
    }
}

fn target_input_test_frame(
    source_device_id: DeviceId,
    target_device_id: DeviceId,
    sequence: u64,
    timestamp_micros: u64,
) -> ProtocolFrame {
    ProtocolFrame {
        sequence,
        timestamp_micros,
        payload: ProtocolPayload::Input(InputEventEnvelope::current(
            source_device_id,
            target_device_id,
            InputEvent::Scroll(kmsync_core::ScrollEvent { dx: 0.0, dy: 0.0 }),
        )),
    }
}

const fn target_probe_session_id(source_device_id: DeviceId) -> u128 {
    source_device_id ^ 0x6b_6d_73_79_6e_63_5f_74_61_72_67_65_74_70_72_6f_u128
}

fn run_capture_send(
    target: SocketAddr,
    profile_name: ProfileName,
    mode: CaptureMode,
    application_exceptions: ApplicationExceptionRules,
    reconnect_count: u64,
) -> Result<(), String> {
    let profile = profile_name.profile();
    let compiled = CompiledProfile::compile(&profile).map_err(|error| format!("{error:?}"))?;
    let sender = QuicEventSender::connect(target)?;
    let mut platform = platform::current_platform();
    let display_layout = platform.display_layout();
    let mut router = CaptureRouter::with_display_layout_and_exceptions(
        mode,
        display_layout,
        application_exceptions,
    );
    let (tx, rx) = sync_channel(1024);
    let queue_stats = CaptureQueueStats::default();
    let local_pointer_hidden = Arc::new(AtomicBool::new(false));
    let metrics_reporter = RuntimeMetricsReporter::start(
        queue_stats.clone(),
        reconnect_count,
        METRICS_REPORT_INTERVAL,
    );
    let tx_queue_stats = queue_stats.clone();
    let tx_thread = thread::spawn(move || {
        let mut sender = sender;
        transmit_captured_events(rx, &mut sender, compiled, tx_queue_stats)
    });

    println!("capturing local input and sending mapped events to {target}");
    println!("profile: {profile_name:?}");
    println!("mode: {}", router.describe());
    let capture_local_pointer_hidden = Arc::clone(&local_pointer_hidden);
    let capture_result = platform.capture_loop(move |captured| {
        let application_id = if router.has_application_exceptions() {
            platform::active_application_id()
        } else {
            None
        };
        let route = enqueue_routed_capture_with_application(
            &tx,
            &queue_stats,
            &mut router,
            captured,
            application_id.as_deref(),
        );
        apply_local_pointer_action(route.local_pointer_action, &capture_local_pointer_hidden);
        route.decision
    });
    restore_local_pointer_if_hidden(&local_pointer_hidden, None);
    drop(metrics_reporter);
    if let Err(error) = capture_result {
        return Err(error);
    }
    match tx_thread.join() {
        Ok(result) => result,
        Err(_) => Err("capture transmit thread panicked".to_string()),
    }
}

fn run_capture_connect(
    config_path: PathBuf,
    target_device_id: String,
    profile_name: ProfileName,
    mode: CaptureMode,
    application_exceptions: ApplicationExceptionRules,
) -> Result<(), String> {
    let mut reconnect_state = client::DirectLanReconnectState::default();
    loop {
        let connection = match refresh_capture_connect_connection(
            &config_path,
            &target_device_id,
            &mut reconnect_state,
        ) {
            Ok(Some(connection)) => connection,
            Ok(None) => {
                return Err("direct LAN connection refresh did not select a candidate".to_string());
            }
            Err(error) if is_retryable_connection_error(&error) => {
                eprintln!("direct LAN connection unavailable; rediscovering: {error}");
                thread::sleep(CAPTURE_CONNECT_RETRY_DELAY);
                continue;
            }
            Err(error) => return Err(error),
        };

        println!(
            "direct LAN connection selected {:?} {} for {} reason={:?} reconnect_count={}",
            connection.attempt.candidate.kind,
            connection.attempt.address,
            target_device_id,
            connection.reason,
            connection.reconnect_count
        );

        match run_capture_send(
            connection.attempt.address,
            profile_name,
            mode,
            application_exceptions.clone(),
            connection.reconnect_count,
        ) {
            Ok(()) => return Ok(()),
            Err(error) if is_retryable_connection_error(&error) => {
                eprintln!("direct LAN connection lost; rediscovering: {error}");
                thread::sleep(CAPTURE_CONNECT_RETRY_DELAY);
            }
            Err(error) => return Err(error),
        }
    }
}

fn refresh_capture_connect_connection(
    config_path: &Path,
    target_device_id: &str,
    reconnect_state: &mut client::DirectLanReconnectState,
) -> Result<Option<client::DirectLanReconnectOutcome>, String> {
    let config = client::ClientConfig::load(config_path)?;
    client::refresh_target_direct_lan_connection(config, target_device_id, reconnect_state, false)
}

fn is_retryable_connection_error(error: &str) -> bool {
    diagnostic_for_error(error).kind == DiagnosticKind::ConnectionFailed
}

trait ProtocolEventSender {
    fn send_event(&mut self, event: ProtocolEvent) -> Result<(), String>;
}

impl ProtocolEventSender for QuicEventSender {
    fn send_event(&mut self, event: ProtocolEvent) -> Result<(), String> {
        self.send(event)
    }
}

#[derive(Clone, Default)]
struct CaptureQueueStats {
    inner: Arc<CaptureQueueStatsInner>,
}

#[derive(Default)]
struct CaptureQueueStatsInner {
    current_depth: AtomicUsize,
    enqueued: AtomicUsize,
    dropped_full: AtomicUsize,
    dropped_disconnected: AtomicUsize,
    last_capture_to_send_micros: AtomicUsize,
    max_capture_to_send_micros: AtomicUsize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CaptureQueueStatsSnapshot {
    current_depth: usize,
    enqueued: usize,
    dropped_full: usize,
    dropped_disconnected: usize,
    last_capture_to_send_micros: usize,
    max_capture_to_send_micros: usize,
}

impl CaptureQueueStats {
    fn record_enqueue_reserved(&self) {
        self.inner.current_depth.fetch_add(1, Ordering::Relaxed);
    }

    fn record_enqueue_committed(&self) {
        self.inner.enqueued.fetch_add(1, Ordering::Relaxed);
    }

    fn record_enqueue_canceled(&self) {
        self.record_dequeued();
    }

    fn record_dequeued(&self) {
        let _ = self.inner.current_depth.fetch_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |current| current.checked_sub(1),
        );
    }

    fn record_dropped_full(&self) {
        self.inner.dropped_full.fetch_add(1, Ordering::Relaxed);
    }

    fn record_dropped_disconnected(&self) {
        self.inner
            .dropped_disconnected
            .fetch_add(1, Ordering::Relaxed);
    }

    fn record_capture_to_send_latency(&self, elapsed: Duration) {
        let micros = usize::try_from(elapsed.as_micros()).unwrap_or(usize::MAX);
        self.inner
            .last_capture_to_send_micros
            .store(micros, Ordering::Relaxed);
        self.inner
            .max_capture_to_send_micros
            .fetch_max(micros, Ordering::Relaxed);
    }

    fn snapshot(&self) -> CaptureQueueStatsSnapshot {
        CaptureQueueStatsSnapshot {
            current_depth: self.inner.current_depth.load(Ordering::Relaxed),
            enqueued: self.inner.enqueued.load(Ordering::Relaxed),
            dropped_full: self.inner.dropped_full.load(Ordering::Relaxed),
            dropped_disconnected: self.inner.dropped_disconnected.load(Ordering::Relaxed),
            last_capture_to_send_micros: self
                .inner
                .last_capture_to_send_micros
                .load(Ordering::Relaxed),
            max_capture_to_send_micros: self
                .inner
                .max_capture_to_send_micros
                .load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessResourceMetrics {
    cpu_total_micros: u64,
    memory_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeMetricsSnapshot {
    input_queue_depth: usize,
    input_queue_enqueued: usize,
    input_queue_dropped_total: usize,
    input_queue_drop_rate_ppm: usize,
    input_queue_last_capture_to_send_micros: usize,
    input_queue_max_capture_to_send_micros: usize,
    reconnect_count: u64,
    process_cpu_total_micros: Option<u64>,
    process_memory_bytes: Option<u64>,
}

impl RuntimeMetricsSnapshot {
    fn collect(input_queue: &CaptureQueueStats, reconnect_count: u64) -> Self {
        Self::from_parts(
            input_queue.snapshot(),
            reconnect_count,
            sample_process_resource_metrics(),
        )
    }

    const fn from_parts(
        input_queue: CaptureQueueStatsSnapshot,
        reconnect_count: u64,
        resources: Option<ProcessResourceMetrics>,
    ) -> Self {
        let dropped_total = input_queue
            .dropped_full
            .saturating_add(input_queue.dropped_disconnected);
        let total_events = input_queue.enqueued.saturating_add(dropped_total);
        let drop_rate = drop_rate_ppm(dropped_total, total_events);

        Self {
            input_queue_depth: input_queue.current_depth,
            input_queue_enqueued: input_queue.enqueued,
            input_queue_dropped_total: dropped_total,
            input_queue_drop_rate_ppm: drop_rate,
            input_queue_last_capture_to_send_micros: input_queue.last_capture_to_send_micros,
            input_queue_max_capture_to_send_micros: input_queue.max_capture_to_send_micros,
            reconnect_count,
            process_cpu_total_micros: match resources {
                Some(metrics) => Some(metrics.cpu_total_micros),
                None => None,
            },
            process_memory_bytes: match resources {
                Some(metrics) => Some(metrics.memory_bytes),
                None => None,
            },
        }
    }
}

const fn drop_rate_ppm(dropped: usize, total: usize) -> usize {
    if total == 0 {
        return 0;
    }
    ((dropped as u128 * 1_000_000) / total as u128) as usize
}

struct RuntimeMetricsReporter {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl RuntimeMetricsReporter {
    fn start(input_queue: CaptureQueueStats, reconnect_count: u64, interval: Duration) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                thread::sleep(interval);
                if thread_stop.load(Ordering::Relaxed) {
                    break;
                }
                log_runtime_metrics(RuntimeMetricsSnapshot::collect(
                    &input_queue,
                    reconnect_count,
                ));
            }
        });

        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for RuntimeMetricsReporter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn log_runtime_metrics(metrics: RuntimeMetricsSnapshot) {
    eprintln!("{}", runtime_metrics_log_line(metrics));
}

fn runtime_metrics_log_line(metrics: RuntimeMetricsSnapshot) -> String {
    format!(
        "metric=runtime input_queue_depth={} input_queue_enqueued={} \
         input_queue_dropped_total={} input_queue_drop_rate_ppm={} \
         input_queue_last_capture_to_send_micros={} \
         input_queue_max_capture_to_send_micros={} reconnect_count={} \
         process_cpu_total_micros={} process_memory_bytes={}",
        metrics.input_queue_depth,
        metrics.input_queue_enqueued,
        metrics.input_queue_dropped_total,
        metrics.input_queue_drop_rate_ppm,
        metrics.input_queue_last_capture_to_send_micros,
        metrics.input_queue_max_capture_to_send_micros,
        metrics.reconnect_count,
        optional_metric_value(metrics.process_cpu_total_micros),
        optional_metric_value(metrics.process_memory_bytes)
    )
}

fn optional_metric_value(value: Option<u64>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "unknown".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CrashReport {
    timestamp_millis: u128,
    app_version: &'static str,
    os: &'static str,
    arch: &'static str,
    panic_payload_kind: &'static str,
    location: Option<CrashReportLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CrashReportLocation {
    file_name: String,
    line: u32,
    column: u32,
}

static CRASH_REPORT_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

fn install_crash_report_hook() {
    let report_dir = crash_report_dir();
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let location = panic_info
            .location()
            .map(|location| (location.file(), location.line(), location.column()));
        let report =
            crash_report_from_panic_parts(unix_timestamp_millis(), panic_info.payload(), location);
        if let Err(error) = write_crash_report(&report_dir, &report) {
            eprintln!("event=crash_report_write_failed error={error}");
        }
        previous_hook(panic_info);
    }));
}

fn crash_report_dir() -> PathBuf {
    if let Some(dir) = env::var_os("KMSYNC_CRASH_REPORT_DIR") {
        return PathBuf::from(dir);
    }

    let base = env::var_os("KMSYNC_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("kmsync"));
    base.join("crash-reports")
}

fn unix_timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn unix_timestamp_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn crash_report_from_panic_parts(
    timestamp_millis: u128,
    payload: &(dyn std::any::Any + Send),
    location: Option<(&str, u32, u32)>,
) -> CrashReport {
    CrashReport {
        timestamp_millis,
        app_version: env!("CARGO_PKG_VERSION"),
        os: env::consts::OS,
        arch: env::consts::ARCH,
        panic_payload_kind: panic_payload_kind(payload),
        location: location.map(|(file, line, column)| CrashReportLocation {
            file_name: crash_report_file_name(file),
            line,
            column,
        }),
    }
}

fn panic_payload_kind(payload: &(dyn std::any::Any + Send)) -> &'static str {
    if payload.is::<&'static str>() {
        "str"
    } else if payload.is::<String>() {
        "string"
    } else {
        "unknown"
    }
}

fn crash_report_file_name(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

fn render_crash_report(report: &CrashReport) -> String {
    let location = match &report.location {
        Some(location) => format!(
            "{}:{}:{}",
            location.file_name, location.line, location.column
        ),
        None => "unknown".to_string(),
    };
    format!(
        "event=crash_report timestamp_millis={} app_version={} os={} arch={} \
         panic_payload_kind={} location={}\n",
        report.timestamp_millis,
        report.app_version,
        report.os,
        report.arch,
        report.panic_payload_kind,
        location
    )
}

fn write_crash_report(dir: &Path, report: &CrashReport) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let sequence = CRASH_REPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!(
        "kmsync-crash-{}-{}-{}.log",
        report.timestamp_millis,
        std::process::id(),
        sequence
    ));
    std::fs::write(&path, render_crash_report(report))?;
    Ok(path)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn sample_process_resource_metrics() -> Option<ProcessResourceMetrics> {
    use std::mem::size_of;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    unsafe {
        let process = GetCurrentProcess();
        let mut creation_time = FILETIME::default();
        let mut exit_time = FILETIME::default();
        let mut kernel_time = FILETIME::default();
        let mut user_time = FILETIME::default();
        if GetProcessTimes(
            process,
            &mut creation_time,
            &mut exit_time,
            &mut kernel_time,
            &mut user_time,
        ) == 0
        {
            return None;
        }

        let mut memory = PROCESS_MEMORY_COUNTERS {
            cb: u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS>()).ok()?,
            ..PROCESS_MEMORY_COUNTERS::default()
        };
        if GetProcessMemoryInfo(process, &mut memory, memory.cb) == 0 {
            return None;
        }

        let cpu_total_micros =
            filetime_100ns(kernel_time).saturating_add(filetime_100ns(user_time)) / 10;
        let memory_bytes = u64::try_from(memory.WorkingSetSize).ok()?;

        Some(ProcessResourceMetrics {
            cpu_total_micros,
            memory_bytes,
        })
    }
}

#[cfg(windows)]
fn filetime_100ns(value: windows_sys::Win32::Foundation::FILETIME) -> u64 {
    u64::from(value.dwLowDateTime) | (u64::from(value.dwHighDateTime) << 32)
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn sample_process_resource_metrics() -> Option<ProcessResourceMetrics> {
    unsafe {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) != 0 {
            return None;
        }
        let usage = usage.assume_init();
        let cpu_total_micros =
            timeval_micros(usage.ru_utime).saturating_add(timeval_micros(usage.ru_stime));
        let memory_bytes = max_rss_bytes(usage.ru_maxrss)?;

        Some(ProcessResourceMetrics {
            cpu_total_micros,
            memory_bytes,
        })
    }
}

#[cfg(unix)]
fn timeval_micros(value: libc::timeval) -> u64 {
    let seconds = u64::try_from(value.tv_sec).unwrap_or(0);
    let micros = u64::try_from(value.tv_usec).unwrap_or(0);
    seconds.saturating_mul(1_000_000).saturating_add(micros)
}

#[cfg(all(unix, target_os = "macos"))]
fn max_rss_bytes(value: libc::c_long) -> Option<u64> {
    u64::try_from(value).ok()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn max_rss_bytes(value: libc::c_long) -> Option<u64> {
    u64::try_from(value).ok()?.checked_mul(1024)
}

#[cfg(not(any(windows, unix)))]
fn sample_process_resource_metrics() -> Option<ProcessResourceMetrics> {
    None
}

#[derive(Debug, Clone, Copy)]
struct QueuedInputEvent {
    event: InputEvent,
    captured_at: Instant,
}

impl QueuedInputEvent {
    fn new(event: InputEvent) -> Self {
        Self {
            event,
            captured_at: Instant::now(),
        }
    }
}

#[derive(Debug, Clone)]
struct TargetedQueuedInputEvent {
    target_device_id: String,
    profile_name: ProfileName,
    event: InputEvent,
    captured_at: Instant,
}

impl TargetedQueuedInputEvent {
    fn new(target_device_id: String, profile_name: ProfileName, event: InputEvent) -> Self {
        Self {
            target_device_id,
            profile_name,
            event,
            captured_at: Instant::now(),
        }
    }
}

#[cfg(test)]
fn enqueue_routed_capture(
    tx: &SyncSender<QueuedInputEvent>,
    stats: &CaptureQueueStats,
    router: &mut CaptureRouter,
    captured: CapturedInput,
) -> RouteResult {
    enqueue_routed_capture_with_application(tx, stats, router, captured, None)
}

fn enqueue_routed_capture_with_application(
    tx: &SyncSender<QueuedInputEvent>,
    stats: &CaptureQueueStats,
    router: &mut CaptureRouter,
    captured: CapturedInput,
    application_id: Option<&str>,
) -> RouteResult {
    let route = router.route_at_with_application(captured, application_id, Instant::now());
    if route.send_remote {
        if let Some(entry_position) = route.entry_position {
            enqueue_input_event(
                tx,
                stats,
                InputEvent::Mouse(kmsync_core::MouseEvent::Position {
                    x_ratio: entry_position.x_ratio,
                    y_ratio: entry_position.y_ratio,
                }),
            );
        }
        enqueue_input_event(tx, stats, captured.event);
    }
    route
}

fn enqueue_desktop_capture(
    tx: &SyncSender<TargetedQueuedInputEvent>,
    stats: &CaptureQueueStats,
    route: &DesktopCaptureRouteResult<'_>,
) {
    if !route.route.send_remote {
        return;
    }
    let (Some(target_device_id), Some(profile_name)) =
        (route.target_device_id.as_ref(), route.profile_name)
    else {
        return;
    };
    if let Some(entry_position) = route.route.entry_position {
        enqueue_targeted_input_event(
            tx,
            stats,
            target_device_id.to_string(),
            profile_name,
            InputEvent::Mouse(kmsync_core::MouseEvent::Position {
                x_ratio: entry_position.x_ratio,
                y_ratio: entry_position.y_ratio,
            }),
        );
    }
    if let Some(event) = route.remote_event {
        enqueue_targeted_input_event(tx, stats, target_device_id.to_string(), profile_name, event);
    }
}

fn enqueue_input_event(
    tx: &SyncSender<QueuedInputEvent>,
    stats: &CaptureQueueStats,
    event: InputEvent,
) {
    stats.record_enqueue_reserved();
    match send_queued_input_event_with_grace(tx, QueuedInputEvent::new(event)) {
        Ok(()) => stats.record_enqueue_committed(),
        Err(TrySendError::Full(_)) => {
            stats.record_enqueue_canceled();
            stats.record_dropped_full();
            let snapshot = stats.snapshot();
            eprintln!(
                "input queue full; dropping newest captured input event \
                 (depth={}, dropped_full={})",
                snapshot.current_depth, snapshot.dropped_full
            )
        }
        Err(TrySendError::Disconnected(_)) => {
            stats.record_enqueue_canceled();
            stats.record_dropped_disconnected();
            let snapshot = stats.snapshot();
            eprintln!(
                "input transmit thread is disconnected \
                 (depth={}, dropped_disconnected={})",
                snapshot.current_depth, snapshot.dropped_disconnected
            )
        }
    }
}

fn enqueue_targeted_input_event(
    tx: &SyncSender<TargetedQueuedInputEvent>,
    stats: &CaptureQueueStats,
    target_device_id: String,
    profile_name: ProfileName,
    event: InputEvent,
) {
    stats.record_enqueue_reserved();
    match send_targeted_queued_input_event_with_grace(
        tx,
        TargetedQueuedInputEvent::new(target_device_id, profile_name, event),
    ) {
        Ok(()) => stats.record_enqueue_committed(),
        Err(TrySendError::Full(_)) => {
            stats.record_enqueue_canceled();
            stats.record_dropped_full();
        }
        Err(TrySendError::Disconnected(_)) => {
            stats.record_enqueue_canceled();
            stats.record_dropped_disconnected();
        }
    }
}

fn send_queued_input_event_with_grace(
    tx: &SyncSender<QueuedInputEvent>,
    event: QueuedInputEvent,
) -> Result<(), TrySendError<QueuedInputEvent>> {
    if !input_event_waits_for_queue_slot(event.event) {
        return tx.try_send(event);
    }
    let deadline = Instant::now() + DESKTOP_RELIABLE_QUEUE_SEND_GRACE;
    let mut event = event;
    loop {
        match tx.try_send(event) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Full(returned)) => {
                if Instant::now() >= deadline {
                    return Err(TrySendError::Full(returned));
                }
                event = returned;
                thread::sleep(DESKTOP_RELIABLE_QUEUE_SEND_RETRY_DELAY);
            }
            Err(TrySendError::Disconnected(returned)) => {
                return Err(TrySendError::Disconnected(returned));
            }
        }
    }
}

fn send_targeted_queued_input_event_with_grace(
    tx: &SyncSender<TargetedQueuedInputEvent>,
    event: TargetedQueuedInputEvent,
) -> Result<(), TrySendError<TargetedQueuedInputEvent>> {
    if !input_event_waits_for_queue_slot(event.event) {
        return tx.try_send(event);
    }
    let deadline = Instant::now() + DESKTOP_RELIABLE_QUEUE_SEND_GRACE;
    let mut event = event;
    loop {
        match tx.try_send(event) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Full(returned)) => {
                if Instant::now() >= deadline {
                    return Err(TrySendError::Full(returned));
                }
                event = returned;
                thread::sleep(DESKTOP_RELIABLE_QUEUE_SEND_RETRY_DELAY);
            }
            Err(TrySendError::Disconnected(returned)) => {
                return Err(TrySendError::Disconnected(returned));
            }
        }
    }
}

fn input_event_waits_for_queue_slot(event: InputEvent) -> bool {
    kmsync_core::InputChannel::for_event(event) == kmsync_core::InputChannel::InputReliable
}

fn transmit_captured_events(
    rx: Receiver<QueuedInputEvent>,
    sender: &mut impl ProtocolEventSender,
    compiled: CompiledProfile,
    stats: CaptureQueueStats,
) -> Result<(), String> {
    let mut sequence = 1_u64;
    let mut pending = None;
    while let Some(queued) = next_transmit_event(&rx, &mut pending, &stats) {
        stats.record_capture_to_send_latency(queued.captured_at.elapsed());
        let mapped = compiled.transform(queued.event);
        sender.send_event(ProtocolEvent {
            sequence,
            timestamp_micros: now_micros()?,
            event: mapped,
        })?;
        sequence = sequence.saturating_add(1);
    }
    Ok(())
}

enum DesktopTargetTransport {
    Direct(QuicEventSender),
    Relay(client::RelayFrameSender),
}

#[derive(Debug, PartialEq, Eq)]
enum DesktopTargetTransportSelection<D, R> {
    Direct(D),
    Relay(R),
}

impl DesktopTargetTransport {
    const fn label(&self) -> &'static str {
        match self {
            Self::Direct(_) => "direct",
            Self::Relay(_) => "relay",
        }
    }

    fn send_event(
        &mut self,
        relay_target_device_id: &str,
        source_device_id: DeviceId,
        target_device_id: DeviceId,
        event: ProtocolEvent,
    ) -> Result<(), String> {
        match self {
            Self::Direct(sender) => {
                sender.send_input_event(source_device_id, target_device_id, event)
            }
            Self::Relay(sender) => sender.send_input_event(
                relay_target_device_id,
                source_device_id,
                target_device_id,
                event,
            ),
        }
    }

    fn send_frame(
        &mut self,
        relay_target_device_id: &str,
        frame: &ProtocolFrame,
    ) -> Result<(), String> {
        match self {
            Self::Direct(sender) => sender.send_frame(frame),
            Self::Relay(sender) => sender.send_frame(relay_target_device_id, frame),
        }
    }
}

struct DesktopTargetSender {
    transport: DesktopTargetTransport,
    compiled: CompiledProfile,
    source_device_id: DeviceId,
    target_device_id: DeviceId,
    sequence: u64,
}

#[derive(Default)]
struct DesktopTransmitBackoff {
    until_by_target: BTreeMap<String, Instant>,
}

impl DesktopTransmitBackoff {
    fn record_failure(&mut self, target_device_id: &str, now: Instant) {
        self.until_by_target.insert(
            target_device_id.to_string(),
            now + DESKTOP_TRANSMIT_FAILURE_BACKOFF,
        );
    }

    fn record_success(&mut self, target_device_id: &str) {
        self.until_by_target.remove(target_device_id);
    }

    fn should_drop(&mut self, target_device_id: &str, event: &InputEvent, now: Instant) -> bool {
        if !matches!(
            event,
            InputEvent::Mouse(kmsync_core::MouseEvent::Move { .. })
        ) {
            return false;
        }
        let Some(until) = self.until_by_target.get(target_device_id).copied() else {
            return false;
        };
        if now < until {
            return true;
        }
        self.until_by_target.remove(target_device_id);
        false
    }
}

fn transmit_desktop_capture_events(
    rx: Receiver<TargetedQueuedInputEvent>,
    config_path: PathBuf,
    stats: CaptureQueueStats,
    counters: Arc<DesktopCaptureRuntimeCounters>,
) {
    let mut senders = BTreeMap::new();
    let mut backoff = DesktopTransmitBackoff::default();
    let mut transmit_failed = false;
    let mut last_progress_write = None;
    let mut pending = None;
    while let Some(queued) = next_targeted_transmit_event(&rx, &mut pending, &stats) {
        let target_device_id = queued.target_device_id.clone();
        stats.record_capture_to_send_latency(queued.captured_at.elapsed());
        let now = Instant::now();
        if backoff.should_drop(&target_device_id, &queued.event, now) {
            continue;
        }
        if let Err(error) = transmit_desktop_capture_event(&config_path, &mut senders, queued) {
            eprintln!("desktop capture transmit failed: {error}");
            transmit_failed = true;
            backoff.record_failure(&target_device_id, now);
            counters.record_transmit_failed();
            if let Err(write_error) = write_desktop_sync_runtime_status(
                &config_path,
                &desktop_capture_transmit_failed_runtime_status(
                    &target_device_id,
                    &error,
                    &counters,
                ),
            ) {
                eprintln!("desktop capture runtime status write failed: {write_error}");
            }
            continue;
        }

        backoff.record_success(&target_device_id);
        counters.record_sent();
        if transmit_failed
            || desktop_runtime_progress_write_due(last_progress_write, Instant::now())
        {
            transmit_failed = false;
            last_progress_write = Some(Instant::now());
            let timestamp = unix_timestamp_seconds();
            if let Err(write_error) = write_desktop_sync_runtime_status(
                &config_path,
                &desktop_capture_transmit_progress_runtime_status(
                    senders.keys().map(String::as_str),
                    &counters,
                    timestamp,
                ),
            ) {
                eprintln!("desktop capture runtime status write failed: {write_error}");
            }
        }
    }
}

fn next_targeted_transmit_event(
    rx: &Receiver<TargetedQueuedInputEvent>,
    pending: &mut Option<TargetedQueuedInputEvent>,
    stats: &CaptureQueueStats,
) -> Option<TargetedQueuedInputEvent> {
    let event = loop {
        let event = pending
            .take()
            .or_else(|| recv_targeted_counted(rx, stats))?;
        if stale_desktop_mouse_move(&event, Instant::now()) {
            continue;
        }
        break event;
    };
    Some(coalesce_targeted_pointer_motion_burst(
        event, rx, pending, stats,
    ))
}

fn stale_desktop_mouse_move(event: &TargetedQueuedInputEvent, now: Instant) -> bool {
    matches!(
        event.event,
        InputEvent::Mouse(kmsync_core::MouseEvent::Move { .. })
    ) && now.duration_since(event.captured_at) >= DESKTOP_STALE_MOUSE_MOVE_DROP_AFTER
}

fn coalesce_targeted_pointer_motion_burst(
    event: TargetedQueuedInputEvent,
    rx: &Receiver<TargetedQueuedInputEvent>,
    pending: &mut Option<TargetedQueuedInputEvent>,
    stats: &CaptureQueueStats,
) -> TargetedQueuedInputEvent {
    if !is_targeted_pointer_motion(&event) {
        return event;
    }

    let mut latest = event;
    loop {
        match try_recv_targeted_counted(rx, stats) {
            Ok(next) if can_coalesce_targeted_pointer_motion(&latest, &next) => {
                latest = coalesce_targeted_pointer_motion(latest, next);
            }
            Ok(next) => {
                *pending = Some(next);
                break;
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
        }
    }
    latest
}

fn recv_targeted_counted(
    rx: &Receiver<TargetedQueuedInputEvent>,
    stats: &CaptureQueueStats,
) -> Option<TargetedQueuedInputEvent> {
    rx.recv().ok().map(|event| {
        stats.record_dequeued();
        event
    })
}

fn try_recv_targeted_counted(
    rx: &Receiver<TargetedQueuedInputEvent>,
    stats: &CaptureQueueStats,
) -> Result<TargetedQueuedInputEvent, TryRecvError> {
    rx.try_recv().map(|event| {
        stats.record_dequeued();
        event
    })
}

fn can_coalesce_targeted_pointer_motion(
    previous: &TargetedQueuedInputEvent,
    next: &TargetedQueuedInputEvent,
) -> bool {
    previous.target_device_id == next.target_device_id
        && previous.profile_name == next.profile_name
        && matches!(
            (&previous.event, &next.event),
            (
                InputEvent::Mouse(kmsync_core::MouseEvent::Position { .. }),
                InputEvent::Mouse(kmsync_core::MouseEvent::Position { .. })
            ) | (
                InputEvent::Mouse(kmsync_core::MouseEvent::Move { .. }),
                InputEvent::Mouse(kmsync_core::MouseEvent::Move { .. })
            )
        )
}

fn coalesce_targeted_pointer_motion(
    mut previous: TargetedQueuedInputEvent,
    next: TargetedQueuedInputEvent,
) -> TargetedQueuedInputEvent {
    match (&mut previous.event, next.event) {
        (
            InputEvent::Mouse(kmsync_core::MouseEvent::Move { dx, dy }),
            InputEvent::Mouse(kmsync_core::MouseEvent::Move {
                dx: next_dx,
                dy: next_dy,
            }),
        ) => {
            *dx += next_dx;
            *dy += next_dy;
            previous
        }
        (
            InputEvent::Mouse(kmsync_core::MouseEvent::Position { .. }),
            InputEvent::Mouse(kmsync_core::MouseEvent::Position { .. }),
        ) => next,
        _ => previous,
    }
}

fn is_targeted_pointer_motion(event: &TargetedQueuedInputEvent) -> bool {
    matches!(
        event.event,
        InputEvent::Mouse(
            kmsync_core::MouseEvent::Move { .. } | kmsync_core::MouseEvent::Position { .. }
        )
    )
}

fn desktop_runtime_progress_write_due(last_write: Option<Instant>, now: Instant) -> bool {
    const INTERVAL: Duration = Duration::from_secs(2);
    last_write.is_none_or(|last_write| now.duration_since(last_write) >= INTERVAL)
}

fn transmit_desktop_capture_event(
    config_path: &Path,
    senders: &mut BTreeMap<String, DesktopTargetSender>,
    queued: TargetedQueuedInputEvent,
) -> Result<(), String> {
    if !senders.contains_key(&queued.target_device_id) {
        let profile = queued.profile_name.profile();
        let compiled = CompiledProfile::compile(&profile).map_err(|error| format!("{error:?}"))?;
        let config = client::ClientConfig::load(config_path)?;
        let identity = client::DeviceIdentity::load_or_generate(&config.identity_path)?;
        let source_device_id = protocol_device_id_from_uuid(&identity.device_id)?;
        let target_device_id = protocol_device_id_from_uuid(&queued.target_device_id)?;
        let transport = connect_desktop_target_transport(
            config,
            &identity.device_id,
            &queued.target_device_id,
        )?;
        senders.insert(
            queued.target_device_id.clone(),
            DesktopTargetSender {
                transport,
                compiled,
                source_device_id,
                target_device_id,
                sequence: 1,
            },
        );
    }

    let target_id = queued.target_device_id.clone();
    let target = senders
        .get_mut(&queued.target_device_id)
        .ok_or_else(|| "desktop capture sender missing after connect".to_string())?;
    let mapped = target.compiled.transform(queued.event);
    let result = target.transport.send_event(
        &target_id,
        target.source_device_id,
        target.target_device_id,
        ProtocolEvent {
            sequence: target.sequence,
            timestamp_micros: now_micros()?,
            event: mapped,
        },
    );
    match result {
        Ok(()) => {
            target.sequence = target.sequence.saturating_add(1);
            Ok(())
        }
        Err(error) => {
            senders.remove(&target_id);
            Err(error)
        }
    }
}

fn connect_desktop_target_transport(
    config: client::ClientConfig,
    source_device_id: &str,
    target_device_id: &str,
) -> Result<DesktopTargetTransport, String> {
    let server_url = config.server_url.clone();
    match connect_desktop_target_transport_with(
        &server_url,
        source_device_id,
        target_device_id,
        |target_device_id, timeout| {
            client::resolve_target_direct_lan_connection_with_timeout(
                config,
                target_device_id,
                timeout,
            )
        },
        QuicEventSender::connect_with_timeout,
        client::RelayFrameSender::connect,
    )? {
        DesktopTargetTransportSelection::Direct(sender) => {
            Ok(DesktopTargetTransport::Direct(sender))
        }
        DesktopTargetTransportSelection::Relay(sender) => Ok(DesktopTargetTransport::Relay(sender)),
    }
}

fn connect_desktop_target_transport_with<D, R, ResolveDirect, ConnectDirect, ConnectRelay>(
    server_url: &str,
    source_device_id: &str,
    target_device_id: &str,
    resolve_direct: ResolveDirect,
    connect_direct: ConnectDirect,
    connect_relay: ConnectRelay,
) -> Result<DesktopTargetTransportSelection<D, R>, String>
where
    ResolveDirect: FnOnce(&str, Duration) -> Result<client::DirectConnectionAttempt, String>,
    ConnectDirect: FnOnce(SocketAddr, Duration) -> Result<D, String>,
    ConnectRelay: FnOnce(&str, &str) -> Result<R, String>,
{
    match resolve_direct(target_device_id, DESKTOP_DIRECT_CONNECT_TIMEOUT) {
        Ok(connection) => {
            match connect_direct(connection.address, DESKTOP_DIRECT_CONNECT_TIMEOUT) {
                Ok(sender) => {
                    println!(
                        "desktop capture direct connection selected {:?} {} for {}",
                        connection.candidate.kind, connection.address, target_device_id
                    );
                    Ok(DesktopTargetTransportSelection::Direct(sender))
                }
                Err(error) => {
                    let direct_error = format!(
                        "direct LAN connection {:?} {} failed: {error}",
                        connection.candidate.kind, connection.address
                    );
                    eprintln!(
                    "desktop capture direct connection failed for {target_device_id}; using server relay: {direct_error}"
                );
                    connect_relay(server_url, source_device_id)
                        .map(DesktopTargetTransportSelection::Relay)
                        .map_err(|relay_error| {
                            format!("{direct_error}; relay connection failed: {relay_error}")
                        })
                }
            }
        }
        Err(direct_error) => {
            eprintln!(
                "desktop capture direct connection unavailable for {target_device_id}; using server relay: {direct_error}"
            );
            connect_relay(server_url, source_device_id)
                .map(DesktopTargetTransportSelection::Relay)
                .map_err(|relay_error| {
                    format!("{direct_error}; relay connection failed: {relay_error}")
                })
        }
    }
}

fn protocol_device_id_from_uuid(device_id: &str) -> Result<DeviceId, String> {
    uuid::Uuid::parse_str(device_id)
        .map(|uuid| uuid.as_u128())
        .map_err(|error| format!("device id '{device_id}' is not a UUID: {error}"))
}

fn next_transmit_event(
    rx: &Receiver<QueuedInputEvent>,
    pending: &mut Option<QueuedInputEvent>,
    stats: &CaptureQueueStats,
) -> Option<QueuedInputEvent> {
    let event = pending.take().or_else(|| recv_counted(rx, stats))?;
    Some(coalesce_mouse_move_burst(event, rx, pending, stats))
}

fn coalesce_mouse_move_burst(
    event: QueuedInputEvent,
    rx: &Receiver<QueuedInputEvent>,
    pending: &mut Option<QueuedInputEvent>,
    stats: &CaptureQueueStats,
) -> QueuedInputEvent {
    if !is_mouse_move(event) {
        return event;
    }

    let mut latest = event;
    loop {
        match try_recv_counted(rx, stats) {
            Ok(next) if is_mouse_move(next) => latest = next,
            Ok(next) => {
                *pending = Some(next);
                break;
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
        }
    }
    latest
}

fn recv_counted(
    rx: &Receiver<QueuedInputEvent>,
    stats: &CaptureQueueStats,
) -> Option<QueuedInputEvent> {
    rx.recv().ok().map(|event| {
        stats.record_dequeued();
        event
    })
}

fn try_recv_counted(
    rx: &Receiver<QueuedInputEvent>,
    stats: &CaptureQueueStats,
) -> Result<QueuedInputEvent, TryRecvError> {
    rx.try_recv().map(|event| {
        stats.record_dequeued();
        event
    })
}

const fn is_mouse_move(event: QueuedInputEvent) -> bool {
    matches!(
        event.event,
        InputEvent::Mouse(kmsync_core::MouseEvent::Move { .. })
    )
}

fn run_clip_get() -> Result<(), String> {
    let adapter = platform::current_platform();
    print!("{}", adapter.get_clipboard_text()?);
    Ok(())
}

fn run_clip_set(text: &str) -> Result<(), String> {
    let mut adapter = platform::current_platform();
    adapter.set_clipboard_text(text)
}

fn run_clip_send(target: SocketAddr) -> Result<(), String> {
    let adapter = platform::current_platform();
    let content = adapter.get_clipboard_content()?;
    let policy = ClipboardSyncPolicy::default();
    policy
        .check_local(
            &content,
            adapter.active_application_id().as_deref(),
            Instant::now(),
            Instant::now(),
        )
        .map_err(|reason| format!("clipboard sync blocked: {}", reason.as_str()))?;
    let mut state = ClipboardSyncState::new(local_clipboard_source_id());
    let clipboard = state.next_local_content(content);
    let mut sender = QuicEventSender::connect(target)?;
    sender.send_frame(&ProtocolFrame {
        sequence: 1,
        timestamp_micros: now_micros()?,
        payload: ProtocolPayload::ClipboardText(clipboard),
    })?;
    println!("sent clipboard content to {target}");
    Ok(())
}

fn run_clip_watch(
    target: SocketAddr,
    interval: Duration,
    policy: ClipboardSyncPolicy,
) -> Result<(), String> {
    let adapter = platform::current_platform();
    let mut sender = QuicEventSender::connect(target)?;
    let mut state = ClipboardSyncState::new(local_clipboard_source_id());
    let mut last_clipboard = adapter
        .get_clipboard_content()
        .unwrap_or_else(|_| ClipboardText::legacy(String::new()));
    let mut sequence = 1_u64;
    println!(
        "watching clipboard via {}, sending changes to {target}",
        adapter.clipboard_watch_backend().as_str()
    );

    loop {
        let content = adapter.wait_for_clipboard_change(&last_clipboard, interval)?;
        if content != last_clipboard {
            let now = Instant::now();
            let source_app = adapter.active_application_id();
            if let Err(reason) = policy.check_local(&content, source_app.as_deref(), now, now) {
                println!(
                    "skipped clipboard update reason={} bytes={}",
                    reason.as_str(),
                    clipboard_content_bytes(&content)
                );
            } else if state.should_send_local_content(&content) {
                let clipboard = state.next_local_content(content.clone());
                let bytes = clipboard_content_bytes(&clipboard);
                sender.send_frame(&ProtocolFrame {
                    sequence,
                    timestamp_micros: now_micros()?,
                    payload: ProtocolPayload::ClipboardText(clipboard),
                })?;
                println!("sent clipboard update seq={sequence} bytes={bytes}");
                sequence = sequence.saturating_add(1);
            }
            last_clipboard = content;
        }
    }
}

fn run_file_send(target: SocketAddr, file_path: &Path, chunk_bytes: usize) -> Result<(), String> {
    let data = std::fs::read(file_path)
        .map_err(|error| format!("failed to read file {}: {error}", file_path.display()))?;
    let file_name = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unnamed");
    let source_id = local_clipboard_source_id();
    let transfer_id = source_id ^ u128::from(now_micros()?);
    let frames = build_file_transfer_frames(
        source_id,
        transfer_id,
        1,
        file_name,
        &data,
        chunk_bytes,
        now_micros()?,
    )?;
    let mut sender = QuicEventSender::connect(target)?;
    for frame in &frames {
        sender.send_frame(frame)?;
    }
    println!(
        "sent file transfer to {target} files=1 bytes={} chunks={}",
        data.len(),
        frames.len().saturating_sub(1)
    );
    Ok(())
}

fn build_file_transfer_frames(
    source_id: DeviceId,
    transfer_id: u128,
    version: u64,
    file_name: &str,
    data: &[u8],
    chunk_bytes: usize,
    timestamp_micros: u64,
) -> Result<Vec<ProtocolFrame>, String> {
    if chunk_bytes == 0 {
        return Err("file chunk bytes must be greater than zero".to_string());
    }
    let total_size = u64::try_from(data.len()).map_err(|_| "file too large".to_string())?;
    let metadata = ClipboardFiles::new(
        source_id,
        version,
        vec![ClipboardFileMetadata::new(
            file_name.to_string(),
            total_size,
            file_content_hash(data),
        )],
    );
    let mut frames = vec![ProtocolFrame {
        sequence: 1,
        timestamp_micros,
        payload: ProtocolPayload::ClipboardFiles(metadata),
    }];

    if data.is_empty() {
        frames.push(ProtocolFrame {
            sequence: 2,
            timestamp_micros: timestamp_micros.saturating_add(1),
            payload: ProtocolPayload::FileChunk(FileTransferChunk::new(
                transfer_id,
                source_id,
                0,
                0,
                0,
                0,
                true,
                Vec::new(),
            )),
        });
        return Ok(frames);
    }

    for (index, chunk) in data.chunks(chunk_bytes).enumerate() {
        let offset = index
            .checked_mul(chunk_bytes)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| "file chunk offset overflow".to_string())?;
        let chunk_index =
            u32::try_from(index).map_err(|_| "file chunk index overflow".to_string())?;
        let next_offset = offset.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        frames.push(ProtocolFrame {
            sequence: u64::try_from(frames.len() + 1)
                .map_err(|_| "file transfer sequence overflow".to_string())?,
            timestamp_micros: timestamp_micros.saturating_add(
                u64::try_from(frames.len()).map_err(|_| "timestamp offset overflow".to_string())?,
            ),
            payload: ProtocolPayload::FileChunk(FileTransferChunk::new(
                transfer_id,
                source_id,
                0,
                offset,
                total_size,
                chunk_index,
                next_offset >= total_size,
                chunk.to_vec(),
            )),
        });
    }

    Ok(frames)
}

fn now_micros() -> Result<u64, String> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?;
    u64::try_from(duration.as_micros()).map_err(|_| "timestamp overflow".to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileName {
    MacToWindows,
    WindowsToMac,
}

impl ProfileName {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "mac-to-windows" => Ok(Self::MacToWindows),
            "windows-to-mac" => Ok(Self::WindowsToMac),
            _ => Err(format!(
                "unknown profile '{value}', expected mac-to-windows or windows-to-mac"
            )),
        }
    }

    fn profile(self) -> Profile {
        match self {
            Self::MacToWindows => Profile::mac_to_windows_default(),
            Self::WindowsToMac => Profile::windows_to_macos_default(),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::MacToWindows => "mac_to_windows",
            Self::WindowsToMac => "windows_to_mac",
        }
    }
}

enum Command {
    Desktop {
        config_path: PathBuf,
        output_path: Option<PathBuf>,
    },
    Info,
    RequestPermissions,
    SelfTest {
        profile: ProfileName,
    },
    Listen {
        bind: SocketAddr,
    },
    SendDemo {
        target: SocketAddr,
        profile: ProfileName,
    },
    TargetProbe {
        config_path: PathBuf,
        target_device_id: String,
    },
    TargetInputTest {
        config_path: PathBuf,
        target_device_id: String,
    },
    CaptureSend {
        target: SocketAddr,
        profile: ProfileName,
        mode: CaptureMode,
        application_exceptions: ApplicationExceptionRules,
    },
    CaptureConnect {
        config_path: PathBuf,
        target_device_id: String,
        profile: ProfileName,
        mode: CaptureMode,
        application_exceptions: ApplicationExceptionRules,
    },
    CoreService {
        config_path: PathBuf,
    },
    Heartbeat {
        config_path: PathBuf,
    },
    ClipGet,
    ClipSet {
        text: String,
    },
    ClipSend {
        target: SocketAddr,
    },
    ClipWatch {
        target: SocketAddr,
        interval: Duration,
        policy: ClipboardSyncPolicy,
    },
    FileSend {
        target: SocketAddr,
        file_path: PathBuf,
        chunk_bytes: usize,
    },
    Devices {
        config_path: PathBuf,
    },
    ConnectionDiagnostics {
        config_path: PathBuf,
        target_device_id: String,
    },
    DesktopDiagnostics {
        config_path: PathBuf,
        probe_targets: bool,
    },
    Profiles {
        config_path: PathBuf,
    },
    ProfileSet {
        config_path: PathBuf,
        source_device_id: String,
        target_device_id: String,
        profile_path: PathBuf,
    },
    UpdateCheck {
        config_path: PathBuf,
        device_id: Option<String>,
        platform: Option<String>,
        version: Option<String>,
        channel: Option<String>,
    },
    WindowsService {
        config_path: PathBuf,
    },
    LocalIpcEndpoint,
    LocalIpcServeOnce {
        endpoint: local_ipc::LocalIpcEndpoint,
    },
    LocalIpcPing {
        endpoint: local_ipc::LocalIpcEndpoint,
    },
    Ui {
        args: Vec<String>,
    },
    Help,
}

struct Args {
    command: Command,
}

impl Args {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        Self::parse_with_default_config(args, default_daemon_config_path())
    }

    fn parse_with_default_config(
        args: impl Iterator<Item = String>,
        default_config_path: PathBuf,
    ) -> Result<Self, String> {
        let mut args = args.filter(|arg| !is_macos_process_serial_number_arg(arg));
        let Some(command) = args.next() else {
            return Ok(Self {
                command: Command::Desktop {
                    config_path: default_config_path,
                    output_path: None,
                },
            });
        };

        match command.as_str() {
            "desktop" => parse_desktop_command(args, default_config_path),
            "info" => Ok(Self {
                command: Command::Info,
            }),
            "request-permissions" | "permissions" => Ok(Self {
                command: Command::RequestPermissions,
            }),
            "self-test" => {
                let profile = parse_profile_arg(args.next())?;
                Ok(Self {
                    command: Command::SelfTest { profile },
                })
            }
            "listen" => {
                let bind = parse_addr_arg(args.next(), "listen requires bind address")?;
                Ok(Self {
                    command: Command::Listen { bind },
                })
            }
            "send-demo" => {
                let target = parse_addr_arg(args.next(), "send-demo requires target address")?;
                let profile = parse_profile_arg(args.next())?;
                Ok(Self {
                    command: Command::SendDemo { target, profile },
                })
            }
            "target-probe" => {
                let config_path = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| "target-probe requires daemon config path".to_string())?;
                let target_device_id = args
                    .next()
                    .ok_or_else(|| "target-probe requires target device id".to_string())?;
                Ok(Self {
                    command: Command::TargetProbe {
                        config_path,
                        target_device_id,
                    },
                })
            }
            "target-input-test" => {
                let config_path = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| "target-input-test requires daemon config path".to_string())?;
                let target_device_id = args
                    .next()
                    .ok_or_else(|| "target-input-test requires target device id".to_string())?;
                Ok(Self {
                    command: Command::TargetInputTest {
                        config_path,
                        target_device_id,
                    },
                })
            }
            "capture-send" => {
                let target = parse_addr_arg(args.next(), "capture-send requires target address")?;
                let profile = parse_profile_arg(args.next())?;
                let mode = parse_capture_mode(args.next(), args.next(), args.next(), args.next())?;
                let application_exceptions = parse_application_exceptions(args.next());
                Ok(Self {
                    command: Command::CaptureSend {
                        target,
                        profile,
                        mode,
                        application_exceptions,
                    },
                })
            }
            "capture-connect" => {
                let config_path = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| "capture-connect requires daemon config path".to_string())?;
                let target_device_id = args
                    .next()
                    .ok_or_else(|| "capture-connect requires target device id".to_string())?;
                let profile = parse_profile_arg(args.next())?;
                let mode = parse_capture_mode(args.next(), args.next(), args.next(), args.next())?;
                let application_exceptions = parse_application_exceptions(args.next());
                Ok(Self {
                    command: Command::CaptureConnect {
                        config_path,
                        target_device_id,
                        profile,
                        mode,
                        application_exceptions,
                    },
                })
            }
            "core-service" => {
                let config_path = args
                    .next()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| default_config_path.clone());
                Ok(Self {
                    command: Command::CoreService { config_path },
                })
            }
            "heartbeat" => {
                let config_path = args
                    .next()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| default_config_path.clone());
                Ok(Self {
                    command: Command::Heartbeat { config_path },
                })
            }
            "clip-get" => Ok(Self {
                command: Command::ClipGet,
            }),
            "clip-set" => {
                let text = args
                    .next()
                    .ok_or_else(|| "clip-set requires text".to_string())?;
                Ok(Self {
                    command: Command::ClipSet { text },
                })
            }
            "clip-send" => {
                let target = parse_addr_arg(args.next(), "clip-send requires target address")?;
                Ok(Self {
                    command: Command::ClipSend { target },
                })
            }
            "clip-watch" => {
                let target = parse_addr_arg(args.next(), "clip-watch requires target address")?;
                let interval = args
                    .next()
                    .map(|value| {
                        value
                            .parse::<u64>()
                            .map(Duration::from_secs)
                            .map_err(|error| format!("invalid interval seconds: {error}"))
                    })
                    .unwrap_or(Ok(Duration::from_secs(1)))?;
                let policy =
                    parse_clipboard_policy(args.next(), args.next(), args.next(), args.next())?;
                Ok(Self {
                    command: Command::ClipWatch {
                        target,
                        interval,
                        policy,
                    },
                })
            }
            "file-send" => {
                let target = parse_addr_arg(args.next(), "file-send requires target address")?;
                let file_path = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| "file-send requires file path".to_string())?;
                let chunk_bytes = parse_file_chunk_bytes(args.next())?;
                Ok(Self {
                    command: Command::FileSend {
                        target,
                        file_path,
                        chunk_bytes,
                    },
                })
            }
            "devices" => {
                let config_path = args
                    .next()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| default_config_path.clone());
                Ok(Self {
                    command: Command::Devices { config_path },
                })
            }
            "profiles" => {
                let config_path = args
                    .next()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| default_config_path.clone());
                Ok(Self {
                    command: Command::Profiles { config_path },
                })
            }
            "connection-diagnostics" => {
                let config_path = args.next().map(PathBuf::from).ok_or_else(|| {
                    "connection-diagnostics requires daemon config path".to_string()
                })?;
                let target_device_id = args.next().ok_or_else(|| {
                    "connection-diagnostics requires target device id".to_string()
                })?;
                Ok(Self {
                    command: Command::ConnectionDiagnostics {
                        config_path,
                        target_device_id,
                    },
                })
            }
            "desktop-diagnostics" | "diagnose-desktop" => {
                let mut config_path = None;
                let mut probe_targets = false;
                for arg in args {
                    if arg == "--probe-targets" {
                        probe_targets = true;
                    } else if config_path.is_none() {
                        config_path = Some(PathBuf::from(arg));
                    } else {
                        return Err(format!("unknown desktop-diagnostics argument: {arg}"));
                    }
                }
                let config_path = config_path.unwrap_or_else(|| default_config_path.clone());
                Ok(Self {
                    command: Command::DesktopDiagnostics {
                        config_path,
                        probe_targets,
                    },
                })
            }
            "profile-set" => {
                let config_path = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| "profile-set requires daemon config path".to_string())?;
                let source_device_id = args
                    .next()
                    .ok_or_else(|| "profile-set requires source device id".to_string())?;
                let target_device_id = args
                    .next()
                    .ok_or_else(|| "profile-set requires target device id".to_string())?;
                let profile_path = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| "profile-set requires profile json path".to_string())?;
                Ok(Self {
                    command: Command::ProfileSet {
                        config_path,
                        source_device_id,
                        target_device_id,
                        profile_path,
                    },
                })
            }
            "update-check" => {
                let config_path = args
                    .next()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| default_config_path.clone());
                Ok(Self {
                    command: Command::UpdateCheck {
                        config_path,
                        device_id: args.next(),
                        platform: args.next(),
                        version: args.next(),
                        channel: args.next(),
                    },
                })
            }
            "windows-service" => {
                let config_path = args
                    .next()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| default_config_path.clone());
                Ok(Self {
                    command: Command::WindowsService { config_path },
                })
            }
            "ipc-endpoint" => Ok(Self {
                command: Command::LocalIpcEndpoint,
            }),
            "ipc-serve-once" => Ok(Self {
                command: Command::LocalIpcServeOnce {
                    endpoint: parse_local_ipc_endpoint(args.next()),
                },
            }),
            "ipc-ping" => Ok(Self {
                command: Command::LocalIpcPing {
                    endpoint: parse_local_ipc_endpoint(args.next()),
                },
            }),
            "status" | "ping" | "layout-editor" | "control-panel" => {
                let mut ui_args = vec![command.to_string()];
                ui_args.extend(args);
                Ok(Self {
                    command: Command::Ui { args: ui_args },
                })
            }
            "help" | "--help" | "-h" => Ok(Self {
                command: Command::Help,
            }),
            other => Err(format!("unknown command '{other}'")),
        }
    }
}

fn parse_desktop_command(
    args: impl Iterator<Item = String>,
    default_config_path: PathBuf,
) -> Result<Args, String> {
    let mut config_path = None;
    let mut output_path = None;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" | "-o" => {
                let output = args
                    .next()
                    .ok_or_else(|| "desktop --output requires an output path".to_string())?;
                output_path = Some(PathBuf::from(output));
            }
            _ if config_path.is_none() => {
                config_path = Some(PathBuf::from(arg));
            }
            _ if output_path.is_none() => {
                output_path = Some(PathBuf::from(arg));
            }
            _ => {
                return Err(
                    "desktop accepts at most one config path and one output path".to_string(),
                )
            }
        }
    }

    Ok(Args {
        command: Command::Desktop {
            config_path: config_path.unwrap_or(default_config_path),
            output_path,
        },
    })
}

fn default_daemon_config_path() -> PathBuf {
    let current_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let exe_path = env::current_exe().unwrap_or_else(|_| PathBuf::from("kmsync"));
    let home_dir = env::var_os("HOME").map(PathBuf::from);
    let appdata_dir = env::var_os("APPDATA").map(PathBuf::from);
    let config_path = default_daemon_config_path_from_env(
        &current_dir,
        &exe_path,
        home_dir.as_deref(),
        appdata_dir.as_deref(),
        env::consts::OS,
    );
    if let Err(error) = seed_default_daemon_config_if_needed(&config_path, &exe_path) {
        eprintln!("kmsync: failed to prepare default config: {error}");
    }
    config_path
}

#[cfg(test)]
fn default_daemon_config_path_from(current_dir: &Path, exe_path: &Path) -> PathBuf {
    default_daemon_config_path_from_env(current_dir, exe_path, None, None, env::consts::OS)
}

fn default_daemon_config_path_from_env(
    current_dir: &Path,
    exe_path: &Path,
    home_dir: Option<&Path>,
    appdata_dir: Option<&Path>,
    os: &str,
) -> PathBuf {
    if os == "windows" && is_windows_installed_package_executable(exe_path) {
        if let Some(path) = default_user_daemon_config_path(home_dir, appdata_dir, os) {
            return path;
        }
    }

    if os == "macos" && is_macos_app_bundle_executable(exe_path) {
        if let Some(path) = default_user_daemon_config_path(home_dir, appdata_dir, os) {
            return path;
        }
    }

    let mut candidates = vec![
        current_dir.join("configs").join(DEFAULT_DAEMON_CONFIG_FILE),
        current_dir.join("config").join(DEFAULT_DAEMON_CONFIG_FILE),
    ];

    if let Some(exe_dir) = exe_path.parent() {
        candidates.push(exe_dir.join("configs").join(DEFAULT_DAEMON_CONFIG_FILE));
        candidates.push(exe_dir.join("config").join(DEFAULT_DAEMON_CONFIG_FILE));
        if let Some(package_dir) = exe_dir.parent() {
            candidates.push(package_dir.join("config").join(DEFAULT_DAEMON_CONFIG_FILE));
            candidates.push(package_dir.join("configs").join(DEFAULT_DAEMON_CONFIG_FILE));
        }
    }

    if let Some(path) = candidates.into_iter().find(|path| path.exists()) {
        return path;
    }

    default_user_daemon_config_path(home_dir, appdata_dir, os)
        .unwrap_or_else(|| PathBuf::from("configs").join(DEFAULT_DAEMON_CONFIG_FILE))
}

fn default_user_daemon_config_path(
    home_dir: Option<&Path>,
    appdata_dir: Option<&Path>,
    os: &str,
) -> Option<PathBuf> {
    match os {
        "macos" => home_dir.map(|home| {
            home.join("Library")
                .join("Application Support")
                .join("KMSync")
                .join(DEFAULT_DAEMON_CONFIG_FILE)
        }),
        "windows" => {
            appdata_dir.map(|appdata| appdata.join("KMSync").join(DEFAULT_DAEMON_CONFIG_FILE))
        }
        _ => home_dir.map(|home| {
            home.join(".config")
                .join("kmsync")
                .join(DEFAULT_DAEMON_CONFIG_FILE)
        }),
    }
}

fn is_macos_app_bundle_executable(exe_path: &Path) -> bool {
    macos_app_bundle_path_from_executable(exe_path).is_some()
}

fn macos_app_bundle_path_from_executable(exe_path: &Path) -> Option<PathBuf> {
    let Some(exe_dir) = exe_path.parent() else {
        return None;
    };
    if exe_dir.file_name().and_then(|name| name.to_str()) != Some("MacOS") {
        return None;
    }
    let Some(contents_dir) = exe_dir.parent() else {
        return None;
    };
    if contents_dir.file_name().and_then(|name| name.to_str()) != Some("Contents") {
        return None;
    }
    let bundle = contents_dir.parent()?;
    if bundle.extension().and_then(|extension| extension.to_str()) != Some("app") {
        return None;
    }
    Some(bundle.to_path_buf())
}

fn is_windows_installed_package_executable(exe_path: &Path) -> bool {
    let Some(exe_dir) = exe_path.parent() else {
        return false;
    };
    exe_dir.join("Uninstall.exe").exists()
}

fn bundled_app_daemon_config_path(exe_path: &Path) -> Option<PathBuf> {
    let exe_dir = exe_path.parent()?;
    if exe_dir.file_name().and_then(|name| name.to_str()) != Some("MacOS") {
        return None;
    }
    let contents_dir = exe_dir.parent()?;
    Some(
        contents_dir
            .join("configs")
            .join(DEFAULT_DAEMON_CONFIG_FILE),
    )
}

fn legacy_package_daemon_config_path(exe_path: &Path) -> Option<PathBuf> {
    exe_path
        .parent()
        .map(|exe_dir| exe_dir.join("configs").join(DEFAULT_DAEMON_CONFIG_FILE))
}

fn seed_default_daemon_config_if_needed(config_path: &Path, exe_path: &Path) -> Result<(), String> {
    if config_path.exists() {
        return Ok(());
    }
    if let Some(legacy_path) = legacy_package_daemon_config_path(exe_path)
        .filter(|path| path.exists() && path != config_path)
    {
        copy_seed_daemon_config(&legacy_path, config_path)?;
        return Ok(());
    }
    let Some(template_path) = bundled_app_daemon_config_path(exe_path) else {
        return Ok(());
    };
    if !template_path.exists() {
        return Ok(());
    }
    copy_seed_daemon_config(&template_path, config_path)
}

fn copy_seed_daemon_config(source_path: &Path, config_path: &Path) -> Result<(), String> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::copy(source_path, config_path).map_err(|error| {
        format!(
            "failed to copy {} to {}: {error}",
            source_path.display(),
            config_path.display()
        )
    })?;
    copy_relative_identity_seed(source_path, config_path)?;
    Ok(())
}

fn copy_relative_identity_seed(source_config: &Path, target_config: &Path) -> Result<(), String> {
    let text = fs::read_to_string(source_config)
        .map_err(|error| format!("failed to read {}: {error}", source_config.display()))?;
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Ok(());
    };
    let identity_path = value
        .get("identity_path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("kmsync-device-identity.json");
    let identity_path = PathBuf::from(identity_path);
    if identity_path.is_absolute() {
        return Ok(());
    }

    let Some(source_parent) = source_config.parent() else {
        return Ok(());
    };
    let Some(target_parent) = target_config.parent() else {
        return Ok(());
    };
    let source_identity = source_parent.join(&identity_path);
    let target_identity = target_parent.join(&identity_path);
    if !source_identity.exists() || target_identity.exists() {
        return Ok(());
    }
    if let Some(parent) = target_identity.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::copy(&source_identity, &target_identity).map_err(|error| {
        format!(
            "failed to copy {} to {}: {error}",
            source_identity.display(),
            target_identity.display()
        )
    })?;
    Ok(())
}

fn parse_addr_arg(value: Option<String>, missing: &str) -> Result<SocketAddr, String> {
    value
        .ok_or_else(|| missing.to_string())?
        .parse()
        .map_err(|error| format!("invalid socket address: {error}"))
}

fn parse_file_chunk_bytes(value: Option<String>) -> Result<usize, String> {
    let chunk_bytes = value
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|error| format!("invalid file chunk bytes: {error}"))
        })
        .unwrap_or(Ok(DEFAULT_FILE_TRANSFER_CHUNK_BYTES))?;
    if chunk_bytes == 0 || chunk_bytes > MAX_FILE_TRANSFER_CHUNK_BYTES {
        return Err(format!(
            "file chunk bytes must be between 1 and {MAX_FILE_TRANSFER_CHUNK_BYTES}"
        ));
    }
    Ok(chunk_bytes)
}

fn parse_profile_arg(value: Option<String>) -> Result<ProfileName, String> {
    value
        .as_deref()
        .map(ProfileName::parse)
        .unwrap_or(Ok(ProfileName::MacToWindows))
}

fn parse_capture_mode(
    edge: Option<String>,
    threshold: Option<String>,
    release_hotkey: Option<String>,
    cooldown_ms: Option<String>,
) -> Result<CaptureMode, String> {
    let Some(edge) = edge else {
        return Ok(CaptureMode::Always);
    };
    if edge == "all" {
        return Ok(CaptureMode::Always);
    }
    if matches!(edge.as_str(), "lock" | "locked" | "local") {
        return Ok(CaptureMode::Locked);
    }
    let edge = Edge::parse(&edge)?;
    let threshold = threshold
        .map(|value| {
            value
                .parse::<f64>()
                .map_err(|error| format!("invalid edge threshold: {error}"))
        })
        .unwrap_or(Ok(2.0))?;
    let release_hotkey = release_hotkey
        .as_deref()
        .map(Hotkey::parse)
        .unwrap_or_else(|| Ok(Hotkey::default_release()))?;
    let cooldown = cooldown_ms
        .map(|value| {
            value
                .parse::<u64>()
                .map(Duration::from_millis)
                .map_err(|error| format!("invalid edge cooldown ms: {error}"))
        })
        .unwrap_or_else(|| Ok(default_edge_cooldown()))?;
    Ok(CaptureMode::Edge {
        edge,
        threshold,
        release_hotkey,
        cooldown,
    })
}

fn parse_clipboard_policy(
    max_bytes: Option<String>,
    enabled: Option<String>,
    ttl_seconds: Option<String>,
    sensitive_apps: Option<String>,
) -> Result<ClipboardSyncPolicy, String> {
    let mut policy = ClipboardSyncPolicy::default();
    if let Some(max_bytes) = max_bytes {
        policy.max_bytes = max_bytes
            .parse::<usize>()
            .map_err(|error| format!("invalid clipboard max bytes: {error}"))?;
    }
    if let Some(enabled) = enabled {
        policy.enabled = parse_clipboard_enabled(&enabled)?;
    }
    if let Some(ttl_seconds) = ttl_seconds {
        policy.ttl = ttl_seconds
            .parse::<u64>()
            .map(Duration::from_secs)
            .map_err(|error| format!("invalid clipboard ttl seconds: {error}"))?;
    }
    if let Some(sensitive_apps) = sensitive_apps {
        for app in sensitive_apps
            .split(',')
            .map(str::trim)
            .filter(|app| !app.is_empty())
            .map(str::to_ascii_lowercase)
        {
            if !policy
                .sensitive_app_blacklist
                .iter()
                .any(|existing| existing == &app)
            {
                policy.sensitive_app_blacklist.push(app);
            }
        }
    }
    Ok(policy)
}

fn parse_clipboard_enabled(value: &str) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "enabled" | "enable" | "on" | "true" | "1" => Ok(true),
        "disabled" | "disable" | "off" | "false" | "0" => Ok(false),
        _ => Err(format!(
            "invalid clipboard sync switch '{value}', expected enabled or disabled"
        )),
    }
}

fn is_macos_process_serial_number_arg(arg: &str) -> bool {
    arg.starts_with("-psn_")
}

fn parse_local_ipc_endpoint(address: Option<String>) -> local_ipc::LocalIpcEndpoint {
    let mut endpoint = local_ipc::default_local_ipc_endpoint();
    if let Some(address) = address {
        endpoint.address = address;
    }
    endpoint
}

fn default_edge_cooldown() -> Duration {
    Duration::from_millis(250)
}

fn print_help() {
    println!("Usage:");
    println!("  kmsync info");
    println!("  kmsync request-permissions");
    println!("  kmsync self-test [mac-to-windows|windows-to-mac]");
    println!("  kmsync listen 0.0.0.0:24800");
    println!("  kmsync send-demo 127.0.0.1:24800 [mac-to-windows|windows-to-mac]");
    println!("  kmsync target-probe configs/daemon.example.json <target_device_id>");
    println!("  kmsync target-input-test configs/daemon.example.json <target_device_id>");
    println!("  kmsync capture-send 127.0.0.1:24800 [mac-to-windows|windows-to-mac] [all|lock|left|right|top|bottom|top-left|top-right|bottom-left|bottom-right] [threshold_px] [release_hotkey] [cooldown_ms] [local_app_csv]");
    println!("  kmsync capture-connect configs/daemon.example.json <target_device_id> [mac-to-windows|windows-to-mac] [all|lock|left|right|top|bottom|top-left|top-right|bottom-left|bottom-right] [threshold_px] [release_hotkey] [cooldown_ms] [local_app_csv]");
    println!("  kmsync core-service configs/daemon.example.json");
    println!("  kmsync heartbeat configs/daemon.example.json");
    println!("  kmsync clip-get");
    println!("  kmsync clip-set \"hello\"");
    println!("  kmsync clip-send 127.0.0.1:24800");
    println!("  kmsync clip-watch 127.0.0.1:24800 [interval_seconds] [max_bytes] [enabled|disabled] [ttl_seconds] [sensitive_app_csv]");
    println!("  kmsync file-send 127.0.0.1:24800 <file_path> [chunk_bytes]");
    println!("  kmsync devices configs/daemon.example.json");
    println!("  kmsync connection-diagnostics configs/daemon.example.json <target_device_id>");
    println!("  kmsync desktop-diagnostics [configs/daemon.example.json] [--probe-targets]");
    println!("  kmsync profiles configs/daemon.example.json");
    println!("  kmsync profile-set configs/daemon.example.json <source_device_id> <target_device_id> configs/mac-to-windows.profile.json");
    println!("  kmsync update-check configs/daemon.example.json [device_id] [windows|macos|linux] [version] [stable]");
    println!("  kmsync windows-service configs/daemon.example.json");
    println!("  kmsync ipc-endpoint");
    println!("  kmsync ipc-serve-once [endpoint]");
    println!("  kmsync ipc-ping [endpoint]");
    println!("  kmsync status [endpoint]");
    println!("  kmsync ping [endpoint]");
    println!("  kmsync layout-editor <profile.json> [output.html]");
    println!("  kmsync control-panel <profile.json> [output.html]");
}

#[derive(Debug, Clone, Copy)]
enum CaptureMode {
    Always,
    Locked,
    Edge {
        edge: Edge,
        threshold: f64,
        release_hotkey: Hotkey,
        cooldown: Duration,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ApplicationExceptionRules {
    patterns: Vec<String>,
}

impl ApplicationExceptionRules {
    fn from_patterns(patterns: Vec<String>) -> Self {
        let patterns = patterns
            .into_iter()
            .map(|pattern| pattern.trim().to_ascii_lowercase())
            .filter(|pattern| !pattern.is_empty())
            .collect();
        Self { patterns }
    }

    fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    fn matches(&self, application_id: Option<&str>) -> bool {
        let Some(application_id) = application_id else {
            return false;
        };
        self.patterns
            .iter()
            .any(|pattern| contains_case_insensitive(application_id, pattern))
    }
}

fn parse_application_exceptions(value: Option<String>) -> ApplicationExceptionRules {
    let patterns = value
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(ToString::to_string)
        .collect();
    ApplicationExceptionRules::from_patterns(patterns)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hotkey {
    key: Key,
    modifiers: Modifiers,
}

impl Hotkey {
    const fn default_release() -> Self {
        Self {
            key: Key::Escape,
            modifiers: Modifiers::CONTROL.with(Modifiers::ALT),
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        let mut modifiers = Modifiers::NONE;
        let mut key = None;

        for raw_part in value.split('+') {
            let part = raw_part.trim().to_ascii_lowercase();
            if part.is_empty() {
                return Err(format!("invalid release hotkey '{value}'"));
            }

            match part.as_str() {
                "ctrl" | "control" => modifiers = modifiers.with(Modifiers::CONTROL),
                "shift" => modifiers = modifiers.with(Modifiers::SHIFT),
                "alt" | "option" => modifiers = modifiers.with(Modifiers::ALT),
                "meta" | "cmd" | "command" | "super" | "win" | "windows" => {
                    modifiers = modifiers.with(Modifiers::META);
                }
                _ => {
                    let parsed_key = parse_hotkey_key(&part)
                        .ok_or_else(|| format!("unknown release hotkey key '{part}'"))?;
                    if key.replace(parsed_key).is_some() {
                        return Err(format!(
                            "release hotkey '{value}' must contain exactly one non-modifier key"
                        ));
                    }
                }
            }
        }

        let key = key.ok_or_else(|| {
            format!("release hotkey '{value}' must contain exactly one non-modifier key")
        })?;
        Ok(Self { key, modifiers })
    }

    fn matches(self, event: KeyEvent) -> bool {
        event.key == self.key
            && event.state == KeyState::Pressed
            && event.modifiers.bits() & self.modifiers.bits() == self.modifiers.bits()
    }
}

fn parse_hotkey_key(value: &str) -> Option<Key> {
    Key::from_name(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Edge {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            "top" => Ok(Self::Top),
            "bottom" => Ok(Self::Bottom),
            "top-left" | "left-top" | "tl" => Ok(Self::TopLeft),
            "top-right" | "right-top" | "tr" => Ok(Self::TopRight),
            "bottom-left" | "left-bottom" | "bl" => Ok(Self::BottomLeft),
            "bottom-right" | "right-bottom" | "br" => Ok(Self::BottomRight),
            _ => Err(format!(
                "unknown capture edge '{value}', expected all, lock, left, right, top, bottom, top-left, top-right, bottom-left, or bottom-right"
            )),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::TopLeft => "top_left",
            Self::TopRight => "top_right",
            Self::BottomLeft => "bottom_left",
            Self::BottomRight => "bottom_right",
        }
    }
}

struct CaptureRouter {
    mode: CaptureMode,
    display_layout: DisplayLayout,
    application_exceptions: ApplicationExceptionRules,
    active: bool,
    cooldown_until: Option<Instant>,
    local_restore_position: Option<PointerPosition>,
}

struct RouteResult {
    send_remote: bool,
    decision: CaptureDecision,
    entry_position: Option<PointerEntryPosition>,
    local_pointer_action: Option<LocalPointerAction>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PointerEntryPosition {
    x_ratio: f32,
    y_ratio: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum LocalPointerAction {
    Hide,
    Restore { position: Option<PointerPosition> },
}

impl RouteResult {
    const fn local(decision: CaptureDecision) -> Self {
        Self {
            send_remote: false,
            decision,
            entry_position: None,
            local_pointer_action: None,
        }
    }

    const fn remote(decision: CaptureDecision) -> Self {
        Self {
            send_remote: true,
            decision,
            entry_position: None,
            local_pointer_action: None,
        }
    }

    const fn remote_with_entry_and_pointer_action(
        decision: CaptureDecision,
        entry_position: Option<PointerEntryPosition>,
        local_pointer_action: Option<LocalPointerAction>,
    ) -> Self {
        Self {
            send_remote: true,
            decision,
            entry_position,
            local_pointer_action,
        }
    }

    const fn with_pointer_action(
        mut self,
        local_pointer_action: Option<LocalPointerAction>,
    ) -> Self {
        self.local_pointer_action = local_pointer_action;
        self
    }
}

fn apply_local_pointer_action(action: Option<LocalPointerAction>, hidden: &AtomicBool) {
    match action {
        Some(LocalPointerAction::Hide) => {
            if !hidden.swap(true, Ordering::Relaxed) {
                platform::hide_local_pointer();
            }
        }
        Some(LocalPointerAction::Restore { position }) => {
            restore_local_pointer_if_hidden(hidden, position)
        }
        None => {}
    }
}

fn restore_local_pointer_if_hidden(hidden: &AtomicBool, position: Option<PointerPosition>) {
    if hidden.swap(false, Ordering::Relaxed) {
        platform::restore_local_pointer(position);
    }
}

impl CaptureRouter {
    #[cfg(test)]
    fn new(mode: CaptureMode, display_bounds: Option<platform::DisplayBounds>) -> Self {
        Self::with_display_layout(mode, DisplayLayout::from_primary(display_bounds))
    }

    #[cfg(test)]
    fn with_application_exceptions(
        mode: CaptureMode,
        display_bounds: Option<platform::DisplayBounds>,
        application_exceptions: ApplicationExceptionRules,
    ) -> Self {
        Self::with_display_layout_and_exceptions(
            mode,
            DisplayLayout::from_primary(display_bounds),
            application_exceptions,
        )
    }

    #[cfg(test)]
    fn with_display_layout(mode: CaptureMode, display_layout: DisplayLayout) -> Self {
        Self::with_display_layout_and_exceptions(
            mode,
            display_layout,
            ApplicationExceptionRules::default(),
        )
    }

    fn with_display_layout_and_exceptions(
        mode: CaptureMode,
        display_layout: DisplayLayout,
        application_exceptions: ApplicationExceptionRules,
    ) -> Self {
        Self {
            mode,
            display_layout,
            application_exceptions,
            active: false,
            cooldown_until: None,
            local_restore_position: None,
        }
    }

    fn describe(&self) -> String {
        match self.mode {
            CaptureMode::Always => {
                "all events forwarded; local input is not suppressed".to_string()
            }
            CaptureMode::Locked => {
                "current device locked; events stay local and remote control is disabled"
                    .to_string()
            }
            CaptureMode::Edge {
                edge,
                threshold,
                release_hotkey,
                cooldown,
            } => format!(
                "edge/corner {:?}, threshold {threshold}px; {:?} releases local control; cooldown {}ms",
                edge,
                release_hotkey,
                cooldown.as_millis()
            ),
        }
    }

    #[cfg(test)]
    fn route(&mut self, captured: CapturedInput) -> RouteResult {
        self.route_at(captured, Instant::now())
    }

    #[cfg(test)]
    fn route_at(&mut self, captured: CapturedInput, now: Instant) -> RouteResult {
        self.route_at_with_application(captured, None, now)
    }

    fn route_at_with_application(
        &mut self,
        captured: CapturedInput,
        application_id: Option<&str>,
        now: Instant,
    ) -> RouteResult {
        if is_system_reserved_shortcut(captured.event) {
            return RouteResult::local(CaptureDecision::Continue);
        }

        if self.application_exceptions.matches(application_id) {
            return RouteResult::local(CaptureDecision::Continue);
        }

        match self.mode {
            CaptureMode::Always => RouteResult::remote(CaptureDecision::Continue),
            CaptureMode::Locked => RouteResult::local(CaptureDecision::Continue),
            CaptureMode::Edge {
                edge,
                threshold,
                release_hotkey,
                cooldown,
            } => {
                if self.is_release_hotkey(captured, release_hotkey) {
                    let local_pointer_action = self.active.then_some(LocalPointerAction::Restore {
                        position: self.local_restore_position,
                    });
                    self.active = false;
                    self.local_restore_position = None;
                    self.cooldown_until = cooldown_deadline(now, cooldown);
                    return RouteResult::local(CaptureDecision::Continue)
                        .with_pointer_action(local_pointer_action);
                }

                let mut entry_position = None;
                let mut local_pointer_action = None;
                if !self.active
                    && !self.cooldown_active(now)
                    && self.at_edge(captured.pointer, edge, threshold)
                {
                    self.active = true;
                    self.local_restore_position = captured.pointer;
                    entry_position = self.entry_position(captured.pointer, edge);
                    local_pointer_action = Some(LocalPointerAction::Hide);
                    println!("remote control activated at {:?} edge", edge);
                }

                if self.active {
                    RouteResult::remote_with_entry_and_pointer_action(
                        CaptureDecision::Suppress,
                        entry_position,
                        local_pointer_action,
                    )
                } else {
                    RouteResult::local(CaptureDecision::Continue)
                }
            }
        }
    }

    fn has_application_exceptions(&self) -> bool {
        !self.application_exceptions.is_empty()
    }

    fn cooldown_active(&mut self, now: Instant) -> bool {
        let Some(deadline) = self.cooldown_until else {
            return false;
        };
        if now < deadline {
            true
        } else {
            self.cooldown_until = None;
            false
        }
    }

    fn at_edge(&self, pointer: Option<PointerPosition>, edge: Edge, threshold: f64) -> bool {
        let (Some(pointer), Some(bounds)) = (pointer, self.display_layout.virtual_bounds()) else {
            return false;
        };
        match edge {
            Edge::Left => pointer.x <= bounds.x + threshold,
            Edge::Right => pointer.x >= bounds.x + bounds.width - threshold,
            Edge::Top => pointer.y <= bounds.y + threshold,
            Edge::Bottom => pointer.y >= bounds.y + bounds.height - threshold,
            Edge::TopLeft => pointer.x <= bounds.x + threshold && pointer.y <= bounds.y + threshold,
            Edge::TopRight => {
                pointer.x >= bounds.x + bounds.width - threshold
                    && pointer.y <= bounds.y + threshold
            }
            Edge::BottomLeft => {
                pointer.x <= bounds.x + threshold
                    && pointer.y >= bounds.y + bounds.height - threshold
            }
            Edge::BottomRight => {
                pointer.x >= bounds.x + bounds.width - threshold
                    && pointer.y >= bounds.y + bounds.height - threshold
            }
        }
    }

    fn entry_position(
        &self,
        pointer: Option<PointerPosition>,
        edge: Edge,
    ) -> Option<PointerEntryPosition> {
        let (Some(pointer), Some(bounds)) = (pointer, self.display_layout.virtual_bounds()) else {
            return None;
        };
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return None;
        }
        let x_ratio = ((pointer.x - bounds.x) / bounds.width).clamp(0.0, 1.0) as f32;
        let y_ratio = ((pointer.y - bounds.y) / bounds.height).clamp(0.0, 1.0) as f32;
        Some(match edge {
            Edge::Left => PointerEntryPosition {
                x_ratio: 1.0,
                y_ratio,
            },
            Edge::Right => PointerEntryPosition {
                x_ratio: 0.0,
                y_ratio,
            },
            Edge::Top => PointerEntryPosition {
                x_ratio,
                y_ratio: 1.0,
            },
            Edge::Bottom => PointerEntryPosition {
                x_ratio,
                y_ratio: 0.0,
            },
            Edge::TopLeft => PointerEntryPosition {
                x_ratio: 1.0,
                y_ratio: 1.0,
            },
            Edge::TopRight => PointerEntryPosition {
                x_ratio: 0.0,
                y_ratio: 1.0,
            },
            Edge::BottomLeft => PointerEntryPosition {
                x_ratio: 1.0,
                y_ratio: 0.0,
            },
            Edge::BottomRight => PointerEntryPosition {
                x_ratio: 0.0,
                y_ratio: 0.0,
            },
        })
    }

    fn is_release_hotkey(&self, captured: CapturedInput, release_hotkey: Hotkey) -> bool {
        let InputEvent::Key(event) = captured.event else {
            return false;
        };
        release_hotkey.matches(event)
    }
}

fn is_system_reserved_shortcut(event: InputEvent) -> bool {
    let InputEvent::Key(event) = event else {
        return false;
    };
    matches!(
        (event.key, event.modifiers),
        (Key::Delete, modifiers)
            if modifiers.contains(Modifiers::CONTROL) && modifiers.contains(Modifiers::ALT)
    ) || matches!(
        (event.key, event.modifiers),
        (Key::L, modifiers) if modifiers.contains(Modifiers::META)
    ) || matches!(
        (event.key, event.modifiers),
        (Key::Space, modifiers) if modifiers.contains(Modifiers::META)
    )
}

fn cooldown_deadline(now: Instant, cooldown: Duration) -> Option<Instant> {
    if cooldown.is_zero() {
        None
    } else {
        now.checked_add(cooldown).or(Some(now))
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod allocation_tracking {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    pub struct CountingAllocator;

    static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
    static TRACKING_ACTIVE: AtomicBool = AtomicBool::new(false);

    thread_local! {
        static TRACK_THIS_THREAD: Cell<bool> = const { Cell::new(false) };
    }

    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if TRACKING_ACTIVE.load(Ordering::Relaxed) {
                TRACK_THIS_THREAD.with(|tracking| {
                    if tracking.get() {
                        ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
                    }
                });
            }
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    pub fn reset() {
        TRACK_THIS_THREAD.with(|tracking| tracking.set(true));
        ALLOCATIONS.store(0, Ordering::SeqCst);
        TRACKING_ACTIVE.store(true, Ordering::SeqCst);
    }

    pub fn count() -> usize {
        TRACKING_ACTIVE.store(false, Ordering::SeqCst);
        TRACK_THIS_THREAD.with(|tracking| tracking.set(false));
        ALLOCATIONS.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
#[global_allocator]
static TEST_ALLOCATOR: allocation_tracking::CountingAllocator =
    allocation_tracking::CountingAllocator;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::DisplayBounds;
    use kmsync_core::{
        ClipboardFormat, InputChannel, InputEventEnvelope, MouseButton, MouseEvent, ScrollEvent,
    };
    use std::collections::VecDeque;

    fn unique_test_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        env::temp_dir().join(format!("kmsync-{name}-{}-{nanos}", std::process::id()))
    }

    #[derive(Default)]
    struct RecordingInjector {
        events: Vec<InputEvent>,
        fail_next: bool,
        clipboard_texts: Vec<String>,
        clipboard_contents: Vec<ClipboardText>,
    }

    impl InputInjector for RecordingInjector {
        fn inject(&mut self, event: InputEvent) -> Result<(), String> {
            if self.fail_next {
                self.fail_next = false;
                Err("injection failed".to_string())
            } else {
                self.events.push(event);
                Ok(())
            }
        }
    }

    impl ClipboardBackend for RecordingInjector {
        fn get_clipboard_text(&self) -> Result<String, String> {
            Ok(self.clipboard_texts.last().cloned().unwrap_or_default())
        }

        fn set_clipboard_text(&mut self, text: &str) -> Result<(), String> {
            self.clipboard_texts.push(text.to_string());
            Ok(())
        }

        fn get_clipboard_content(&self) -> Result<ClipboardText, String> {
            let text = self.get_clipboard_text()?;
            Ok(ClipboardText::from_local_text(0, 0, text))
        }

        fn set_clipboard_content(&mut self, clipboard: &ClipboardText) -> Result<(), String> {
            self.clipboard_contents.push(clipboard.clone());
            if clipboard.format != ClipboardFormat::Image {
                self.clipboard_texts.push(clipboard.text.clone());
            }
            Ok(())
        }
    }

    struct RecordingFrameReceiver {
        frames: VecDeque<Result<ProtocolFrame, String>>,
    }

    impl ProtocolFrameReceiver for RecordingFrameReceiver {
        fn recv_frame(&mut self) -> Result<ProtocolFrame, String> {
            self.frames
                .pop_front()
                .unwrap_or_else(|| Err("receiver exhausted".to_string()))
        }
    }

    const BOUNDS: DisplayBounds = DisplayBounds {
        x: 10.0,
        y: 20.0,
        width: 100.0,
        height: 80.0,
    };

    fn captured_move(x: f64, y: f64) -> CapturedInput {
        CapturedInput {
            event: InputEvent::Mouse(MouseEvent::Move { dx: 1.0, dy: 1.0 }),
            pointer: Some(PointerPosition { x, y }),
        }
    }

    fn captured_key(key: Key, modifiers: Modifiers) -> CapturedInput {
        CapturedInput {
            event: InputEvent::Key(KeyEvent {
                key,
                state: KeyState::Pressed,
                modifiers,
            }),
            pointer: None,
        }
    }

    fn input_payload(event: InputEvent) -> ProtocolPayload {
        ProtocolPayload::Input(InputEventEnvelope::legacy(event))
    }

    #[test]
    fn user_diagnostic_identifies_missing_permissions() {
        let diagnostic = diagnostic_for_error(
            "failed to install macOS event tap; check Input Monitoring permission",
        );

        assert_eq!(diagnostic.kind, DiagnosticKind::PermissionMissing);
        assert_eq!(diagnostic.title, "Permission required");
        assert!(diagnostic
            .next_steps
            .iter()
            .any(|step| step.contains("Input Monitoring")));
    }

    #[test]
    fn user_diagnostic_identifies_connection_failures() {
        let diagnostic = diagnostic_for_error("request failed: connection refused");

        assert_eq!(diagnostic.kind, DiagnosticKind::ConnectionFailed);
        assert_eq!(diagnostic.title, "Connection failed");
        assert!(diagnostic
            .next_steps
            .iter()
            .any(|step| step.contains("network")));
    }

    #[test]
    fn user_diagnostic_identifies_relay_target_offline_as_connection_failure() {
        let diagnostic = diagnostic_for_error(
            "target probe send failed via relay: relay route failed: TargetOffline",
        );

        assert_eq!(diagnostic.kind, DiagnosticKind::ConnectionFailed);
        assert_eq!(diagnostic.title, "Connection failed");
    }

    #[test]
    fn user_diagnostic_identifies_local_ipc_core_service_failures() {
        let diagnostic =
            diagnostic_for_error("local IPC I/O failed: Connection refused (os error 61)");

        assert_eq!(diagnostic.title, "Core service unavailable");
        assert!(diagnostic
            .next_steps
            .iter()
            .any(|step| step.contains("core-service")));
        assert!(diagnostic
            .next_steps
            .iter()
            .any(|step| step.contains("/Applications/KMSync.app")));
    }

    #[test]
    fn direct_lan_candidate_failures_are_retryable_connection_errors() {
        assert_eq!(
            diagnostic_for_error(
                "all direct LAN candidates failed; MdnsLan 10.0.0.9:24800: timed out"
            )
            .kind,
            DiagnosticKind::ConnectionFailed
        );
        assert_eq!(
            diagnostic_for_error("no direct LAN candidates available for target device").kind,
            DiagnosticKind::ConnectionFailed
        );
    }

    #[test]
    fn user_diagnostic_identifies_injection_failures() {
        let diagnostic = diagnostic_for_error("SendInput sent 0/1 events");

        assert_eq!(diagnostic.kind, DiagnosticKind::InjectionFailed);
        assert_eq!(diagnostic.title, "Input injection failed");
        assert!(diagnostic
            .next_steps
            .iter()
            .any(|step| step.contains("interactive desktop")));
    }

    #[test]
    fn daemon_error_carries_structured_kind_into_user_diagnostic() {
        let error = DaemonError::new(
            DiagnosticKind::InjectionFailed,
            "native injector returned a platform failure",
        );

        assert_eq!(error.kind(), DiagnosticKind::InjectionFailed);
        let formatted = format_user_diagnostic(&error);

        assert!(formatted.contains("kmsync: Input injection failed"));
        assert!(formatted.contains("native injector returned a platform failure"));
        assert!(formatted.contains("interactive desktop"));
    }

    #[test]
    fn run_argument_errors_are_structured_daemon_errors() {
        let error = run_with_args(["capture-connect".to_string()].into_iter())
            .expect_err("missing capture-connect args should fail");

        assert_eq!(error.kind(), DiagnosticKind::Unknown);
        assert!(error
            .to_string()
            .contains("capture-connect requires daemon config path"));
    }

    #[test]
    fn formatted_user_diagnostic_includes_context_and_next_steps() {
        let error = DaemonError::from_message("unsupported macOS key: MediaPlay");
        let formatted = format_user_diagnostic(&error);

        assert!(formatted.contains("kmsync: Input injection failed"));
        assert!(formatted.contains("details: unsupported macOS key: MediaPlay"));
        assert!(formatted.contains("next steps:"));
        assert!(formatted.contains("keyboard mapping"));
    }

    #[test]
    fn self_test_report_covers_local_capability_and_network_checks() {
        let report = render_self_test_report(SelfTestReport {
            profile_name: ProfileName::MacToWindows,
            input_event_type: "key",
            mapped_event_type: "key",
            capabilities: platform::PlatformCapabilities {
                input_capture: true,
                input_injection: false,
                clipboard_text: true,
            },
            permission_checks: vec![platform::PlatformPermissionCheck {
                id: "macos.accessibility",
                label: "macOS Accessibility",
                status: platform::PermissionStatus::Missing,
                guidance: "Grant Accessibility permission to KMSync.",
            }],
            permission_hints: &["Enable Accessibility for input injection."],
            network_quic: Ok(()),
        });

        assert!(report.contains("self-test"));
        assert!(report.contains("profile=MacToWindows profile_mapping=ok"));
        assert!(report.contains("input_capture=ok"));
        assert!(report.contains("input_injection=unavailable"));
        assert!(report.contains("clipboard_text=ok"));
        assert!(report.contains("permission_check=macos.accessibility"));
        assert!(report.contains("status=missing"));
        assert!(report.contains("label=\"macOS Accessibility\""));
        assert!(report.contains("guidance=\"Grant Accessibility permission to KMSync.\""));
        assert!(report.contains("network_quic=ok"));
        assert!(report.contains("permission_hint=Enable Accessibility for input injection."));
    }

    #[test]
    fn self_test_report_includes_network_failure() {
        let report = render_self_test_report(SelfTestReport {
            profile_name: ProfileName::WindowsToMac,
            input_event_type: "key",
            mapped_event_type: "key",
            capabilities: platform::PlatformCapabilities {
                input_capture: false,
                input_injection: false,
                clipboard_text: false,
            },
            permission_checks: Vec::new(),
            permission_hints: &[],
            network_quic: Err("bind failed".to_string()),
        });

        assert!(report.contains("network_quic=failed error=bind failed"));
        assert!(report.contains("input_capture=unavailable"));
        assert!(report.contains("clipboard_text=unavailable"));
    }

    #[test]
    fn args_parse_accepts_connection_diagnostics_command() {
        let args = Args::parse(
            [
                "connection-diagnostics",
                "configs/daemon.example.json",
                "windows-device",
            ]
            .into_iter()
            .map(String::from),
        )
        .expect("parse connection diagnostics");

        match args.command {
            Command::ConnectionDiagnostics {
                config_path,
                target_device_id,
            } => {
                assert_eq!(config_path, PathBuf::from("configs/daemon.example.json"));
                assert_eq!(target_device_id, "windows-device");
            }
            _ => panic!("expected connection diagnostics command"),
        }
    }

    #[test]
    fn args_parse_accepts_desktop_diagnostics_with_default_config() {
        let default_config =
            PathBuf::from("user/Library/Application Support/KMSync/daemon.example.json");
        let args = Args::parse_with_default_config(
            ["desktop-diagnostics"].into_iter().map(String::from),
            default_config.clone(),
        )
        .expect("parse desktop diagnostics");

        match args.command {
            Command::DesktopDiagnostics {
                config_path,
                probe_targets,
            } => {
                assert_eq!(config_path, default_config);
                assert!(!probe_targets);
            }
            _ => panic!("expected desktop diagnostics command"),
        }
    }

    #[test]
    fn args_parse_accepts_desktop_diagnostics_probe_targets_flag() {
        let default_config =
            PathBuf::from("user/Library/Application Support/KMSync/daemon.example.json");
        let args = Args::parse_with_default_config(
            ["desktop-diagnostics", "--probe-targets"]
                .into_iter()
                .map(String::from),
            default_config.clone(),
        )
        .expect("parse desktop diagnostics probe targets");

        match args.command {
            Command::DesktopDiagnostics {
                config_path,
                probe_targets,
            } => {
                assert_eq!(config_path, default_config);
                assert!(probe_targets);
            }
            _ => panic!("expected desktop diagnostics command"),
        }
    }

    #[test]
    fn args_parse_accepts_update_check_command() {
        let args = Args::parse(
            [
                "update-check",
                "configs/daemon.example.json",
                "windows-device",
                "windows",
                "0.1.0",
                "stable",
            ]
            .into_iter()
            .map(String::from),
        )
        .expect("parse update check");

        match args.command {
            Command::UpdateCheck {
                config_path,
                device_id,
                platform,
                version,
                channel,
            } => {
                assert_eq!(config_path, PathBuf::from("configs/daemon.example.json"));
                assert_eq!(device_id.as_deref(), Some("windows-device"));
                assert_eq!(platform.as_deref(), Some("windows"));
                assert_eq!(version.as_deref(), Some("0.1.0"));
                assert_eq!(channel.as_deref(), Some("stable"));
            }
            _ => panic!("expected update check command"),
        }
    }

    #[test]
    fn args_parse_accepts_request_permissions_command() {
        let args = Args::parse(["request-permissions"].into_iter().map(String::from))
            .expect("parse request permissions");

        assert!(matches!(args.command, Command::RequestPermissions));
    }

    #[test]
    fn args_parse_accepts_local_ipc_commands() {
        let default_endpoint = local_ipc::default_local_ipc_endpoint();

        let endpoint_args = Args::parse(["ipc-endpoint"].into_iter().map(String::from))
            .expect("parse ipc endpoint");
        assert!(matches!(endpoint_args.command, Command::LocalIpcEndpoint));

        let ping_args = Args::parse(
            ["ipc-ping", "custom-endpoint"]
                .into_iter()
                .map(String::from),
        )
        .expect("parse ipc ping");
        match ping_args.command {
            Command::LocalIpcPing { endpoint } => {
                assert_eq!(endpoint.transport, default_endpoint.transport);
                assert_eq!(endpoint.address, "custom-endpoint");
            }
            _ => panic!("expected local ipc ping command"),
        }

        let serve_args = Args::parse(["ipc-serve-once"].into_iter().map(String::from))
            .expect("parse ipc serve once");
        match serve_args.command {
            Command::LocalIpcServeOnce { endpoint } => {
                assert_eq!(endpoint, default_endpoint);
            }
            _ => panic!("expected local ipc serve once command"),
        }
    }

    #[test]
    fn args_parse_accepts_ui_control_commands_on_daemon_entrypoint() {
        let status_args = Args::parse(["status", "custom-endpoint"].into_iter().map(String::from))
            .expect("parse daemon status command");
        match status_args.command {
            Command::Ui { args } => {
                assert_eq!(args, vec!["status", "custom-endpoint"]);
            }
            _ => panic!("expected ui command passthrough"),
        }

        let control_panel_args = Args::parse(
            [
                "control-panel",
                "configs/mac-to-windows.profile.json",
                "target/kmsync-control.html",
            ]
            .into_iter()
            .map(String::from),
        )
        .expect("parse daemon control-panel command");
        match control_panel_args.command {
            Command::Ui { args } => {
                assert_eq!(
                    args,
                    vec![
                        "control-panel",
                        "configs/mac-to-windows.profile.json",
                        "target/kmsync-control.html"
                    ]
                );
            }
            _ => panic!("expected ui command passthrough"),
        }
    }

    #[test]
    fn args_parse_accepts_file_send_command() {
        let args = Args::parse(
            ["file-send", "127.0.0.1:24800", "fixtures/secret.txt", "512"]
                .into_iter()
                .map(String::from),
        )
        .expect("parse file-send");

        match args.command {
            Command::FileSend {
                target,
                file_path,
                chunk_bytes,
            } => {
                assert_eq!(target, "127.0.0.1:24800".parse().expect("target"));
                assert_eq!(file_path, PathBuf::from("fixtures/secret.txt"));
                assert_eq!(chunk_bytes, 512);
            }
            _ => panic!("expected file-send command"),
        }
    }

    #[test]
    fn file_transfer_frames_split_metadata_and_chunks_without_paths() {
        let frames = build_file_transfer_frames(
            20,
            0xfeed,
            7,
            "C:/Users/alice/Desktop/secret.txt",
            b"abcdefghij",
            4,
            100,
        )
        .expect("build file transfer frames");

        assert_eq!(frames.len(), 4);
        match &frames[0].payload {
            ProtocolPayload::ClipboardFiles(files) => {
                assert_eq!(files.files.len(), 1);
                assert_eq!(files.files[0].name, "secret.txt");
                assert_eq!(files.files[0].byte_len, 10);
            }
            _ => panic!("expected file metadata frame"),
        }
        for (index, frame) in frames[1..].iter().enumerate() {
            match &frame.payload {
                ProtocolPayload::FileChunk(chunk) => {
                    assert_eq!(chunk.chunk_index, u32::try_from(index).expect("index"));
                    assert_eq!(chunk.data.len(), if index == 2 { 2 } else { 4 });
                    assert_eq!(chunk.is_final, index == 2);
                }
                _ => panic!("expected file chunk frame"),
            }
        }
    }

    #[test]
    fn args_parse_accepts_core_service_command() {
        let default_config = PathBuf::from("configs/daemon.example.json");
        let default_args = Args::parse_with_default_config(
            ["core-service"].into_iter().map(String::from),
            default_config.clone(),
        )
        .expect("parse default core service");
        match default_args.command {
            Command::CoreService { config_path } => {
                assert_eq!(config_path, default_config);
            }
            _ => panic!("expected core service command"),
        }

        let custom_args = Args::parse(
            ["core-service", "configs/custom-daemon.json"]
                .into_iter()
                .map(String::from),
        )
        .expect("parse custom core service");
        match custom_args.command {
            Command::CoreService { config_path } => {
                assert_eq!(config_path, PathBuf::from("configs/custom-daemon.json"));
            }
            _ => panic!("expected core service command"),
        }
    }

    #[test]
    fn args_parse_ignores_macos_process_serial_number_before_command() {
        let default_config = PathBuf::from("configs/daemon.example.json");
        let args = Args::parse_with_default_config(
            ["-psn_0_12345", "core-service"]
                .into_iter()
                .map(String::from),
            default_config.clone(),
        )
        .expect("parse LaunchServices core service");

        match args.command {
            Command::CoreService { config_path } => {
                assert_eq!(config_path, default_config);
            }
            _ => panic!("expected core service command"),
        }
    }

    #[test]
    fn macos_launch_services_context_does_not_require_psn_arg_when_parent_is_launchd() {
        let context = macos_core_service_launch_context_from_args_executable_and_parent(
            ["core-service", "configs/daemon.example.json"]
                .into_iter()
                .map(String::from),
            Path::new("/Applications/KMSync.app/Contents/MacOS/kmsync"),
            Some(1),
        );

        assert_eq!(
            context.as_deref(),
            Some(MACOS_CORE_SERVICE_LAUNCH_SERVICES_CONTEXT)
        );
    }

    #[test]
    fn macos_core_service_skips_foreground_permission_capture_worker() {
        assert!(!core_service_should_spawn_desktop_capture("macos"));
        assert!(core_service_should_spawn_desktop_capture("windows"));
        assert!(core_service_should_spawn_desktop_capture("linux"));
    }

    #[test]
    fn native_desktop_window_owns_macos_foreground_capture() {
        assert!(desktop_should_spawn_foreground_capture("macos"));
        assert!(!desktop_should_spawn_foreground_capture("windows"));
        assert!(!desktop_should_spawn_foreground_capture("linux"));
    }

    #[test]
    fn args_parse_without_arguments_starts_core_service_for_desktop_launch() {
        let default_config = PathBuf::from("portable/config/daemon.example.json");
        let args = Args::parse_with_default_config(std::iter::empty(), default_config.clone())
            .expect("parse default desktop launch");

        match args.command {
            Command::Desktop {
                config_path,
                output_path,
            } => {
                assert_eq!(config_path, default_config);
                assert_eq!(output_path, None);
            }
            _ => panic!("expected default desktop launch to open native desktop window"),
        }
    }

    #[test]
    fn desktop_launch_without_output_uses_native_window() {
        assert_eq!(desktop_launch_mode(None), DesktopLaunchMode::NativeWindow);
    }

    #[test]
    fn desktop_launch_with_output_keeps_html_export() {
        assert_eq!(
            desktop_launch_mode(Some(Path::new("target/kmsync-desktop.html"))),
            DesktopLaunchMode::HtmlExport(PathBuf::from("target/kmsync-desktop.html"))
        );
    }

    #[test]
    fn desktop_autostart_keeps_existing_core_service_when_local_ipc_responds() {
        let action = desktop_core_service_autostart_action(
            Ok(CoreServiceHealth {
                version: env!("CARGO_PKG_VERSION").to_string(),
                input_hot_path: "daemon_data_plane".to_string(),
                config_path: Some(PathBuf::from(
                    "/Users/alice/Library/Application Support/KMSync/daemon.example.json",
                )),
                device_id: Some("device-a".to_string()),
                launch_context: Some(MACOS_CORE_SERVICE_LAUNCH_SERVICES_CONTEXT.to_string()),
            }),
            Path::new("/Applications/KMSync.app/Contents/MacOS/kmsync"),
            Path::new("/Users/alice/Library/Application Support/KMSync/daemon.example.json"),
            "macos",
        );

        assert_eq!(action, CoreServiceAutostartAction::AlreadyRunning);
    }

    #[test]
    fn desktop_autostart_restarts_core_service_with_mismatched_config_path() {
        let action = desktop_core_service_autostart_action(
            Ok(CoreServiceHealth {
                version: env!("CARGO_PKG_VERSION").to_string(),
                input_hot_path: "daemon_data_plane".to_string(),
                config_path: Some(PathBuf::from(
                    "/Users/admin/Library/Application Support/KMSync/daemon.example.json",
                )),
                device_id: Some("admin-device".to_string()),
                launch_context: Some(MACOS_CORE_SERVICE_LAUNCH_SERVICES_CONTEXT.to_string()),
            }),
            Path::new("/Applications/KMSync.app/Contents/MacOS/kmsync"),
            Path::new("/Users/alice/Library/Application Support/KMSync/daemon.example.json"),
            "macos",
        );

        assert!(matches!(action, CoreServiceAutostartAction::Restart { .. }));
    }

    #[test]
    fn desktop_autostart_uses_launch_services_for_macos_app_bundle() {
        let action = desktop_core_service_autostart_action(
            Err("local ipc unavailable".to_string()),
            Path::new("/Applications/KMSync.app/Contents/MacOS/kmsync"),
            Path::new("/Users/alice/Library/Application Support/KMSync/daemon.example.json"),
            "macos",
        );

        assert_eq!(
            action,
            CoreServiceAutostartAction::Start(CoreServiceAutostartCommand {
                program: PathBuf::from("/usr/bin/open"),
                args: vec![
                    "-n".to_string(),
                    "/Applications/KMSync.app".to_string(),
                    "--args".to_string(),
                    "core-service".to_string(),
                    "/Users/alice/Library/Application Support/KMSync/daemon.example.json"
                        .to_string(),
                ],
            })
        );
    }

    #[test]
    fn desktop_autostart_restarts_macos_core_service_without_launch_services_context() {
        let action = desktop_core_service_autostart_action(
            Ok(CoreServiceHealth {
                version: env!("CARGO_PKG_VERSION").to_string(),
                input_hot_path: "daemon_data_plane".to_string(),
                config_path: Some(PathBuf::from(
                    "/Users/alice/Library/Application Support/KMSync/daemon.example.json",
                )),
                device_id: Some("device-a".to_string()),
                launch_context: None,
            }),
            Path::new("/Applications/KMSync.app/Contents/MacOS/kmsync"),
            Path::new("/Users/alice/Library/Application Support/KMSync/daemon.example.json"),
            "macos",
        );

        assert!(matches!(action, CoreServiceAutostartAction::Restart { .. }));
    }

    #[test]
    fn desktop_autostart_runs_core_service_from_current_binary_for_portable_launches() {
        let action = desktop_core_service_autostart_action(
            Err("local ipc unavailable".to_string()),
            Path::new("/opt/kmsync/kmsync"),
            Path::new("/home/alice/.config/kmsync/daemon.example.json"),
            "linux",
        );

        assert_eq!(
            action,
            CoreServiceAutostartAction::Start(CoreServiceAutostartCommand {
                program: PathBuf::from("/opt/kmsync/kmsync"),
                args: vec![
                    "core-service".to_string(),
                    "/home/alice/.config/kmsync/daemon.example.json".to_string(),
                ],
            })
        );
    }

    #[test]
    fn desktop_autostart_replaces_legacy_ipc_status_that_is_not_the_hot_path() {
        let action = desktop_core_service_autostart_action(
            Ok(CoreServiceHealth {
                version: env!("CARGO_PKG_VERSION").to_string(),
                input_hot_path: "not_on_local_ipc".to_string(),
                config_path: None,
                device_id: None,
                launch_context: None,
            }),
            Path::new("/Applications/KMSync.app/Contents/MacOS/kmsync"),
            Path::new("/Users/alice/Library/Application Support/KMSync/daemon.example.json"),
            "macos",
        );

        assert!(matches!(action, CoreServiceAutostartAction::Restart { .. }));
    }

    #[test]
    fn desktop_autostart_restarts_incompatible_macos_core_service_before_launch_services() {
        let action = desktop_core_service_autostart_action(
            Ok(CoreServiceHealth {
                version: env!("CARGO_PKG_VERSION").to_string(),
                input_hot_path: "not_on_local_ipc".to_string(),
                config_path: None,
                device_id: None,
                launch_context: None,
            }),
            Path::new("/Applications/KMSync.app/Contents/MacOS/kmsync"),
            Path::new("/Users/alice/Library/Application Support/KMSync/daemon.example.json"),
            "macos",
        );

        assert_eq!(
            action,
            CoreServiceAutostartAction::Restart {
                stop: CoreServiceAutostartCommand {
                    program: PathBuf::from("/usr/bin/pkill"),
                    args: vec![
                        "-f".to_string(),
                        "/Applications/KMSync\\.app/Contents/MacOS/kmsync .*core-service"
                            .to_string(),
                    ],
                },
                start: CoreServiceAutostartCommand {
                    program: PathBuf::from("/usr/bin/open"),
                    args: vec![
                        "-n".to_string(),
                        "/Applications/KMSync.app".to_string(),
                        "--args".to_string(),
                        "core-service".to_string(),
                        "/Users/alice/Library/Application Support/KMSync/daemon.example.json"
                            .to_string(),
                    ],
                },
            }
        );
    }

    #[test]
    fn desktop_autostart_restarts_incompatible_windows_core_service_before_user_companion() {
        let root = unique_test_dir("windows-autostart-restart");
        let install_dir = root.join("Program Files").join("KMSync");
        std::fs::create_dir_all(&install_dir).expect("create install dir");
        let exe_path = install_dir.join("kmsync.exe");
        std::fs::write(install_dir.join("Uninstall.exe"), "").expect("write uninstall marker");
        let config_path = root
            .join("AppData")
            .join("Roaming")
            .join("KMSync")
            .join("daemon.example.json");

        let action = desktop_core_service_autostart_action(
            Ok(CoreServiceHealth {
                version: "0.0.0".to_string(),
                input_hot_path: "not_on_local_ipc".to_string(),
                config_path: None,
                device_id: None,
                launch_context: None,
            }),
            &exe_path,
            &config_path,
            "windows",
        );

        let CoreServiceAutostartAction::Restart { stop, start } = action else {
            panic!("expected incompatible Windows core-service to be restarted");
        };
        assert_eq!(stop.program, PathBuf::from("powershell.exe"));
        assert!(stop.args.iter().any(|arg| arg.contains("core-service")));
        assert!(stop.args.iter().any(|arg| arg.contains("kmsync.exe")));
        assert_eq!(start.program, exe_path);
        assert_eq!(
            start.args,
            vec![
                "core-service".to_string(),
                config_path.display().to_string(),
            ]
        );

        std::fs::remove_dir_all(root).expect("cleanup temp package");
    }

    #[test]
    fn desktop_autostart_replaces_local_ipc_with_mismatched_version() {
        let action = desktop_core_service_autostart_action(
            Ok(CoreServiceHealth {
                version: "0.0.0".to_string(),
                input_hot_path: "daemon_data_plane".to_string(),
                config_path: Some(PathBuf::from(
                    "/home/alice/.config/kmsync/daemon.example.json",
                )),
                device_id: Some("device-a".to_string()),
                launch_context: None,
            }),
            Path::new("/opt/kmsync/kmsync"),
            Path::new("/home/alice/.config/kmsync/daemon.example.json"),
            "linux",
        );

        assert!(matches!(action, CoreServiceAutostartAction::Start(_)));
    }

    #[test]
    fn args_parse_accepts_desktop_command_with_output_path() {
        let args = Args::parse(
            [
                "desktop",
                "configs/daemon.example.json",
                "target/kmsync-desktop.html",
            ]
            .into_iter()
            .map(String::from),
        )
        .expect("parse desktop command");

        match args.command {
            Command::Desktop {
                config_path,
                output_path,
            } => {
                assert_eq!(config_path, PathBuf::from("configs/daemon.example.json"));
                assert_eq!(
                    output_path,
                    Some(PathBuf::from("target/kmsync-desktop.html"))
                );
            }
            _ => panic!("expected desktop command"),
        }
    }

    #[test]
    fn args_parse_accepts_desktop_output_flag_with_default_config() {
        let default_config =
            PathBuf::from("user/Library/Application Support/KMSync/daemon.example.json");
        let args = Args::parse_with_default_config(
            ["desktop", "--output", "target/kmsync-desktop.html"]
                .into_iter()
                .map(String::from),
            default_config.clone(),
        )
        .expect("parse desktop output flag");

        match args.command {
            Command::Desktop {
                config_path,
                output_path,
            } => {
                assert_eq!(config_path, default_config);
                assert_eq!(
                    output_path,
                    Some(PathBuf::from("target/kmsync-desktop.html"))
                );
            }
            _ => panic!("expected desktop command"),
        }
    }

    #[test]
    fn help_command_prints_help_without_running_info() {
        let args = Args::parse_with_default_config(
            ["--help"].into_iter().map(String::from),
            PathBuf::from("portable/config/daemon.example.json"),
        )
        .expect("parse help");

        assert!(matches!(args.command, Command::Help));
    }

    #[test]
    fn default_daemon_config_path_prefers_portable_config_next_to_bin_dir() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = env::temp_dir().join(format!("kmsync-portable-config-{suffix}"));
        let bin_dir = root.join("bin");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(config_dir.join("daemon.example.json"), "{}").expect("write config");

        let config_path = default_daemon_config_path_from(
            Path::new("unrelated-current-dir"),
            &bin_dir.join("kmsync.exe"),
        );

        assert_eq!(config_path, config_dir.join("daemon.example.json"));

        std::fs::remove_dir_all(root).expect("cleanup temp package");
    }

    #[test]
    fn default_daemon_config_path_uses_macos_user_config_when_no_portable_config_exists() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = env::temp_dir().join(format!("kmsync-macos-user-config-{suffix}"));
        let home = root.join("home");
        std::fs::create_dir_all(&home).expect("create home dir");

        let config_path = default_daemon_config_path_from_env(
            Path::new("/"),
            Path::new("/usr/local/bin/kmsync"),
            Some(home.as_path()),
            None,
            "macos",
        );

        assert_eq!(
            config_path,
            home.join("Library")
                .join("Application Support")
                .join("KMSync")
                .join("daemon.example.json")
        );

        std::fs::remove_dir_all(root).expect("cleanup temp package");
    }

    #[test]
    fn default_daemon_config_path_uses_macos_user_config_for_installed_app_bundle() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = env::temp_dir().join(format!("kmsync-macos-app-config-{suffix}"));
        let home = root.join("home");
        let contents_dir = root
            .join("Applications")
            .join("KMSync.app")
            .join("Contents");
        let macos_dir = contents_dir.join("MacOS");
        let bundled_config_dir = contents_dir.join("configs");
        std::fs::create_dir_all(&home).expect("create home dir");
        std::fs::create_dir_all(&macos_dir).expect("create macos dir");
        std::fs::create_dir_all(&bundled_config_dir).expect("create bundled config dir");
        std::fs::write(bundled_config_dir.join(DEFAULT_DAEMON_CONFIG_FILE), "{}")
            .expect("write bundled config");

        let config_path = default_daemon_config_path_from_env(
            Path::new("/"),
            &macos_dir.join("kmsync"),
            Some(home.as_path()),
            None,
            "macos",
        );

        assert_eq!(
            config_path,
            home.join("Library")
                .join("Application Support")
                .join("KMSync")
                .join(DEFAULT_DAEMON_CONFIG_FILE)
        );

        std::fs::remove_dir_all(root).expect("cleanup temp package");
    }

    #[test]
    fn default_daemon_config_path_uses_windows_user_config_for_installed_package() {
        let root = unique_test_dir("windows-installed-config");
        let appdata = root.join("AppData").join("Roaming");
        let install_dir = root.join("Program Files").join("KMSync");
        let install_config_dir = install_dir.join("configs");
        std::fs::create_dir_all(&appdata).expect("create appdata");
        std::fs::create_dir_all(&install_config_dir).expect("create install config dir");
        std::fs::write(install_dir.join("Uninstall.exe"), "").expect("write uninstall marker");
        std::fs::write(install_config_dir.join(DEFAULT_DAEMON_CONFIG_FILE), "{}")
            .expect("write install config");

        let config_path = default_daemon_config_path_from_env(
            Path::new(r"C:\Windows\System32"),
            &install_dir.join("kmsync.exe"),
            None,
            Some(appdata.as_path()),
            "windows",
        );

        assert_eq!(
            config_path,
            appdata.join("KMSync").join(DEFAULT_DAEMON_CONFIG_FILE)
        );

        std::fs::remove_dir_all(root).expect("cleanup temp package");
    }

    #[test]
    fn seed_default_daemon_config_migrates_legacy_config_and_identity() {
        let root = unique_test_dir("windows-config-migration");
        let install_dir = root.join("Program Files").join("KMSync");
        let legacy_config_dir = install_dir.join("configs");
        let user_config_dir = root.join("AppData").join("Roaming").join("KMSync");
        let user_config_path = user_config_dir.join(DEFAULT_DAEMON_CONFIG_FILE);
        std::fs::create_dir_all(&legacy_config_dir).expect("create legacy config dir");
        std::fs::write(
            legacy_config_dir.join(DEFAULT_DAEMON_CONFIG_FILE),
            r#"{
  "server_url": "http://203.0.113.10:24888",
  "device_name": "Existing PC",
  "identity_path": "kmsync-device-identity.json",
  "listen_port": 24800,
  "heartbeat_interval_seconds": 15,
  "role": "client"
}
"#,
        )
        .expect("write legacy config");
        std::fs::write(
            legacy_config_dir.join("kmsync-device-identity.json"),
            r#"{"device_id":"stable-device","public_key":"ed25519:stable","private_key_ref":{"store":"system","service":"kmsync-device-identity","account":"device-stable"}}"#,
        )
        .expect("write legacy identity");

        seed_default_daemon_config_if_needed(&user_config_path, &install_dir.join("kmsync.exe"))
            .expect("seed config");

        let migrated_config = std::fs::read_to_string(&user_config_path).expect("read config");
        let migrated_identity =
            std::fs::read_to_string(user_config_dir.join("kmsync-device-identity.json"))
                .expect("read identity");
        assert!(migrated_config.contains("\"device_name\": \"Existing PC\""));
        assert!(migrated_identity.contains("\"device_id\":\"stable-device\""));

        std::fs::remove_dir_all(root).expect("cleanup temp package");
    }

    #[test]
    fn desktop_server_probe_result_sets_terminal_statuses() {
        assert_eq!(
            desktop_server_probe_result_to_state(Ok(())),
            (kmsync_core::DesktopConnectionState::Connected, None)
        );

        let (state, error) =
            desktop_server_probe_result_to_state(Err("request failed".to_string()));

        assert_eq!(state, kmsync_core::DesktopConnectionState::Disconnected);
        assert_eq!(error.as_deref(), Some("request failed"));
    }

    #[test]
    fn core_service_plan_binds_data_plane_and_keeps_input_off_local_ipc() {
        let config = client::ClientConfig {
            server_url: "http://127.0.0.1:24888".to_string(),
            device_name: "devbox".to_string(),
            role: kmsync_core::DesktopRole::Client,
            listen_port: 24_800,
            heartbeat_interval_seconds: 15,
            identity_path: PathBuf::from("identity.json"),
        };
        let plan =
            CoreServicePlan::from_config(PathBuf::from("configs/daemon.example.json"), &config);

        assert_eq!(
            plan.bind,
            SocketAddr::from(([0, 0, 0, 0], config.listen_port))
        );
        assert_eq!(
            plan.config_path,
            PathBuf::from("configs/daemon.example.json")
        );
        assert_eq!(plan.ipc_endpoint, local_ipc::default_local_ipc_endpoint());
        assert_eq!(plan.input_hot_path, "daemon_data_plane");
        assert_eq!(plan.control_plane, "local_ipc_and_heartbeat");
    }

    #[test]
    fn desktop_capture_plan_routes_master_layout_edges_to_online_peers() {
        let state = kmsync_core::DesktopState {
            device: kmsync_core::DesktopDeviceState {
                id: Some("master-device".to_string()),
                name: "Master".to_string(),
                os: "windows".to_string(),
                app_version: "0.1.0".to_string(),
                role: kmsync_core::DesktopRole::Master,
                sync_relay_status_known: false,
                sync_relay_online: false,
            },
            layout: kmsync_core::DesktopLayout {
                left: Some("offline-device".to_string()),
                right: Some("right-device".to_string()),
                top: None,
                bottom: None,
            },
            devices: vec![
                kmsync_core::DesktopPeerState {
                    id: "right-device".to_string(),
                    name: "Right".to_string(),
                    os: "macos".to_string(),
                    online: true,
                    sync_relay_status_known: true,
                    sync_relay_online: true,
                    lan_ips: vec!["192.168.1.20".to_string()],
                    public_ip: None,
                    listen_port: Some(24_800),
                    last_seen_at: Some(123),
                    display: Some(kmsync_core::DesktopDisplayState {
                        width_px: 1000,
                        height_px: 700,
                    }),
                },
                kmsync_core::DesktopPeerState {
                    id: "offline-device".to_string(),
                    name: "Offline".to_string(),
                    os: "linux".to_string(),
                    online: false,
                    sync_relay_status_known: true,
                    sync_relay_online: false,
                    lan_ips: vec!["192.168.1.21".to_string()],
                    public_ip: None,
                    listen_port: Some(24_800),
                    last_seen_at: Some(100),
                    display: None,
                },
            ],
            ..kmsync_core::DesktopState::default()
        };

        let plan = desktop_capture_plan_from_state(&state);

        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.targets[0].edge, Edge::Right);
        assert_eq!(plan.targets[0].target_device_id, "right-device");
        assert_eq!(plan.targets[0].profile_name, ProfileName::WindowsToMac);
        assert_eq!(
            plan.targets[0].display,
            Some(kmsync_core::DesktopDisplayState {
                width_px: 1000,
                height_px: 700,
            })
        );
    }

    #[test]
    fn desktop_capture_plan_is_empty_for_unconfigured_clients() {
        let state = kmsync_core::DesktopState {
            device: kmsync_core::DesktopDeviceState {
                id: Some("client-device".to_string()),
                name: "Client".to_string(),
                os: "windows".to_string(),
                app_version: "0.1.0".to_string(),
                role: kmsync_core::DesktopRole::Client,
                sync_relay_status_known: false,
                sync_relay_online: false,
            },
            layout: kmsync_core::DesktopLayout {
                right: Some("right-device".to_string()),
                ..kmsync_core::DesktopLayout::default()
            },
            ..kmsync_core::DesktopState::default()
        };

        assert!(desktop_capture_plan_from_state(&state).targets.is_empty());
    }

    #[test]
    fn desktop_capture_idle_log_line_is_privacy_safe() {
        let line = desktop_capture_idle_log_line();

        assert_eq!(line, "desktop_capture=idle reason=no_online_layout_targets");
        assert!(!line.contains("key"));
        assert!(!line.contains("mouse"));
    }

    #[test]
    fn desktop_capture_plan_log_line_lists_targets_without_input_content() {
        let plan = DesktopCapturePlan {
            targets: vec![DesktopCaptureTarget {
                edge: Edge::Right,
                target_device_id: "right-device".to_string(),
                profile_name: ProfileName::MacToWindows,
                display: None,
            }],
        };

        let line = desktop_capture_plan_log_line(&plan);

        assert_eq!(
            line,
            "desktop_capture=armed targets=1 target=right-device edge=right profile=mac_to_windows"
        );
        assert!(!line.contains("key="));
        assert!(!line.contains("clipboard"));
    }

    #[test]
    fn desktop_sync_runtime_status_round_trips_next_to_config() {
        let root = unique_test_dir("desktop-sync-runtime");
        std::fs::create_dir_all(&root).expect("create runtime dir");
        let config_path = root.join(DEFAULT_DAEMON_CONFIG_FILE);
        let runtime = kmsync_core::DesktopSyncRuntimeState {
            state: kmsync_core::DesktopSyncRuntimeKind::Armed,
            error: None,
            targets: vec!["right-device".to_string()],
            updated_at: Some(123),
            ..kmsync_core::DesktopSyncRuntimeState::default()
        };

        write_desktop_sync_runtime_status(&config_path, &runtime).expect("write runtime");

        assert_eq!(
            desktop_sync_runtime_status_path(&config_path),
            root.join("desktop-runtime.json")
        );
        assert_eq!(
            read_desktop_sync_runtime_status(&config_path)
                .expect("read runtime")
                .state,
            runtime.state
        );
        let stored = read_desktop_sync_runtime_status(&config_path).expect("read stored runtime");
        assert_eq!(stored.targets, runtime.targets);
        assert_eq!(stored.error, runtime.error);
        assert!(
            stored.updated_at.is_some(),
            "runtime status should record write time"
        );

        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn desktop_capture_plan_runtime_status_lists_targets() {
        let plan = DesktopCapturePlan {
            targets: vec![DesktopCaptureTarget {
                edge: Edge::Right,
                target_device_id: "right-device".to_string(),
                profile_name: ProfileName::MacToWindows,
                display: None,
            }],
        };

        let runtime = desktop_capture_plan_runtime_status(&plan);

        assert_eq!(runtime.state, kmsync_core::DesktopSyncRuntimeKind::Armed);
        assert_eq!(runtime.targets, vec!["right-device"]);
        assert_eq!(runtime.error, None);
    }

    #[test]
    fn desktop_capture_observed_runtime_status_reports_capture_and_route_counts() {
        let plan = DesktopCapturePlan {
            targets: vec![DesktopCaptureTarget {
                edge: Edge::Right,
                target_device_id: "right-device".to_string(),
                profile_name: ProfileName::MacToWindows,
                display: None,
            }],
        };
        let counters = DesktopCaptureRuntimeCounters::default();
        counters.record_captured();
        counters.record_captured();
        counters.record_routed();
        counters.record_capture_observation(
            Some(PointerPosition {
                x: 1432.2,
                y: 811.7,
            }),
            Some(Edge::Right),
            Some("right-device"),
            true,
        );

        let runtime = desktop_capture_observed_runtime_status(&plan, &counters)
            .expect("observed status should be available without transmit failure");

        assert_eq!(runtime.state, kmsync_core::DesktopSyncRuntimeKind::Armed);
        assert_eq!(runtime.targets, vec!["right-device"]);
        assert_eq!(runtime.captured_events, 2);
        assert_eq!(runtime.routed_events, 1);
        assert_eq!(runtime.last_capture_pointer_x, Some(1432));
        assert_eq!(runtime.last_capture_pointer_y, Some(812));
        assert_eq!(runtime.last_capture_edge.as_deref(), Some("right"));
        assert_eq!(runtime.last_capture_target.as_deref(), Some("right-device"));
        assert!(runtime.last_capture_routed);
    }

    #[test]
    fn desktop_capture_observed_runtime_status_pauses_after_transmit_failure() {
        let plan = DesktopCapturePlan {
            targets: vec![DesktopCaptureTarget {
                edge: Edge::Right,
                target_device_id: "right-device".to_string(),
                profile_name: ProfileName::MacToWindows,
                display: None,
            }],
        };
        let counters = DesktopCaptureRuntimeCounters::default();
        counters.record_captured();
        counters.record_routed();
        counters.record_transmit_failed();

        let runtime = desktop_capture_observed_runtime_status(&plan, &counters);

        assert_eq!(runtime, None);
    }

    #[test]
    fn desktop_capture_failed_runtime_status_preserves_targets_and_error() {
        let plan = DesktopCapturePlan {
            targets: vec![DesktopCaptureTarget {
                edge: Edge::Right,
                target_device_id: "right-device".to_string(),
                profile_name: ProfileName::MacToWindows,
                display: None,
            }],
        };

        let runtime = desktop_capture_failed_runtime_status(&plan, "missing permission");

        assert_eq!(runtime.state, kmsync_core::DesktopSyncRuntimeKind::Failed);
        assert_eq!(runtime.targets, vec!["right-device"]);
        assert_eq!(runtime.error.as_deref(), Some("missing permission"));
    }

    #[test]
    fn desktop_capture_probe_failed_runtime_status_records_target_transport_and_error() {
        let plan = DesktopCapturePlan {
            targets: vec![DesktopCaptureTarget {
                edge: Edge::Right,
                target_device_id: "right-device".to_string(),
                profile_name: ProfileName::MacToWindows,
                display: None,
            }],
        };
        let failure = TargetProbeFailure {
            transport: Some("relay".to_string()),
            message: "target probe send failed via relay: relay route failed: TargetOffline"
                .to_string(),
        };

        let runtime = desktop_capture_probe_failed_runtime_status(&plan, "right-device", &failure);

        assert_eq!(runtime.state, kmsync_core::DesktopSyncRuntimeKind::Failed);
        assert_eq!(runtime.targets, vec!["right-device"]);
        let error = runtime.error.as_deref().expect("probe error");
        assert!(error.contains("right-device"));
        assert!(error.contains("transport=relay"));
        assert!(error.contains("TargetOffline"));
    }

    #[test]
    fn desktop_capture_startup_probe_writer_does_not_block_capture_startup() {
        let plan = DesktopCapturePlan {
            targets: vec![DesktopCaptureTarget {
                edge: Edge::Right,
                target_device_id: "right-device".to_string(),
                profile_name: ProfileName::MacToWindows,
                display: None,
            }],
        };
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();

        let handle = spawn_desktop_capture_startup_probe_status_writer_with(
            plan,
            move |_target_device_id| {
                entered_tx.send(()).expect("signal probe entered");
                release_rx.recv().expect("release probe");
                Err(TargetProbeFailure {
                    transport: Some("relay".to_string()),
                    message: "blocked probe released".to_string(),
                })
            },
            |_runtime| {},
        );

        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("startup probe should be running in the background");
        release_tx.send(()).expect("release background probe");
        handle.join().expect("probe writer thread");
    }

    #[test]
    fn desktop_capture_transmit_failed_runtime_status_names_target_and_error() {
        let counters = DesktopCaptureRuntimeCounters::default();
        let runtime = desktop_capture_transmit_failed_runtime_status(
            "right-device",
            "direct connection refused",
            &counters,
        );

        assert_eq!(runtime.state, kmsync_core::DesktopSyncRuntimeKind::Failed);
        assert_eq!(runtime.targets, vec!["right-device"]);
        let error = runtime.error.as_deref().expect("runtime error");
        assert!(error.contains("right-device"));
        assert!(error.contains("direct connection refused"));
    }

    #[test]
    fn desktop_capture_transmit_failed_runtime_status_preserves_observed_counts() {
        let counters = DesktopCaptureRuntimeCounters::default();
        counters.record_captured();
        counters.record_captured();
        counters.record_routed();
        counters.record_sent();

        let runtime = desktop_capture_transmit_failed_runtime_status(
            "right-device",
            "relay offline",
            &counters,
        );

        assert_eq!(runtime.state, kmsync_core::DesktopSyncRuntimeKind::Failed);
        assert_eq!(runtime.captured_events, 2);
        assert_eq!(runtime.routed_events, 1);
        assert_eq!(runtime.sent_events, 1);
    }

    #[test]
    fn desktop_capture_transmit_recovered_runtime_status_clears_error() {
        let targets = ["right-device", "left-device"];

        let runtime = desktop_capture_transmit_recovered_runtime_status(targets);

        assert_eq!(runtime.state, kmsync_core::DesktopSyncRuntimeKind::Armed);
        assert_eq!(runtime.targets, vec!["right-device", "left-device"]);
        assert_eq!(runtime.error, None);
    }

    #[test]
    fn desktop_capture_transmit_progress_runtime_status_records_sent_events() {
        let targets = ["right-device"];
        let counters = DesktopCaptureRuntimeCounters::default();
        counters.record_captured();
        counters.record_routed();
        counters.record_sent();
        counters.record_sent();
        counters.record_sent();

        let runtime = desktop_capture_transmit_progress_runtime_status(targets, &counters, 123);

        assert_eq!(runtime.state, kmsync_core::DesktopSyncRuntimeKind::Armed);
        assert_eq!(runtime.targets, vec!["right-device"]);
        assert_eq!(runtime.error, None);
        assert_eq!(runtime.captured_events, 1);
        assert_eq!(runtime.routed_events, 1);
        assert_eq!(runtime.sent_events, 3);
        assert_eq!(runtime.last_sent_at, Some(123));
    }

    #[test]
    fn desktop_transmit_backoff_drops_mouse_moves_but_allows_reliable_input() {
        let mut backoff = DesktopTransmitBackoff::default();
        let now = Instant::now();
        let target = "right-device";

        backoff.record_failure(target, now);

        assert!(backoff.should_drop(
            target,
            &InputEvent::Mouse(MouseEvent::Move { dx: 1.0, dy: 0.0 }),
            now + Duration::from_millis(50),
        ));
        assert!(!backoff.should_drop(
            target,
            &InputEvent::Mouse(MouseEvent::Button {
                button: MouseButton::Left,
                state: KeyState::Pressed,
            }),
            now + Duration::from_millis(50),
        ));
        assert!(!backoff.should_drop(
            target,
            &InputEvent::Mouse(MouseEvent::Move { dx: 1.0, dy: 0.0 }),
            now + DESKTOP_TRANSMIT_FAILURE_BACKOFF + Duration::from_millis(1),
        ));
    }

    #[test]
    fn desktop_capture_transmit_failure_updates_runtime_status() {
        let root = unique_test_dir("desktop-capture-transmit-failed");
        std::fs::create_dir_all(&root).expect("create runtime dir");
        let config_path = root.join(DEFAULT_DAEMON_CONFIG_FILE);
        let identity_path = root.join("identity.json");
        std::fs::write(
            &config_path,
            format!(
                r#"{{
                    "server_url": "http://127.0.0.1:24888",
                    "device_name": "Master Mac",
                    "role": "master",
                    "listen_port": 24800,
                    "heartbeat_interval_seconds": 30,
                    "identity_path": "{}"
                }}"#,
                identity_path.display()
            ),
        )
        .expect("write config");
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        tx.send(TargetedQueuedInputEvent::new(
            "not-a-uuid".to_string(),
            ProfileName::MacToWindows,
            InputEvent::Mouse(MouseEvent::Move { dx: 1.0, dy: 0.0 }),
        ))
        .expect("queue targeted event");
        drop(tx);

        transmit_desktop_capture_events(
            rx,
            config_path.clone(),
            CaptureQueueStats::default(),
            Arc::new(DesktopCaptureRuntimeCounters::default()),
        );

        let runtime = read_desktop_sync_runtime_status(&config_path).expect("read runtime status");
        assert_eq!(runtime.state, kmsync_core::DesktopSyncRuntimeKind::Failed);
        assert!(
            runtime
                .error
                .as_deref()
                .is_some_and(|error| error.contains("not-a-uuid")),
            "runtime error should name the failed target id, got {:?}",
            runtime.error
        );

        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn desktop_input_listener_runtime_status_marks_receiver_ready() {
        let runtime = desktop_input_listener_runtime_status();

        assert_eq!(
            runtime.state,
            kmsync_core::DesktopSyncRuntimeKind::Listening
        );
        assert_eq!(runtime.error, None);
        assert!(runtime.targets.is_empty());
    }

    #[test]
    fn desktop_input_listener_failed_runtime_status_preserves_injection_error() {
        let runtime = desktop_input_listener_failed_runtime_status("SendInput sent 0/1 events");

        assert_eq!(runtime.state, kmsync_core::DesktopSyncRuntimeKind::Failed);
        assert_eq!(runtime.error.as_deref(), Some("SendInput sent 0/1 events"));
        assert!(runtime.targets.is_empty());
    }

    #[test]
    fn desktop_input_listener_progress_runtime_status_records_received_events() {
        let runtime = desktop_input_listener_progress_runtime_status(4, 456);

        assert_eq!(
            runtime.state,
            kmsync_core::DesktopSyncRuntimeKind::Listening
        );
        assert_eq!(runtime.error, None);
        assert_eq!(runtime.received_events, 4);
        assert_eq!(runtime.last_received_at, Some(456));
    }

    #[test]
    fn desktop_input_injection_progress_runtime_status_records_injected_events() {
        let runtime = desktop_input_injection_progress_runtime_status(2, 789);

        assert_eq!(
            runtime.state,
            kmsync_core::DesktopSyncRuntimeKind::Listening
        );
        assert_eq!(runtime.error, None);
        assert_eq!(runtime.injected_events, 2);
        assert_eq!(runtime.last_injected_at, Some(789));
    }

    #[test]
    fn desktop_capture_supervisor_only_manages_master_runtime_state() {
        let master = kmsync_core::DesktopState {
            device: kmsync_core::DesktopDeviceState {
                role: kmsync_core::DesktopRole::Master,
                ..kmsync_core::DesktopDeviceState::default()
            },
            ..kmsync_core::DesktopState::default()
        };
        let client = kmsync_core::DesktopState {
            device: kmsync_core::DesktopDeviceState {
                role: kmsync_core::DesktopRole::Client,
                ..kmsync_core::DesktopDeviceState::default()
            },
            ..kmsync_core::DesktopState::default()
        };

        assert!(desktop_capture_supervisor_should_manage_runtime(&master));
        assert!(!desktop_capture_supervisor_should_manage_runtime(&client));
    }

    #[test]
    fn input_listener_runtime_status_follows_current_config_role() {
        let root = unique_test_dir("desktop-listener-runtime-role");
        std::fs::create_dir_all(&root).expect("create runtime dir");
        let config_path = root.join(DEFAULT_DAEMON_CONFIG_FILE);
        std::fs::write(
            &config_path,
            r#"{
                "server_url": "http://127.0.0.1:24888",
                "device_name": "Role Switcher",
                "role": "master",
                "master_device_id": "current-device",
                "listen_port": 24800,
                "heartbeat_interval_seconds": 30,
                "layout": {
                    "right": "right-device"
                }
            }"#,
        )
        .expect("write config");
        let capture_runtime = kmsync_core::DesktopSyncRuntimeState {
            state: kmsync_core::DesktopSyncRuntimeKind::Armed,
            targets: vec!["right-device".to_string()],
            captured_events: 2,
            ..kmsync_core::DesktopSyncRuntimeState::default()
        };
        write_desktop_sync_runtime_status(&config_path, &capture_runtime).expect("write capture");

        write_input_listener_runtime_status_if_configured(Some(&config_path));

        let runtime = read_desktop_sync_runtime_status(&config_path).expect("read runtime");
        assert_eq!(runtime.state, kmsync_core::DesktopSyncRuntimeKind::Armed);
        assert_eq!(runtime.captured_events, 2);

        desktop_config::set_role_in_config_file(
            &config_path,
            kmsync_core::DesktopRole::Client,
            Some("master-device"),
        )
        .expect("switch to client");

        write_input_listener_runtime_status_if_configured(Some(&config_path));

        let runtime = read_desktop_sync_runtime_status(&config_path).expect("read runtime");
        assert_eq!(
            runtime.state,
            kmsync_core::DesktopSyncRuntimeKind::Listening
        );

        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn desktop_capture_router_routes_each_configured_edge_to_its_target() {
        let plan = DesktopCapturePlan {
            targets: vec![
                DesktopCaptureTarget {
                    edge: Edge::Left,
                    target_device_id: "left-device".to_string(),
                    profile_name: ProfileName::WindowsToMac,
                    display: None,
                },
                DesktopCaptureTarget {
                    edge: Edge::Right,
                    target_device_id: "right-device".to_string(),
                    profile_name: ProfileName::WindowsToMac,
                    display: None,
                },
            ],
        };
        let mut router = DesktopCaptureRouter::with_display_layout(
            plan,
            DisplayLayout::from_primary(Some(BOUNDS)),
        );

        let left = router.route(CapturedInput {
            event: InputEvent::Mouse(MouseEvent::Move { dx: -1.0, dy: 0.0 }),
            pointer: Some(PointerPosition { x: 0.0, y: 50.0 }),
        });
        assert_eq!(left.target_device_id.as_deref(), Some("left-device"));
        assert_eq!(left.route.decision, CaptureDecision::Suppress);

        let release = router.route(captured_key(
            Key::Escape,
            Modifiers::CONTROL.with(Modifiers::ALT),
        ));
        assert_eq!(release.route.decision, CaptureDecision::Continue);

        let right = router.route_at(
            captured_move(110.0, 50.0),
            Instant::now() + Duration::from_millis(300),
        );
        assert_eq!(right.target_device_id.as_deref(), Some("right-device"));
        assert_eq!(right.route.decision, CaptureDecision::Suppress);
    }

    #[test]
    fn desktop_capture_router_replaces_plan_when_layout_changes() {
        let mut router = DesktopCaptureRouter::with_display_layout(
            DesktopCapturePlan {
                targets: vec![DesktopCaptureTarget {
                    edge: Edge::Right,
                    target_device_id: "old-right-device".to_string(),
                    profile_name: ProfileName::MacToWindows,
                    display: None,
                }],
            },
            DisplayLayout::from_primary(Some(BOUNDS)),
        );

        let old = router.route(captured_move(110.0, 50.0));
        assert_eq!(old.target_device_id.as_deref(), Some("old-right-device"));
        assert_eq!(old.route.decision, CaptureDecision::Suppress);

        router.replace_plan(DesktopCapturePlan {
            targets: vec![DesktopCaptureTarget {
                edge: Edge::Right,
                target_device_id: "new-right-device".to_string(),
                profile_name: ProfileName::MacToWindows,
                display: None,
            }],
        });

        let refreshed = router.route(captured_move(110.0, 50.0));
        assert_eq!(
            refreshed.target_device_id.as_deref(),
            Some("new-right-device")
        );
        assert_eq!(refreshed.route.decision, CaptureDecision::Suppress);
    }

    #[test]
    fn desktop_capture_enqueues_entry_position_once_before_remote_motion() {
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let stats = CaptureQueueStats::default();
        let mut router = DesktopCaptureRouter::with_display_layout(
            DesktopCapturePlan {
                targets: vec![DesktopCaptureTarget {
                    edge: Edge::Right,
                    target_device_id: "right-device".to_string(),
                    profile_name: ProfileName::MacToWindows,
                    display: None,
                }],
            },
            DisplayLayout::from_primary(Some(BOUNDS)),
        );

        let entry = captured_move(110.0, 60.0);
        let entry_route = router.route(entry);
        enqueue_desktop_capture(&tx, &stats, &entry_route);

        assert_eq!(
            rx.try_recv().expect("entry position").event,
            InputEvent::Mouse(MouseEvent::Position {
                x_ratio: 0.0,
                y_ratio: 0.5
            })
        );
        assert_eq!(
            rx.try_recv().expect("activation move").event,
            InputEvent::Mouse(MouseEvent::Move { dx: 1.0, dy: 1.0 })
        );

        let moved = CapturedInput {
            event: InputEvent::Mouse(MouseEvent::Move { dx: 25.0, dy: 8.0 }),
            pointer: Some(PointerPosition { x: 110.0, y: 60.0 }),
        };
        let move_route = router.route(moved);
        enqueue_desktop_capture(&tx, &stats, &move_route);

        assert_eq!(
            rx.try_recv().expect("remote move").event,
            InputEvent::Mouse(MouseEvent::Move { dx: 25.0, dy: 8.0 })
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn desktop_capture_keeps_remote_motion_relative_after_entry_position() {
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let stats = CaptureQueueStats::default();
        let mut router = DesktopCaptureRouter::with_display_layout(
            DesktopCapturePlan {
                targets: vec![DesktopCaptureTarget {
                    edge: Edge::Right,
                    target_device_id: "right-device".to_string(),
                    profile_name: ProfileName::MacToWindows,
                    display: None,
                }],
            },
            DisplayLayout::from_primary(Some(BOUNDS)),
        );

        let entry_route = router.route(captured_move(110.0, 60.0));
        enqueue_desktop_capture(&tx, &stats, &entry_route);
        let moved = CapturedInput {
            event: InputEvent::Mouse(MouseEvent::Move { dx: 25.0, dy: 8.0 }),
            pointer: Some(PointerPosition { x: 110.0, y: 60.0 }),
        };
        let move_route = router.route(moved);
        enqueue_desktop_capture(&tx, &stats, &move_route);

        assert_eq!(
            rx.try_recv().expect("entry position").event,
            InputEvent::Mouse(MouseEvent::Position {
                x_ratio: 0.0,
                y_ratio: 0.5
            })
        );
        assert_eq!(
            rx.try_recv().expect("activation remote move").event,
            InputEvent::Mouse(MouseEvent::Move { dx: 1.0, dy: 1.0 })
        );
        assert_eq!(
            rx.try_recv().expect("relative remote move").event,
            InputEvent::Mouse(MouseEvent::Move { dx: 25.0, dy: 8.0 })
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn desktop_capture_does_not_activate_when_mouse_moves_back_from_edge() {
        let mut router = DesktopCaptureRouter::with_display_layout(
            DesktopCapturePlan {
                targets: vec![DesktopCaptureTarget {
                    edge: Edge::Right,
                    target_device_id: "right-device".to_string(),
                    profile_name: ProfileName::MacToWindows,
                    display: None,
                }],
            },
            DisplayLayout::from_primary(Some(BOUNDS)),
        );

        let route = router.route(CapturedInput {
            event: InputEvent::Mouse(MouseEvent::Move { dx: -3.0, dy: 0.0 }),
            pointer: Some(PointerPosition { x: 110.0, y: 60.0 }),
        });

        assert_eq!(route.route.decision, CaptureDecision::Continue);
        assert!(!route.route.send_remote);
        assert_eq!(route.route.local_pointer_action, None);
        assert_eq!(route.target_device_id, None);
    }

    #[test]
    fn desktop_capture_syncs_pointer_position_before_mouse_button() {
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let stats = CaptureQueueStats::default();
        let mut router = DesktopCaptureRouter::with_display_layout(
            DesktopCapturePlan {
                targets: vec![DesktopCaptureTarget {
                    edge: Edge::Right,
                    target_device_id: "right-device".to_string(),
                    profile_name: ProfileName::MacToWindows,
                    display: None,
                }],
            },
            DisplayLayout::from_primary(Some(BOUNDS)),
        );

        let entry_route = router.route(captured_move(110.0, 60.0));
        enqueue_desktop_capture(&tx, &stats, &entry_route);
        let move_route = router.route(CapturedInput {
            event: InputEvent::Mouse(MouseEvent::Move { dx: 25.0, dy: 8.0 }),
            pointer: Some(PointerPosition { x: 110.0, y: 60.0 }),
        });
        enqueue_desktop_capture(&tx, &stats, &move_route);
        for _ in 0..3 {
            rx.try_recv().expect("drain prior pointer event");
        }

        let button_route = router.route(CapturedInput {
            event: InputEvent::Mouse(MouseEvent::Button {
                button: MouseButton::Left,
                state: KeyState::Pressed,
            }),
            pointer: Some(PointerPosition { x: 110.0, y: 60.0 }),
        });
        enqueue_desktop_capture(&tx, &stats, &button_route);

        let synced = rx.try_recv().expect("pointer sync before button").event;
        let clicked = rx.try_recv().expect("button after pointer sync").event;

        let InputEvent::Mouse(MouseEvent::Position { x_ratio, y_ratio }) = synced else {
            panic!("expected pointer position before mouse button");
        };
        assert!((x_ratio - 0.26).abs() < f32::EPSILON);
        assert!((y_ratio - 0.6125).abs() < f32::EPSILON);
        assert_eq!(
            clicked,
            InputEvent::Mouse(MouseEvent::Button {
                button: MouseButton::Left,
                state: KeyState::Pressed,
            })
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn desktop_capture_releases_when_remote_pointer_crosses_return_edge() {
        let mut router = DesktopCaptureRouter::with_display_layout(
            DesktopCapturePlan {
                targets: vec![DesktopCaptureTarget {
                    edge: Edge::Right,
                    target_device_id: "right-device".to_string(),
                    profile_name: ProfileName::MacToWindows,
                    display: None,
                }],
            },
            DisplayLayout::from_primary(Some(BOUNDS)),
        );

        let activated = router.route(captured_move(110.0, 60.0));
        assert_eq!(activated.route.decision, CaptureDecision::Suppress);
        let moved_into_remote = router.route(CapturedInput {
            event: InputEvent::Mouse(MouseEvent::Move { dx: 20.0, dy: 0.0 }),
            pointer: Some(PointerPosition { x: 110.0, y: 60.0 }),
        });
        assert_eq!(moved_into_remote.route.decision, CaptureDecision::Suppress);
        let returned = router.route(CapturedInput {
            event: InputEvent::Mouse(MouseEvent::Move { dx: -25.0, dy: 0.0 }),
            pointer: Some(PointerPosition { x: 110.0, y: 60.0 }),
        });

        assert_eq!(returned.route.decision, CaptureDecision::Continue);
        assert!(!returned.route.send_remote);
        assert_eq!(
            returned.route.local_pointer_action,
            Some(LocalPointerAction::Restore {
                position: Some(PointerPosition { x: 110.0, y: 60.0 })
            })
        );
    }

    #[test]
    fn desktop_capture_uses_remote_display_size_for_return_edge() {
        let local_bounds = DisplayBounds {
            x: 0.0,
            y: 0.0,
            width: 2000.0,
            height: 1200.0,
        };
        let mut router = DesktopCaptureRouter::with_display_layout(
            DesktopCapturePlan {
                targets: vec![DesktopCaptureTarget {
                    edge: Edge::Right,
                    target_device_id: "right-device".to_string(),
                    profile_name: ProfileName::MacToWindows,
                    display: Some(kmsync_core::DesktopDisplayState {
                        width_px: 1000,
                        height_px: 700,
                    }),
                }],
            },
            DisplayLayout::from_primary(Some(local_bounds)),
        );

        let activated = router.route(CapturedInput {
            event: InputEvent::Mouse(MouseEvent::Move { dx: 1.0, dy: 0.0 }),
            pointer: Some(PointerPosition {
                x: 1999.0,
                y: 600.0,
            }),
        });
        assert_eq!(activated.route.decision, CaptureDecision::Suppress);
        let moved_to_remote_right_edge = router.route(CapturedInput {
            event: InputEvent::Mouse(MouseEvent::Move {
                dx: 1000.0,
                dy: 0.0,
            }),
            pointer: Some(PointerPosition {
                x: 1999.0,
                y: 600.0,
            }),
        });
        assert_eq!(
            moved_to_remote_right_edge.route.decision,
            CaptureDecision::Suppress
        );
        let returned_to_remote_left_edge = router.route(CapturedInput {
            event: InputEvent::Mouse(MouseEvent::Move {
                dx: -1000.0,
                dy: 0.0,
            }),
            pointer: Some(PointerPosition {
                x: 1999.0,
                y: 600.0,
            }),
        });

        assert_eq!(
            returned_to_remote_left_edge.route.decision,
            CaptureDecision::Continue
        );
        assert!(!returned_to_remote_left_edge.route.send_remote);
    }

    #[test]
    fn desktop_capture_route_active_mouse_move_hot_path_has_zero_heap_allocations() {
        let mut router = DesktopCaptureRouter::with_display_layout(
            DesktopCapturePlan {
                targets: vec![DesktopCaptureTarget {
                    edge: Edge::Right,
                    target_device_id: "right-device".to_string(),
                    profile_name: ProfileName::MacToWindows,
                    display: None,
                }],
            },
            DisplayLayout::from_primary(Some(BOUNDS)),
        );

        let activated = router.route(captured_move(110.0, 60.0));
        assert_eq!(activated.route.decision, CaptureDecision::Suppress);

        allocation_tracking::reset();
        let moved = router.route(CapturedInput {
            event: InputEvent::Mouse(MouseEvent::Move { dx: 3.0, dy: 0.0 }),
            pointer: Some(PointerPosition { x: 110.0, y: 60.0 }),
        });
        let route_allocations = allocation_tracking::count();

        assert_eq!(moved.route.decision, CaptureDecision::Suppress);
        assert_eq!(moved.target_device_id.as_deref(), Some("right-device"));
        assert_eq!(route_allocations, 0);
    }

    #[test]
    fn desktop_transmit_coalesces_queued_pointer_positions_to_latest() {
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let stats = CaptureQueueStats::default();
        tx.send(TargetedQueuedInputEvent::new(
            "right-device".to_string(),
            ProfileName::MacToWindows,
            InputEvent::Mouse(MouseEvent::Position {
                x_ratio: 0.10,
                y_ratio: 0.50,
            }),
        ))
        .expect("queue first position");
        tx.send(TargetedQueuedInputEvent::new(
            "right-device".to_string(),
            ProfileName::MacToWindows,
            InputEvent::Mouse(MouseEvent::Position {
                x_ratio: 0.25,
                y_ratio: 0.60,
            }),
        ))
        .expect("queue latest position");
        tx.send(TargetedQueuedInputEvent::new(
            "right-device".to_string(),
            ProfileName::MacToWindows,
            InputEvent::Key(KeyEvent {
                key: Key::A,
                state: KeyState::Pressed,
                modifiers: Modifiers::NONE,
            }),
        ))
        .expect("queue key");
        drop(tx);

        let mut pending = None;
        let first = next_targeted_transmit_event(&rx, &mut pending, &stats).expect("first event");
        let second = next_targeted_transmit_event(&rx, &mut pending, &stats).expect("second event");

        assert_eq!(
            first.event,
            InputEvent::Mouse(MouseEvent::Position {
                x_ratio: 0.25,
                y_ratio: 0.60,
            })
        );
        assert!(matches!(second.event, InputEvent::Key(_)));
        assert!(next_targeted_transmit_event(&rx, &mut pending, &stats).is_none());
    }

    #[test]
    fn desktop_transmit_coalesces_relative_pointer_moves_by_summing_delta() {
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let stats = CaptureQueueStats::default();
        for (dx, dy) in [(1.0, 2.0), (3.0, -1.0), (-2.0, 4.0)] {
            tx.send(TargetedQueuedInputEvent::new(
                "right-device".to_string(),
                ProfileName::MacToWindows,
                InputEvent::Mouse(MouseEvent::Move { dx, dy }),
            ))
            .expect("queue move");
        }
        tx.send(TargetedQueuedInputEvent::new(
            "right-device".to_string(),
            ProfileName::MacToWindows,
            InputEvent::Key(KeyEvent {
                key: Key::A,
                state: KeyState::Pressed,
                modifiers: Modifiers::NONE,
            }),
        ))
        .expect("queue key");
        drop(tx);

        let mut pending = None;
        let first = next_targeted_transmit_event(&rx, &mut pending, &stats).expect("first event");
        let second = next_targeted_transmit_event(&rx, &mut pending, &stats).expect("second event");

        assert_eq!(
            first.event,
            InputEvent::Mouse(MouseEvent::Move { dx: 2.0, dy: 5.0 })
        );
        assert!(matches!(second.event, InputEvent::Key(_)));
        assert!(next_targeted_transmit_event(&rx, &mut pending, &stats).is_none());
    }

    #[test]
    fn desktop_transmit_drops_stale_relative_mouse_move_before_reliable_input() {
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let stats = CaptureQueueStats::default();
        let stale_capture_time = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .expect("instant can subtract one second");
        tx.send(TargetedQueuedInputEvent {
            target_device_id: "right-device".to_string(),
            profile_name: ProfileName::MacToWindows,
            event: InputEvent::Mouse(MouseEvent::Move { dx: 200.0, dy: 0.0 }),
            captured_at: stale_capture_time,
        })
        .expect("queue stale move");
        tx.send(TargetedQueuedInputEvent::new(
            "right-device".to_string(),
            ProfileName::MacToWindows,
            InputEvent::Key(KeyEvent {
                key: Key::A,
                state: KeyState::Pressed,
                modifiers: Modifiers::NONE,
            }),
        ))
        .expect("queue reliable key");
        drop(tx);

        let mut pending = None;
        let first = next_targeted_transmit_event(&rx, &mut pending, &stats).expect("first event");

        assert!(matches!(first.event, InputEvent::Key(_)));
        assert!(next_targeted_transmit_event(&rx, &mut pending, &stats).is_none());
    }

    #[test]
    fn desktop_transmit_preserves_entry_position_before_relative_move() {
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let stats = CaptureQueueStats::default();
        tx.send(TargetedQueuedInputEvent::new(
            "right-device".to_string(),
            ProfileName::MacToWindows,
            InputEvent::Mouse(MouseEvent::Position {
                x_ratio: 0.0,
                y_ratio: 0.5,
            }),
        ))
        .expect("queue entry");
        tx.send(TargetedQueuedInputEvent::new(
            "right-device".to_string(),
            ProfileName::MacToWindows,
            InputEvent::Mouse(MouseEvent::Move { dx: 10.0, dy: 2.0 }),
        ))
        .expect("queue move");
        drop(tx);

        let mut pending = None;
        let first = next_targeted_transmit_event(&rx, &mut pending, &stats).expect("entry event");
        let second = next_targeted_transmit_event(&rx, &mut pending, &stats).expect("move event");

        assert_eq!(
            first.event,
            InputEvent::Mouse(MouseEvent::Position {
                x_ratio: 0.0,
                y_ratio: 0.5,
            })
        );
        assert_eq!(
            second.event,
            InputEvent::Mouse(MouseEvent::Move { dx: 10.0, dy: 2.0 })
        );
        assert!(next_targeted_transmit_event(&rx, &mut pending, &stats).is_none());
    }

    #[test]
    fn desktop_capture_waits_briefly_to_preserve_reliable_input_when_queue_is_full() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let stats = CaptureQueueStats::default();
        tx.send(TargetedQueuedInputEvent::new(
            "right-device".to_string(),
            ProfileName::MacToWindows,
            InputEvent::Mouse(MouseEvent::Move { dx: 40.0, dy: 0.0 }),
        ))
        .expect("fill queue with mouse move");
        let drain = thread::spawn(move || {
            thread::sleep(Duration::from_millis(5));
            let first = rx.recv().expect("drain queued mouse move").event;
            let second = rx
                .recv_timeout(Duration::from_millis(200))
                .expect("reliable input should be queued after a slot opens")
                .event;
            (first, second)
        });

        enqueue_targeted_input_event(
            &tx,
            &stats,
            "right-device".to_string(),
            ProfileName::MacToWindows,
            InputEvent::Key(KeyEvent {
                key: Key::A,
                state: KeyState::Pressed,
                modifiers: Modifiers::NONE,
            }),
        );
        drop(tx);
        let (first, second) = drain.join().expect("drain queued events");

        assert!(matches!(first, InputEvent::Mouse(MouseEvent::Move { .. })));
        assert!(matches!(second, InputEvent::Key(_)));
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.enqueued, 1);
        assert_eq!(snapshot.dropped_full, 0);
    }

    #[test]
    fn desktop_capture_refreshes_router_plan_from_latest_state() {
        let mut router = DesktopCaptureRouter::with_display_layout(
            DesktopCapturePlan {
                targets: vec![DesktopCaptureTarget {
                    edge: Edge::Right,
                    target_device_id: "old-right-device".to_string(),
                    profile_name: ProfileName::MacToWindows,
                    display: None,
                }],
            },
            DisplayLayout::from_primary(Some(BOUNDS)),
        );
        let mut runtime_plan = DesktopCapturePlan {
            targets: vec![DesktopCaptureTarget {
                edge: Edge::Right,
                target_device_id: "old-right-device".to_string(),
                profile_name: ProfileName::MacToWindows,
                display: None,
            }],
        };
        let state = kmsync_core::DesktopState {
            device: kmsync_core::DesktopDeviceState {
                id: Some("master-device".to_string()),
                role: kmsync_core::DesktopRole::Master,
                os: "macos".to_string(),
                ..kmsync_core::DesktopDeviceState::default()
            },
            layout: kmsync_core::DesktopLayout {
                right: Some("new-right-device".to_string()),
                ..kmsync_core::DesktopLayout::default()
            },
            devices: vec![kmsync_core::DesktopPeerState {
                id: "new-right-device".to_string(),
                name: "Windows".to_string(),
                os: "windows".to_string(),
                online: true,
                ..kmsync_core::DesktopPeerState::default()
            }],
            ..kmsync_core::DesktopState::default()
        };

        assert!(refresh_desktop_capture_router_plan_from_state(
            &mut router,
            &mut runtime_plan,
            &state
        ));
        assert_eq!(runtime_plan.targets[0].target_device_id, "new-right-device");

        let refreshed = router.route(captured_move(110.0, 50.0));
        assert_eq!(
            refreshed.target_device_id.as_deref(),
            Some("new-right-device")
        );
    }

    #[test]
    fn desktop_capture_applies_pending_plan_updates_without_blocking_capture_path() {
        let mut router = DesktopCaptureRouter::with_display_layout(
            DesktopCapturePlan {
                targets: vec![DesktopCaptureTarget {
                    edge: Edge::Right,
                    target_device_id: "old-right-device".to_string(),
                    profile_name: ProfileName::MacToWindows,
                    display: None,
                }],
            },
            DisplayLayout::from_primary(Some(BOUNDS)),
        );
        let mut runtime_plan = DesktopCapturePlan {
            targets: vec![DesktopCaptureTarget {
                edge: Edge::Right,
                target_device_id: "old-right-device".to_string(),
                profile_name: ProfileName::MacToWindows,
                display: None,
            }],
        };
        let (tx, rx) = std::sync::mpsc::sync_channel(2);
        tx.send(DesktopCapturePlan {
            targets: vec![DesktopCaptureTarget {
                edge: Edge::Right,
                target_device_id: "new-right-device".to_string(),
                profile_name: ProfileName::MacToWindows,
                display: None,
            }],
        })
        .expect("queue plan");
        drop(tx);

        assert!(apply_pending_desktop_capture_plan_updates(
            &rx,
            &mut router,
            &mut runtime_plan
        ));
        let refreshed = router.route(captured_move(110.0, 50.0));
        assert_eq!(
            refreshed.target_device_id.as_deref(),
            Some("new-right-device")
        );
        assert!(!apply_pending_desktop_capture_plan_updates(
            &rx,
            &mut router,
            &mut runtime_plan
        ));
    }

    #[test]
    fn desktop_direct_transport_preserves_target_device_id_in_input_envelope() {
        let source_device_id = "11111111-1111-4111-8111-111111111111";
        let target_device_id = "22222222-2222-4222-8222-222222222222";
        let expected_source_id = uuid::Uuid::parse_str(source_device_id)
            .expect("source uuid")
            .as_u128();
        let expected_target_id = uuid::Uuid::parse_str(target_device_id)
            .expect("target uuid")
            .as_u128();
        let mut receiver = QuicEventReceiver::bind("127.0.0.1:0".parse().expect("bind address"))
            .expect("bind receiver");
        let sender = QuicEventSender::connect(receiver.local_addr().expect("local address"))
            .expect("connect sender");
        let mut transport = DesktopTargetTransport::Direct(sender);

        transport
            .send_event(
                target_device_id,
                expected_source_id,
                expected_target_id,
                ProtocolEvent {
                    sequence: 7,
                    timestamp_micros: 123,
                    event: InputEvent::Key(KeyEvent {
                        key: Key::A,
                        state: KeyState::Pressed,
                        modifiers: Modifiers::NONE,
                    }),
                },
            )
            .expect("send desktop event");

        let frame = receiver.recv_frame().expect("receive frame");
        let ProtocolPayload::Input(input) = frame.payload else {
            panic!("expected input payload");
        };
        assert_eq!(input.source_device_id, expected_source_id);
        assert_eq!(input.target_device_id, expected_target_id);
    }

    #[test]
    fn target_probe_control_frame_does_not_carry_input_payload() {
        let source_device_id = uuid::Uuid::parse_str("11111111-1111-4111-8111-111111111111")
            .expect("source uuid")
            .as_u128();

        let frame = target_probe_control_frame(source_device_id, 7, 123);

        assert_eq!(frame.sequence, 7);
        assert_eq!(frame.timestamp_micros, 123);
        match frame.payload {
            ProtocolPayload::Control(ControlMessage::Heartbeat {
                source_device_id: actual_source_device_id,
                sequence,
                ..
            }) => {
                assert_eq!(actual_source_device_id, source_device_id);
                assert_eq!(sequence, 7);
            }
            _ => panic!("expected control heartbeat payload"),
        }
    }

    #[test]
    fn target_input_test_frame_carries_reliable_noop_input_payload() {
        let source_device_id = uuid::Uuid::parse_str("11111111-1111-4111-8111-111111111111")
            .expect("source uuid")
            .as_u128();
        let target_device_id = uuid::Uuid::parse_str("22222222-2222-4222-8222-222222222222")
            .expect("target uuid")
            .as_u128();

        let frame = target_input_test_frame(source_device_id, target_device_id, 9, 456);

        assert_eq!(frame.sequence, 9);
        assert_eq!(frame.timestamp_micros, 456);
        match frame.payload {
            ProtocolPayload::Input(input) => {
                assert_eq!(input.source_device_id, source_device_id);
                assert_eq!(input.target_device_id, target_device_id);
                assert_eq!(input.channel, InputChannel::InputReliable);
                assert_eq!(
                    input.event,
                    InputEvent::Scroll(ScrollEvent { dx: 0.0, dy: 0.0 })
                );
            }
            _ => panic!("expected input payload"),
        }
    }

    #[test]
    fn desktop_direct_transport_sends_target_probe_as_control_frame() {
        let source_device_id = uuid::Uuid::parse_str("11111111-1111-4111-8111-111111111111")
            .expect("source uuid")
            .as_u128();
        let target_device_id = "22222222-2222-4222-8222-222222222222";
        let mut receiver = QuicEventReceiver::bind("127.0.0.1:0".parse().expect("bind address"))
            .expect("bind receiver");
        let sender = QuicEventSender::connect(receiver.local_addr().expect("local address"))
            .expect("connect sender");
        let mut transport = DesktopTargetTransport::Direct(sender);

        transport
            .send_frame(
                target_device_id,
                &target_probe_control_frame(source_device_id, 7, 123),
            )
            .expect("send probe frame");

        let frame = receiver.recv_frame().expect("receive frame");
        assert!(matches!(frame.payload, ProtocolPayload::Control(_)));
    }

    #[test]
    fn desktop_target_transport_falls_back_to_relay_when_direct_connect_fails() {
        let target_address = "10.0.0.8:24800"
            .parse::<SocketAddr>()
            .expect("target address");
        let direct_attempt = client::DirectConnectionAttempt {
            candidate: client::ConnectionCandidate {
                device_id: "22222222-2222-4222-8222-222222222222".to_string(),
                kind: client::ConnectionCandidateKind::BackendLan,
                address: target_address.to_string(),
                priority: client::ConnectionCandidateKind::BackendLan.priority(),
            },
            address: target_address,
            failed_attempts: Vec::new(),
        };
        let mut relay_attempted = false;

        let selected: DesktopTargetTransportSelection<&str, &str> =
            connect_desktop_target_transport_with(
                "http://relay.example",
                "11111111-1111-4111-8111-111111111111",
                "22222222-2222-4222-8222-222222222222",
                |_target_device_id, _timeout| Ok(direct_attempt),
                |_address, _timeout| Err("udp blocked".to_string()),
                |server_url, source_device_id| {
                    relay_attempted = true;
                    assert_eq!(server_url, "http://relay.example");
                    assert_eq!(source_device_id, "11111111-1111-4111-8111-111111111111");
                    Ok("relay-transport")
                },
            )
            .expect("relay fallback");

        assert_eq!(
            selected,
            DesktopTargetTransportSelection::Relay("relay-transport")
        );
        assert!(relay_attempted);
    }

    #[test]
    fn desktop_target_transport_uses_short_direct_connect_timeout_before_relay() {
        let target_address = "10.0.0.8:24800"
            .parse::<SocketAddr>()
            .expect("target address");
        let direct_attempt = client::DirectConnectionAttempt {
            candidate: client::ConnectionCandidate {
                device_id: "22222222-2222-4222-8222-222222222222".to_string(),
                kind: client::ConnectionCandidateKind::BackendLan,
                address: target_address.to_string(),
                priority: client::ConnectionCandidateKind::BackendLan.priority(),
            },
            address: target_address,
            failed_attempts: Vec::new(),
        };
        let mut observed_resolve_timeout = None;
        let mut observed_timeout = None;

        let selected: DesktopTargetTransportSelection<&str, &str> =
            connect_desktop_target_transport_with(
                "http://relay.example",
                "11111111-1111-4111-8111-111111111111",
                "22222222-2222-4222-8222-222222222222",
                |_target_device_id, timeout| {
                    observed_resolve_timeout = Some(timeout);
                    Ok(direct_attempt)
                },
                |_address, timeout| {
                    observed_timeout = Some(timeout);
                    Err("udp blocked".to_string())
                },
                |_server_url, _source_device_id| Ok("relay-transport"),
            )
            .expect("relay fallback");

        assert_eq!(
            selected,
            DesktopTargetTransportSelection::Relay("relay-transport")
        );
        assert_eq!(
            observed_resolve_timeout,
            Some(DESKTOP_DIRECT_CONNECT_TIMEOUT)
        );
        assert_eq!(observed_timeout, Some(DESKTOP_DIRECT_CONNECT_TIMEOUT));
        assert!(DESKTOP_DIRECT_CONNECT_TIMEOUT <= Duration::from_millis(200));
    }

    #[test]
    fn core_service_keeps_running_when_heartbeat_temporarily_fails() {
        let action = core_service_action_for_worker_result(CoreServiceThreadResult::Heartbeat(
            Err("request failed: connection refused".to_string()),
        ));

        assert!(matches!(action, CoreServiceWorkerAction::Continue));

        let action = core_service_action_for_worker_result(CoreServiceThreadResult::DataPlane(
            Err("bind failed".to_string()),
        ));

        match action {
            CoreServiceWorkerAction::Stop(Err(error)) => {
                assert_eq!(error, "core service data_plane failed: bind failed");
            }
            _ => panic!("data-plane failure should stop core-service"),
        }
    }

    #[test]
    fn packaging_autostart_uses_core_service_entrypoint() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        let macos = std::fs::read_to_string(root.join("packaging/macos/build-pkg.sh"))
            .expect("read macOS packaging script");
        let normalized_macos = macos.replace("\r\n", "\n");
        let windows = std::fs::read_to_string(root.join("packaging/windows/kmsync.nsi"))
            .expect("read Windows packaging script");

        assert!(normalized_macos.contains("APP_BUNDLE=\"/Applications/KMSync.app\""));
        assert!(!normalized_macos.contains("cat > \"${PKG_ROOT}/Library/LaunchAgents"));
        assert!(!normalized_macos.contains("<key>KeepAlive</key>"));
        assert!(!normalized_macos.contains("launchctl bootstrap \"gui/$uid\""));
        assert!(!normalized_macos.contains("APP_EXECUTABLE="));
        assert!(!normalized_macos.contains("<string>/usr/local/bin/kmsync</string>"));
        assert!(!normalized_macos
            .contains("<string>/usr/local/share/kmsync/configs/daemon.example.json</string>"));
        assert!(windows.contains("\"$INSTDIR\\${APP_EXE}\" core-service"));
    }

    #[test]
    fn args_parse_accepts_windows_service_command() {
        let default_config = PathBuf::from("configs/daemon.example.json");
        let default_args = Args::parse_with_default_config(
            ["windows-service"].into_iter().map(String::from),
            default_config.clone(),
        )
        .expect("parse default windows service");
        match default_args.command {
            Command::WindowsService { config_path } => {
                assert_eq!(config_path, default_config);
            }
            _ => panic!("expected windows service command"),
        }

        let custom_args = Args::parse(
            [
                "windows-service",
                "C:\\Program Files\\KMSync\\configs\\daemon.example.json",
            ]
            .into_iter()
            .map(String::from),
        )
        .expect("parse custom windows service");
        match custom_args.command {
            Command::WindowsService { config_path } => {
                assert_eq!(
                    config_path,
                    PathBuf::from("C:\\Program Files\\KMSync\\configs\\daemon.example.json")
                );
            }
            _ => panic!("expected windows service command"),
        }
    }

    #[test]
    fn windows_service_entrypoint_is_separate_from_user_companion_hot_path() {
        let binary = Path::new(r"C:\Program Files\KMSync\kmsync.exe");

        assert_eq!(WINDOWS_SERVICE_NAME, "KMSyncCoreService");
        assert_eq!(
            windows_service_command_line(binary),
            r#""C:\Program Files\KMSync\kmsync.exe" windows-service"#
        );
        assert_eq!(
            windows_companion_command_line(binary),
            r#""C:\Program Files\KMSync\kmsync.exe" core-service"#
        );
    }

    #[test]
    fn windows_packaging_starts_only_user_companion_for_interactive_input() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        let windows = std::fs::read_to_string(root.join("packaging/windows/kmsync.nsi"))
            .expect("read Windows packaging script");

        assert!(windows.contains("core-service"));
        assert!(windows.contains("Software\\Microsoft\\Windows\\CurrentVersion\\Run"));
        assert!(windows.contains("ExecShell \"open\" \"$INSTDIR\\${APP_EXE}\" \"core-service\""));
        assert!(windows.contains("sc.exe stop \"KMSyncCoreService\""));
        assert!(windows.contains("sc.exe delete \"KMSyncCoreService\""));
        assert!(!windows.contains("sc.exe create"));
        assert!(!windows.contains("sc.exe start"));
        assert!(!windows.contains("windows-service"));
        assert!(!windows.contains("core-service \"$INSTDIR\\configs\\daemon.example.json\""));
    }

    #[test]
    fn windows_packaging_allows_inbound_quic_sync_port() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        let windows = std::fs::read_to_string(root.join("packaging/windows/kmsync.nsi"))
            .expect("read Windows packaging script");

        assert!(windows.contains("advfirewall firewall add rule"));
        assert!(windows.contains("name=\"KMSync Input Sync\""));
        assert!(windows.contains("dir=in"));
        assert!(windows.contains("program=\"$INSTDIR\\${APP_EXE}\""));
        assert!(windows.contains("protocol=UDP"));
        assert!(windows.contains("localport=24800"));
        assert!(windows.contains("advfirewall firewall delete rule name=\"KMSync Input Sync\""));
    }

    #[test]
    fn windows_portable_packaging_includes_receiver_firewall_helpers() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        let build = std::fs::read_to_string(root.join("packaging/windows/build-portable.sh"))
            .expect("read Windows portable build script");
        let firewall = std::fs::read_to_string(root.join("packaging/windows/enable-firewall.cmd"))
            .expect("read Windows firewall helper");
        let starter =
            std::fs::read_to_string(root.join("packaging/windows/start-core-service.cmd"))
                .expect("read Windows core-service helper");
        let guide =
            std::fs::read_to_string(root.join("docs/USER_GUIDE.md")).expect("read user guide");

        assert!(build.contains("x86_64-pc-windows-gnu"));
        assert!(build.contains("enable-firewall.cmd"));
        assert!(build.contains("start-core-service.cmd"));
        assert!(build.contains("USER_GUIDE.md"));
        assert!(firewall.contains("Start-Process"));
        assert!(firewall.contains("Verb RunAs"));
        assert!(firewall.contains("advfirewall firewall add rule"));
        assert!(firewall.contains("name=\"KMSync Input Sync\""));
        assert!(firewall.contains("protocol=UDP"));
        assert!(firewall.contains("localport=24800"));
        assert!(starter.contains("core-service"));
        assert!(starter.contains("daemon.example.json"));
        assert!(guide.contains("enable-firewall.cmd"));
        assert!(guide.contains("start-core-service.cmd"));
    }

    #[test]
    fn windows_packaging_marks_install_before_first_core_service_launch() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        let windows = std::fs::read_to_string(root.join("packaging/windows/kmsync.nsi"))
            .expect("read Windows packaging script");

        let uninstall = windows
            .find("WriteUninstaller \"$INSTDIR\\Uninstall.exe\"")
            .expect("write uninstaller marker");
        let autostart = windows
            .find("ExecShell \"open\" \"$INSTDIR\\${APP_EXE}\" \"core-service\"")
            .expect("start user companion");

        assert!(
            uninstall < autostart,
            "Windows first-launch core-service must see the installed-package marker"
        );
    }

    #[test]
    fn windows_packaging_copies_default_daemon_config_under_configs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        let windows = std::fs::read_to_string(root.join("packaging/windows/kmsync.nsi"))
            .expect("read Windows packaging script");

        let daemon_config = windows
            .find("File /oname=daemon.example.json \"..\\..\\configs\\daemon.example.json\"")
            .expect("daemon config install entry");
        let before_daemon_config = &windows[..daemon_config];
        let last_configs_output = before_daemon_config
            .rfind("SetOutPath \"$INSTDIR\\configs\"")
            .expect("configs output before daemon config");
        let last_docs_output = before_daemon_config
            .rfind("SetOutPath \"$INSTDIR\\docs\"")
            .unwrap_or(0);

        assert!(
            last_configs_output > last_docs_output,
            "daemon.example.json must be copied to $INSTDIR\\configs, not docs"
        );
        assert!(windows.contains("Delete \"$INSTDIR\\configs\\daemon.example.json\""));
        assert!(!windows.contains("Delete \"$INSTDIR\\docs\\daemon.example.json\""));
    }

    #[test]
    fn windows_packaging_exposes_single_desktop_executable_with_ui_commands_on_daemon() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        let workspace =
            std::fs::read_to_string(root.join("Cargo.toml")).expect("read workspace manifest");
        let macos = std::fs::read_to_string(root.join("packaging/macos/build-pkg.sh"))
            .expect("read macOS packaging script");
        let windows = std::fs::read_to_string(root.join("packaging/windows/kmsync.nsi"))
            .expect("read Windows packaging script");
        let windows_build = std::fs::read_to_string(root.join("packaging/windows/build-nsis.ps1"))
            .expect("read Windows build script");
        assert!(workspace.contains("\"crates/kmsync-ui\""));
        assert!(!root.join("crates/kmsync-ui/src/main.rs").exists());
        assert!(!macos.contains("/usr/local/bin/kmsync-ui"));
        assert!(!macos.contains("/usr/local/bin/kmsync-server"));
        assert!(!macos.contains("STAGING_DIR}/kmsync-ui"));
        assert!(!macos.contains("STAGING_DIR}/kmsync-server"));
        assert!(!windows.contains("kmsync-ui.exe"));
        assert!(!windows.contains("kmsync-server.exe"));
        assert!(!windows.contains("UI_EXE"));
        assert!(!windows.contains("SERVER_EXE"));
        assert!(!windows_build.contains("\"kmsync-ui\""));
        assert!(!windows_build.contains("\"kmsync-server\""));
        assert!(!windows_build.contains("kmsync-ui.exe"));
        assert!(!windows_build.contains("kmsync-server.exe"));
        assert!(windows.contains(
            "CreateShortCut \"$SMPROGRAMS\\KMSync\\KMSync status.lnk\" \"$INSTDIR\\${APP_EXE}\" \"status\""
        ));
        assert!(windows.contains("!define APP_EXE \"kmsync.exe\""));
    }

    #[test]
    fn desktop_packaging_uses_kmsync_executable_name_and_no_console_subsystem() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        let main_rs =
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"))
                .expect("read main source");
        let macos = std::fs::read_to_string(root.join("packaging/macos/build-pkg.sh"))
            .expect("read macOS packaging script");
        let windows = std::fs::read_to_string(root.join("packaging/windows/kmsync.nsi"))
            .expect("read Windows packaging script");
        let windows_build = std::fs::read_to_string(root.join("packaging/windows/build-nsis.ps1"))
            .expect("read Windows build script");

        assert!(main_rs.contains("windows_subsystem = \"windows\""));
        assert!(windows.contains("!define APP_EXE \"kmsync.exe\""));
        assert!(windows.contains(
            "OutFile \"..\\..\\dist\\windows\\kmsync-${APP_VERSION}-windows-x64-setup.exe\""
        ));
        assert!(windows_build
            .contains("$Installer = Join-Path $Dist \"kmsync-$Version-windows-x64-setup.exe\""));
        assert!(windows_build.contains("\"kmsync\""));
        assert!(macos.contains("/usr/local/bin/kmsync"));
        assert!(macos.contains("PKG_PATH=\"${DIST_DIR}/kmsync-${VERSION}-macos.pkg\""));
    }

    #[test]
    fn linux_server_packaging_includes_relay_status_deploy_helper() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        let deploy = std::fs::read_to_string(root.join("packaging/linux/deploy-server.sh"))
            .expect("read Linux server deploy helper");
        let readme = std::fs::read_to_string(root.join("packaging/linux/README.md"))
            .expect("read Linux server README");

        assert!(deploy.contains("scp"));
        assert!(deploy.contains("ssh"));
        assert!(deploy.contains("kmsync-server.backup"));
        assert!(deploy.contains("--strip-components=1"));
        assert!(deploy.contains("systemctl restart kmsync-server"));
        assert!(deploy.contains("/health"));
        assert!(deploy.contains("relay_rx_status"));
        assert!(readme.contains("deploy-server.sh"));
        assert!(readme.contains("relay_rx_status"));
    }

    #[test]
    fn local_ipc_status_response_reports_out_of_band_hot_path() {
        let response = handle_local_ipc_request(local_ipc::LocalIpcRequest::Status);

        match response {
            local_ipc::LocalIpcResponse::Status {
                service,
                version,
                input_hot_path,
                platform_transport,
                config_path,
                device_id,
                ..
            } => {
                assert_eq!(service, "kmsync");
                assert_eq!(version, env!("CARGO_PKG_VERSION"));
                assert_eq!(input_hot_path, "not_on_local_ipc");
                assert_eq!(config_path, None);
                assert_eq!(device_id, None);
                assert_eq!(
                    platform_transport,
                    local_ipc::default_local_ipc_endpoint().transport.as_str()
                );
            }
            _ => panic!("expected local ipc status response"),
        }
    }

    #[test]
    fn local_ipc_status_response_reports_daemon_hot_path_when_served_by_core_service() {
        let response = handle_local_ipc_request_with_config_path(
            local_ipc::LocalIpcRequest::Status,
            Some(Path::new("configs/daemon.example.json")),
        );

        match response {
            local_ipc::LocalIpcResponse::Status {
                service,
                version,
                input_hot_path,
                platform_transport,
                config_path,
                ..
            } => {
                assert_eq!(service, "kmsync");
                assert_eq!(version, env!("CARGO_PKG_VERSION"));
                assert_eq!(input_hot_path, "daemon_data_plane");
                assert_eq!(config_path.as_deref(), Some("configs/daemon.example.json"));
                assert_eq!(
                    platform_transport,
                    local_ipc::default_local_ipc_endpoint().transport.as_str()
                );
            }
            _ => panic!("expected local ipc status response"),
        }
    }

    #[test]
    fn local_ipc_status_response_does_not_create_missing_identity_file() {
        let root = unique_test_dir("local-ipc-status-no-identity-write");
        let config_path = root.join("daemon.example.json");
        let identity_path = root.join("identity.json");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(
            &config_path,
            r#"{
  "server_url": "http://127.0.0.1:24888",
  "device_name": "Status Only",
  "identity_path": "identity.json",
  "listen_port": 24800,
  "heartbeat_interval_seconds": 15
}
"#,
        )
        .expect("write config");

        let response = handle_local_ipc_request_with_config_path(
            local_ipc::LocalIpcRequest::Status,
            Some(&config_path),
        );

        match response {
            local_ipc::LocalIpcResponse::Status {
                config_path: reported_config_path,
                device_id,
                ..
            } => {
                let expected_config_path = config_path.display().to_string();
                assert_eq!(
                    reported_config_path.as_deref(),
                    Some(expected_config_path.as_str())
                );
                assert_eq!(device_id, None);
            }
            _ => panic!("expected local ipc status response"),
        }
        assert!(!identity_path.exists());

        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn local_ipc_desktop_state_reads_config_and_layout_updates_write_back() {
        let root = unique_test_dir("desktop-ipc-config");
        let config_path = root.join("daemon.example.json");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(
            &config_path,
            r#"{
                "server_url": "http://127.0.0.1:24888",
                "device_name": "Development Mac",
                "listen_port": 24800,
                "heartbeat_interval_seconds": 15,
                "role": "master"
            }"#,
        )
        .expect("write config");

        let response = handle_local_ipc_request_with_config_path(
            local_ipc::LocalIpcRequest::GetDesktopState,
            Some(&config_path),
        );
        match response {
            local_ipc::LocalIpcResponse::DesktopState { state } => {
                assert_eq!(state.device.name, "Development Mac");
                assert_eq!(
                    state.server_state,
                    kmsync_core::DesktopConnectionState::Disconnected
                );
                assert!(state.server_error.is_some());
            }
            _ => panic!("expected desktop state"),
        }

        let layout = kmsync_core::DesktopLayout {
            left: Some("left-device".to_string()),
            right: None,
            top: None,
            bottom: Some("bottom-device".to_string()),
        };
        let response = handle_local_ipc_request_with_config_path(
            local_ipc::LocalIpcRequest::SetLayout {
                layout: layout.clone(),
            },
            Some(&config_path),
        );
        match response {
            local_ipc::LocalIpcResponse::ConfigApplied { state } => {
                assert_eq!(state.layout, layout);
            }
            _ => panic!("expected config applied"),
        }

        let text = std::fs::read_to_string(&config_path).expect("read updated config");
        assert!(text.contains("\"left\": \"left-device\""));
        assert!(text.contains("\"bottom\": \"bottom-device\""));

        let response = handle_local_ipc_request_with_config_path(
            local_ipc::LocalIpcRequest::SetServerEndpoint {
                host: "203.0.113.10".to_string(),
                port: 24_889,
            },
            Some(&config_path),
        );
        match response {
            local_ipc::LocalIpcResponse::ConfigApplied { state } => {
                assert_eq!(
                    state.network.server_url.as_deref(),
                    Some("http://203.0.113.10:24889")
                );
                assert_eq!(state.network.server_host.as_deref(), Some("203.0.113.10"));
                assert_eq!(state.network.server_port, Some(24_889));
            }
            _ => panic!("expected config applied"),
        }

        let text = std::fs::read_to_string(&config_path).expect("read updated config");
        assert!(text.contains("\"server_url\": \"http://203.0.113.10:24889\""));
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn local_ipc_desktop_state_includes_sync_runtime_failure() {
        let root = unique_test_dir("desktop-sync-runtime-state");
        let config_path = root.join("daemon.example.json");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(
            &config_path,
            r#"{
                "server_url": "http://127.0.0.1:9",
                "device_name": "Master Mac",
                "listen_port": 24800,
                "heartbeat_interval_seconds": 15,
                "role": "master"
            }"#,
        )
        .expect("write config");
        write_desktop_sync_runtime_status(
            &config_path,
            &kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Failed,
                error: Some("macOS Input Monitoring permission is missing".to_string()),
                targets: vec!["right-device".to_string()],
                updated_at: None,
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
        )
        .expect("write runtime");

        let response = handle_local_ipc_request_with_config_path(
            local_ipc::LocalIpcRequest::GetDesktopState,
            Some(&config_path),
        );

        match response {
            local_ipc::LocalIpcResponse::DesktopState { state } => {
                assert_eq!(
                    state.sync_runtime.state,
                    kmsync_core::DesktopSyncRuntimeKind::Failed
                );
                assert_eq!(state.sync_runtime.targets, vec!["right-device"]);
                assert!(
                    state
                        .master_error
                        .as_deref()
                        .is_some_and(|error| error.contains("Input Monitoring")),
                    "sync runtime failure should be visible to UI"
                );
            }
            _ => panic!("expected desktop state"),
        }

        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn macos_master_ignores_stale_background_service_permission_runtime() {
        let root = unique_test_dir("stale-background-runtime");
        let config_path = root.join("daemon.example.json");
        std::fs::create_dir_all(&root).expect("create root");
        write_desktop_sync_runtime_status(
            &config_path,
            &kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Failed,
                error: Some(
                    "macOS Input Monitoring permission is missing for the KMSync background service"
                        .to_string(),
                ),
                targets: vec!["right-device".to_string()],
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
        )
        .expect("write stale runtime");
        let mut state = kmsync_core::DesktopState {
            device: kmsync_core::DesktopDeviceState {
                os: "macos".to_string(),
                role: kmsync_core::DesktopRole::Master,
                ..kmsync_core::DesktopDeviceState::default()
            },
            ..kmsync_core::DesktopState::default()
        };

        attach_desktop_sync_runtime_status(&config_path, &mut state).expect("attach runtime");

        assert_eq!(
            state.sync_runtime.state,
            kmsync_core::DesktopSyncRuntimeKind::Unknown
        );
        assert_eq!(state.sync_runtime.error, None);
        assert_eq!(state.master_error, None);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn desktop_state_surfaces_incompatible_core_service_status() {
        let mut state = kmsync_core::DesktopState::default();

        attach_core_service_health_status(
            &mut state,
            Ok(CoreServiceHealth {
                version: env!("CARGO_PKG_VERSION").to_string(),
                input_hot_path: "not_on_local_ipc".to_string(),
                config_path: None,
                device_id: None,
                launch_context: None,
            }),
        );

        let error = state.master_error.as_deref().expect("core service error");
        assert!(error.contains("旧后台服务"));
        assert!(error.contains("同步热路径"));
        assert!(error.contains("input_hot_path=not_on_local_ipc"));
        assert!(error.contains("重新安装或重启"));
    }

    #[test]
    fn core_service_issue_message_names_macos_launch_context_mismatch() {
        let message = core_service_health_issue_message_for_exe(
            &CoreServiceHealth {
                version: env!("CARGO_PKG_VERSION").to_string(),
                input_hot_path: "daemon_data_plane".to_string(),
                config_path: Some(PathBuf::from(
                    "/Users/alice/Library/Application Support/KMSync/daemon.example.json",
                )),
                device_id: Some("device-a".to_string()),
                launch_context: None,
            },
            Path::new("/Applications/KMSync.app/Contents/MacOS/kmsync"),
            "macos",
        );

        assert!(message.contains("后台服务启动方式不兼容"));
        assert!(message.contains("launch_context=-"));
    }

    #[test]
    fn desktop_state_surfaces_unreachable_core_service_status() {
        let mut state = kmsync_core::DesktopState::default();

        attach_core_service_health_status(
            &mut state,
            Err("local IPC I/O failed: Connection refused (os error 61)".to_string()),
        );

        let error = state.master_error.as_deref().expect("core service error");
        assert!(error.contains("后台服务未运行"));
        assert!(error.contains("同步通道无法工作"));
        assert!(error.contains("重启 KMSync"));
    }

    #[test]
    fn desktop_state_prioritizes_unreachable_core_service_over_stale_sync_error() {
        let mut state = kmsync_core::DesktopState {
            master_error: Some(
                "同步通道失败：发送到 right-device 失败：relay route failed: TargetOffline"
                    .to_string(),
            ),
            ..kmsync_core::DesktopState::default()
        };

        attach_core_service_health_status(
            &mut state,
            Err("local IPC I/O failed: Connection refused (os error 61)".to_string()),
        );

        let error = state.master_error.as_deref().expect("core service error");
        assert!(error.contains("后台服务未运行"));
        assert!(!error.contains("TargetOffline"));
    }

    #[test]
    fn desktop_state_prioritizes_incompatible_core_service_over_stale_sync_error() {
        let mut state = kmsync_core::DesktopState {
            master_error: Some(
                "同步通道失败：发送到 right-device 失败：relay route failed: TargetOffline"
                    .to_string(),
            ),
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Failed,
                error: Some("relay route failed: TargetOffline".to_string()),
                targets: vec!["right-device".to_string()],
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..kmsync_core::DesktopState::default()
        };

        attach_core_service_health_status(
            &mut state,
            Ok(CoreServiceHealth {
                version: env!("CARGO_PKG_VERSION").to_string(),
                input_hot_path: "not_on_local_ipc".to_string(),
                config_path: None,
                device_id: None,
                launch_context: None,
            }),
        );

        let error = state.master_error.as_deref().expect("core service error");
        assert!(error.contains("检测到旧后台服务或非同步热路径"));
        assert!(!error.contains("TargetOffline"));
        assert_eq!(
            state.sync_runtime.state,
            kmsync_core::DesktopSyncRuntimeKind::Failed
        );
        assert!(state
            .sync_runtime
            .error
            .as_deref()
            .is_some_and(|error| error.contains("检测到旧后台服务或非同步热路径")));
    }

    #[test]
    fn desktop_state_marks_stale_armed_runtime_failed_when_core_service_unreachable() {
        let mut state = kmsync_core::DesktopState {
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Armed,
                targets: vec!["right-device".to_string()],
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..kmsync_core::DesktopState::default()
        };

        attach_core_service_health_status(
            &mut state,
            Err("local IPC I/O failed: Connection refused (os error 61)".to_string()),
        );

        assert_eq!(
            state.sync_runtime.state,
            kmsync_core::DesktopSyncRuntimeKind::Failed
        );
        assert!(state
            .sync_runtime
            .error
            .as_deref()
            .is_some_and(|error| error.contains("后台服务未运行")));
        assert!(state.sync_runtime.targets.is_empty());
    }

    #[test]
    fn desktop_render_state_prioritizes_unreachable_core_service_over_armed_runtime() {
        let mut state = kmsync_core::DesktopState {
            device: kmsync_core::DesktopDeviceState {
                id: Some("master-device".to_string()),
                role: kmsync_core::DesktopRole::Master,
                ..kmsync_core::DesktopDeviceState::default()
            },
            master_state: kmsync_core::DesktopConnectionState::SelfDevice,
            layout: kmsync_core::DesktopLayout {
                right: Some("right-device".to_string()),
                ..kmsync_core::DesktopLayout::default()
            },
            devices: vec![kmsync_core::DesktopPeerState {
                id: "right-device".to_string(),
                name: "Right PC".to_string(),
                online: true,
                sync_relay_status_known: false,
                sync_relay_online: false,
                ..kmsync_core::DesktopPeerState::default()
            }],
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Armed,
                targets: vec!["right-device".to_string()],
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..kmsync_core::DesktopState::default()
        };

        attach_desktop_render_core_service_health_status(
            &mut state,
            Err("local IPC I/O failed: Connection refused (os error 61)".to_string()),
        );
        let html =
            kmsync_ui::desktop_panel::render_desktop_panel(&state).expect("render desktop panel");

        assert!(html.contains("主电脑：需处理"));
        assert!(!html.contains("主电脑：接收端待验证"));
    }

    #[test]
    fn desktop_diagnostic_report_surfaces_core_service_target_and_runtime_counts() {
        let state = kmsync_core::DesktopState {
            config_path: Some(
                "/Users/alice/Library/Application Support/KMSync/daemon.example.json".to_string(),
            ),
            device: kmsync_core::DesktopDeviceState {
                id: Some("master-device".to_string()),
                name: "Master Mac".to_string(),
                os: "macos".to_string(),
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                role: kmsync_core::DesktopRole::Master,
                sync_relay_status_known: false,
                sync_relay_online: false,
            },
            server_state: kmsync_core::DesktopConnectionState::Connected,
            master_state: kmsync_core::DesktopConnectionState::SelfDevice,
            master_device_id: Some("master-device".to_string()),
            master_error: Some("后台服务未运行，可能仍在启动或启动失败".to_string()),
            layout: kmsync_core::DesktopLayout {
                right: Some("right-device".to_string()),
                ..kmsync_core::DesktopLayout::default()
            },
            devices: vec![kmsync_core::DesktopPeerState {
                id: "right-device".to_string(),
                name: "Windows PC".to_string(),
                os: "windows".to_string(),
                online: true,
                sync_relay_status_known: false,
                sync_relay_online: false,
                lan_ips: vec!["192.168.30.99".to_string()],
                ..kmsync_core::DesktopPeerState::default()
            }],
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Failed,
                error: Some("relay route failed: TargetOffline".to_string()),
                targets: vec!["right-device".to_string()],
                captured_events: 0,
                routed_events: 0,
                sent_events: 0,
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..kmsync_core::DesktopState::default()
        };

        let report = render_desktop_diagnostic_report(&state);

        assert!(report.contains("desktop diagnostic report"));
        assert!(report.contains("role=master"));
        assert!(report.contains("core_service=unavailable"));
        assert!(
            report.contains("layout_left=- layout_right=right-device layout_top=- layout_bottom=-")
        );
        assert!(report.contains("sync_runtime=failed"));
        assert!(report.contains("sync_targets=right-device"));
        assert!(report.contains("captured_events=0 routed_events=0 sent_events=0"));
        assert!(report.contains("target edge=right id=right-device name=Windows PC online=true"));
        assert!(report.contains("relay_rx_online=unknown"));
        assert!(report.contains("warning=server_relay_status_unavailable"));
        assert!(report.contains("warning=sync_runtime_failed"));
    }

    #[test]
    fn desktop_diagnostic_report_surfaces_last_capture_observation() {
        let state = kmsync_core::DesktopState {
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Armed,
                captured_events: 4,
                routed_events: 2,
                sent_events: 1,
                last_capture_pointer_x: Some(1432),
                last_capture_pointer_y: Some(812),
                last_capture_edge: Some("right".to_string()),
                last_capture_target: Some("right-device".to_string()),
                last_capture_routed: true,
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..kmsync_core::DesktopState::default()
        };

        let report = render_desktop_diagnostic_report(&state);

        assert!(report
            .contains("last_capture pointer=1432,812 routed=true edge=right target=right-device"));
    }

    #[test]
    fn desktop_diagnostic_report_surfaces_target_probe_results() {
        let state = kmsync_core::DesktopState::default();
        let probes = [DesktopTargetProbeDiagnostic {
            edge: Edge::Right,
            target_device_id: "right-device".to_string(),
            status: DesktopTargetProbeStatus::Failed,
            transport: Some("relay".to_string()),
            error: Some(
                "target probe send failed via relay: relay route failed: TargetOffline".to_string(),
            ),
        }];

        let report = render_desktop_diagnostic_report_with_target_probes(&state, &probes);

        assert!(report
            .contains("target_probe edge=right id=right-device status=failed transport=relay"));
        assert!(report.contains("relay route failed: TargetOffline"));
    }

    #[test]
    fn desktop_diagnostic_report_warns_when_target_lan_is_not_on_local_subnet() {
        let state = kmsync_core::DesktopState {
            device: kmsync_core::DesktopDeviceState {
                role: kmsync_core::DesktopRole::Master,
                ..kmsync_core::DesktopDeviceState::default()
            },
            network: kmsync_core::DesktopNetworkState {
                lan_ips: vec!["192.168.50.226".to_string()],
                ..kmsync_core::DesktopNetworkState::default()
            },
            layout: kmsync_core::DesktopLayout {
                right: Some("right-device".to_string()),
                ..kmsync_core::DesktopLayout::default()
            },
            devices: vec![kmsync_core::DesktopPeerState {
                id: "right-device".to_string(),
                name: "Windows PC".to_string(),
                online: true,
                lan_ips: vec!["192.168.30.99".to_string()],
                ..kmsync_core::DesktopPeerState::default()
            }],
            ..kmsync_core::DesktopState::default()
        };

        let report = render_desktop_diagnostic_report(&state);

        assert!(report.contains("warning=target_lan_not_on_local_subnet"));
    }

    #[test]
    fn desktop_diagnostic_report_marks_available_core_service_as_ok() {
        let state = kmsync_core::DesktopState {
            master_error: None,
            ..kmsync_core::DesktopState::default()
        };

        let report = render_desktop_diagnostic_report(&state);

        assert!(report.contains("core_service=ok\n"));
        assert!(!report.contains("core_service=ok_or_unknown"));
    }

    #[test]
    fn desktop_diagnostic_report_warns_when_background_permission_scope_differs() {
        let state = kmsync_core::DesktopState {
            device: kmsync_core::DesktopDeviceState {
                role: kmsync_core::DesktopRole::Master,
                ..kmsync_core::DesktopDeviceState::default()
            },
            permissions: vec![kmsync_core::DesktopPermissionState {
                key: "macos.input_monitoring".to_string(),
                status: "granted".to_string(),
                label: "macOS Input Monitoring".to_string(),
                guidance: Some("Grant Input Monitoring to KMSync.app.".to_string()),
            }],
            sync_runtime: kmsync_core::DesktopSyncRuntimeState {
                state: kmsync_core::DesktopSyncRuntimeKind::Failed,
                error: Some(
                    "macOS Input Monitoring permission is missing for KMSync.app".to_string(),
                ),
                ..kmsync_core::DesktopSyncRuntimeState::default()
            },
            ..kmsync_core::DesktopState::default()
        };

        let report = render_desktop_diagnostic_report(&state);

        assert!(report.contains(
            "permission_check=macos.input_monitoring status=granted label=\"macOS Input Monitoring\""
        ));
        assert!(report.contains("warning=background_permission_scope_mismatch"));
    }

    #[test]
    fn macos_launch_agent_diagnostic_marks_launch_services_launcher_as_current() {
        let diagnostic = macos_launch_agent_diagnostic_from_plist(
            PathBuf::from("/Library/LaunchAgents/com.kmsync.mvp.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.kmsync.mvp</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/bin/open</string>
        <string>-W</string>
        <string>-n</string>
        <string>-g</string>
        <string>/Applications/KMSync.app</string>
        <string>--args</string>
        <string>core-service</string>
    </array>
</dict>
</plist>"#,
        );

        assert_eq!(diagnostic.state, MacosLaunchAgentState::LaunchServicesApp);
        assert_eq!(diagnostic.program.as_deref(), Some("/usr/bin/open"));
    }

    #[test]
    fn macos_launch_agent_diagnostic_marks_direct_binary_launcher_as_legacy() {
        let diagnostic = macos_launch_agent_diagnostic_from_plist(
            PathBuf::from("/Library/LaunchAgents/com.kmsync.mvp.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.kmsync.mvp</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Applications/KMSync.app/Contents/MacOS/kmsync</string>
        <string>core-service</string>
        <string>/Users/alice/Library/Application Support/KMSync/daemon.example.json</string>
    </array>
</dict>
</plist>"#,
        );

        assert_eq!(diagnostic.state, MacosLaunchAgentState::DirectAppBinary);
        assert_eq!(
            diagnostic.program.as_deref(),
            Some("/Applications/KMSync.app/Contents/MacOS/kmsync")
        );
    }

    #[test]
    fn macos_launch_agent_diagnostic_marks_open_without_core_service_args_as_legacy() {
        let diagnostic = macos_launch_agent_diagnostic_from_plist(
            PathBuf::from("/Library/LaunchAgents/com.kmsync.mvp.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.kmsync.mvp</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/bin/open</string>
        <string>-a</string>
        <string>KMSync</string>
    </array>
</dict>
</plist>"#,
        );

        assert_eq!(diagnostic.state, MacosLaunchAgentState::LegacyOpen);
        assert_eq!(diagnostic.program.as_deref(), Some("/usr/bin/open"));
    }

    #[test]
    fn desktop_diagnostic_report_surfaces_legacy_macos_launch_agent_warning() {
        let state = kmsync_core::DesktopState::default();
        let launch_agent = MacosLaunchAgentDiagnostic {
            path: PathBuf::from("/Library/LaunchAgents/com.kmsync.mvp.plist"),
            state: MacosLaunchAgentState::LegacyOpen,
            program: Some("/usr/bin/open".to_string()),
            error: None,
        };

        let report =
            render_desktop_diagnostic_report_with_launch_agent(&state, Some(&launch_agent));

        assert!(report.contains("launch_agent_path=/Library/LaunchAgents/com.kmsync.mvp.plist"));
        assert!(report.contains("launch_agent_state=legacy_open"));
        assert!(report.contains("launch_agent_program=/usr/bin/open"));
        assert!(report.contains("warning=legacy_macos_launch_agent"));
    }

    #[test]
    fn desktop_diagnostic_report_surfaces_direct_macos_launch_agent_warning() {
        let state = kmsync_core::DesktopState::default();
        let launch_agent = MacosLaunchAgentDiagnostic {
            path: PathBuf::from("/Library/LaunchAgents/com.kmsync.mvp.plist"),
            state: MacosLaunchAgentState::DirectAppBinary,
            program: Some("/Applications/KMSync.app/Contents/MacOS/kmsync".to_string()),
            error: None,
        };

        let report =
            render_desktop_diagnostic_report_with_launch_agent(&state, Some(&launch_agent));

        assert!(report.contains("launch_agent_state=direct_app_binary"));
        assert!(report.contains("warning=direct_macos_launch_agent"));
    }

    #[test]
    fn macos_installed_app_diagnostic_marks_hash_mismatch_as_outdated() {
        let diagnostic = macos_installed_app_diagnostic_from_hashes(
            PathBuf::from("/Applications/KMSync.app/Contents/MacOS/kmsync"),
            Ok("old-app-hash".to_string()),
            Ok("current-build-hash".to_string()),
        );

        assert_eq!(diagnostic.state, MacosInstalledAppState::Outdated);
        assert_eq!(diagnostic.installed_hash.as_deref(), Some("old-app-hash"));
        assert_eq!(
            diagnostic.current_hash.as_deref(),
            Some("current-build-hash")
        );
    }

    #[test]
    fn desktop_diagnostic_report_surfaces_outdated_installed_macos_app_warning() {
        let state = kmsync_core::DesktopState::default();
        let installed_app = MacosInstalledAppDiagnostic {
            path: PathBuf::from("/Applications/KMSync.app/Contents/MacOS/kmsync"),
            state: MacosInstalledAppState::Outdated,
            installed_hash: Some("old-app-hash".to_string()),
            current_hash: Some("current-build-hash".to_string()),
            error: None,
        };

        let report = render_desktop_diagnostic_report_with_macos_context(
            &state,
            None,
            Some(&installed_app),
            &[],
        );

        assert!(
            report.contains("installed_app_path=/Applications/KMSync.app/Contents/MacOS/kmsync")
        );
        assert!(report.contains("installed_app_state=outdated"));
        assert!(report.contains("installed_app_hash=old-app-hash"));
        assert!(report.contains("current_exe_hash=current-build-hash"));
        assert!(report.contains("warning=installed_macos_app_outdated"));
    }

    #[test]
    fn desktop_state_surfaces_mismatched_core_service_config_path() {
        let mut state = kmsync_core::DesktopState {
            config_path: Some(
                "/Users/alice/Library/Application Support/KMSync/daemon.example.json".to_string(),
            ),
            ..kmsync_core::DesktopState::default()
        };

        attach_core_service_health_status(
            &mut state,
            Ok(CoreServiceHealth {
                version: env!("CARGO_PKG_VERSION").to_string(),
                input_hot_path: "daemon_data_plane".to_string(),
                config_path: Some(PathBuf::from(
                    "/Users/admin/Library/Application Support/KMSync/daemon.example.json",
                )),
                device_id: Some("admin-device".to_string()),
                launch_context: None,
            }),
        );

        let error = state.master_error.as_deref().expect("core service error");
        assert!(error.contains("后台服务配置不一致"));
        assert!(error.contains("admin-device"));
        assert!(error.contains("重新安装或重启"));
    }

    #[test]
    fn args_parse_accepts_capture_connect_command() {
        let args = Args::parse(
            [
                "capture-connect",
                "configs/daemon.example.json",
                "windows-device",
                "windows-to-mac",
                "right",
                "4.0",
                "ctrl+shift+f12",
                "750",
            ]
            .into_iter()
            .map(String::from),
        )
        .expect("parse capture connect");

        match args.command {
            Command::CaptureConnect {
                config_path,
                target_device_id,
                profile,
                mode:
                    CaptureMode::Edge {
                        edge,
                        threshold,
                        release_hotkey,
                        cooldown,
                    },
                ..
            } => {
                assert_eq!(config_path, PathBuf::from("configs/daemon.example.json"));
                assert_eq!(target_device_id, "windows-device");
                assert!(matches!(profile, ProfileName::WindowsToMac));
                assert!(matches!(edge, Edge::Right));
                assert_eq!(threshold, 4.0);
                assert_eq!(
                    release_hotkey,
                    Hotkey {
                        key: Key::F12,
                        modifiers: Modifiers::CONTROL.with(Modifiers::SHIFT),
                    }
                );
                assert_eq!(cooldown, Duration::from_millis(750));
            }
            _ => panic!("expected capture connect command"),
        }
    }

    #[test]
    fn args_parse_accepts_target_probe_command() {
        let args = Args::parse(
            [
                "target-probe",
                "configs/daemon.example.json",
                "22222222-2222-4222-8222-222222222222",
            ]
            .into_iter()
            .map(String::from),
        )
        .expect("parse target probe");

        match args.command {
            Command::TargetProbe {
                config_path,
                target_device_id,
            } => {
                assert_eq!(config_path, PathBuf::from("configs/daemon.example.json"));
                assert_eq!(target_device_id, "22222222-2222-4222-8222-222222222222");
            }
            _ => panic!("expected target probe command"),
        }
    }

    #[test]
    fn args_parse_accepts_target_input_test_command() {
        let args = Args::parse(
            [
                "target-input-test",
                "configs/daemon.example.json",
                "22222222-2222-4222-8222-222222222222",
            ]
            .into_iter()
            .map(String::from),
        )
        .expect("parse target input test");

        match args.command {
            Command::TargetInputTest {
                config_path,
                target_device_id,
            } => {
                assert_eq!(config_path, PathBuf::from("configs/daemon.example.json"));
                assert_eq!(target_device_id, "22222222-2222-4222-8222-222222222222");
            }
            _ => panic!("expected target input test command"),
        }
    }

    #[test]
    fn args_parse_accepts_capture_send_application_exceptions() {
        let args = Args::parse(
            [
                "capture-send",
                "127.0.0.1:24800",
                "mac-to-windows",
                "right",
                "4.0",
                "ctrl+alt+escape",
                "500",
                "Code,Photoshop",
            ]
            .into_iter()
            .map(String::from),
        )
        .expect("parse capture send exceptions");

        match args.command {
            Command::CaptureSend {
                application_exceptions,
                ..
            } => {
                assert!(application_exceptions.matches(Some("Code.exe")));
                assert!(application_exceptions.matches(Some("Adobe Photoshop 2026")));
                assert!(!application_exceptions.matches(Some("Terminal.exe")));
            }
            _ => panic!("expected capture send command"),
        }
    }

    fn reliable_input_payload(event: InputEvent) -> ProtocolPayload {
        ProtocolPayload::Input(InputEventEnvelope::new(
            10,
            20,
            1,
            InputChannel::InputReliable,
            event,
        ))
    }

    #[test]
    fn always_mode_forwards_without_suppressing_local_input() {
        let mut router = CaptureRouter::new(CaptureMode::Always, Some(BOUNDS));

        let route = router.route(captured_move(50.0, 50.0));

        assert!(route.send_remote);
        assert_eq!(route.decision, CaptureDecision::Continue);
    }

    #[test]
    fn application_exception_keeps_matching_app_local() {
        let mut router = CaptureRouter::with_application_exceptions(
            CaptureMode::Always,
            Some(BOUNDS),
            ApplicationExceptionRules::from_patterns(vec!["code".to_string()]),
        );

        let blocked = router.route_at_with_application(
            captured_key(Key::A, Modifiers::NONE),
            Some("Code.exe"),
            Instant::now(),
        );
        assert!(!blocked.send_remote);
        assert_eq!(blocked.decision, CaptureDecision::Continue);

        let forwarded = router.route_at_with_application(
            captured_key(Key::A, Modifiers::NONE),
            Some("Terminal.exe"),
            Instant::now(),
        );
        assert!(forwarded.send_remote);
        assert_eq!(forwarded.decision, CaptureDecision::Continue);
    }

    #[test]
    fn edge_mode_ignores_events_until_pointer_reaches_configured_edge() {
        let mut router = CaptureRouter::new(
            CaptureMode::Edge {
                edge: Edge::Right,
                threshold: 2.0,
                release_hotkey: Hotkey::default_release(),
                cooldown: Duration::from_millis(0),
            },
            Some(BOUNDS),
        );

        let route = router.route(captured_move(40.0, 50.0));

        assert!(!route.send_remote);
        assert_eq!(route.decision, CaptureDecision::Continue);
    }

    #[test]
    fn edge_mode_activates_at_threshold_and_suppresses_local_input() {
        let mut router = CaptureRouter::new(
            CaptureMode::Edge {
                edge: Edge::Right,
                threshold: 2.0,
                release_hotkey: Hotkey::default_release(),
                cooldown: Duration::from_millis(0),
            },
            Some(BOUNDS),
        );

        let route = router.route(captured_move(108.0, 50.0));

        assert!(route.send_remote);
        assert_eq!(route.decision, CaptureDecision::Suppress);
    }

    #[test]
    fn edge_mode_hides_local_pointer_only_when_remote_control_activates() {
        let mut router = CaptureRouter::new(
            CaptureMode::Edge {
                edge: Edge::Right,
                threshold: 2.0,
                release_hotkey: Hotkey::default_release(),
                cooldown: Duration::ZERO,
            },
            Some(BOUNDS),
        );

        let activated = router.route(captured_move(108.0, 60.0));
        let forwarded = router.route(captured_move(70.0, 60.0));

        assert_eq!(
            activated.local_pointer_action,
            Some(LocalPointerAction::Hide)
        );
        assert_eq!(forwarded.local_pointer_action, None);
    }

    #[test]
    fn edge_mode_restores_local_pointer_when_remote_control_releases() {
        let mut router = CaptureRouter::new(
            CaptureMode::Edge {
                edge: Edge::Right,
                threshold: 2.0,
                release_hotkey: Hotkey::default_release(),
                cooldown: Duration::ZERO,
            },
            Some(BOUNDS),
        );

        let activated = router.route(captured_move(108.0, 60.0));
        let released = router.route(captured_key(
            Key::Escape,
            Modifiers::CONTROL.with(Modifiers::ALT),
        ));

        assert_eq!(
            activated.local_pointer_action,
            Some(LocalPointerAction::Hide)
        );
        assert_eq!(
            released.local_pointer_action,
            Some(LocalPointerAction::Restore {
                position: Some(PointerPosition { x: 108.0, y: 60.0 })
            })
        );
        assert_eq!(released.decision, CaptureDecision::Continue);
    }

    #[test]
    fn edge_mode_enqueues_entry_position_before_first_remote_move() {
        let (tx, rx) = std::sync::mpsc::sync_channel(2);
        let stats = CaptureQueueStats::default();
        let mut router = CaptureRouter::new(
            CaptureMode::Edge {
                edge: Edge::Right,
                threshold: 2.0,
                release_hotkey: Hotkey::default_release(),
                cooldown: Duration::ZERO,
            },
            Some(BOUNDS),
        );

        let route = enqueue_routed_capture(&tx, &stats, &mut router, captured_move(108.0, 60.0));

        assert!(route.send_remote);
        assert_eq!(route.decision, CaptureDecision::Suppress);
        assert_eq!(
            rx.try_recv().expect("entry position").event,
            InputEvent::Mouse(MouseEvent::Position {
                x_ratio: 0.0,
                y_ratio: 0.5
            })
        );
        assert_eq!(
            rx.try_recv().expect("captured move").event,
            InputEvent::Mouse(MouseEvent::Move { dx: 1.0, dy: 1.0 })
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn edge_mode_uses_virtual_desktop_edge_for_multi_display_layout() {
        let mut router = CaptureRouter::with_display_layout(
            CaptureMode::Edge {
                edge: Edge::Right,
                threshold: 2.0,
                release_hotkey: Hotkey::default_release(),
                cooldown: Duration::ZERO,
            },
            platform::DisplayLayout::new(vec![
                BOUNDS,
                DisplayBounds {
                    x: 100.0,
                    y: 0.0,
                    width: 100.0,
                    height: 100.0,
                },
            ]),
        );

        let internal_boundary = router.route(captured_move(99.0, 50.0));
        assert!(!internal_boundary.send_remote);
        assert_eq!(internal_boundary.decision, CaptureDecision::Continue);

        let outer_boundary = router.route(captured_move(199.0, 50.0));
        assert!(outer_boundary.send_remote);
        assert_eq!(outer_boundary.decision, CaptureDecision::Suppress);
    }

    #[test]
    fn hot_corner_mode_activates_only_inside_configured_corner_square() {
        let mut router = CaptureRouter::new(
            CaptureMode::Edge {
                edge: Edge::TopLeft,
                threshold: 4.0,
                release_hotkey: Hotkey::default_release(),
                cooldown: Duration::ZERO,
            },
            Some(BOUNDS),
        );

        let near_left_edge_only = router.route(captured_move(12.0, 60.0));
        let near_top_edge_only = router.route(captured_move(60.0, 22.0));
        let in_hot_corner = router.route(captured_move(12.0, 22.0));

        assert!(!near_left_edge_only.send_remote);
        assert_eq!(near_left_edge_only.decision, CaptureDecision::Continue);
        assert!(!near_top_edge_only.send_remote);
        assert_eq!(near_top_edge_only.decision, CaptureDecision::Continue);
        assert!(in_hot_corner.send_remote);
        assert_eq!(in_hot_corner.decision, CaptureDecision::Suppress);
    }

    #[test]
    fn locked_capture_mode_keeps_current_device_local() {
        let mut router = CaptureRouter::new(CaptureMode::Locked, Some(BOUNDS));

        let pointer = router.route(captured_move(110.0, 60.0));
        let key = router.route(captured_key(Key::C, Modifiers::CONTROL));

        assert!(!pointer.send_remote);
        assert_eq!(pointer.decision, CaptureDecision::Continue);
        assert!(!key.send_remote);
        assert_eq!(key.decision, CaptureDecision::Continue);
    }

    #[test]
    fn edge_mode_keeps_forwarding_after_activation() {
        let mut router = CaptureRouter::new(
            CaptureMode::Edge {
                edge: Edge::Left,
                threshold: 2.0,
                release_hotkey: Hotkey::default_release(),
                cooldown: Duration::from_millis(0),
            },
            Some(BOUNDS),
        );

        let activated = router.route(captured_move(12.0, 50.0));
        let later = router.route(captured_move(60.0, 50.0));

        assert!(activated.send_remote);
        assert!(later.send_remote);
        assert_eq!(later.decision, CaptureDecision::Suppress);
    }

    #[test]
    fn release_hotkey_deactivates_remote_control() {
        let mut router = CaptureRouter::new(
            CaptureMode::Edge {
                edge: Edge::Right,
                threshold: 2.0,
                release_hotkey: Hotkey::default_release(),
                cooldown: Duration::from_millis(0),
            },
            Some(BOUNDS),
        );

        let activated = router.route(captured_move(108.0, 50.0));
        let released = router.route(captured_key(
            Key::Escape,
            Modifiers::CONTROL.with(Modifiers::ALT),
        ));
        let local = router.route(captured_move(60.0, 50.0));

        assert!(activated.send_remote);
        assert!(!released.send_remote);
        assert_eq!(released.decision, CaptureDecision::Continue);
        assert!(!local.send_remote);
        assert_eq!(local.decision, CaptureDecision::Continue);
    }

    #[test]
    fn edge_mode_uses_configured_release_hotkey() {
        let mut router = CaptureRouter::new(
            CaptureMode::Edge {
                edge: Edge::Right,
                threshold: 2.0,
                release_hotkey: Hotkey {
                    key: Key::F12,
                    modifiers: Modifiers::CONTROL.with(Modifiers::SHIFT),
                },
                cooldown: Duration::from_millis(0),
            },
            Some(BOUNDS),
        );

        let activated = router.route(captured_move(108.0, 50.0));
        let default_hotkey = router.route(captured_key(
            Key::Escape,
            Modifiers::CONTROL.with(Modifiers::ALT),
        ));
        let released = router.route(captured_key(
            Key::F12,
            Modifiers::CONTROL.with(Modifiers::SHIFT),
        ));
        let local = router.route(captured_move(60.0, 50.0));

        assert!(activated.send_remote);
        assert!(default_hotkey.send_remote);
        assert_eq!(default_hotkey.decision, CaptureDecision::Suppress);
        assert!(!released.send_remote);
        assert_eq!(released.decision, CaptureDecision::Continue);
        assert!(!local.send_remote);
    }

    #[test]
    fn edge_mode_keeps_system_reserved_shortcuts_local_while_active() {
        let mut router = CaptureRouter::new(
            CaptureMode::Edge {
                edge: Edge::Right,
                threshold: 2.0,
                release_hotkey: Hotkey::default_release(),
                cooldown: Duration::ZERO,
            },
            Some(BOUNDS),
        );
        let activated = router.route(captured_move(108.0, 50.0));
        assert!(activated.send_remote);
        assert_eq!(activated.decision, CaptureDecision::Suppress);

        for captured in [
            captured_key(Key::Delete, Modifiers::CONTROL.with(Modifiers::ALT)),
            captured_key(Key::L, Modifiers::META),
            captured_key(Key::Space, Modifiers::META),
        ] {
            let route = router.route(captured);

            assert!(!route.send_remote);
            assert_eq!(route.decision, CaptureDecision::Continue);
        }
    }

    #[test]
    fn always_mode_keeps_system_reserved_shortcuts_local() {
        let mut router = CaptureRouter::new(CaptureMode::Always, Some(BOUNDS));

        let route = router.route(captured_key(Key::L, Modifiers::META));

        assert!(!route.send_remote);
        assert_eq!(route.decision, CaptureDecision::Continue);
    }

    #[test]
    fn parse_capture_mode_accepts_custom_release_hotkey() {
        let mode = parse_capture_mode(
            Some("right".to_string()),
            Some("4.0".to_string()),
            Some("ctrl+shift+f12".to_string()),
            None,
        )
        .expect("parse capture mode");

        let CaptureMode::Edge {
            edge,
            threshold,
            release_hotkey,
            cooldown,
        } = mode
        else {
            panic!("expected edge mode");
        };
        assert!(matches!(edge, Edge::Right));
        assert_eq!(threshold, 4.0);
        assert_eq!(
            release_hotkey,
            Hotkey {
                key: Key::F12,
                modifiers: Modifiers::CONTROL.with(Modifiers::SHIFT),
            }
        );
        assert_eq!(cooldown, default_edge_cooldown());
    }

    #[test]
    fn edge_mode_respects_cooldown_after_release_hotkey() {
        let mut router = CaptureRouter::new(
            CaptureMode::Edge {
                edge: Edge::Right,
                threshold: 2.0,
                release_hotkey: Hotkey::default_release(),
                cooldown: Duration::from_millis(250),
            },
            Some(BOUNDS),
        );
        let now = Instant::now();

        let activated = router.route_at(captured_move(108.0, 50.0), now);
        let released = router.route_at(
            captured_key(Key::Escape, Modifiers::CONTROL.with(Modifiers::ALT)),
            now + Duration::from_millis(10),
        );
        let during_cooldown =
            router.route_at(captured_move(108.0, 50.0), now + Duration::from_millis(100));
        let after_cooldown =
            router.route_at(captured_move(108.0, 50.0), now + Duration::from_millis(300));

        assert!(activated.send_remote);
        assert!(!released.send_remote);
        assert!(!during_cooldown.send_remote);
        assert_eq!(during_cooldown.decision, CaptureDecision::Continue);
        assert!(after_cooldown.send_remote);
        assert_eq!(after_cooldown.decision, CaptureDecision::Suppress);
    }

    #[test]
    fn parse_capture_mode_accepts_edge_cooldown() {
        let mode = parse_capture_mode(
            Some("right".to_string()),
            Some("4.0".to_string()),
            Some("ctrl+shift+f12".to_string()),
            Some("750".to_string()),
        )
        .expect("parse capture mode");

        let CaptureMode::Edge { cooldown, .. } = mode else {
            panic!("expected edge mode");
        };
        assert_eq!(cooldown, Duration::from_millis(750));
    }

    #[test]
    fn parse_capture_mode_accepts_hot_corner_and_locked_mode() {
        let corner = parse_capture_mode(
            Some("top-left".to_string()),
            Some("4.0".to_string()),
            None,
            None,
        )
        .expect("parse hot corner");
        let CaptureMode::Edge {
            edge, threshold, ..
        } = corner
        else {
            panic!("expected edge mode");
        };
        assert!(matches!(edge, Edge::TopLeft));
        assert_eq!(threshold, 4.0);

        let locked =
            parse_capture_mode(Some("lock".to_string()), None, None, None).expect("parse lock");
        assert!(matches!(locked, CaptureMode::Locked));
    }

    #[test]
    fn edge_mode_does_not_activate_without_display_bounds() {
        let mut router = CaptureRouter::new(
            CaptureMode::Edge {
                edge: Edge::Right,
                threshold: 2.0,
                release_hotkey: Hotkey::default_release(),
                cooldown: Duration::from_millis(0),
            },
            None,
        );

        let route = router.route(captured_move(108.0, 50.0));

        assert!(!route.send_remote);
        assert_eq!(route.decision, CaptureDecision::Continue);
    }

    #[test]
    fn inject_failure_releases_tracked_remote_input() {
        let mut injector = RecordingInjector::default();
        let mut state = RemoteInputState::default();
        inject_or_release_on_error(
            &mut injector,
            &mut state,
            InputEvent::Key(KeyEvent {
                key: Key::LeftControl,
                state: KeyState::Pressed,
                modifiers: Modifiers::CONTROL,
            }),
        )
        .expect("press control");
        inject_or_release_on_error(
            &mut injector,
            &mut state,
            InputEvent::Key(KeyEvent {
                key: Key::C,
                state: KeyState::Pressed,
                modifiers: Modifiers::CONTROL,
            }),
        )
        .expect("press c");

        injector.fail_next = true;
        let result = inject_or_release_on_error(
            &mut injector,
            &mut state,
            InputEvent::Scroll(kmsync_core::ScrollEvent { dx: 0.0, dy: 1.0 }),
        );

        assert_eq!(result, Err("injection failed".to_string()));
        assert_eq!(
            injector.events,
            vec![
                InputEvent::Key(KeyEvent {
                    key: Key::LeftControl,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::CONTROL,
                }),
                InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::CONTROL,
                }),
                InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Released,
                    modifiers: Modifiers::NONE,
                }),
                InputEvent::Key(KeyEvent {
                    key: Key::LeftControl,
                    state: KeyState::Released,
                    modifiers: Modifiers::NONE,
                }),
            ]
        );
        assert!(state.release_all().is_empty());
    }

    #[test]
    fn disconnect_error_releases_tracked_mouse_buttons() {
        let mut injector = RecordingInjector::default();
        let mut state = RemoteInputState::default();
        inject_or_release_on_error(
            &mut injector,
            &mut state,
            InputEvent::Mouse(MouseEvent::Button {
                button: MouseButton::Left,
                state: KeyState::Pressed,
            }),
        )
        .expect("press left button");

        let result = release_error_or(&mut injector, &mut state, "recv failed".to_string());

        assert_eq!(result, Err("recv failed".to_string()));
        assert_eq!(
            injector.events,
            vec![
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Pressed,
                }),
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Released,
                }),
            ]
        );
        assert!(state.release_all().is_empty());
    }

    #[test]
    fn listener_does_not_log_input_packets_by_default() {
        let frame = ProtocolFrame {
            sequence: 7,
            timestamp_micros: 8,
            payload: input_payload(InputEvent::Key(KeyEvent {
                key: Key::C,
                state: KeyState::Pressed,
                modifiers: Modifiers::CONTROL,
            })),
        };

        assert_eq!(listener_log_line(&frame), None);
    }

    #[test]
    fn listener_logs_clipboard_metadata_without_content() {
        let clipboard = ClipboardText::new(20, 3, "secret text".to_string());
        let expected = format!(
            "received event=clipboard_text bytes=11 source=20 version=3 hash={}",
            clipboard.content_hash
        );
        let frame = ProtocolFrame {
            sequence: 7,
            timestamp_micros: 8,
            payload: ProtocolPayload::ClipboardText(clipboard),
        };

        assert_eq!(listener_log_line(&frame), Some(expected));
    }

    #[test]
    fn listener_logs_file_transfer_metadata_without_content() {
        let files = kmsync_core::ClipboardFiles::new(
            20,
            4,
            vec![kmsync_core::ClipboardFileMetadata::new(
                "secret-plan.pdf".to_string(),
                4096,
                0xabc,
            )],
        );
        let files_frame = ProtocolFrame {
            sequence: 5,
            timestamp_micros: 6,
            payload: ProtocolPayload::ClipboardFiles(files.clone()),
        };
        let chunk = kmsync_core::FileTransferChunk::new(
            0xfeed,
            20,
            0,
            0,
            4096,
            0,
            false,
            b"secret file bytes".to_vec(),
        );
        let chunk_frame = ProtocolFrame {
            sequence: 6,
            timestamp_micros: 7,
            payload: ProtocolPayload::FileChunk(chunk),
        };

        let files_log = listener_log_line(&files_frame).expect("file metadata log");
        let chunk_log = listener_log_line(&chunk_frame).expect("file chunk log");

        assert!(files_log.contains("event=clipboard_files"));
        assert!(files_log.contains("files=1"));
        assert!(files_log.contains("bytes=4096"));
        assert!(files_log.contains(&format!("hash={}", files.content_hash)));
        assert!(!files_log.contains("secret-plan"));
        assert!(chunk_log.contains("event=file_chunk"));
        assert!(chunk_log.contains("bytes=17"));
        assert!(!chunk_log.contains("secret file bytes"));
    }

    #[test]
    fn input_event_log_type_omits_key_and_clipboard_content() {
        assert_eq!(
            input_event_log_type(&InputEvent::Key(KeyEvent {
                key: Key::C,
                state: KeyState::Pressed,
                modifiers: Modifiers::CONTROL,
            })),
            "key"
        );
        assert_eq!(
            input_event_log_type(&InputEvent::Mouse(MouseEvent::Move { dx: 1.0, dy: 2.0 })),
            "mouse_move"
        );
        assert_eq!(
            input_event_log_type(&InputEvent::Mouse(MouseEvent::Button {
                button: MouseButton::Left,
                state: KeyState::Pressed,
            })),
            "mouse_button"
        );
        assert_eq!(
            input_event_log_type(&InputEvent::Scroll(kmsync_core::ScrollEvent {
                dx: 0.0,
                dy: 1.0,
            })),
            "scroll"
        );
    }

    #[test]
    fn receive_loop_enqueues_frames_until_receiver_error() {
        let frame = ProtocolFrame {
            sequence: 1,
            timestamp_micros: 2,
            payload: input_payload(InputEvent::Scroll(kmsync_core::ScrollEvent {
                dx: 0.0,
                dy: 1.0,
            })),
        };
        let mut receiver = RecordingFrameReceiver {
            frames: VecDeque::from([Ok(frame.clone()), Err("recv failed".to_string())]),
        };
        let (input_tx, input_rx) = std::sync::mpsc::sync_channel(2);
        let (clipboard_tx, _clipboard_rx) = std::sync::mpsc::sync_channel(2);
        let (control_tx, _control_rx) = std::sync::mpsc::sync_channel(2);

        let result = receive_remote_frames(
            &mut receiver,
            input_tx,
            clipboard_tx,
            control_tx,
            ListenerLatencyStats::default(),
            None,
        );

        assert_eq!(result, Err("recv failed".to_string()));
        assert_eq!(input_rx.try_recv().expect("queued frame").frame, frame);
        assert!(input_rx.try_recv().is_err());
    }

    #[test]
    fn receive_loop_does_not_block_input_when_clipboard_queue_is_full() {
        let input_frame = ProtocolFrame {
            sequence: 2,
            timestamp_micros: 3,
            payload: input_payload(InputEvent::Scroll(kmsync_core::ScrollEvent {
                dx: 0.0,
                dy: 1.0,
            })),
        };
        let mut receiver = RecordingFrameReceiver {
            frames: VecDeque::from([
                Ok(ProtocolFrame {
                    sequence: 1,
                    timestamp_micros: 2,
                    payload: ProtocolPayload::ClipboardText(ClipboardText::new(
                        20,
                        1,
                        "slow clipboard".to_string(),
                    )),
                }),
                Ok(input_frame.clone()),
                Err("recv failed".to_string()),
            ]),
        };
        let (input_tx, input_rx) = std::sync::mpsc::sync_channel(1);
        let (clipboard_tx, _clipboard_rx) = std::sync::mpsc::sync_channel(0);
        let (control_tx, _control_rx) = std::sync::mpsc::sync_channel(1);

        let result = receive_remote_frames(
            &mut receiver,
            input_tx,
            clipboard_tx,
            control_tx,
            ListenerLatencyStats::default(),
            None,
        );

        assert_eq!(result, Err("recv failed".to_string()));
        assert_eq!(
            input_rx.try_recv().expect("queued input").frame,
            input_frame
        );
        assert!(input_rx.try_recv().is_err());
    }

    #[test]
    fn receive_loop_records_send_to_receive_latency() {
        let sent_at = now_micros().expect("timestamp").saturating_sub(500);
        let mut receiver = RecordingFrameReceiver {
            frames: VecDeque::from([
                Ok(ProtocolFrame {
                    sequence: 1,
                    timestamp_micros: sent_at,
                    payload: input_payload(InputEvent::Scroll(kmsync_core::ScrollEvent {
                        dx: 0.0,
                        dy: 1.0,
                    })),
                }),
                Err("recv failed".to_string()),
            ]),
        };
        let (input_tx, _input_rx) = std::sync::mpsc::sync_channel(1);
        let (clipboard_tx, _clipboard_rx) = std::sync::mpsc::sync_channel(1);
        let (control_tx, _control_rx) = std::sync::mpsc::sync_channel(1);
        let stats = ListenerLatencyStats::default();

        let result = receive_remote_frames(
            &mut receiver,
            input_tx,
            clipboard_tx,
            control_tx,
            stats.clone(),
            None,
        );

        assert_eq!(result, Err("recv failed".to_string()));
        assert!(stats.snapshot().last_send_to_receive_micros >= 500);
    }

    #[test]
    fn receive_loop_drops_unreliable_mouse_move_when_input_queue_is_full() {
        let occupied = ProtocolFrame {
            sequence: 1,
            timestamp_micros: 1,
            payload: reliable_input_payload(InputEvent::Mouse(MouseEvent::Position {
                x_ratio: 0.0,
                y_ratio: 0.5,
            })),
        };
        let stale_move = ProtocolFrame {
            sequence: 2,
            timestamp_micros: 2,
            payload: ProtocolPayload::Input(InputEventEnvelope::new(
                10,
                20,
                1,
                InputChannel::InputUnreliable,
                InputEvent::Mouse(MouseEvent::Move { dx: 8.0, dy: 0.0 }),
            )),
        };
        let (input_tx, input_rx) = std::sync::mpsc::sync_channel(1);
        input_tx
            .send(ReceivedInputFrame {
                frame: occupied.clone(),
                received_at: Instant::now(),
            })
            .expect("fill input queue");

        let queued =
            queue_received_input_frame(&input_tx, stale_move, Instant::now()).expect("queue input");

        assert!(!queued);
        assert_eq!(input_rx.try_recv().expect("occupied frame").frame, occupied);
        assert!(input_rx.try_recv().is_err());
    }

    #[test]
    fn receive_loop_routes_control_frames_away_from_input_and_clipboard() {
        let control = kmsync_core::ControlMessage::heartbeat(20, 0xabc, 9);
        let frame = ProtocolFrame {
            sequence: 1,
            timestamp_micros: 2,
            payload: ProtocolPayload::Control(control.clone()),
        };
        let mut receiver = RecordingFrameReceiver {
            frames: VecDeque::from([Ok(frame), Err("recv failed".to_string())]),
        };
        let (input_tx, input_rx) = std::sync::mpsc::sync_channel(1);
        let (clipboard_tx, clipboard_rx) = std::sync::mpsc::sync_channel(1);
        let (control_tx, control_rx) = std::sync::mpsc::sync_channel(1);

        let result = receive_remote_frames(
            &mut receiver,
            input_tx,
            clipboard_tx,
            control_tx,
            ListenerLatencyStats::default(),
            None,
        );

        assert_eq!(result, Err("recv failed".to_string()));
        assert!(input_rx.try_recv().is_err());
        assert!(clipboard_rx.try_recv().is_err());
        assert_eq!(
            control_rx.try_recv().expect("queued control").message,
            control
        );
    }

    #[test]
    fn injection_loop_injects_queued_frames_and_releases_when_closed() {
        let (tx, rx) = std::sync::mpsc::sync_channel(2);
        tx.send(ReceivedInputFrame {
            frame: ProtocolFrame {
                sequence: 1,
                timestamp_micros: 2,
                payload: input_payload(InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Pressed,
                })),
            },
            received_at: Instant::now(),
        })
        .expect("queue press");
        drop(tx);
        let mut adapter = RecordingInjector::default();

        inject_received_frames(rx, &mut adapter, ListenerLatencyStats::default(), None)
            .expect("inject frames");

        assert_eq!(
            adapter.events,
            vec![
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Pressed,
                }),
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Released,
                }),
            ]
        );
    }

    #[test]
    fn injection_loop_writes_injection_progress_runtime_status() {
        let root = unique_test_dir("desktop-input-injection-progress");
        std::fs::create_dir_all(&root).expect("create runtime root");
        let config_path = root.join(DEFAULT_DAEMON_CONFIG_FILE);
        std::fs::write(
            &config_path,
            r#"{
                "server_url": "http://127.0.0.1:24888",
                "device_name": "Client",
                "role": "client",
                "master_device_id": "master-device",
                "listen_port": 24800,
                "heartbeat_interval_seconds": 30
            }"#,
        )
        .expect("write config");
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        tx.send(ReceivedInputFrame {
            frame: ProtocolFrame {
                sequence: 1,
                timestamp_micros: 2,
                payload: input_payload(InputEvent::Scroll(kmsync_core::ScrollEvent {
                    dx: 0.0,
                    dy: 1.0,
                })),
            },
            received_at: Instant::now(),
        })
        .expect("queue input");
        drop(tx);
        let mut adapter = RecordingInjector::default();

        inject_received_frames(
            rx,
            &mut adapter,
            ListenerLatencyStats::default(),
            Some(config_path.clone()),
        )
        .expect("inject input");

        let runtime = read_desktop_sync_runtime_status(&config_path).expect("read runtime");
        assert_eq!(
            runtime.state,
            kmsync_core::DesktopSyncRuntimeKind::Listening
        );
        assert_eq!(runtime.injected_events, 1);
        assert!(runtime.last_injected_at.is_some());

        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn injection_loop_releases_all_tracked_input_when_connection_closes() {
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        for (sequence, event) in [
            (
                1,
                InputEvent::Key(KeyEvent {
                    key: Key::LeftControl,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::CONTROL,
                }),
            ),
            (
                2,
                InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::CONTROL,
                }),
            ),
            (
                3,
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Pressed,
                }),
            ),
        ] {
            tx.send(ReceivedInputFrame {
                frame: ProtocolFrame {
                    sequence,
                    timestamp_micros: sequence + 10,
                    payload: input_payload(event),
                },
                received_at: Instant::now(),
            })
            .expect("queue tracked input");
        }
        drop(tx);
        let mut adapter = RecordingInjector::default();

        inject_received_frames(rx, &mut adapter, ListenerLatencyStats::default(), None)
            .expect("release tracked input on connection close");

        assert_eq!(
            adapter.events,
            vec![
                InputEvent::Key(KeyEvent {
                    key: Key::LeftControl,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::CONTROL,
                }),
                InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::CONTROL,
                }),
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Pressed,
                }),
                InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Released,
                    modifiers: Modifiers::NONE,
                }),
                InputEvent::Key(KeyEvent {
                    key: Key::LeftControl,
                    state: KeyState::Released,
                    modifiers: Modifiers::NONE,
                }),
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Released,
                }),
            ]
        );
    }

    #[test]
    fn injection_loop_releases_tracked_input_when_target_device_changes() {
        let (tx, rx) = std::sync::mpsc::sync_channel(3);
        for (sequence, target_device_id, event) in [
            (
                1,
                10,
                InputEvent::Key(KeyEvent {
                    key: Key::LeftControl,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::CONTROL,
                }),
            ),
            (
                2,
                10,
                InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::CONTROL,
                }),
            ),
            (
                3,
                20,
                InputEvent::Key(KeyEvent {
                    key: Key::A,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::CONTROL,
                }),
            ),
        ] {
            tx.send(ReceivedInputFrame {
                frame: ProtocolFrame {
                    sequence,
                    timestamp_micros: sequence + 10,
                    payload: ProtocolPayload::Input(InputEventEnvelope::current(
                        1,
                        target_device_id,
                        event,
                    )),
                },
                received_at: Instant::now(),
            })
            .expect("queue target input");
        }
        drop(tx);
        let mut adapter = RecordingInjector::default();

        inject_received_frames(rx, &mut adapter, ListenerLatencyStats::default(), None)
            .expect("target switch releases tracked input");

        assert_eq!(
            adapter.events,
            vec![
                InputEvent::Key(KeyEvent {
                    key: Key::LeftControl,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::CONTROL,
                }),
                InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::CONTROL,
                }),
                InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Released,
                    modifiers: Modifiers::NONE,
                }),
                InputEvent::Key(KeyEvent {
                    key: Key::LeftControl,
                    state: KeyState::Released,
                    modifiers: Modifiers::NONE,
                }),
                InputEvent::Key(KeyEvent {
                    key: Key::A,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::CONTROL,
                }),
                InputEvent::Key(KeyEvent {
                    key: Key::A,
                    state: KeyState::Released,
                    modifiers: Modifiers::NONE,
                }),
            ]
        );
    }

    #[test]
    fn injection_loop_orders_reliable_input_before_injecting() {
        let (tx, rx) = std::sync::mpsc::sync_channel(2);
        tx.send(ReceivedInputFrame {
            frame: ProtocolFrame {
                sequence: 2,
                timestamp_micros: 2,
                payload: reliable_input_payload(InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Released,
                    modifiers: Modifiers::NONE,
                })),
            },
            received_at: Instant::now(),
        })
        .expect("queue release");
        tx.send(ReceivedInputFrame {
            frame: ProtocolFrame {
                sequence: 1,
                timestamp_micros: 1,
                payload: reliable_input_payload(InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::NONE,
                })),
            },
            received_at: Instant::now(),
        })
        .expect("queue press");
        drop(tx);
        let mut adapter = RecordingInjector::default();

        inject_received_frames(rx, &mut adapter, ListenerLatencyStats::default(), None)
            .expect("inject reliable frames");

        assert_eq!(
            adapter.events,
            vec![
                InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::NONE,
                }),
                InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Released,
                    modifiers: Modifiers::NONE,
                }),
            ]
        );
    }

    #[test]
    fn injection_loop_drops_duplicate_reliable_input_sequence() {
        let (tx, rx) = std::sync::mpsc::sync_channel(3);
        for (sequence, state) in [
            (1, KeyState::Pressed),
            (1, KeyState::Pressed),
            (2, KeyState::Released),
        ] {
            tx.send(ReceivedInputFrame {
                frame: ProtocolFrame {
                    sequence,
                    timestamp_micros: sequence,
                    payload: reliable_input_payload(InputEvent::Key(KeyEvent {
                        key: Key::C,
                        state,
                        modifiers: Modifiers::NONE,
                    })),
                },
                received_at: Instant::now(),
            })
            .expect("queue reliable input");
        }
        drop(tx);
        let mut adapter = RecordingInjector::default();

        inject_received_frames(rx, &mut adapter, ListenerLatencyStats::default(), None)
            .expect("inject reliable frames");

        assert_eq!(
            adapter.events,
            vec![
                InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::NONE,
                }),
                InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Released,
                    modifiers: Modifiers::NONE,
                }),
            ]
        );
    }

    #[test]
    fn injection_loop_does_not_block_reliable_input_after_unreliable_sequence_gap() {
        let (tx, rx) = std::sync::mpsc::sync_channel(3);
        for (sequence, channel, event) in [
            (
                1,
                InputChannel::InputReliable,
                InputEvent::Mouse(MouseEvent::Position {
                    x_ratio: 0.0,
                    y_ratio: 0.5,
                }),
            ),
            (
                2,
                InputChannel::InputUnreliable,
                InputEvent::Mouse(MouseEvent::Move { dx: 12.0, dy: 3.0 }),
            ),
            (
                3,
                InputChannel::InputReliable,
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Pressed,
                }),
            ),
        ] {
            tx.send(ReceivedInputFrame {
                frame: ProtocolFrame {
                    sequence,
                    timestamp_micros: sequence,
                    payload: ProtocolPayload::Input(InputEventEnvelope::new(
                        10, 20, 1, channel, event,
                    )),
                },
                received_at: Instant::now(),
            })
            .expect("queue input");
        }
        drop(tx);
        let mut adapter = RecordingInjector::default();

        inject_received_frames(rx, &mut adapter, ListenerLatencyStats::default(), None)
            .expect("inject frames");

        assert_eq!(
            adapter.events,
            vec![
                InputEvent::Mouse(MouseEvent::Position {
                    x_ratio: 0.0,
                    y_ratio: 0.5,
                }),
                InputEvent::Mouse(MouseEvent::Move { dx: 12.0, dy: 3.0 }),
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Pressed,
                }),
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Released,
                }),
            ]
        );
    }

    #[test]
    fn injection_loop_skips_lost_unreliable_move_before_reliable_click() {
        let (tx, rx) = std::sync::mpsc::sync_channel(2);
        for (sequence, event) in [
            (
                1,
                InputEvent::Mouse(MouseEvent::Position {
                    x_ratio: 0.0,
                    y_ratio: 0.5,
                }),
            ),
            (
                3,
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Pressed,
                }),
            ),
        ] {
            tx.send(ReceivedInputFrame {
                frame: ProtocolFrame {
                    sequence,
                    timestamp_micros: sequence,
                    payload: reliable_input_payload(event),
                },
                received_at: Instant::now(),
            })
            .expect("queue reliable input");
        }
        drop(tx);
        let mut adapter = RecordingInjector::default();

        inject_received_frames(rx, &mut adapter, ListenerLatencyStats::default(), None)
            .expect("inject frames");

        assert_eq!(
            adapter.events,
            vec![
                InputEvent::Mouse(MouseEvent::Position {
                    x_ratio: 0.0,
                    y_ratio: 0.5,
                }),
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Pressed,
                }),
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Released,
                }),
            ]
        );
    }

    #[test]
    fn injection_loop_drops_stale_unreliable_move_before_reliable_click() {
        let (tx, rx) = std::sync::mpsc::sync_channel(3);
        let stale_received_at = Instant::now() - DESKTOP_STALE_MOUSE_MOVE_DROP_AFTER;
        for (sequence, channel, event, received_at) in [
            (
                1,
                InputChannel::InputReliable,
                InputEvent::Mouse(MouseEvent::Position {
                    x_ratio: 0.0,
                    y_ratio: 0.5,
                }),
                Instant::now(),
            ),
            (
                2,
                InputChannel::InputUnreliable,
                InputEvent::Mouse(MouseEvent::Move { dx: 500.0, dy: 0.0 }),
                stale_received_at,
            ),
            (
                3,
                InputChannel::InputReliable,
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Pressed,
                }),
                Instant::now(),
            ),
        ] {
            tx.send(ReceivedInputFrame {
                frame: ProtocolFrame {
                    sequence,
                    timestamp_micros: sequence,
                    payload: ProtocolPayload::Input(InputEventEnvelope::new(
                        10, 20, 1, channel, event,
                    )),
                },
                received_at,
            })
            .expect("queue input");
        }
        drop(tx);
        let mut adapter = RecordingInjector::default();

        inject_received_frames(rx, &mut adapter, ListenerLatencyStats::default(), None)
            .expect("inject frames");

        assert_eq!(
            adapter.events,
            vec![
                InputEvent::Mouse(MouseEvent::Position {
                    x_ratio: 0.0,
                    y_ratio: 0.5,
                }),
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Pressed,
                }),
                InputEvent::Mouse(MouseEvent::Button {
                    button: MouseButton::Left,
                    state: KeyState::Released,
                }),
            ]
        );
    }

    #[test]
    fn input_gap_recovery_deadline_does_not_slide_when_more_frames_arrive() {
        let start = Instant::now();
        let mut recovery = InputGapRecovery::default();

        assert_eq!(
            recovery.next_action(true, start),
            InputGapRecoveryAction::Wait(RELIABLE_INPUT_GAP_RECOVERY_TIMEOUT)
        );
        assert_eq!(
            recovery.next_action(true, start + RELIABLE_INPUT_GAP_RECOVERY_TIMEOUT / 2),
            InputGapRecoveryAction::Wait(RELIABLE_INPUT_GAP_RECOVERY_TIMEOUT / 2)
        );
        assert_eq!(
            recovery.next_action(true, start + RELIABLE_INPUT_GAP_RECOVERY_TIMEOUT),
            InputGapRecoveryAction::Recover
        );
        assert_eq!(
            recovery.next_action(true, start + RELIABLE_INPUT_GAP_RECOVERY_TIMEOUT),
            InputGapRecoveryAction::Wait(RELIABLE_INPUT_GAP_RECOVERY_TIMEOUT)
        );
        assert_eq!(
            recovery.next_action(false, start + RELIABLE_INPUT_GAP_RECOVERY_TIMEOUT),
            InputGapRecoveryAction::Block
        );
    }

    #[test]
    fn injection_loop_waits_for_entry_position_before_unreliable_move() {
        let (tx, rx) = std::sync::mpsc::sync_channel(2);
        for (sequence, channel, event) in [
            (
                2,
                InputChannel::InputUnreliable,
                InputEvent::Mouse(MouseEvent::Move { dx: 12.0, dy: 3.0 }),
            ),
            (
                1,
                InputChannel::InputReliable,
                InputEvent::Mouse(MouseEvent::Position {
                    x_ratio: 0.0,
                    y_ratio: 0.5,
                }),
            ),
        ] {
            tx.send(ReceivedInputFrame {
                frame: ProtocolFrame {
                    sequence,
                    timestamp_micros: sequence,
                    payload: ProtocolPayload::Input(InputEventEnvelope::new(
                        10, 20, 1, channel, event,
                    )),
                },
                received_at: Instant::now(),
            })
            .expect("queue input");
        }
        drop(tx);
        let mut adapter = RecordingInjector::default();

        inject_received_frames(rx, &mut adapter, ListenerLatencyStats::default(), None)
            .expect("inject frames");

        assert_eq!(
            adapter.events,
            vec![
                InputEvent::Mouse(MouseEvent::Position {
                    x_ratio: 0.0,
                    y_ratio: 0.5,
                }),
                InputEvent::Mouse(MouseEvent::Move { dx: 12.0, dy: 3.0 }),
            ]
        );
    }

    #[test]
    fn injection_loop_recovers_from_large_reliable_sequence_gap() {
        let (tx, rx) = std::sync::mpsc::sync_channel(2);
        tx.send(ReceivedInputFrame {
            frame: ProtocolFrame {
                sequence: 1,
                timestamp_micros: 1,
                payload: reliable_input_payload(InputEvent::Key(KeyEvent {
                    key: Key::LeftControl,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::CONTROL,
                })),
            },
            received_at: Instant::now(),
        })
        .expect("queue press");
        tx.send(ReceivedInputFrame {
            frame: ProtocolFrame {
                sequence: 70,
                timestamp_micros: 70,
                payload: reliable_input_payload(InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::NONE,
                })),
            },
            received_at: Instant::now(),
        })
        .expect("queue post-gap press");
        drop(tx);
        let mut adapter = RecordingInjector::default();

        inject_received_frames(rx, &mut adapter, ListenerLatencyStats::default(), None)
            .expect("inject reliable frames");

        assert_eq!(
            adapter.events,
            vec![
                InputEvent::Key(KeyEvent {
                    key: Key::LeftControl,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::CONTROL,
                }),
                InputEvent::Key(KeyEvent {
                    key: Key::LeftControl,
                    state: KeyState::Released,
                    modifiers: Modifiers::NONE,
                }),
                InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Pressed,
                    modifiers: Modifiers::NONE,
                }),
                InputEvent::Key(KeyEvent {
                    key: Key::C,
                    state: KeyState::Released,
                    modifiers: Modifiers::NONE,
                }),
            ]
        );
    }

    #[test]
    fn injection_loop_records_receive_to_inject_latency() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        tx.send(ReceivedInputFrame {
            frame: ProtocolFrame {
                sequence: 1,
                timestamp_micros: 2,
                payload: input_payload(InputEvent::Scroll(kmsync_core::ScrollEvent {
                    dx: 0.0,
                    dy: 1.0,
                })),
            },
            received_at: Instant::now()
                .checked_sub(Duration::from_micros(700))
                .expect("instant can subtract small duration"),
        })
        .expect("queue input");
        drop(tx);
        let mut adapter = RecordingInjector::default();
        let stats = ListenerLatencyStats::default();

        inject_received_frames(rx, &mut adapter, stats.clone(), None).expect("inject input");

        assert!(stats.snapshot().last_receive_to_inject_micros >= 700);
    }

    #[test]
    fn injection_loop_records_end_to_end_input_latency() {
        let sent_at = now_micros().expect("timestamp").saturating_sub(900);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        tx.send(ReceivedInputFrame {
            frame: ProtocolFrame {
                sequence: 1,
                timestamp_micros: sent_at,
                payload: input_payload(InputEvent::Scroll(kmsync_core::ScrollEvent {
                    dx: 0.0,
                    dy: 1.0,
                })),
            },
            received_at: Instant::now(),
        })
        .expect("queue input");
        drop(tx);
        let mut adapter = RecordingInjector::default();
        let stats = ListenerLatencyStats::default();

        inject_received_frames(rx, &mut adapter, stats.clone(), None).expect("inject input");

        assert!(stats.snapshot().last_end_to_end_input_micros >= 900);
    }

    #[test]
    fn clipboard_loop_applies_text_on_separate_worker() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        tx.send(ReceivedClipboardFrame {
            clipboard: ClipboardText::new(20, 1, "secret text".to_string()),
            received_at: Instant::now(),
        })
        .expect("queue clipboard");
        drop(tx);
        let mut adapter = RecordingInjector::default();

        apply_clipboard_frames(rx, &mut adapter).expect("apply clipboard");

        assert_eq!(adapter.clipboard_texts, vec!["secret text".to_string()]);
    }

    #[test]
    fn clipboard_loop_applies_html_with_plain_text_fallback() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let clipboard = ClipboardText::html(
            20,
            1,
            "<strong>secret text</strong>".to_string(),
            "secret text".to_string(),
        );
        tx.send(ReceivedClipboardFrame {
            clipboard: clipboard.clone(),
            received_at: Instant::now(),
        })
        .expect("queue clipboard");
        drop(tx);
        let mut adapter = RecordingInjector::default();

        apply_clipboard_frames(rx, &mut adapter).expect("apply clipboard");

        assert_eq!(adapter.clipboard_contents, vec![clipboard]);
    }

    #[test]
    fn clipboard_loop_applies_image_content() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let clipboard = ClipboardText::image(20, 1, 2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]);
        tx.send(ReceivedClipboardFrame {
            clipboard: clipboard.clone(),
            received_at: Instant::now(),
        })
        .expect("queue clipboard");
        drop(tx);
        let mut adapter = RecordingInjector::default();

        apply_clipboard_frames(rx, &mut adapter).expect("apply clipboard");

        assert_eq!(adapter.clipboard_contents, vec![clipboard]);
        assert!(adapter.clipboard_texts.is_empty());
    }

    #[test]
    fn local_clipboard_text_classifies_http_urls() {
        let mut adapter = RecordingInjector::default();
        adapter
            .clipboard_texts
            .push("https://example.com/path?q=1".to_string());

        let clipboard = adapter.get_clipboard_content().expect("clipboard content");

        assert_eq!(clipboard.format, ClipboardFormat::Url);
        assert_eq!(clipboard.text, "https://example.com/path?q=1");
        assert_eq!(clipboard.html, None);
    }

    #[test]
    fn clipboard_policy_blocks_disabled_oversize_expired_and_sensitive_sources() {
        let clipboard = ClipboardText::new(10, 1, "secret".to_string());
        let now = Instant::now();

        let disabled = ClipboardSyncPolicy {
            enabled: false,
            ..ClipboardSyncPolicy::default()
        };
        assert_eq!(
            disabled.check_local(&clipboard, None, now, now),
            Err(ClipboardPolicyBlock::SyncDisabled)
        );

        let size_limited = ClipboardSyncPolicy {
            max_bytes: 4,
            ..ClipboardSyncPolicy::default()
        };
        assert_eq!(
            size_limited.check_local(&clipboard, None, now, now),
            Err(ClipboardPolicyBlock::TooLarge {
                bytes: 6,
                max_bytes: 4
            })
        );

        let image = ClipboardText::image(10, 1, 2, 1, vec![0; 8]);
        let image_limited = ClipboardSyncPolicy {
            max_bytes: 7,
            ..ClipboardSyncPolicy::default()
        };
        assert_eq!(
            image_limited.check_local(&image, None, now, now),
            Err(ClipboardPolicyBlock::TooLarge {
                bytes: 8,
                max_bytes: 7
            })
        );

        let expiring = ClipboardSyncPolicy {
            ttl: Duration::from_millis(1),
            ..ClipboardSyncPolicy::default()
        };
        assert_eq!(
            expiring.check_local(&clipboard, None, now - Duration::from_millis(5), now),
            Err(ClipboardPolicyBlock::Expired)
        );

        let sensitive_blocklist = ClipboardSyncPolicy {
            sensitive_app_blacklist: vec!["onepassword".to_string(), "bitwarden".to_string()],
            ..ClipboardSyncPolicy::default()
        };
        assert_eq!(
            sensitive_blocklist.check_local(&clipboard, Some("BitWarden"), now, now),
            Err(ClipboardPolicyBlock::SensitiveApp)
        );
    }

    #[test]
    fn clipboard_policy_filters_known_password_manager_sources_by_default() {
        let policy = ClipboardSyncPolicy::default();
        let clipboard = ClipboardText::new(10, 1, "password".to_string());
        let now = Instant::now();

        assert_eq!(
            policy.check_local(&clipboard, Some("1Password.exe"), now, now),
            Err(ClipboardPolicyBlock::SensitiveApp)
        );
        assert_eq!(
            policy.check_local(&clipboard, Some("KeePassXC"), now, now),
            Err(ClipboardPolicyBlock::SensitiveApp)
        );
        assert_eq!(
            policy.check_local(&clipboard, Some("Code.exe"), now, now),
            Ok(())
        );
    }

    #[test]
    fn args_parse_accepts_clip_watch_clipboard_policy() {
        let args = Args::parse(
            [
                "clip-watch",
                "127.0.0.1:24800",
                "2",
                "4096",
                "disabled",
                "30",
                "OnePassword,Bitwarden",
            ]
            .into_iter()
            .map(String::from),
        )
        .expect("parse clip watch policy");

        match args.command {
            Command::ClipWatch {
                target,
                interval,
                policy,
            } => {
                assert_eq!(target, "127.0.0.1:24800".parse().expect("target"));
                assert_eq!(interval, Duration::from_secs(2));
                assert!(!policy.enabled);
                assert_eq!(policy.max_bytes, 4096);
                assert_eq!(policy.ttl, Duration::from_secs(30));
                assert!(policy
                    .sensitive_app_blacklist
                    .contains(&"onepassword".to_string()));
                assert!(policy
                    .sensitive_app_blacklist
                    .contains(&"bitwarden".to_string()));
            }
            _ => panic!("expected clip watch command"),
        }
    }

    #[test]
    fn clipboard_loop_suppresses_own_and_duplicate_remote_frames() {
        let local_source_id = 10;
        let remote = kmsync_core::ClipboardText::new(20, 1, "secret text".to_string());
        let (tx, rx) = std::sync::mpsc::sync_channel(3);
        for clipboard in [
            remote.clone(),
            remote,
            kmsync_core::ClipboardText::new(local_source_id, 2, "echo".to_string()),
        ] {
            tx.send(ReceivedClipboardFrame {
                clipboard,
                received_at: Instant::now(),
            })
            .expect("queue clipboard");
        }
        drop(tx);
        let mut adapter = RecordingInjector::default();
        let mut state = ClipboardSyncState::new(local_source_id);

        apply_clipboard_frames_with_state(
            rx,
            &mut adapter,
            &mut state,
            &ClipboardSyncPolicy::default(),
        )
        .expect("apply clipboard");

        assert_eq!(adapter.clipboard_texts, vec!["secret text".to_string()]);
        assert!(!state.should_send_local_text("secret text"));
        assert!(state.should_send_local_text("manual change"));
    }

    #[test]
    fn clipboard_loop_skips_remote_frames_blocked_by_policy() {
        let now = Instant::now();
        let (tx, rx) = std::sync::mpsc::sync_channel(3);
        for (clipboard, received_at) in [
            (ClipboardText::new(20, 1, "large".to_string()), now),
            (
                ClipboardText::new(20, 2, "old".to_string()),
                now - Duration::from_secs(5),
            ),
            (ClipboardText::new(20, 3, "ok".to_string()), now),
        ] {
            tx.send(ReceivedClipboardFrame {
                clipboard,
                received_at,
            })
            .expect("queue clipboard");
        }
        drop(tx);
        let mut adapter = RecordingInjector::default();
        let mut state = ClipboardSyncState::new(10);
        let policy = ClipboardSyncPolicy {
            max_bytes: 4,
            ttl: Duration::from_secs(1),
            ..ClipboardSyncPolicy::default()
        };

        apply_clipboard_frames_with_state(rx, &mut adapter, &mut state, &policy)
            .expect("apply clipboard");

        assert_eq!(adapter.clipboard_texts, vec!["ok".to_string()]);
    }

    #[derive(Default)]
    struct RecordingProtocolSender {
        events: Vec<ProtocolEvent>,
    }

    impl ProtocolEventSender for RecordingProtocolSender {
        fn send_event(&mut self, event: ProtocolEvent) -> Result<(), String> {
            self.events.push(event);
            Ok(())
        }
    }

    #[derive(Default)]
    struct CountingEventSender {
        sent: usize,
    }

    impl ProtocolEventSender for CountingEventSender {
        fn send_event(&mut self, _event: ProtocolEvent) -> Result<(), String> {
            self.sent += 1;
            Ok(())
        }
    }

    #[test]
    fn capture_callback_enqueues_routed_events_without_transmitting() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let stats = CaptureQueueStats::default();
        let mut router = CaptureRouter::new(CaptureMode::Always, Some(BOUNDS));
        let captured = captured_key(Key::C, Modifiers::META);

        let route = enqueue_routed_capture(&tx, &stats, &mut router, captured);

        assert!(route.send_remote);
        assert_eq!(route.decision, CaptureDecision::Continue);
        assert_eq!(
            rx.try_recv().expect("queued event").event,
            InputEvent::Key(KeyEvent {
                key: Key::C,
                state: KeyState::Pressed,
                modifiers: Modifiers::META,
            })
        );
    }

    #[test]
    fn capture_queue_stats_track_depth_and_full_drops() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let stats = CaptureQueueStats::default();
        let mut router = CaptureRouter::new(CaptureMode::Always, Some(BOUNDS));

        enqueue_routed_capture(
            &tx,
            &stats,
            &mut router,
            captured_key(Key::A, Modifiers::NONE),
        );
        enqueue_routed_capture(
            &tx,
            &stats,
            &mut router,
            captured_key(Key::B, Modifiers::NONE),
        );
        let after_enqueue = stats.snapshot();

        assert_eq!(after_enqueue.current_depth, 1);
        assert_eq!(after_enqueue.enqueued, 1);
        assert_eq!(after_enqueue.dropped_full, 1);
        assert_eq!(after_enqueue.dropped_disconnected, 0);

        drop(tx);
        let compiled =
            CompiledProfile::compile(&Profile::mac_to_windows_default()).expect("compile profile");
        let mut sender = RecordingProtocolSender::default();
        transmit_captured_events(rx, &mut sender, compiled, stats.clone()).expect("transmit event");

        assert_eq!(stats.snapshot().current_depth, 0);
        assert_eq!(sender.events.len(), 1);
    }

    #[test]
    fn runtime_metrics_include_queue_drop_rate_reconnect_count_and_process_resources() {
        let queue = CaptureQueueStatsSnapshot {
            current_depth: 2,
            enqueued: 2,
            dropped_full: 1,
            dropped_disconnected: 1,
            last_capture_to_send_micros: 50,
            max_capture_to_send_micros: 75,
        };
        let resources = ProcessResourceMetrics {
            cpu_total_micros: 42_000,
            memory_bytes: 8 * 1024 * 1024,
        };

        let metrics = RuntimeMetricsSnapshot::from_parts(queue, 7, Some(resources));

        assert_eq!(metrics.input_queue_depth, 2);
        assert_eq!(metrics.input_queue_enqueued, 2);
        assert_eq!(metrics.input_queue_dropped_total, 2);
        assert_eq!(metrics.input_queue_drop_rate_ppm, 500_000);
        assert_eq!(metrics.input_queue_last_capture_to_send_micros, 50);
        assert_eq!(metrics.input_queue_max_capture_to_send_micros, 75);
        assert_eq!(metrics.reconnect_count, 7);
        assert_eq!(metrics.process_cpu_total_micros, Some(42_000));
        assert_eq!(metrics.process_memory_bytes, Some(8 * 1024 * 1024));
    }

    #[test]
    fn runtime_metrics_log_line_is_anonymous() {
        let queue = CaptureQueueStatsSnapshot {
            current_depth: 1,
            enqueued: 10,
            dropped_full: 1,
            dropped_disconnected: 0,
            last_capture_to_send_micros: 20,
            max_capture_to_send_micros: 30,
        };
        let metrics = RuntimeMetricsSnapshot::from_parts(queue, 3, None);

        let line = runtime_metrics_log_line(metrics);

        assert!(line.starts_with("metric=runtime "));
        assert!(line.contains("input_queue_depth=1"));
        assert!(line.contains("reconnect_count=3"));
        assert!(!line.contains("device_id"));
        assert!(!line.contains("clipboard"));
        assert!(!line.contains("source_id"));
        assert!(!line.contains("key="));
        assert!(!line.contains("text="));
        assert!(!line.contains("profile"));
    }

    #[test]
    fn crash_report_omits_panic_payload_content_and_local_paths() {
        let secret_payload = "secret clipboard token from C:\\Users\\Alice".to_string();

        let report = crash_report_from_panic_parts(
            123_456,
            &secret_payload,
            Some(("C:\\Users\\Alice\\work\\kmsync\\src\\main.rs", 77, 9)),
        );
        let rendered = render_crash_report(&report);

        assert!(rendered.contains("event=crash_report"));
        assert!(rendered.contains("timestamp_millis=123456"));
        assert!(rendered.contains("panic_payload_kind=string"));
        assert!(rendered.contains("location=main.rs:77:9"));
        assert!(!rendered.contains("secret clipboard token"));
        assert!(!rendered.contains("C:\\Users\\Alice"));
    }

    #[test]
    fn crash_report_writer_creates_anonymous_report_file() {
        let dir = std::env::temp_dir().join(format!(
            "kmsync-crash-report-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time after epoch")
                .as_nanos()
        ));
        let secret_payload = "password manager clipboard body".to_string();
        let report = crash_report_from_panic_parts(
            234_567,
            &secret_payload,
            Some(("/home/alice/kmsync/crates/kmsync/src/main.rs", 88, 11)),
        );

        let path = write_crash_report(&dir, &report).expect("write crash report");
        let contents = std::fs::read_to_string(&path).expect("read crash report");

        assert!(path.starts_with(&dir));
        assert!(contents.contains("event=crash_report"));
        assert!(contents.contains("location=main.rs:88:11"));
        assert!(!contents.contains("password manager"));
        assert!(!contents.contains("/home/alice"));

        std::fs::remove_dir_all(dir).expect("remove crash report test dir");
    }

    #[test]
    fn tx_loop_maps_sequences_and_sends_queued_events() {
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        tx.send(QueuedInputEvent::new(InputEvent::Key(KeyEvent {
            key: Key::C,
            state: KeyState::Pressed,
            modifiers: Modifiers::META,
        })))
        .expect("queue key");
        tx.send(QueuedInputEvent::new(InputEvent::Scroll(
            kmsync_core::ScrollEvent { dx: 1.0, dy: 2.0 },
        )))
        .expect("queue scroll");
        drop(tx);
        let compiled =
            CompiledProfile::compile(&Profile::mac_to_windows_default()).expect("compile profile");
        let mut sender = RecordingProtocolSender::default();

        transmit_captured_events(rx, &mut sender, compiled, CaptureQueueStats::default())
            .expect("transmit events");

        assert_eq!(sender.events.len(), 2);
        assert_eq!(sender.events[0].sequence, 1);
        assert_eq!(
            sender.events[0].event,
            InputEvent::Key(KeyEvent {
                key: Key::C,
                state: KeyState::Pressed,
                modifiers: Modifiers::CONTROL,
            })
        );
        assert_eq!(sender.events[1].sequence, 2);
        assert_eq!(
            sender.events[1].event,
            InputEvent::Scroll(kmsync_core::ScrollEvent { dx: -1.0, dy: -2.0 })
        );
    }

    #[test]
    fn tx_loop_records_capture_to_send_latency_with_monotonic_clock() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let captured_at = Instant::now()
            .checked_sub(Duration::from_micros(500))
            .expect("instant can subtract small duration");
        tx.send(QueuedInputEvent {
            event: InputEvent::Scroll(kmsync_core::ScrollEvent { dx: 0.0, dy: 1.0 }),
            captured_at,
        })
        .expect("queue event");
        drop(tx);
        let stats = CaptureQueueStats::default();
        let compiled =
            CompiledProfile::compile(&Profile::mac_to_windows_default()).expect("compile profile");
        let mut sender = RecordingProtocolSender::default();

        transmit_captured_events(rx, &mut sender, compiled, stats.clone())
            .expect("transmit events");

        let snapshot = stats.snapshot();
        assert!(snapshot.last_capture_to_send_micros >= 500);
        assert!(snapshot.max_capture_to_send_micros >= snapshot.last_capture_to_send_micros);
    }

    #[test]
    fn tx_loop_coalesces_queued_mouse_move_burst_to_latest() {
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        tx.send(QueuedInputEvent::new(InputEvent::Mouse(MouseEvent::Move {
            dx: 1.0,
            dy: 1.0,
        })))
        .expect("queue first move");
        tx.send(QueuedInputEvent::new(InputEvent::Mouse(MouseEvent::Move {
            dx: 2.0,
            dy: 2.0,
        })))
        .expect("queue second move");
        tx.send(QueuedInputEvent::new(InputEvent::Mouse(MouseEvent::Move {
            dx: 3.0,
            dy: 4.0,
        })))
        .expect("queue latest move");
        tx.send(QueuedInputEvent::new(InputEvent::Key(KeyEvent {
            key: Key::C,
            state: KeyState::Pressed,
            modifiers: Modifiers::META,
        })))
        .expect("queue reliable key");
        drop(tx);
        let compiled =
            CompiledProfile::compile(&Profile::mac_to_windows_default()).expect("compile profile");
        let mut sender = RecordingProtocolSender::default();

        transmit_captured_events(rx, &mut sender, compiled, CaptureQueueStats::default())
            .expect("transmit events");

        assert_eq!(sender.events.len(), 2);
        assert_eq!(sender.events[0].sequence, 1);
        assert_eq!(
            sender.events[0].event,
            InputEvent::Mouse(MouseEvent::Move { dx: 3.0, dy: 4.0 })
        );
        assert_eq!(sender.events[1].sequence, 2);
        assert_eq!(
            sender.events[1].event,
            InputEvent::Key(KeyEvent {
                key: Key::C,
                state: KeyState::Pressed,
                modifiers: Modifiers::CONTROL,
            })
        );
    }

    #[test]
    fn tx_loop_coalesces_high_frequency_mouse_move_burst() {
        const MOVE_COUNT: usize = 4096;
        let (tx, rx) = std::sync::mpsc::sync_channel(MOVE_COUNT);
        for index in 0..MOVE_COUNT {
            tx.send(QueuedInputEvent::new(InputEvent::Mouse(MouseEvent::Move {
                dx: index as f32,
                dy: -(index as f32),
            })))
            .expect("queue mouse move");
        }
        drop(tx);
        let compiled =
            CompiledProfile::compile(&Profile::mac_to_windows_default()).expect("compile profile");
        let mut sender = RecordingProtocolSender::default();

        transmit_captured_events(rx, &mut sender, compiled, CaptureQueueStats::default())
            .expect("transmit moves");

        assert_eq!(sender.events.len(), 1);
        assert_eq!(sender.events[0].sequence, 1);
        assert_eq!(
            sender.events[0].event,
            InputEvent::Mouse(MouseEvent::Move {
                dx: (MOVE_COUNT - 1) as f32,
                dy: -((MOVE_COUNT - 1) as f32),
            })
        );
    }

    #[test]
    fn mouse_move_capture_to_send_hot_path_has_zero_heap_allocations() {
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let stats = CaptureQueueStats::default();
        let mut router = CaptureRouter::new(
            CaptureMode::Edge {
                edge: Edge::Right,
                threshold: 2.0,
                release_hotkey: Hotkey::default_release(),
                cooldown: Duration::ZERO,
            },
            Some(BOUNDS),
        );
        router.active = true;
        let compiled =
            CompiledProfile::compile(&Profile::mac_to_windows_default()).expect("compile profile");
        let mut sender = CountingEventSender::default();

        allocation_tracking::reset();
        let route = enqueue_routed_capture(&tx, &stats, &mut router, captured_move(60.0, 60.0));
        let enqueue_allocations = allocation_tracking::count();
        drop(tx);
        allocation_tracking::reset();
        transmit_captured_events(rx, &mut sender, compiled, stats).expect("transmit mouse move");
        let transmit_allocations = allocation_tracking::count();

        assert!(route.send_remote);
        assert_eq!(route.decision, CaptureDecision::Suppress);
        assert_eq!(sender.sent, 1);
        assert_eq!(enqueue_allocations, 0);
        assert_eq!(transmit_allocations, 0);
    }
}

#[cfg(test)]
mod packaging_tests {
    use std::path::Path;

    fn workspace_root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root")
    }

    #[test]
    fn packaging_scripts_cover_autostart_permission_guidance_and_uninstall_cleanup() {
        let root = workspace_root();
        let macos = std::fs::read_to_string(root.join("packaging/macos/build-pkg.sh"))
            .expect("read macOS packaging script");
        let windows = std::fs::read_to_string(root.join("packaging/windows/kmsync.nsi"))
            .expect("read Windows packaging script");

        assert!(macos.contains("Library/LaunchAgents"));
        assert!(macos.contains("com.kmsync.mvp.plist"));
        assert!(macos.contains("Applications/KMSync.app"));
        assert!(macos.contains("CFBundleIconFile"));
        assert!(macos.contains("KMSync.icns"));
        assert!(macos.contains("USER_GUIDE.md"));
        assert!(macos.contains("uninstall-macos.sh"));
        assert!(macos.contains("launchctl bootout"));
        assert!(macos.contains("cat > \"${SCRIPTS_DIR}/preinstall\""));
        assert!(macos.contains("rm -rf \"/Applications/KMSync.app\""));
        assert!(macos.contains("rm -f \"/Library/LaunchAgents/${LAUNCH_AGENT_ID}.plist\""));
        assert!(macos.contains("stat -f %Su /dev/console"));
        assert!(!macos.contains("cat > \"${PKG_ROOT}/Library/LaunchAgents"));
        assert!(!macos.contains("launchctl bootstrap \"gui/$uid\""));
        assert!(!macos.contains("launchctl kickstart -k \"gui/$uid/${label}\""));

        assert!(windows.contains("Software\\Microsoft\\Windows\\CurrentVersion\\Run"));
        assert!(windows.contains("ExecShell \"open\" \"$INSTDIR\\${APP_EXE}\" \"core-service\""));
        assert!(windows.contains("DeleteRegValue HKLM"));
        assert!(windows.contains("$SMPROGRAMS\\KMSync"));
        assert!(windows.contains("USER_GUIDE.md"));
        assert!(windows.contains("permissions"));
    }

    #[test]
    fn packaging_scripts_support_distribution_signing() {
        let root = workspace_root();
        let macos = std::fs::read_to_string(root.join("packaging/macos/build-pkg.sh"))
            .expect("read macOS packaging script");
        let windows = std::fs::read_to_string(root.join("packaging/windows/build-nsis.ps1"))
            .expect("read Windows packaging script");

        assert!(macos.contains("codesign"));
        assert!(macos.contains("productsign"));
        assert!(macos.contains("xcrun notarytool submit"));
        assert!(macos.contains("xcrun stapler staple"));
        assert!(macos.contains("APPLE_TEAM_ID"));

        assert!(windows.contains("signtool.exe"));
        assert!(windows.contains("AuthenticodeCertificateThumbprint"));
        assert!(windows.contains("TimestampUrl"));
        assert!(windows.contains("Sign-AuthenticodeFile"));
    }

    #[test]
    fn windows_permission_hints_do_not_recommend_service_for_interactive_input() {
        let root = workspace_root();
        let windows_platform =
            std::fs::read_to_string(root.join("crates/kmsync/src/platform/windows.rs"))
                .expect("read Windows platform source");

        assert!(windows_platform.contains("Run as the interactive desktop user"));
        assert!(windows_platform.contains("user-mode companion"));
        assert!(!windows_platform.contains("Use the Windows Service as the system anchor"));
    }

    #[test]
    fn windows_mouse_move_injection_uses_tracked_absolute_position() {
        let root = workspace_root();
        let windows_platform =
            std::fs::read_to_string(root.join("crates/kmsync/src/platform/windows.rs"))
                .expect("read Windows platform source");

        assert!(windows_platform.contains("remote_pointer: RemotePointerState"));
        assert!(windows_platform.contains("fn inject_mouse_move("));
        assert!(windows_platform.contains("absolute_mouse_position_input_for_position"));
        assert!(!windows_platform.contains("dwFlags: MOUSEEVENTF_MOVE,\n"));
    }

    #[test]
    fn windows_absolute_pointer_mapping_targets_last_virtual_desktop_pixel() {
        let root = workspace_root();
        let windows_platform =
            std::fs::read_to_string(root.join("crates/kmsync/src/platform/windows.rs"))
                .expect("read Windows platform source");

        assert!(windows_platform.contains("bounds.x + bounds.width - 1.0"));
        assert!(windows_platform.contains("bounds.y + bounds.height - 1.0"));
        assert!(windows_platform.contains("length - 1.0"));
    }

    #[test]
    fn macos_packaging_ad_hoc_signs_local_builds_for_tcc_permissions() {
        let root = workspace_root();
        let macos = std::fs::read_to_string(root.join("packaging/macos/build-pkg.sh"))
            .expect("read macOS packaging script");

        assert!(macos.contains("CODESIGN_IDENTITY not set; ad-hoc signing"));
        assert!(macos.contains("--sign -"));
        assert!(macos.contains("sign_app_bundle_if_configured"));
        assert!(macos.contains("\"${APP_ROOT}\""));
        assert!(macos.contains("NSInputMonitoringUsageDescription"));
    }

    #[test]
    fn macos_packaging_disables_bundle_relocation_to_install_under_applications() {
        let root = workspace_root();
        let macos = std::fs::read_to_string(root.join("packaging/macos/build-pkg.sh"))
            .expect("read macOS packaging script");

        assert!(
            macos.contains("BundleIsRelocatable"),
            "macOS pkg must disable PackageKit bundle relocation so KMSync.app is installed under /Applications"
        );
        assert!(macos.contains("<false/>"));
        assert!(macos.contains("--component-plist"));
        assert!(macos.contains("\"${COMPONENT_PLIST}\""));
    }
}
