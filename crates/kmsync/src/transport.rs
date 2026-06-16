use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Mutex;
use std::sync::{
    mpsc::{sync_channel, Receiver, SyncSender},
    Arc,
};
use std::time::Duration;

use bytes::Bytes;
use chacha20poly1305::{
    aead::{Aead, Payload},
    ChaCha20Poly1305, KeyInit, Nonce,
};
use hkdf::Hkdf;
use kmsync_core::{
    DecodeError, DeviceId, InputEventEnvelope, ProtocolEvent, ProtocolFrame, ProtocolPayload,
    TransportLane,
};
use sha2::Sha256;
use tokio::runtime::{Builder as TokioRuntimeBuilder, Runtime};

const QUIC_STREAM_FRAME_LIMIT_BYTES: usize = 16 * 1024 * 1024;
const QUIC_DATAGRAM_FRAME_BYTES: usize = 1200;
const QUIC_CHANNEL_CAPACITY: usize = 1024;
const QUIC_CONNECT_TIMEOUT: Duration = Duration::from_millis(800);
const QUIC_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const QUIC_SOCKET_BUFFER_BYTES: usize = 1 << 20;
const DATA_PLANE_MAGIC: &[u8; 4] = b"SYE1";
const DATA_PLANE_VERSION: u8 = 1;
const DATA_PLANE_HEADER_LEN: usize = 22;
const DATA_PLANE_REPLAY_WINDOW: u64 = 64;
const DATA_PLANE_DEFAULT_ROTATION_FRAMES: u64 = 4096;

pub struct QuicEventSender {
    runtime: Runtime,
    _endpoint: quinn::Endpoint,
    connection: quinn::Connection,
    sealer: DataPlaneSealer,
    input_reliable: Option<quinn::SendStream>,
    clipboard: Option<quinn::SendStream>,
    control: Option<quinn::SendStream>,
}

impl QuicEventSender {
    pub fn connect(target: SocketAddr) -> Result<Self, String> {
        Self::connect_with_timeout(target, QUIC_CONNECT_TIMEOUT)
    }

    pub fn connect_with_timeout(target: SocketAddr, timeout: Duration) -> Result<Self, String> {
        let runtime = quic_runtime("kmsync-quic-tx", 1)?;
        let mut endpoint = {
            let _guard = runtime.enter();
            quinn::Endpoint::client(any_addr_for_peer(target)).map_err(format_quic_error)?
        };
        endpoint.set_default_client_config(quic_client_config()?);
        let connection = runtime
            .block_on(async {
                let connecting = endpoint
                    .connect(target, "kmsync-peer")
                    .map_err(format_quic_error)?;
                tokio::time::timeout(timeout, connecting)
                    .await
                    .map_err(|_| {
                        format!(
                            "timed out after {}ms while connecting QUIC peer {target}",
                            timeout.as_millis()
                        )
                    })?
                    .map_err(format_quic_error)
            })
            .map_err(|error| format!("failed to connect QUIC peer {target}: {error}"))?;
        let sealer = DataPlaneSealer::for_connection(&connection)?;
        Ok(Self {
            runtime,
            _endpoint: endpoint,
            connection,
            sealer,
            input_reliable: None,
            clipboard: None,
            control: None,
        })
    }

    pub fn send(&mut self, event: ProtocolEvent) -> Result<(), String> {
        self.send_input_event(0, 0, event)
    }

    pub fn send_input_event(
        &mut self,
        source_device_id: DeviceId,
        target_device_id: DeviceId,
        event: ProtocolEvent,
    ) -> Result<(), String> {
        let frame = ProtocolFrame {
            sequence: event.sequence,
            timestamp_micros: event.timestamp_micros,
            payload: ProtocolPayload::Input(InputEventEnvelope::current(
                source_device_id,
                target_device_id,
                event.event,
            )),
        };
        self.send_frame(&frame)
    }

    pub fn send_frame(&mut self, frame: &ProtocolFrame) -> Result<(), String> {
        match quic_delivery_mode_for_frame(frame) {
            QuicDeliveryMode::Datagram(lane) => self.send_datagram_frame(lane, frame),
            QuicDeliveryMode::ReliableStream(lane) => self.send_stream_frame(lane, frame),
        }
    }

    fn send_datagram_frame(&mut self, lane: QuicLane, frame: &ProtocolFrame) -> Result<(), String> {
        if lane != QuicLane::InputUnreliable {
            return Err(format!(
                "QUIC datagram lane must be input_unreliable, got {lane:?}"
            ));
        }
        let sealed = self.sealer.seal_frame(lane, frame)?;
        if sealed.len() > QUIC_DATAGRAM_FRAME_BYTES {
            return Err(format!(
                "encrypted QUIC datagram frame is too large: {} > {QUIC_DATAGRAM_FRAME_BYTES}",
                sealed.len()
            ));
        }
        self.connection
            .send_datagram(Bytes::from(sealed))
            .map_err(|error| format!("failed to send QUIC datagram: {error}"))
    }

    fn send_stream_frame(&mut self, lane: QuicLane, frame: &ProtocolFrame) -> Result<(), String> {
        self.ensure_lane_stream(lane)?;
        let sealed = self.sealer.seal_frame(lane, frame)?;
        let len = u32::try_from(sealed.len())
            .map_err(|_| "QUIC stream frame is too large".to_string())?
            .to_le_bytes();
        let runtime = &self.runtime;
        let stream = match lane {
            QuicLane::InputReliable => self
                .input_reliable
                .as_mut()
                .ok_or_else(|| "QUIC input_reliable stream is not open".to_string())?,
            QuicLane::Clipboard => self
                .clipboard
                .as_mut()
                .ok_or_else(|| "QUIC clipboard stream is not open".to_string())?,
            QuicLane::Control => self
                .control
                .as_mut()
                .ok_or_else(|| "QUIC control stream is not open".to_string())?,
            QuicLane::InputUnreliable => {
                return Err("input_unreliable must use QUIC datagrams".to_string());
            }
        };
        runtime.block_on(async {
            stream
                .write_all(&len)
                .await
                .map_err(|error| format!("failed to write QUIC stream frame length: {error}"))?;
            stream
                .write_all(&sealed)
                .await
                .map_err(|error| format!("failed to write QUIC stream frame: {error}"))
        })
    }

    fn ensure_lane_stream(&mut self, lane: QuicLane) -> Result<(), String> {
        let needs_open = match lane {
            QuicLane::InputReliable => self.input_reliable.is_none(),
            QuicLane::Clipboard => self.clipboard.is_none(),
            QuicLane::Control => self.control.is_none(),
            QuicLane::InputUnreliable => {
                return Err("input_unreliable must use QUIC datagrams".to_string());
            }
        };
        if needs_open {
            let stream = self.open_lane_stream(lane)?;
            match lane {
                QuicLane::InputReliable => self.input_reliable = Some(stream),
                QuicLane::Clipboard => self.clipboard = Some(stream),
                QuicLane::Control => self.control = Some(stream),
                QuicLane::InputUnreliable => unreachable!("checked above"),
            }
        }
        Ok(())
    }

    fn open_lane_stream(&self, lane: QuicLane) -> Result<quinn::SendStream, String> {
        self.runtime.block_on(async {
            let mut stream = self
                .connection
                .open_uni()
                .await
                .map_err(|error| format!("failed to open QUIC {lane:?} stream: {error}"))?;
            stream
                .write_all(&[encode_quic_lane(lane)])
                .await
                .map_err(|error| format!("failed to write QUIC {lane:?} stream header: {error}"))?;
            Ok(stream)
        })
    }
}

pub struct QuicEventReceiver {
    _runtime: Runtime,
    endpoint: quinn::Endpoint,
    frames: Receiver<Result<ProtocolFrame, String>>,
}

impl QuicEventReceiver {
    pub fn bind(bind: SocketAddr) -> Result<Self, String> {
        Self::bind_with_revoked_devices(bind, [])
    }

    fn bind_with_revoked_devices(
        bind: SocketAddr,
        revoked_devices: impl IntoIterator<Item = DeviceId>,
    ) -> Result<Self, String> {
        let runtime = quic_runtime("kmsync-quic-rx", 2)?;
        let server_config = quic_server_config()?;
        let endpoint = {
            let _guard = runtime.enter();
            quinn::Endpoint::server(server_config, bind).map_err(format_quic_error)?
        };
        let (tx, rx) = sync_channel(QUIC_CHANNEL_CAPACITY);
        let accept_endpoint = endpoint.clone();
        let revoked_devices = revoked_devices.into_iter().collect::<Vec<_>>();
        runtime.spawn(async move {
            accept_quic_connections(accept_endpoint, tx, revoked_devices).await;
        });
        Ok(Self {
            _runtime: runtime,
            endpoint,
            frames: rx,
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, String> {
        self.endpoint
            .local_addr()
            .map_err(|error| error.to_string())
    }

    pub fn recv_frame(&mut self) -> Result<ProtocolFrame, String> {
        recv_quic_frame_result(
            self.frames
                .recv()
                .map_err(|_| "QUIC receiver stopped before a frame was available".to_string())?,
        )
    }

    #[cfg(test)]
    fn recv_frame_timeout(&mut self, timeout: Duration) -> Result<ProtocolFrame, String> {
        match self.frames.recv_timeout(timeout) {
            Ok(result) => recv_quic_frame_result(result),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                Err("timed out waiting for QUIC frame".to_string())
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err("QUIC receiver stopped before a frame was available".to_string())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuicLane {
    InputUnreliable,
    InputReliable,
    Clipboard,
    Control,
}

impl QuicLane {
    const fn transport_lane(self) -> TransportLane {
        match self {
            Self::InputUnreliable => TransportLane::InputUnreliable,
            Self::InputReliable => TransportLane::InputReliable,
            Self::Clipboard => TransportLane::Clipboard,
            Self::Control => TransportLane::Control,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuicDeliveryMode {
    Datagram(QuicLane),
    ReliableStream(QuicLane),
}

fn quic_delivery_mode_for_frame(frame: &ProtocolFrame) -> QuicDeliveryMode {
    let lane = quic_lane_for_transport_lane(frame.payload.transport_lane());
    match lane {
        QuicLane::InputUnreliable => QuicDeliveryMode::Datagram(lane),
        QuicLane::InputReliable | QuicLane::Clipboard | QuicLane::Control => {
            QuicDeliveryMode::ReliableStream(lane)
        }
    }
}

const fn quic_lane_for_transport_lane(lane: TransportLane) -> QuicLane {
    match lane {
        TransportLane::InputUnreliable => QuicLane::InputUnreliable,
        TransportLane::InputReliable => QuicLane::InputReliable,
        TransportLane::Clipboard => QuicLane::Clipboard,
        TransportLane::Control => QuicLane::Control,
    }
}

const fn encode_quic_lane(lane: QuicLane) -> u8 {
    match lane {
        QuicLane::InputUnreliable => 1,
        QuicLane::InputReliable => 2,
        QuicLane::Clipboard => 3,
        QuicLane::Control => 4,
    }
}

fn decode_quic_lane(value: u8) -> Result<QuicLane, String> {
    match value {
        1 => Ok(QuicLane::InputUnreliable),
        2 => Ok(QuicLane::InputReliable),
        3 => Ok(QuicLane::Clipboard),
        4 => Ok(QuicLane::Control),
        _ => Err(format!("unknown QUIC lane {value}")),
    }
}

async fn accept_quic_connections(
    endpoint: quinn::Endpoint,
    tx: SyncSender<Result<ProtocolFrame, String>>,
    revoked_devices: Vec<DeviceId>,
) {
    while let Some(incoming) = endpoint.accept().await {
        let tx = tx.clone();
        let revoked_devices = revoked_devices.clone();
        tokio::spawn(async move {
            match incoming.await {
                Ok(connection) => drive_quic_connection(connection, tx, revoked_devices).await,
                Err(error) => {
                    send_quic_result(&tx, Err(format!("failed QUIC handshake: {error}")));
                }
            }
        });
    }
}

async fn drive_quic_connection(
    connection: quinn::Connection,
    tx: SyncSender<Result<ProtocolFrame, String>>,
    revoked_devices: Vec<DeviceId>,
) {
    let opener = match DataPlaneOpener::for_connection(&connection) {
        Ok(opener) => Arc::new(Mutex::new(opener.with_revoked_devices(revoked_devices))),
        Err(error) => {
            send_quic_result(&tx, Err(error));
            return;
        }
    };
    let datagram_connection = connection.clone();
    let datagram_tx = tx.clone();
    let datagram_opener = Arc::clone(&opener);
    tokio::spawn(async move {
        read_quic_datagrams(datagram_connection, datagram_tx, datagram_opener).await;
    });
    while let Ok(stream) = connection.accept_uni().await {
        let stream_tx = tx.clone();
        let stream_opener = Arc::clone(&opener);
        tokio::spawn(async move {
            read_quic_stream(stream, stream_tx, stream_opener).await;
        });
    }
}

async fn read_quic_datagrams(
    connection: quinn::Connection,
    tx: SyncSender<Result<ProtocolFrame, String>>,
    opener: Arc<Mutex<DataPlaneOpener>>,
) {
    loop {
        match connection.read_datagram().await {
            Ok(bytes) => {
                let decoded = open_data_plane_frame(&opener, QuicLane::InputUnreliable, &bytes);
                if send_quic_result(&tx, decoded) {
                    continue;
                }
                break;
            }
            Err(error) => {
                send_quic_result(&tx, Err(format!("failed to read QUIC datagram: {error}")));
                break;
            }
        }
    }
}

async fn read_quic_stream(
    mut stream: quinn::RecvStream,
    tx: SyncSender<Result<ProtocolFrame, String>>,
    opener: Arc<Mutex<DataPlaneOpener>>,
) {
    let mut lane_buffer = [0; 1];
    let lane = match stream.read(&mut lane_buffer).await {
        Ok(Some(1)) => match decode_quic_lane(lane_buffer[0]) {
            Ok(lane) => lane,
            Err(error) => {
                send_quic_result(&tx, Err(error));
                return;
            }
        },
        Ok(Some(_)) | Ok(None) => return,
        Err(error) => {
            send_quic_result(
                &tx,
                Err(format!("failed to read QUIC stream lane header: {error}")),
            );
            return;
        }
    };

    loop {
        let mut len_buffer = [0; 4];
        match stream.read_exact(&mut len_buffer).await {
            Ok(()) => {}
            Err(quinn::ReadExactError::FinishedEarly(0)) => break,
            Err(error) => {
                send_quic_result(
                    &tx,
                    Err(format!("failed to read QUIC stream frame length: {error}")),
                );
                break;
            }
        }
        let len = u32::from_le_bytes(len_buffer) as usize;
        if len > QUIC_STREAM_FRAME_LIMIT_BYTES {
            send_quic_result(
                &tx,
                Err(format!(
                    "QUIC stream frame exceeds limit: {len} > {QUIC_STREAM_FRAME_LIMIT_BYTES}"
                )),
            );
            break;
        }
        let mut buffer = vec![0; len];
        if let Err(error) = stream.read_exact(&mut buffer).await {
            send_quic_result(
                &tx,
                Err(format!("failed to read QUIC stream frame body: {error}")),
            );
            break;
        }
        let decoded = open_data_plane_frame(&opener, lane, &buffer);
        if !send_quic_result(&tx, decoded) {
            break;
        }
    }
}

fn open_data_plane_frame(
    opener: &Mutex<DataPlaneOpener>,
    lane: QuicLane,
    sealed: &[u8],
) -> Result<ProtocolFrame, String> {
    opener
        .lock()
        .map_err(|_| "data-plane security state is poisoned".to_string())?
        .open_frame(lane, sealed)
}

fn send_quic_result(
    tx: &SyncSender<Result<ProtocolFrame, String>>,
    result: Result<ProtocolFrame, String>,
) -> bool {
    tx.send(result).is_ok()
}

fn recv_quic_frame_result(result: Result<ProtocolFrame, String>) -> Result<ProtocolFrame, String> {
    result
}

fn quic_runtime(thread_name: &'static str, worker_threads: usize) -> Result<Runtime, String> {
    TokioRuntimeBuilder::new_multi_thread()
        .worker_threads(worker_threads)
        .thread_name(thread_name)
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| format!("failed to create QUIC runtime: {error}"))
}

fn quic_server_config() -> Result<quinn::ServerConfig, String> {
    let cert = rcgen::generate_simple_self_signed(vec!["kmsync-peer".to_string()])
        .map_err(|error| format!("failed to generate QUIC certificate: {error}"))?;
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
    let cert = rustls::pki_types::CertificateDer::from(cert.cert);
    let mut server_config = quinn::ServerConfig::with_single_cert(vec![cert], key.into())
        .map_err(|error| format!("failed to create QUIC server config: {error}"))?;
    server_config.transport_config(quic_transport_config());
    Ok(server_config)
}

fn quic_client_config() -> Result<quinn::ClientConfig, String> {
    let provider = quinn::rustls::crypto::ring::default_provider();
    let client_crypto = rustls::ClientConfig::builder_with_provider(provider.into())
        .with_safe_default_protocol_versions()
        .map_err(|error| format!("failed to configure QUIC TLS versions: {error}"))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(KMSyncQuicServerVerifier))
        .with_no_client_auth();
    let mut client_config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto)
            .map_err(|error| format!("failed to create QUIC client config: {error}"))?,
    ));
    client_config.transport_config(quic_transport_config());
    Ok(client_config)
}

#[derive(Clone)]
struct DataPlaneSecurityConfig {
    secret: [u8; 32],
    rotation_frames: u64,
    replay_window: u64,
}

impl DataPlaneSecurityConfig {
    fn from_connection(connection: &quinn::Connection) -> Result<Self, String> {
        let mut secret = [0; 32];
        connection
            .export_keying_material(&mut secret, b"kmsync-data-plane-v1", b"frame-encryption")
            .map_err(|error| format!("failed to export QUIC data-plane key: {error:?}"))?;
        Ok(Self::from_secret(secret))
    }

    fn from_secret(secret: [u8; 32]) -> Self {
        Self {
            secret,
            rotation_frames: DATA_PLANE_DEFAULT_ROTATION_FRAMES,
            replay_window: DATA_PLANE_REPLAY_WINDOW,
        }
    }

    #[cfg(test)]
    fn from_test_secret(secret: [u8; 32]) -> Self {
        Self::from_secret(secret)
    }

    #[cfg(test)]
    const fn with_rotation_frames(mut self, rotation_frames: u64) -> Self {
        self.rotation_frames = rotation_frames;
        self
    }

    fn key_for_epoch(&self, epoch: u64) -> Result<[u8; 32], String> {
        let hkdf = Hkdf::<Sha256>::new(Some(b"kmsync-data-plane-key-v1"), &self.secret);
        let mut key = [0; 32];
        let mut epoch_info = [0; 24];
        let prefix = b"frame-key-epoch-";
        epoch_info[..prefix.len()].copy_from_slice(prefix);
        epoch_info[prefix.len()..].copy_from_slice(&epoch.to_le_bytes());
        hkdf.expand(&epoch_info, &mut key)
            .map_err(|_| "failed to derive data-plane frame key".to_string())?;
        Ok(key)
    }
}

struct DataPlaneSealer {
    config: DataPlaneSecurityConfig,
    epoch: u64,
    frames_in_epoch: u64,
    next_sequence: u64,
}

impl DataPlaneSealer {
    fn for_connection(connection: &quinn::Connection) -> Result<Self, String> {
        Ok(Self::new(DataPlaneSecurityConfig::from_connection(
            connection,
        )?))
    }

    fn new(config: DataPlaneSecurityConfig) -> Self {
        Self {
            config,
            epoch: 0,
            frames_in_epoch: 0,
            next_sequence: 1,
        }
    }

    fn seal_frame(&mut self, lane: QuicLane, frame: &ProtocolFrame) -> Result<Vec<u8>, String> {
        if self.frames_in_epoch >= self.config.rotation_frames.max(1) {
            self.epoch = self.epoch.saturating_add(1);
            self.frames_in_epoch = 0;
        }
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.frames_in_epoch = self.frames_in_epoch.saturating_add(1);

        let encoded = frame.encode_vec();
        let header = data_plane_header(lane, self.epoch, sequence);
        let key = self.config.key_for_epoch(self.epoch)?;
        let cipher = ChaCha20Poly1305::new_from_slice(&key)
            .map_err(|_| "failed to initialize data-plane cipher".to_string())?;
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&data_plane_nonce(self.epoch, sequence)?),
                Payload {
                    msg: &encoded,
                    aad: &header,
                },
            )
            .map_err(|_| "failed to encrypt data-plane frame".to_string())?;
        let mut sealed = Vec::with_capacity(header.len().saturating_add(ciphertext.len()));
        sealed.extend_from_slice(&header);
        sealed.extend_from_slice(&ciphertext);
        Ok(sealed)
    }
}

struct DataPlaneOpener {
    config: DataPlaneSecurityConfig,
    replay_guard: DataPlaneReplayGuard,
    revoked_devices: Vec<DeviceId>,
}

impl DataPlaneOpener {
    fn for_connection(connection: &quinn::Connection) -> Result<Self, String> {
        Ok(Self::new(DataPlaneSecurityConfig::from_connection(
            connection,
        )?))
    }

    fn new(config: DataPlaneSecurityConfig) -> Self {
        Self {
            replay_guard: DataPlaneReplayGuard::new(config.replay_window),
            config,
            revoked_devices: Vec::new(),
        }
    }

    fn with_revoked_devices(mut self, revoked_devices: impl IntoIterator<Item = DeviceId>) -> Self {
        self.revoked_devices = revoked_devices.into_iter().collect();
        self
    }

    fn open_frame(
        &mut self,
        expected_lane: QuicLane,
        sealed: &[u8],
    ) -> Result<ProtocolFrame, String> {
        let header = data_plane_parse_header(sealed)?;
        if header.lane != expected_lane {
            return Err(format!(
                "data-plane lane mismatch: header={:?} expected={expected_lane:?}",
                header.lane
            ));
        }
        let key = self.config.key_for_epoch(header.epoch)?;
        let cipher = ChaCha20Poly1305::new_from_slice(&key)
            .map_err(|_| "failed to initialize data-plane cipher".to_string())?;
        let ciphertext = &sealed[DATA_PLANE_HEADER_LEN..];
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&data_plane_nonce(header.epoch, header.sequence)?),
                Payload {
                    msg: ciphertext,
                    aad: &sealed[..DATA_PLANE_HEADER_LEN],
                },
            )
            .map_err(|_| "failed to decrypt data-plane frame".to_string())?;
        let frame = ProtocolFrame::decode(&plaintext).map_err(format_decode_error)?;
        if frame.payload.transport_lane() != expected_lane.transport_lane() {
            return Err(format!(
                "data-plane payload lane mismatch: expected={:?} frame={:?}",
                expected_lane.transport_lane(),
                frame.payload.transport_lane()
            ));
        }
        if let Some(device_id) = revoked_frame_device_id(&frame, &self.revoked_devices) {
            return Err(format!(
                "revoked device {device_id} attempted data-plane frame"
            ));
        }
        self.replay_guard.accept(header.epoch, header.sequence)?;
        Ok(frame)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DataPlaneHeader {
    lane: QuicLane,
    epoch: u64,
    sequence: u64,
}

fn data_plane_header(lane: QuicLane, epoch: u64, sequence: u64) -> [u8; DATA_PLANE_HEADER_LEN] {
    let mut header = [0; DATA_PLANE_HEADER_LEN];
    header[..4].copy_from_slice(DATA_PLANE_MAGIC);
    header[4] = DATA_PLANE_VERSION;
    header[5] = encode_quic_lane(lane);
    header[6..14].copy_from_slice(&epoch.to_le_bytes());
    header[14..22].copy_from_slice(&sequence.to_le_bytes());
    header
}

fn data_plane_parse_header(sealed: &[u8]) -> Result<DataPlaneHeader, String> {
    if sealed.len() < DATA_PLANE_HEADER_LEN {
        return Err("data-plane envelope is too short".to_string());
    }
    if &sealed[..4] != DATA_PLANE_MAGIC {
        return Err("data-plane envelope has bad magic".to_string());
    }
    if sealed[4] != DATA_PLANE_VERSION {
        return Err(format!(
            "unsupported data-plane envelope version {}",
            sealed[4]
        ));
    }
    let lane = decode_quic_lane(sealed[5])?;
    let epoch = u64::from_le_bytes(sealed[6..14].try_into().expect("header length checked"));
    let sequence = u64::from_le_bytes(sealed[14..22].try_into().expect("header length checked"));
    Ok(DataPlaneHeader {
        lane,
        epoch,
        sequence,
    })
}

#[cfg(test)]
fn data_plane_epoch(sealed: &[u8]) -> Result<u64, String> {
    data_plane_parse_header(sealed).map(|header| header.epoch)
}

fn data_plane_nonce(epoch: u64, sequence: u64) -> Result<[u8; 12], String> {
    let epoch = u32::try_from(epoch)
        .map_err(|_| "data-plane key epoch exceeded nonce space".to_string())?;
    let mut nonce = [0; 12];
    nonce[..4].copy_from_slice(&epoch.to_le_bytes());
    nonce[4..].copy_from_slice(&sequence.to_le_bytes());
    Ok(nonce)
}

#[derive(Default)]
struct DataPlaneReplayGuard {
    window: u64,
    epochs: std::collections::HashMap<u64, DataPlaneReplayWindow>,
}

impl DataPlaneReplayGuard {
    fn new(window: u64) -> Self {
        Self {
            window: window.max(1).min(128),
            epochs: std::collections::HashMap::new(),
        }
    }

    fn accept(&mut self, epoch: u64, sequence: u64) -> Result<(), String> {
        self.epochs
            .entry(epoch)
            .or_default()
            .accept(sequence, self.window)
    }
}

#[derive(Default)]
struct DataPlaneReplayWindow {
    highest: u64,
    seen: u128,
}

impl DataPlaneReplayWindow {
    fn accept(&mut self, sequence: u64, window: u64) -> Result<(), String> {
        if sequence > self.highest {
            let shift = sequence - self.highest;
            self.seen = if shift >= 128 {
                1
            } else {
                (self.seen << shift) | 1
            };
            self.highest = sequence;
            return Ok(());
        }

        let offset = self.highest - sequence;
        if offset >= window {
            return Err(format!(
                "data-plane replay window rejected old sequence {sequence}"
            ));
        }
        let bit = 1_u128 << offset;
        if self.seen & bit != 0 {
            return Err(format!(
                "data-plane replay detected duplicate sequence {sequence}"
            ));
        }
        self.seen |= bit;
        Ok(())
    }
}

fn revoked_frame_device_id(
    frame: &ProtocolFrame,
    revoked_devices: &[DeviceId],
) -> Option<DeviceId> {
    if revoked_devices.is_empty() {
        return None;
    }
    frame_device_ids(frame)
        .into_iter()
        .flatten()
        .find(|device_id| revoked_devices.contains(device_id))
}

fn frame_device_ids(frame: &ProtocolFrame) -> [Option<DeviceId>; 2] {
    match &frame.payload {
        ProtocolPayload::Input(input) => {
            [Some(input.source_device_id), Some(input.target_device_id)]
        }
        ProtocolPayload::ClipboardText(clipboard) => [Some(clipboard.source_id), None],
        ProtocolPayload::ClipboardFiles(files) => [Some(files.source_id), None],
        ProtocolPayload::FileChunk(chunk) => [Some(chunk.source_id), None],
        ProtocolPayload::Control(message) => [Some(control_source_device_id(message)), None],
    }
}

const fn control_source_device_id(message: &kmsync_core::ControlMessage) -> DeviceId {
    match message {
        kmsync_core::ControlMessage::Heartbeat {
            source_device_id, ..
        }
        | kmsync_core::ControlMessage::Capabilities {
            source_device_id, ..
        }
        | kmsync_core::ControlMessage::ConfigVersion {
            source_device_id, ..
        }
        | kmsync_core::ControlMessage::SessionState {
            source_device_id, ..
        } => *source_device_id,
    }
}

fn quic_transport_config() -> Arc<quinn::TransportConfig> {
    let mut config = quinn::TransportConfig::default();
    config.max_idle_timeout(Some(
        QUIC_IDLE_TIMEOUT
            .try_into()
            .expect("30 second idle timeout is valid"),
    ));
    config.max_concurrent_uni_streams(256_u32.into());
    config.datagram_receive_buffer_size(Some(QUIC_SOCKET_BUFFER_BYTES));
    config.datagram_send_buffer_size(QUIC_SOCKET_BUFFER_BYTES);
    Arc::new(config)
}

#[derive(Debug)]
struct KMSyncQuicServerVerifier;

impl rustls::client::danger::ServerCertVerifier for KMSyncQuicServerVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
        ]
    }
}

fn any_addr_for_peer(peer: SocketAddr) -> SocketAddr {
    match peer.ip() {
        IpAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        IpAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
    }
}

fn format_decode_error(error: DecodeError) -> String {
    format!("decode error: {error:?}")
}

fn format_quic_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quic_delivery_mode_uses_datagrams_for_mouse_motion_only() {
        let mouse_move = ProtocolFrame {
            sequence: 1,
            timestamp_micros: 10,
            payload: ProtocolPayload::Input(InputEventEnvelope::current(
                1,
                2,
                kmsync_core::InputEvent::Mouse(kmsync_core::MouseEvent::Move { dx: 2.0, dy: -1.0 }),
            )),
        };
        let key = ProtocolFrame {
            sequence: 2,
            timestamp_micros: 11,
            payload: ProtocolPayload::Input(InputEventEnvelope::current(
                1,
                2,
                kmsync_core::InputEvent::Key(kmsync_core::KeyEvent {
                    key: kmsync_core::Key::A,
                    state: kmsync_core::KeyState::Pressed,
                    modifiers: kmsync_core::Modifiers::NONE,
                }),
            )),
        };
        let clipboard = ProtocolFrame {
            sequence: 3,
            timestamp_micros: 12,
            payload: ProtocolPayload::ClipboardText(kmsync_core::ClipboardText::new(
                1,
                1,
                "hello".to_string(),
            )),
        };

        assert_eq!(
            quic_delivery_mode_for_frame(&mouse_move),
            QuicDeliveryMode::Datagram(QuicLane::InputUnreliable)
        );
        assert_eq!(
            quic_delivery_mode_for_frame(&key),
            QuicDeliveryMode::ReliableStream(QuicLane::InputReliable)
        );
        assert_eq!(
            quic_delivery_mode_for_frame(&clipboard),
            QuicDeliveryMode::ReliableStream(QuicLane::Clipboard)
        );
    }

    #[test]
    fn quic_round_trips_datagram_and_reliable_stream_frames() {
        let mut receiver = QuicEventReceiver::bind("127.0.0.1:0".parse().expect("bind address"))
            .expect("bind quic receiver");
        let mut sender = QuicEventSender::connect(receiver.local_addr().expect("local address"))
            .expect("connect quic sender");

        let mouse_move = ProtocolFrame {
            sequence: 1,
            timestamp_micros: 10,
            payload: ProtocolPayload::Input(InputEventEnvelope::current(
                1,
                2,
                kmsync_core::InputEvent::Mouse(kmsync_core::MouseEvent::Move { dx: 2.0, dy: -1.0 }),
            )),
        };
        let key = ProtocolFrame {
            sequence: 2,
            timestamp_micros: 11,
            payload: ProtocolPayload::Input(InputEventEnvelope::current(
                1,
                2,
                kmsync_core::InputEvent::Key(kmsync_core::KeyEvent {
                    key: kmsync_core::Key::A,
                    state: kmsync_core::KeyState::Pressed,
                    modifiers: kmsync_core::Modifiers::NONE,
                }),
            )),
        };
        let clipboard = ProtocolFrame {
            sequence: 3,
            timestamp_micros: 12,
            payload: ProtocolPayload::ClipboardText(kmsync_core::ClipboardText::new(
                1,
                1,
                "hello".to_string(),
            )),
        };

        sender.send_frame(&mouse_move).expect("send mouse move");
        sender.send_frame(&key).expect("send key");
        sender.send_frame(&clipboard).expect("send clipboard");

        let mut received = Vec::new();
        for _ in 0..3 {
            received.push(
                receiver
                    .recv_frame_timeout(std::time::Duration::from_secs(5))
                    .expect("receive quic frame"),
            );
        }
        received.sort_by_key(|frame| frame.sequence);

        assert_eq!(received, vec![mouse_move, key, clipboard]);
    }

    #[test]
    fn quic_reliable_input_stream_preserves_frame_order() {
        let mut receiver = QuicEventReceiver::bind("127.0.0.1:0".parse().expect("bind address"))
            .expect("bind quic receiver");
        let mut sender = QuicEventSender::connect(receiver.local_addr().expect("local address"))
            .expect("connect quic sender");

        let first = ProtocolFrame {
            sequence: 1,
            timestamp_micros: 10,
            payload: ProtocolPayload::Input(InputEventEnvelope::current(
                1,
                2,
                kmsync_core::InputEvent::Key(kmsync_core::KeyEvent {
                    key: kmsync_core::Key::A,
                    state: kmsync_core::KeyState::Pressed,
                    modifiers: kmsync_core::Modifiers::NONE,
                }),
            )),
        };
        let second = ProtocolFrame {
            sequence: 2,
            timestamp_micros: 11,
            payload: ProtocolPayload::Input(InputEventEnvelope::current(
                1,
                2,
                kmsync_core::InputEvent::Key(kmsync_core::KeyEvent {
                    key: kmsync_core::Key::A,
                    state: kmsync_core::KeyState::Released,
                    modifiers: kmsync_core::Modifiers::NONE,
                }),
            )),
        };

        sender.send_frame(&first).expect("send first");
        sender.send_frame(&second).expect("send second");

        assert_eq!(
            receiver
                .recv_frame_timeout(std::time::Duration::from_secs(5))
                .expect("receive first"),
            first
        );
        assert_eq!(
            receiver
                .recv_frame_timeout(std::time::Duration::from_secs(5))
                .expect("receive second"),
            second
        );
    }

    #[test]
    fn quic_connect_to_unreachable_peer_returns_after_timeout() {
        let target = "203.0.113.1:24800".parse().expect("test address");
        let started_at = std::time::Instant::now();

        let error = match QuicEventSender::connect_with_timeout(target, Duration::from_millis(100))
        {
            Ok(_) => panic!("unreachable peer should not connect"),
            Err(error) => error,
        };

        assert!(
            started_at.elapsed() < Duration::from_secs(2),
            "connect should return quickly, got {:?}",
            started_at.elapsed()
        );
        assert!(error.contains("QUIC peer"));
    }

    #[test]
    fn quic_receiver_rejects_revoked_device_frames() {
        let mut receiver = QuicEventReceiver::bind_with_revoked_devices(
            "127.0.0.1:0".parse().expect("bind address"),
            [42],
        )
        .expect("bind quic receiver");
        let mut sender = QuicEventSender::connect(receiver.local_addr().expect("local address"))
            .expect("connect quic sender");
        let frame = reliable_key_frame(1, 42, 9);

        sender.send_frame(&frame).expect("send revoked frame");

        let error = receiver
            .recv_frame_timeout(std::time::Duration::from_secs(5))
            .expect_err("revoked frame should be rejected by receiver");
        assert!(error.contains("revoked device 42"));
    }

    #[test]
    fn data_plane_security_encrypts_frames_before_transport_delivery() {
        let config = DataPlaneSecurityConfig::from_test_secret([7; 32]);
        let mut sealer = DataPlaneSealer::new(config.clone());
        let mut opener = DataPlaneOpener::new(config);
        let frame = reliable_key_frame(1, 42, 9);

        let sealed = sealer
            .seal_frame(QuicLane::InputReliable, &frame)
            .expect("seal frame");

        assert!(ProtocolFrame::decode(&sealed).is_err());
        assert!(!sealed
            .windows(b"KMSYNC".len())
            .any(|window| window == b"KMSYNC"));
        assert_eq!(
            opener
                .open_frame(QuicLane::InputReliable, &sealed)
                .expect("open frame"),
            frame
        );
    }

    #[test]
    fn data_plane_security_rejects_replayed_envelopes() {
        let config = DataPlaneSecurityConfig::from_test_secret([9; 32]);
        let mut sealer = DataPlaneSealer::new(config.clone());
        let mut opener = DataPlaneOpener::new(config);
        let sealed = sealer
            .seal_frame(QuicLane::InputReliable, &reliable_key_frame(1, 1, 2))
            .expect("seal frame");

        opener
            .open_frame(QuicLane::InputReliable, &sealed)
            .expect("first open succeeds");

        let error = opener
            .open_frame(QuicLane::InputReliable, &sealed)
            .expect_err("replayed envelope must fail");
        assert!(error.contains("replay"));
    }

    #[test]
    fn data_plane_security_rotates_keys_after_configured_frame_count() {
        let config = DataPlaneSecurityConfig::from_test_secret([11; 32]).with_rotation_frames(2);
        let mut sealer = DataPlaneSealer::new(config.clone());
        let mut opener = DataPlaneOpener::new(config);

        let first = sealer
            .seal_frame(QuicLane::InputReliable, &reliable_key_frame(1, 1, 2))
            .expect("seal first");
        let second = sealer
            .seal_frame(QuicLane::InputReliable, &reliable_key_frame(2, 1, 2))
            .expect("seal second");
        let third = sealer
            .seal_frame(QuicLane::InputReliable, &reliable_key_frame(3, 1, 2))
            .expect("seal third");

        assert_eq!(data_plane_epoch(&first).expect("first epoch"), 0);
        assert_eq!(data_plane_epoch(&second).expect("second epoch"), 0);
        assert_eq!(data_plane_epoch(&third).expect("third epoch"), 1);
        opener
            .open_frame(QuicLane::InputReliable, &first)
            .expect("open first");
        opener
            .open_frame(QuicLane::InputReliable, &second)
            .expect("open second");
        opener
            .open_frame(QuicLane::InputReliable, &third)
            .expect("open third");
    }

    #[test]
    fn data_plane_security_rejects_revoked_device_frames() {
        let config = DataPlaneSecurityConfig::from_test_secret([13; 32]);
        let mut sealer = DataPlaneSealer::new(config.clone());
        let mut opener = DataPlaneOpener::new(config).with_revoked_devices([42]);
        let sealed = sealer
            .seal_frame(QuicLane::InputReliable, &reliable_key_frame(1, 42, 9))
            .expect("seal frame");

        let error = opener
            .open_frame(QuicLane::InputReliable, &sealed)
            .expect_err("revoked source should be rejected");

        assert!(error.contains("revoked device"));
    }

    fn reliable_key_frame(
        sequence: u64,
        source_device_id: kmsync_core::DeviceId,
        target_device_id: kmsync_core::DeviceId,
    ) -> ProtocolFrame {
        ProtocolFrame {
            sequence,
            timestamp_micros: 10,
            payload: ProtocolPayload::Input(InputEventEnvelope::current(
                source_device_id,
                target_device_id,
                kmsync_core::InputEvent::Key(kmsync_core::KeyEvent {
                    key: kmsync_core::Key::A,
                    state: kmsync_core::KeyState::Pressed,
                    modifiers: kmsync_core::Modifiers::NONE,
                }),
            )),
        }
    }
}
