use std::fmt;
use std::io::{Read, Write};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{DesktopLayout, DesktopRole, DesktopState};

pub const LOCAL_IPC_MAX_FRAME_BYTES: usize = 1 << 20;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalIpcTransport {
    WindowsNamedPipe,
    UnixDomainSocket,
}

impl LocalIpcTransport {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WindowsNamedPipe => "windows_named_pipe",
            Self::UnixDomainSocket => "unix_domain_socket",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalIpcEndpoint {
    pub transport: LocalIpcTransport,
    pub address: String,
}

impl LocalIpcEndpoint {
    pub fn new(transport: LocalIpcTransport, address: impl Into<String>) -> Self {
        Self {
            transport,
            address: address.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalIpcRequest {
    Ping {
        nonce: u64,
    },
    Status,
    GetDesktopState,
    SetDeviceRole {
        role: DesktopRole,
        master_device_id: Option<String>,
    },
    SetLayout {
        layout: DesktopLayout,
    },
    SetServerEndpoint {
        host: String,
        port: u16,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalIpcResponse {
    Pong {
        nonce: u64,
    },
    Status {
        service: String,
        version: String,
        input_hot_path: String,
        platform_transport: String,
    },
    DesktopState {
        state: DesktopState,
    },
    ConfigApplied {
        state: DesktopState,
    },
    Error {
        code: String,
        message: String,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub enum LocalIpcError {
    Io(String),
    Encode(String),
    Decode(String),
    FrameTooLarge {
        len: usize,
        max: usize,
    },
    UnsupportedTransport {
        transport: LocalIpcTransport,
        address: String,
    },
}

impl fmt::Display for LocalIpcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "local IPC I/O failed: {error}"),
            Self::Encode(error) => write!(formatter, "local IPC encode failed: {error}"),
            Self::Decode(error) => write!(formatter, "local IPC decode failed: {error}"),
            Self::FrameTooLarge { len, max } => {
                write!(
                    formatter,
                    "local IPC frame too large: {len} bytes exceeds {max}"
                )
            }
            Self::UnsupportedTransport { transport, address } => write!(
                formatter,
                "local IPC transport {} is not available for endpoint {address}",
                transport.as_str()
            ),
        }
    }
}

impl std::error::Error for LocalIpcError {}

impl From<std::io::Error> for LocalIpcError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

pub fn default_local_ipc_endpoint() -> LocalIpcEndpoint {
    #[cfg(windows)]
    {
        LocalIpcEndpoint::new(
            LocalIpcTransport::WindowsNamedPipe,
            r"\\.\pipe\kmsync-core-service",
        )
    }

    #[cfg(unix)]
    {
        LocalIpcEndpoint::new(
            LocalIpcTransport::UnixDomainSocket,
            default_unix_socket_path().to_string_lossy().into_owned(),
        )
    }
}

#[cfg(unix)]
fn default_unix_socket_path() -> std::path::PathBuf {
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|path| !path.is_empty()) {
        return std::path::PathBuf::from(runtime_dir)
            .join("kmsync")
            .join("core-service.sock");
    }

    std::env::temp_dir().join("kmsync-core-service.sock")
}

pub fn write_request_frame<W: Write>(
    writer: &mut W,
    request: &LocalIpcRequest,
) -> Result<(), LocalIpcError> {
    write_json_frame(writer, request)
}

pub fn read_request_frame<R: Read>(reader: &mut R) -> Result<LocalIpcRequest, LocalIpcError> {
    read_json_frame(reader)
}

pub fn write_response_frame<W: Write>(
    writer: &mut W,
    response: &LocalIpcResponse,
) -> Result<(), LocalIpcError> {
    write_json_frame(writer, response)
}

pub fn read_response_frame<R: Read>(reader: &mut R) -> Result<LocalIpcResponse, LocalIpcError> {
    read_json_frame(reader)
}

fn write_json_frame<W: Write, T: Serialize>(
    writer: &mut W,
    value: &T,
) -> Result<(), LocalIpcError> {
    let payload =
        serde_json::to_vec(value).map_err(|error| LocalIpcError::Encode(error.to_string()))?;
    if payload.len() > LOCAL_IPC_MAX_FRAME_BYTES {
        return Err(LocalIpcError::FrameTooLarge {
            len: payload.len(),
            max: LOCAL_IPC_MAX_FRAME_BYTES,
        });
    }
    let len =
        u32::try_from(payload.len()).map_err(|error| LocalIpcError::Encode(error.to_string()))?;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

fn read_json_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> Result<T, LocalIpcError> {
    let mut len_bytes = [0; 4];
    reader.read_exact(&mut len_bytes)?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > LOCAL_IPC_MAX_FRAME_BYTES {
        return Err(LocalIpcError::FrameTooLarge {
            len,
            max: LOCAL_IPC_MAX_FRAME_BYTES,
        });
    }

    let mut payload = vec![0; len];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload).map_err(|error| LocalIpcError::Decode(error.to_string()))
}

pub struct LocalIpcClient {
    connection: LocalIpcConnection,
}

impl LocalIpcClient {
    pub fn connect(endpoint: &LocalIpcEndpoint) -> Result<Self, LocalIpcError> {
        Ok(Self {
            connection: LocalIpcConnection::connect(endpoint)?,
        })
    }

    pub fn request(
        &mut self,
        request: &LocalIpcRequest,
    ) -> Result<LocalIpcResponse, LocalIpcError> {
        write_request_frame(&mut self.connection, request)?;
        read_response_frame(&mut self.connection)
    }
}

pub struct LocalIpcServer {
    listener: LocalIpcListener,
}

impl LocalIpcServer {
    pub fn bind(endpoint: &LocalIpcEndpoint) -> Result<Self, LocalIpcError> {
        Ok(Self {
            listener: LocalIpcListener::bind(endpoint)?,
        })
    }

    pub fn serve_one<F>(self, handler: F) -> Result<(), LocalIpcError>
    where
        F: FnOnce(LocalIpcRequest) -> LocalIpcResponse,
    {
        let mut connection = self.listener.accept()?;
        let request = read_request_frame(&mut connection)?;
        let response = handler(request);
        write_response_frame(&mut connection, &response)
    }
}

enum LocalIpcConnection {
    #[cfg(windows)]
    Windows(windows_named_pipe::NamedPipeConnection),
    #[cfg(unix)]
    Unix(std::os::unix::net::UnixStream),
}

impl LocalIpcConnection {
    fn connect(endpoint: &LocalIpcEndpoint) -> Result<Self, LocalIpcError> {
        #[cfg(windows)]
        if endpoint.transport == LocalIpcTransport::WindowsNamedPipe {
            return windows_named_pipe::NamedPipeConnection::connect(&endpoint.address)
                .map(Self::Windows)
                .map_err(LocalIpcError::from);
        }

        #[cfg(unix)]
        if endpoint.transport == LocalIpcTransport::UnixDomainSocket {
            return std::os::unix::net::UnixStream::connect(&endpoint.address)
                .map(Self::Unix)
                .map_err(LocalIpcError::from);
        }

        Err(LocalIpcError::UnsupportedTransport {
            transport: endpoint.transport,
            address: endpoint.address.clone(),
        })
    }
}

impl Read for LocalIpcConnection {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            #[cfg(windows)]
            Self::Windows(connection) => connection.read(buffer),
            #[cfg(unix)]
            Self::Unix(connection) => connection.read(buffer),
        }
    }
}

impl Write for LocalIpcConnection {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            #[cfg(windows)]
            Self::Windows(connection) => connection.write(buffer),
            #[cfg(unix)]
            Self::Unix(connection) => connection.write(buffer),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            #[cfg(windows)]
            Self::Windows(connection) => connection.flush(),
            #[cfg(unix)]
            Self::Unix(connection) => connection.flush(),
        }
    }
}

enum LocalIpcListener {
    #[cfg(windows)]
    Windows(windows_named_pipe::NamedPipeListener),
    #[cfg(unix)]
    Unix {
        listener: Option<std::os::unix::net::UnixListener>,
        path: std::path::PathBuf,
    },
}

impl LocalIpcListener {
    fn bind(endpoint: &LocalIpcEndpoint) -> Result<Self, LocalIpcError> {
        #[cfg(windows)]
        if endpoint.transport == LocalIpcTransport::WindowsNamedPipe {
            return windows_named_pipe::NamedPipeListener::bind(&endpoint.address)
                .map(Self::Windows)
                .map_err(LocalIpcError::from);
        }

        #[cfg(unix)]
        if endpoint.transport == LocalIpcTransport::UnixDomainSocket {
            let path = std::path::PathBuf::from(&endpoint.address);
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent)?;
            }
            let listener = bind_unix_listener(&path)?;
            return Ok(Self::Unix {
                listener: Some(listener),
                path,
            });
        }

        Err(LocalIpcError::UnsupportedTransport {
            transport: endpoint.transport,
            address: endpoint.address.clone(),
        })
    }

    fn accept(self) -> Result<LocalIpcConnection, LocalIpcError> {
        #[cfg(windows)]
        match self {
            #[cfg(windows)]
            Self::Windows(listener) => listener
                .accept()
                .map(LocalIpcConnection::Windows)
                .map_err(LocalIpcError::from),
        }

        #[cfg(unix)]
        {
            let mut this = self;
            let Self::Unix { listener, .. } = &mut this;
            let listener = listener.take().ok_or_else(|| {
                LocalIpcError::Io("local IPC listener already accepted".to_string())
            })?;
            let (stream, _) = listener.accept()?;
            Ok(LocalIpcConnection::Unix(stream))
        }
    }
}

#[cfg(unix)]
impl Drop for LocalIpcListener {
    fn drop(&mut self) {
        let Self::Unix { path, .. } = self;
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(unix)]
fn bind_unix_listener(path: &std::path::Path) -> std::io::Result<std::os::unix::net::UnixListener> {
    match std::os::unix::net::UnixListener::bind(path) {
        Ok(listener) => Ok(listener),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            if std::os::unix::net::UnixStream::connect(path).is_ok() {
                return Err(error);
            }
            let _ = std::fs::remove_file(path);
            std::os::unix::net::UnixListener::bind(path)
        }
        Err(error) => Err(error),
    }
}

#[allow(unsafe_code)]
#[cfg(windows)]
mod windows_named_pipe {
    use std::ffi::OsStr;
    use std::io::{Read, Write};
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
        PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };

    pub(super) struct NamedPipeListener {
        handle: HANDLE,
    }

    impl NamedPipeListener {
        pub(super) fn bind(name: &str) -> std::io::Result<Self> {
            let name = wide_string(name);
            let handle = unsafe {
                CreateNamedPipeW(
                    name.as_ptr(),
                    PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    PIPE_UNLIMITED_INSTANCES,
                    64 * 1024,
                    64 * 1024,
                    0,
                    null(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self { handle })
        }

        pub(super) fn accept(self) -> std::io::Result<NamedPipeConnection> {
            let connected = unsafe { ConnectNamedPipe(self.handle, null_mut()) };
            if connected == 0 {
                let error = unsafe { GetLastError() };
                if error != ERROR_PIPE_CONNECTED {
                    return Err(std::io::Error::last_os_error());
                }
            }

            let handle = self.handle;
            std::mem::forget(self);
            Ok(NamedPipeConnection { handle })
        }
    }

    impl Drop for NamedPipeListener {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }

    pub(super) struct NamedPipeConnection {
        handle: HANDLE,
    }

    impl NamedPipeConnection {
        pub(super) fn connect(name: &str) -> std::io::Result<Self> {
            let name = wide_string(name);
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self { handle })
        }
    }

    impl Read for NamedPipeConnection {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if buffer.is_empty() {
                return Ok(0);
            }

            let mut read = 0;
            let len = buffer.len().min(u32::MAX as usize) as u32;
            let ok = unsafe {
                ReadFile(
                    self.handle,
                    buffer.as_mut_ptr().cast(),
                    len,
                    &mut read,
                    null_mut(),
                )
            };
            if ok == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(read as usize)
            }
        }
    }

    impl Write for NamedPipeConnection {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            if buffer.is_empty() {
                return Ok(0);
            }

            let mut written = 0;
            let len = buffer.len().min(u32::MAX as usize) as u32;
            let ok = unsafe {
                WriteFile(
                    self.handle,
                    buffer.as_ptr().cast(),
                    len,
                    &mut written,
                    null_mut(),
                )
            };
            if ok == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(written as usize)
            }
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Drop for NamedPipeConnection {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }

    fn wide_string(value: &str) -> Vec<u16> {
        OsStr::new(value).encode_wide().chain(Some(0)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DesktopConnectionState, DesktopLayout, DesktopRole, DesktopState};
    use std::io::Cursor;
    use std::sync::mpsc::sync_channel;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn codec_round_trips_local_ipc_requests_and_responses() {
        let request = LocalIpcRequest::Ping { nonce: 42 };
        let mut request_bytes = Vec::new();
        write_request_frame(&mut request_bytes, &request).expect("write request frame");

        let decoded_request =
            read_request_frame(&mut Cursor::new(request_bytes)).expect("read request frame");

        assert_eq!(decoded_request, request);

        let response = LocalIpcResponse::Pong { nonce: 42 };
        let mut response_bytes = Vec::new();
        write_response_frame(&mut response_bytes, &response).expect("write response frame");

        let decoded_response =
            read_response_frame(&mut Cursor::new(response_bytes)).expect("read response frame");

        assert_eq!(decoded_response, response);
    }

    #[test]
    fn codec_round_trips_desktop_state_requests_and_layout_updates() {
        let get_state = LocalIpcRequest::GetDesktopState;
        let mut request_bytes = Vec::new();
        write_request_frame(&mut request_bytes, &get_state).expect("write get desktop state");
        assert_eq!(
            read_request_frame(&mut Cursor::new(request_bytes)).expect("read get desktop state"),
            get_state
        );

        let layout = DesktopLayout {
            left: None,
            right: Some("device-right".to_string()),
            top: None,
            bottom: Some("device-bottom".to_string()),
        };
        let set_layout = LocalIpcRequest::SetLayout {
            layout: layout.clone(),
        };
        let mut request_bytes = Vec::new();
        write_request_frame(&mut request_bytes, &set_layout).expect("write set layout");
        assert_eq!(
            read_request_frame(&mut Cursor::new(request_bytes)).expect("read set layout"),
            set_layout
        );

        let set_server_endpoint = LocalIpcRequest::SetServerEndpoint {
            host: "203.0.113.10".to_string(),
            port: 24_888,
        };
        let mut request_bytes = Vec::new();
        write_request_frame(&mut request_bytes, &set_server_endpoint)
            .expect("write set server endpoint");
        assert_eq!(
            read_request_frame(&mut Cursor::new(request_bytes)).expect("read set server endpoint"),
            set_server_endpoint
        );

        let state = DesktopState {
            device: crate::DesktopDeviceState {
                role: DesktopRole::Master,
                ..crate::DesktopDeviceState::default()
            },
            server_state: DesktopConnectionState::Connecting,
            master_state: DesktopConnectionState::SelfDevice,
            layout,
            ..DesktopState::default()
        };
        let response = LocalIpcResponse::DesktopState { state };
        let mut response_bytes = Vec::new();
        write_response_frame(&mut response_bytes, &response).expect("write desktop state");
        assert_eq!(
            read_response_frame(&mut Cursor::new(response_bytes)).expect("read desktop state"),
            response
        );
    }

    #[test]
    fn codec_rejects_oversized_local_ipc_frames() {
        let too_large = u32::try_from(LOCAL_IPC_MAX_FRAME_BYTES)
            .expect("max frame fits u32")
            .saturating_add(1);
        let mut bytes = too_large.to_le_bytes().to_vec();

        let error = read_request_frame(&mut Cursor::new(&mut bytes)).expect_err("oversized frame");

        assert!(matches!(
            error,
            LocalIpcError::FrameTooLarge {
                len,
                max
            } if len == usize::try_from(too_large).expect("too large fits usize")
                && max == LOCAL_IPC_MAX_FRAME_BYTES
        ));
    }

    #[test]
    fn default_endpoint_uses_platform_local_ipc_transport() {
        let endpoint = default_local_ipc_endpoint();

        #[cfg(windows)]
        {
            assert_eq!(endpoint.transport, LocalIpcTransport::WindowsNamedPipe);
            assert!(endpoint.address.starts_with(r"\\.\pipe\"));
        }

        #[cfg(unix)]
        {
            assert_eq!(endpoint.transport, LocalIpcTransport::UnixDomainSocket);
            assert!(endpoint.address.ends_with(".sock"));
        }
    }

    #[test]
    fn platform_local_ipc_ping_round_trips() {
        let endpoint = platform_test_endpoint();
        let server_endpoint = endpoint.clone();
        let (ready_tx, ready_rx) = sync_channel(1);

        let server = std::thread::spawn(move || {
            let server = LocalIpcServer::bind(&server_endpoint).expect("bind local ipc server");
            ready_tx.send(()).expect("server ready");
            server
                .serve_one(|request| match request {
                    LocalIpcRequest::Ping { nonce } => LocalIpcResponse::Pong { nonce },
                    other => LocalIpcResponse::Error {
                        code: "unexpected_request".to_string(),
                        message: format!("{other:?}"),
                    },
                })
                .expect("serve one local ipc request");
        });

        ready_rx.recv().expect("server ready");
        let mut client = LocalIpcClient::connect(&endpoint).expect("connect local ipc client");
        let response = client
            .request(&LocalIpcRequest::Ping { nonce: 7 })
            .expect("request pong");

        assert_eq!(response, LocalIpcResponse::Pong { nonce: 7 });
        server.join().expect("server thread");
    }

    fn platform_test_endpoint() -> LocalIpcEndpoint {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();

        #[cfg(windows)]
        {
            LocalIpcEndpoint::new(
                LocalIpcTransport::WindowsNamedPipe,
                format!(r"\\.\pipe\kmsync-ipc-test-{}-{unique}", std::process::id()),
            )
        }

        #[cfg(unix)]
        {
            let path = std::env::temp_dir().join(format!(
                "kmsync-ipc-test-{}-{unique}.sock",
                std::process::id()
            ));
            LocalIpcEndpoint::new(
                LocalIpcTransport::UnixDomainSocket,
                path.to_string_lossy().into_owned(),
            )
        }
    }
}
