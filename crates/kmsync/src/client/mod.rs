use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;
use kmsync_core::{CompiledProfile, Profile};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::transport::QuicEventSender;

const MDNS_SERVICE_TYPE: &str = "_kmsync._udp.local";
const MDNS_MULTICAST_ENDPOINT: &str = "224.0.0.251:5353";
const MDNS_DISCOVERY_TIMEOUT: Duration = Duration::from_millis(350);
const DNS_TYPE_A: u16 = 1;
const DNS_TYPE_PTR: u16 = 12;
const DNS_TYPE_TXT: u16 = 16;
const DNS_TYPE_AAAA: u16 = 28;
const DNS_TYPE_SRV: u16 = 33;
const DNS_CLASS_IN: u16 = 1;
const DNS_CLASS_QU: u16 = 0x8000;
const DEVICE_IDENTITY_SECRET_SERVICE: &str = "com.kmsync.device-identity";
const DEVICE_IDENTITY_SECRET_STORE_SYSTEM: &str = "system";

#[derive(Debug, Clone, Deserialize)]
pub struct ClientConfig {
    pub server_url: String,
    pub email: String,
    #[serde(default)]
    pub email_login_code: Option<String>,
    pub device_name: String,
    pub listen_port: u16,
    pub heartbeat_interval_seconds: u64,
    #[serde(default = "default_identity_path")]
    pub identity_path: PathBuf,
}

impl ClientConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        serde_json::from_str(&text)
            .map_err(|error| format!("failed to parse {}: {error}", path.display()))
    }
}

fn default_identity_path() -> PathBuf {
    PathBuf::from("kmsync-device-identity.json")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceIdentity {
    pub public_key: String,
    pub private_key: String,
}

impl DeviceIdentity {
    pub fn load_or_generate(path: &Path) -> Result<Self, String> {
        let secret_store = SystemDeviceIdentitySecretStore;
        Self::load_or_generate_with_store(path, &secret_store)
    }

    fn load_or_generate_with_store<S>(path: &Path, secret_store: &S) -> Result<Self, String>
    where
        S: DeviceIdentitySecretStore,
    {
        if path.exists() {
            let text = fs::read_to_string(path)
                .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
            let stored: DeviceIdentityFile = serde_json::from_str(&text)
                .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
            return stored.load(path, secret_store);
        }

        let identity = Self::generate();
        let private_key_ref = DeviceIdentitySecretRef::for_public_key(&identity.public_key);
        secret_store.store_private_key(&private_key_ref, &identity.private_key)?;
        write_device_identity_file(path, &identity.public_key, private_key_ref)?;
        Ok(identity)
    }

    fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        Self {
            public_key: format!(
                "ed25519:{}",
                hex_encode(&signing_key.verifying_key().to_bytes())
            ),
            private_key: format!("ed25519:{}", hex_encode(&signing_key.to_bytes())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DeviceIdentitySecretRef {
    #[serde(default = "default_device_identity_secret_store")]
    store: String,
    service: String,
    account: String,
}

impl DeviceIdentitySecretRef {
    fn for_public_key(public_key: &str) -> Self {
        Self {
            store: DEVICE_IDENTITY_SECRET_STORE_SYSTEM.to_string(),
            service: DEVICE_IDENTITY_SECRET_SERVICE.to_string(),
            account: format!("device-{}", sanitize_secret_account(public_key)),
        }
    }
}

fn default_device_identity_secret_store() -> String {
    DEVICE_IDENTITY_SECRET_STORE_SYSTEM.to_string()
}

fn sanitize_secret_account(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeviceIdentityFile {
    public_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    private_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    private_key_ref: Option<DeviceIdentitySecretRef>,
}

impl DeviceIdentityFile {
    fn load<S>(self, path: &Path, secret_store: &S) -> Result<DeviceIdentity, String>
    where
        S: DeviceIdentitySecretStore,
    {
        if let Some(private_key_ref) = self.private_key_ref {
            let private_key = secret_store.load_private_key(&private_key_ref)?;
            return Ok(DeviceIdentity {
                public_key: self.public_key,
                private_key,
            });
        }

        if let Some(private_key) = self.private_key {
            let private_key_ref = DeviceIdentitySecretRef::for_public_key(&self.public_key);
            secret_store.store_private_key(&private_key_ref, &private_key)?;
            write_device_identity_file(path, &self.public_key, private_key_ref)?;
            return Ok(DeviceIdentity {
                public_key: self.public_key,
                private_key,
            });
        }

        Err(format!(
            "device identity {} is missing a private key reference",
            path.display()
        ))
    }
}

fn write_device_identity_file(
    path: &Path,
    public_key: &str,
    private_key_ref: DeviceIdentitySecretRef,
) -> Result<(), String> {
    let stored = DeviceIdentityFile {
        public_key: public_key.to_string(),
        private_key: None,
        private_key_ref: Some(private_key_ref),
    };
    write_device_identity_metadata(path, &stored)
}

fn write_device_identity_metadata(path: &Path, stored: &DeviceIdentityFile) -> Result<(), String> {
    let text = serde_json::to_string_pretty(stored)
        .map_err(|error| format!("failed to encode device identity: {error}"))?;
    write_device_identity_text(path, &text)
}

fn write_device_identity_text(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(path, text)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    Ok(())
}

trait DeviceIdentitySecretStore {
    fn store_private_key(
        &self,
        reference: &DeviceIdentitySecretRef,
        private_key: &str,
    ) -> Result<(), String>;

    fn load_private_key(&self, reference: &DeviceIdentitySecretRef) -> Result<String, String>;
}

#[cfg(all(test, any(target_os = "windows", target_os = "macos")))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceIdentitySecretStoreKind {
    #[cfg(target_os = "windows")]
    WindowsCredentialManager,
    #[cfg(target_os = "macos")]
    MacosKeychain,
}

struct SystemDeviceIdentitySecretStore;

#[cfg(all(test, any(target_os = "windows", target_os = "macos")))]
impl SystemDeviceIdentitySecretStore {
    const fn kind() -> DeviceIdentitySecretStoreKind {
        #[cfg(target_os = "windows")]
        {
            DeviceIdentitySecretStoreKind::WindowsCredentialManager
        }
        #[cfg(target_os = "macos")]
        {
            DeviceIdentitySecretStoreKind::MacosKeychain
        }
    }
}

impl DeviceIdentitySecretStore for SystemDeviceIdentitySecretStore {
    fn store_private_key(
        &self,
        reference: &DeviceIdentitySecretRef,
        private_key: &str,
    ) -> Result<(), String> {
        store_system_private_key(reference, private_key)
    }

    fn load_private_key(&self, reference: &DeviceIdentitySecretRef) -> Result<String, String> {
        load_system_private_key(reference)
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn store_system_private_key(
    reference: &DeviceIdentitySecretRef,
    private_key: &str,
) -> Result<(), String> {
    let entry = keyring::Entry::new(&reference.service, &reference.account)
        .map_err(|error| format!("failed to open system credential store: {error}"))?;
    entry
        .set_password(private_key)
        .map_err(|error| format!("failed to store private key in system credential store: {error}"))
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn load_system_private_key(reference: &DeviceIdentitySecretRef) -> Result<String, String> {
    let entry = keyring::Entry::new(&reference.service, &reference.account)
        .map_err(|error| format!("failed to open system credential store: {error}"))?;
    entry.get_password().map_err(|error| {
        format!("failed to load private key from system credential store: {error}")
    })
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn store_system_private_key(
    _reference: &DeviceIdentitySecretRef,
    _private_key: &str,
) -> Result<(), String> {
    Err("system credential store is only configured for macOS Keychain and Windows Credential Manager".to_string())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn load_system_private_key(_reference: &DeviceIdentitySecretRef) -> Result<String, String> {
    Err("system credential store is only configured for macOS Keychain and Windows Credential Manager".to_string())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

#[derive(Debug, Clone)]
pub struct ControlClient {
    server_url: String,
    agent: ureq::Agent,
}

impl ControlClient {
    #[must_use]
    pub fn new(server_url: String) -> Self {
        Self {
            server_url: server_url.trim_end_matches('/').to_string(),
            agent: ureq::Agent::new_with_defaults(),
        }
    }

    pub fn start_email_login(&self, email: &str) -> Result<EmailLoginStartResponse, String> {
        let request = EmailLoginStartRequest {
            email: email.to_string(),
        };
        post_json(
            &self.agent,
            &format!("{}/v1/auth/email/start", self.server_url),
            None,
            &request,
        )
    }

    pub fn verify_email_login(&self, email: &str, code: &str) -> Result<LoginResponse, String> {
        let request = EmailLoginVerifyRequest {
            email: email.to_string(),
            code: code.to_string(),
        };
        post_json(
            &self.agent,
            &format!("{}/v1/auth/email/verify", self.server_url),
            None,
            &request,
        )
    }

    pub fn register_device(
        &self,
        access_token: &str,
        request: &RegisterDeviceRequest,
    ) -> Result<RegisterDeviceResponse, String> {
        post_json(
            &self.agent,
            &format!("{}/v1/devices/register", self.server_url),
            Some(access_token),
            request,
        )
    }

    pub fn heartbeat(
        &self,
        access_token: &str,
        device_id: &str,
        request: &HeartbeatRequest,
    ) -> Result<HeartbeatResponse, String> {
        post_json(
            &self.agent,
            &format!("{}/v1/devices/{}/heartbeat", self.server_url, device_id),
            Some(access_token),
            request,
        )
    }

    pub fn list_devices(&self, access_token: &str) -> Result<Vec<DeviceWithPresence>, String> {
        get_json(
            &self.agent,
            &format!("{}/v1/devices", self.server_url),
            access_token,
        )
    }

    pub fn list_profiles(&self, access_token: &str) -> Result<Vec<DeviceProfile>, String> {
        get_json(
            &self.agent,
            &format!("{}/v1/profiles", self.server_url),
            access_token,
        )
    }

    pub fn upsert_profile(
        &self,
        access_token: &str,
        request: &UpsertProfileRequest,
    ) -> Result<DeviceProfile, String> {
        put_json(
            &self.agent,
            &format!("{}/v1/profiles", self.server_url),
            access_token,
            request,
        )
    }

    pub fn check_release(
        &self,
        request: &ReleaseCheckRequest,
    ) -> Result<ReleaseCheckResponse, String> {
        get_json_without_auth(&self.agent, &release_check_url(&self.server_url, request))
    }
}

#[derive(Debug, Serialize)]
struct EmailLoginStartRequest {
    email: String,
}

#[derive(Debug, Deserialize)]
pub struct EmailLoginStartResponse {
    pub email: String,
    pub expires_at: u64,
}

#[derive(Debug, Serialize)]
struct EmailLoginVerifyRequest {
    email: String,
    code: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginResponse {
    pub user_id: String,
    pub access_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseCheckRequest {
    pub platform: String,
    pub version: String,
    pub channel: String,
    pub device_id: Option<String>,
}

impl ReleaseCheckRequest {
    fn current(
        device_id: Option<String>,
        platform: Option<String>,
        version: Option<String>,
        channel: Option<String>,
    ) -> Self {
        Self {
            platform: platform.unwrap_or_else(default_release_platform),
            version: version.unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
            channel: channel.unwrap_or_else(|| "stable".to_string()),
            device_id,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ReleaseCheckResponse {
    pub platform: String,
    pub channel: String,
    pub latest_version: String,
    pub min_supported_version: String,
    pub update_available: bool,
    pub force_update: bool,
    pub rollout_percent: u8,
    pub rollout_bucket: Option<u8>,
    pub rollout_eligible: bool,
    pub auto_update_action: String,
    pub download_url: String,
    pub installer_sha256: String,
    pub signature_url: String,
}

#[derive(Debug, Serialize)]
pub struct RegisterDeviceRequest {
    pub name: String,
    pub os_type: String,
    pub os_version: String,
    pub app_version: String,
    pub public_key: String,
}

#[derive(Debug, Deserialize)]
pub struct RegisterDeviceResponse {
    pub device_id: String,
}

#[derive(Debug, Serialize)]
pub struct HeartbeatRequest {
    pub lan_ips: Vec<String>,
    pub listen_port: u16,
    pub nat_type: String,
}

#[derive(Debug, Deserialize)]
pub struct HeartbeatResponse {
    pub online: bool,
    pub last_seen_at: u64,
}

#[derive(Debug, Deserialize)]
pub struct DeviceWithPresence {
    pub device: Device,
    pub presence: Option<Presence>,
}

#[derive(Debug, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub os_type: String,
    pub os_version: String,
    pub app_version: String,
    #[serde(default)]
    pub public_key: String,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct Presence {
    pub online: bool,
    pub lan_ips: Vec<String>,
    pub public_ip: String,
    pub listen_port: u16,
    pub nat_type: String,
    pub last_seen_at: u64,
}

#[derive(Debug, Deserialize)]
pub struct DeviceProfile {
    pub id: String,
    pub source_device_id: String,
    pub target_device_id: String,
    pub config: Value,
    pub version: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone)]
pub struct CompiledDeviceProfile {
    pub id: String,
    pub source_device_id: String,
    pub target_device_id: String,
    pub version: u64,
    pub profile: CompiledProfile,
}

#[derive(Debug, Serialize)]
pub struct UpsertProfileRequest {
    pub source_device_id: String,
    pub target_device_id: String,
    pub config: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionCandidateKind {
    MdnsLan,
    BackendLan,
    NatTraversal,
    Relay,
}

impl ConnectionCandidateKind {
    #[must_use]
    pub const fn priority(self) -> u16 {
        match self {
            Self::MdnsLan => 400,
            Self::BackendLan => 300,
            Self::NatTraversal => 200,
            Self::Relay => 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionCandidate {
    pub device_id: String,
    pub kind: ConnectionCandidateKind,
    pub address: String,
    pub priority: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanDiscoveryEndpoint {
    pub device_id: String,
    pub address: SocketAddr,
}

#[derive(Debug)]
pub struct DiscoveredLanDevice<'a> {
    pub device: &'a Device,
    pub candidates: Vec<ConnectionCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectConnectionAttempt {
    pub candidate: ConnectionCandidate,
    pub address: SocketAddr,
    pub failed_attempts: Vec<DirectConnectionFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectConnectionFailure {
    pub candidate: ConnectionCandidate,
    pub error: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectLanReconnectReason {
    InitialConnection,
    ConnectionLost,
    LocalNetworkChanged,
    RemoteCandidatesChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectLanReconnectOutcome {
    pub reason: DirectLanReconnectReason,
    pub attempt: DirectConnectionAttempt,
    pub reconnect_count: u64,
}

#[derive(Debug, Clone, Default)]
pub struct DirectLanReconnectState {
    current: Option<DirectConnectionAttempt>,
    local_lan_ips: Vec<IpAddr>,
    direct_candidates: Vec<ConnectionCandidate>,
    reconnect_count: u64,
}

impl DirectLanReconnectState {
    pub fn refresh<F>(
        &mut self,
        local_lan_ips: &[IpAddr],
        candidates: &[ConnectionCandidate],
        current_connection_healthy: bool,
        connector: F,
    ) -> Result<Option<DirectLanReconnectOutcome>, String>
    where
        F: FnMut(&ConnectionCandidate, SocketAddr) -> Result<(), String>,
    {
        let local_lan_ips = normalize_lan_ip_snapshot(local_lan_ips);
        let direct_candidates = normalize_direct_candidate_snapshot(candidates);
        let Some(reason) = self.reconnect_reason(
            current_connection_healthy,
            &local_lan_ips,
            &direct_candidates,
        ) else {
            return Ok(None);
        };

        let had_current_connection = self.current.is_some();
        match try_direct_lan_connection(&direct_candidates, connector) {
            Ok(attempt) => {
                if had_current_connection {
                    self.reconnect_count = self.reconnect_count.saturating_add(1);
                }
                self.local_lan_ips = local_lan_ips;
                self.direct_candidates = direct_candidates;
                self.current = Some(attempt.clone());
                Ok(Some(DirectLanReconnectOutcome {
                    reason,
                    attempt,
                    reconnect_count: self.reconnect_count,
                }))
            }
            Err(error) => {
                if !current_connection_healthy {
                    self.current = None;
                }
                Err(error)
            }
        }
    }

    fn reconnect_reason(
        &self,
        current_connection_healthy: bool,
        local_lan_ips: &[IpAddr],
        direct_candidates: &[ConnectionCandidate],
    ) -> Option<DirectLanReconnectReason> {
        if self.current.is_none() {
            return Some(DirectLanReconnectReason::InitialConnection);
        }
        if !current_connection_healthy {
            return Some(DirectLanReconnectReason::ConnectionLost);
        }
        if self.local_lan_ips != local_lan_ips {
            return Some(DirectLanReconnectReason::LocalNetworkChanged);
        }
        if self.direct_candidates != direct_candidates {
            return Some(DirectLanReconnectReason::RemoteCandidatesChanged);
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NatTraversalCandidate {
    pub device_id: String,
    pub address: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayCandidate {
    pub device_id: String,
    pub relay_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPeerIdentity {
    pub device_id: String,
    pub public_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedTargetConnectionCandidates {
    pub identity: VerifiedPeerIdentity,
    pub candidates: Vec<ConnectionCandidate>,
}

fn verified_target_connection_candidates(
    target_device_id: &str,
    mdns_endpoints: &[LanDiscoveryEndpoint],
    devices: &[DeviceWithPresence],
    nat_candidates: &[NatTraversalCandidate],
    relay_candidates: &[RelayCandidate],
) -> Result<VerifiedTargetConnectionCandidates, String> {
    let identity = verify_target_device_identity(target_device_id, devices)?;
    let candidates = collect_connection_candidates(
        target_device_id,
        mdns_endpoints,
        devices,
        nat_candidates,
        relay_candidates,
    );
    Ok(VerifiedTargetConnectionCandidates {
        identity,
        candidates,
    })
}

fn verify_target_device_identity(
    target_device_id: &str,
    devices: &[DeviceWithPresence],
) -> Result<VerifiedPeerIdentity, String> {
    let target = devices
        .iter()
        .find(|item| item.device.id == target_device_id)
        .ok_or_else(|| format!("target device '{target_device_id}' was not found"))?;
    if target.device.disabled {
        return Err(format!("target device '{target_device_id}' is disabled"));
    }
    if !is_ed25519_public_key(&target.device.public_key) {
        return Err(format!(
            "invalid target device public key for '{target_device_id}'"
        ));
    }
    Ok(VerifiedPeerIdentity {
        device_id: target.device.id.clone(),
        public_key: target.device.public_key.clone(),
    })
}

fn is_ed25519_public_key(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("ed25519:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[must_use]
pub fn collect_connection_candidates(
    target_device_id: &str,
    mdns_endpoints: &[LanDiscoveryEndpoint],
    devices: &[DeviceWithPresence],
    nat_candidates: &[NatTraversalCandidate],
    relay_candidates: &[RelayCandidate],
) -> Vec<ConnectionCandidate> {
    let mut candidates = Vec::new();

    for endpoint in mdns_endpoints
        .iter()
        .filter(|endpoint| endpoint.device_id == target_device_id)
    {
        push_candidate(
            &mut candidates,
            &endpoint.device_id,
            ConnectionCandidateKind::MdnsLan,
            endpoint.address.to_string(),
        );
    }

    for item in devices
        .iter()
        .filter(|item| item.device.id == target_device_id)
    {
        let Some(presence) = item.presence.as_ref().filter(|presence| presence.online) else {
            continue;
        };
        for lan_ip in &presence.lan_ips {
            let Ok(ip) = lan_ip.parse::<IpAddr>() else {
                continue;
            };
            push_candidate(
                &mut candidates,
                &item.device.id,
                ConnectionCandidateKind::BackendLan,
                SocketAddr::new(ip, presence.listen_port).to_string(),
            );
        }
    }

    for candidate in nat_candidates
        .iter()
        .filter(|candidate| candidate.device_id == target_device_id)
    {
        push_candidate(
            &mut candidates,
            &candidate.device_id,
            ConnectionCandidateKind::NatTraversal,
            candidate.address.to_string(),
        );
    }

    for candidate in relay_candidates
        .iter()
        .filter(|candidate| candidate.device_id == target_device_id)
        .filter(|candidate| !candidate.relay_url.trim().is_empty())
    {
        push_candidate(
            &mut candidates,
            &candidate.device_id,
            ConnectionCandidateKind::Relay,
            candidate.relay_url.clone(),
        );
    }

    sort_connection_candidates(&mut candidates);
    dedupe_candidates_by_address(candidates)
}

#[must_use]
pub fn discover_same_account_lan_devices<'a>(
    devices: &'a [DeviceWithPresence],
    mdns_endpoints: &[LanDiscoveryEndpoint],
) -> Vec<DiscoveredLanDevice<'a>> {
    devices
        .iter()
        .filter_map(|item| {
            let candidates =
                collect_connection_candidates(&item.device.id, mdns_endpoints, devices, &[], &[]);
            if candidates.is_empty() {
                None
            } else {
                Some(DiscoveredLanDevice {
                    device: &item.device,
                    candidates,
                })
            }
        })
        .collect()
}

pub fn try_direct_lan_connection<F>(
    candidates: &[ConnectionCandidate],
    mut connector: F,
) -> Result<DirectConnectionAttempt, String>
where
    F: FnMut(&ConnectionCandidate, SocketAddr) -> Result<(), String>,
{
    let mut failed_attempts = Vec::new();
    let mut saw_direct_candidate = false;

    for candidate in candidates.iter().filter(|candidate| {
        matches!(
            candidate.kind,
            ConnectionCandidateKind::MdnsLan | ConnectionCandidateKind::BackendLan
        )
    }) {
        saw_direct_candidate = true;
        let address = match candidate.address.parse::<SocketAddr>() {
            Ok(address) => address,
            Err(error) => {
                failed_attempts.push(DirectConnectionFailure {
                    candidate: candidate.clone(),
                    error: format!("invalid LAN socket address: {error}"),
                });
                continue;
            }
        };

        match connector(candidate, address) {
            Ok(()) => {
                return Ok(DirectConnectionAttempt {
                    candidate: candidate.clone(),
                    address,
                    failed_attempts,
                });
            }
            Err(error) => failed_attempts.push(DirectConnectionFailure {
                candidate: candidate.clone(),
                error,
            }),
        }
    }

    if saw_direct_candidate {
        Err(format_direct_connection_failures(&failed_attempts))
    } else {
        Err("no direct LAN candidates available for target device".to_string())
    }
}

pub fn refresh_target_direct_lan_connection(
    config: ClientConfig,
    target_device_id: &str,
    reconnect_state: &mut DirectLanReconnectState,
    current_connection_healthy: bool,
) -> Result<Option<DirectLanReconnectOutcome>, String> {
    let client = ControlClient::new(config.server_url.clone());
    let login = login_with_email_code(&client, &config)?;
    let devices = client.list_devices(&login.access_token)?;

    let mdns_endpoints = discover_mdns_lan_endpoints(MDNS_DISCOVERY_TIMEOUT).unwrap_or_default();
    let verified = verified_target_connection_candidates(
        target_device_id,
        &mdns_endpoints,
        &devices,
        &[],
        &[],
    )?;
    let local_lan_ips = discover_local_lan_ips();
    reconnect_state.refresh(
        &local_lan_ips,
        &verified.candidates,
        current_connection_healthy,
        |_candidate, address| QuicEventSender::connect(address).map(|_| ()),
    )
}

#[allow(dead_code)]
pub fn resolve_target_direct_lan_connection(
    config: ClientConfig,
    target_device_id: &str,
) -> Result<DirectConnectionAttempt, String> {
    let mut reconnect_state = DirectLanReconnectState::default();
    refresh_target_direct_lan_connection(config, target_device_id, &mut reconnect_state, false)?
        .map(|outcome| outcome.attempt)
        .ok_or_else(|| "direct LAN connection refresh did not select a candidate".to_string())
}

fn format_direct_connection_failures(failed_attempts: &[DirectConnectionFailure]) -> String {
    let mut message = String::from("all direct LAN candidates failed");
    for failure in failed_attempts {
        let _ = write!(
            message,
            "; {:?} {}: {}",
            failure.candidate.kind, failure.candidate.address, failure.error
        );
    }
    message
}

#[must_use]
pub fn discover_local_lan_ips() -> Vec<IpAddr> {
    let mut ips = Vec::new();
    for target in [
        SocketAddr::from((Ipv4Addr::new(224, 0, 0, 251), 5353)),
        SocketAddr::from((Ipv4Addr::new(192, 168, 1, 1), 9)),
        SocketAddr::from((Ipv4Addr::new(10, 0, 0, 1), 9)),
        SocketAddr::from((Ipv4Addr::new(172, 16, 0, 1), 9)),
        SocketAddr::from((Ipv4Addr::new(8, 8, 8, 8), 53)),
        SocketAddr::from((
            Ipv6Addr::new(0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888),
            53,
        )),
    ] {
        if let Some(ip) = local_ip_for_route(target) {
            ips.push(ip);
        }
    }
    normalize_lan_ip_snapshot(&ips)
}

fn local_ip_for_route(target: SocketAddr) -> Option<IpAddr> {
    let bind = if target.is_ipv4() {
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))
    } else {
        SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0))
    };
    let socket = UdpSocket::bind(bind).ok()?;
    socket.connect(target).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

fn normalize_lan_ip_snapshot(ips: &[IpAddr]) -> Vec<IpAddr> {
    let mut ips = ips
        .iter()
        .copied()
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
        .collect::<Vec<_>>();
    ips.sort_unstable();
    ips.dedup();
    ips
}

fn normalize_direct_candidate_snapshot(
    candidates: &[ConnectionCandidate],
) -> Vec<ConnectionCandidate> {
    let mut direct_candidates = candidates
        .iter()
        .filter(|candidate| {
            matches!(
                candidate.kind,
                ConnectionCandidateKind::MdnsLan | ConnectionCandidateKind::BackendLan
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    sort_connection_candidates(&mut direct_candidates);
    dedupe_candidates_by_address(direct_candidates)
}

fn push_candidate(
    candidates: &mut Vec<ConnectionCandidate>,
    device_id: &str,
    kind: ConnectionCandidateKind,
    address: String,
) {
    candidates.push(ConnectionCandidate {
        device_id: device_id.to_string(),
        kind,
        address,
        priority: kind.priority(),
    });
}

fn sort_connection_candidates(candidates: &mut [ConnectionCandidate]) {
    candidates.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.address.cmp(&right.address))
    });
}

fn dedupe_candidates_by_address(candidates: Vec<ConnectionCandidate>) -> Vec<ConnectionCandidate> {
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.address.clone()))
        .collect()
}

fn login_with_email_code(
    client: &ControlClient,
    config: &ClientConfig,
) -> Result<LoginResponse, String> {
    let challenge = client.start_email_login(&config.email)?;
    let code = email_login_code(config)
        .ok_or_else(|| missing_email_login_code_error(config, &challenge))?;
    client.verify_email_login(&config.email, &code)
}

fn email_login_code(config: &ClientConfig) -> Option<String> {
    config
        .email_login_code
        .as_deref()
        .map(str::trim)
        .filter(|code| !code.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            std::env::var("KMSYNC_EMAIL_LOGIN_CODE")
                .ok()
                .map(|code| code.trim().to_string())
                .filter(|code| !code.is_empty())
        })
}

fn missing_email_login_code_error(
    config: &ClientConfig,
    challenge: &EmailLoginStartResponse,
) -> String {
    let email = if challenge.email.is_empty() {
        &config.email
    } else {
        &challenge.email
    };
    format!(
        "email login challenge started for {} and expires_at={}; retrieve the code delivered by /v1/auth/email/start, then set email_login_code in the daemon config or KMSYNC_EMAIL_LOGIN_CODE",
        email,
        challenge.expires_at
    )
}

fn release_check_url(server_url: &str, request: &ReleaseCheckRequest) -> String {
    let mut url = format!(
        "{}/v1/releases/check?platform={}&version={}&channel={}",
        server_url.trim_end_matches('/'),
        encode_query_component(&request.platform),
        encode_query_component(&request.version),
        encode_query_component(&request.channel)
    );
    if let Some(device_id) = request
        .device_id
        .as_deref()
        .map(str::trim)
        .filter(|device_id| !device_id.is_empty())
    {
        let _ = write!(url, "&device_id={}", encode_query_component(device_id));
    }
    url
}

fn encode_query_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn default_release_platform() -> String {
    match std::env::consts::OS {
        "macos" => "macos".to_string(),
        "windows" => "windows".to_string(),
        "linux" => "linux".to_string(),
        other => other.to_string(),
    }
}

pub fn print_update_check(
    config: ClientConfig,
    device_id: Option<String>,
    platform: Option<String>,
    version: Option<String>,
    channel: Option<String>,
) -> Result<(), String> {
    let rollout_device_id = match device_id {
        Some(device_id) if !device_id.trim().is_empty() => Some(device_id),
        _ => Some(DeviceIdentity::load_or_generate(&config.identity_path)?.public_key),
    };
    let request = ReleaseCheckRequest::current(rollout_device_id, platform, version, channel);
    let client = ControlClient::new(config.server_url.clone());
    let release = client.check_release(&request)?;
    print!("{}", render_release_check_report(&release));
    Ok(())
}

fn render_release_check_report(release: &ReleaseCheckResponse) -> String {
    let mut report = String::from("release check\n");
    let _ = writeln!(
        report,
        "platform={} channel={} latest_version={} min_supported_version={}",
        release.platform, release.channel, release.latest_version, release.min_supported_version
    );
    let _ = writeln!(
        report,
        "update_available={} force_update={} auto_update_action={}",
        release.update_available, release.force_update, release.auto_update_action
    );
    let rollout_bucket = release
        .rollout_bucket
        .map_or_else(|| "none".to_string(), |bucket| bucket.to_string());
    let _ = writeln!(
        report,
        "rollout_percent={} rollout_bucket={} rollout_eligible={}",
        release.rollout_percent, rollout_bucket, release.rollout_eligible
    );
    let _ = writeln!(report, "download_url={}", release.download_url);
    let _ = writeln!(report, "installer_sha256={}", release.installer_sha256);
    let _ = writeln!(report, "signature_url={}", release.signature_url);
    report.push_str("privacy=release_metadata_only\n");
    report
}

pub fn run_heartbeat_loop(config: ClientConfig) -> Result<(), String> {
    let client = ControlClient::new(config.server_url.clone());
    let login = login_with_email_code(&client, &config)?;
    let identity = DeviceIdentity::load_or_generate(&config.identity_path)?;
    let device = client.register_device(
        &login.access_token,
        &build_register_device_request(&config, &identity),
    )?;

    println!(
        "registered device {} for {}, heartbeat every {}s",
        device.device_id, config.email, config.heartbeat_interval_seconds
    );

    loop {
        let request = HeartbeatRequest {
            lan_ips: discover_lan_ips(),
            listen_port: config.listen_port,
            nat_type: "unknown".to_string(),
        };
        let response = client.heartbeat(&login.access_token, &device.device_id, &request)?;
        println!(
            "heartbeat ok: online={} last_seen_at={}",
            response.online, response.last_seen_at
        );
        thread::sleep(Duration::from_secs(
            config.heartbeat_interval_seconds.max(5),
        ));
    }
}

fn build_register_device_request(
    config: &ClientConfig,
    identity: &DeviceIdentity,
) -> RegisterDeviceRequest {
    RegisterDeviceRequest {
        name: config.device_name.clone(),
        os_type: std::env::consts::OS.to_string(),
        os_version: "unknown".to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        public_key: identity.public_key.clone(),
    }
}

pub fn print_devices(config: ClientConfig) -> Result<(), String> {
    let client = ControlClient::new(config.server_url.clone());
    let login = login_with_email_code(&client, &config)?;
    let devices = client.list_devices(&login.access_token)?;
    let mdns_endpoints = discover_mdns_lan_endpoints(MDNS_DISCOVERY_TIMEOUT).unwrap_or_default();
    let discovered_lan_devices = discover_same_account_lan_devices(&devices, &mdns_endpoints);
    println!("user_id: {}", login.user_id);
    for item in &devices {
        println!(
            "{} | {} | {} {} | app {}",
            item.device.id,
            item.device.name,
            item.device.os_type,
            item.device.os_version,
            item.device.app_version
        );
        if let Some(presence) = &item.presence {
            println!(
                "  online={} lan={:?} public={}:{} nat={} last_seen={}",
                presence.online,
                presence.lan_ips,
                presence.public_ip,
                presence.listen_port,
                presence.nat_type,
                presence.last_seen_at
            );
        } else {
            println!("  offline");
        }
        if let Some(discovered) = discovered_lan_devices
            .iter()
            .find(|discovered| discovered.device.id == item.device.id)
        {
            for candidate in &discovered.candidates {
                println!(
                    "  candidate {:?} priority={} address={}",
                    candidate.kind, candidate.priority, candidate.address
                );
            }
        }
    }
    Ok(())
}

pub fn print_connection_diagnostics(
    config: ClientConfig,
    target_device_id: &str,
) -> Result<(), String> {
    let client = ControlClient::new(config.server_url.clone());
    let login = login_with_email_code(&client, &config)?;
    let devices = client.list_devices(&login.access_token)?;
    let mdns_endpoints = discover_mdns_lan_endpoints(MDNS_DISCOVERY_TIMEOUT).unwrap_or_default();
    println!(
        "{}",
        render_connection_diagnostic_report(target_device_id, &mdns_endpoints, &devices, &[], &[])
    );
    Ok(())
}

#[must_use]
pub fn render_connection_diagnostic_report(
    target_device_id: &str,
    mdns_endpoints: &[LanDiscoveryEndpoint],
    devices: &[DeviceWithPresence],
    nat_candidates: &[NatTraversalCandidate],
    relay_candidates: &[RelayCandidate],
) -> String {
    let target = devices
        .iter()
        .find(|item| item.device.id == target_device_id);
    let account_mdns_endpoints = if target.is_some() {
        mdns_endpoints
    } else {
        &[]
    };
    let candidates = collect_connection_candidates(
        target_device_id,
        account_mdns_endpoints,
        devices,
        nat_candidates,
        relay_candidates,
    );

    let mut report = String::from("connection diagnostic report\n");
    let _ = writeln!(report, "target_device_id={target_device_id}");
    match target {
        Some(item) => {
            let _ = writeln!(
                report,
                "device={} os={} {} app={}",
                item.device.name,
                item.device.os_type,
                item.device.os_version,
                item.device.app_version
            );
            if let Some(presence) = &item.presence {
                let _ = writeln!(
                    report,
                    "presence online={} lan_count={} public={}:{} nat={} last_seen={}",
                    presence.online,
                    presence.lan_ips.len(),
                    presence.public_ip,
                    presence.listen_port,
                    presence.nat_type,
                    presence.last_seen_at
                );
            } else {
                report.push_str("presence offline\n");
            }
        }
        None => {
            report.push_str("device=not_found\n");
        }
    }

    let _ = writeln!(report, "candidate_count={}", candidates.len());
    if candidates.is_empty() {
        report.push_str("recommendation=refresh_presence_or_check_network\n");
    } else {
        report.push_str("candidates:\n");
        for candidate in candidates {
            let _ = writeln!(
                report,
                "  {:?} priority={} address={}",
                candidate.kind,
                candidate.priority,
                sanitize_connection_endpoint(&candidate.address)
            );
        }
        report.push_str("recommendation=try_candidates_in_priority_order\n");
    }
    report.push_str("privacy=connection_metadata_only\n");
    report
}

fn sanitize_connection_endpoint(address: &str) -> String {
    let mut sanitized = address
        .split(['?', '#'])
        .next()
        .unwrap_or(address)
        .to_string();

    if let Some(scheme_end) = sanitized.find("://") {
        let authority_start = scheme_end + 3;
        if let Some(at) = sanitized[authority_start..].find('@') {
            let authority_end = authority_start + at + 1;
            sanitized.replace_range(authority_start..authority_end, "<redacted>@");
        }
    }

    sanitized
}

pub fn discover_mdns_lan_endpoints(timeout: Duration) -> Result<Vec<LanDiscoveryEndpoint>, String> {
    let socket =
        UdpSocket::bind("0.0.0.0:0").map_err(|error| format!("mDNS bind failed: {error}"))?;
    socket
        .set_read_timeout(Some(timeout))
        .map_err(|error| format!("mDNS read timeout failed: {error}"))?;
    socket
        .set_write_timeout(Some(timeout))
        .map_err(|error| format!("mDNS write timeout failed: {error}"))?;

    let query = build_mdns_ptr_query();
    socket
        .send_to(&query, MDNS_MULTICAST_ENDPOINT)
        .map_err(|error| format!("mDNS query failed: {error}"))?;

    let deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now);
    let mut endpoints = Vec::new();
    let mut packet = [0_u8; 1500];

    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        if remaining.is_zero() {
            break;
        }
        socket
            .set_read_timeout(Some(remaining))
            .map_err(|error| format!("mDNS read timeout failed: {error}"))?;

        match socket.recv_from(&mut packet) {
            Ok((len, _source)) => endpoints.extend(parse_mdns_lan_endpoints(&packet[..len])),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(error) => return Err(format!("mDNS receive failed: {error}")),
        }
    }

    dedupe_lan_endpoints(endpoints)
}

fn build_mdns_ptr_query() -> Vec<u8> {
    let mut packet = Vec::with_capacity(64);
    packet.extend_from_slice(&0_u16.to_be_bytes());
    packet.extend_from_slice(&0_u16.to_be_bytes());
    packet.extend_from_slice(&1_u16.to_be_bytes());
    packet.extend_from_slice(&0_u16.to_be_bytes());
    packet.extend_from_slice(&0_u16.to_be_bytes());
    packet.extend_from_slice(&0_u16.to_be_bytes());
    append_dns_name(&mut packet, MDNS_SERVICE_TYPE);
    packet.extend_from_slice(&DNS_TYPE_PTR.to_be_bytes());
    packet.extend_from_slice(&(DNS_CLASS_IN | DNS_CLASS_QU).to_be_bytes());
    packet
}

fn parse_mdns_lan_endpoints(packet: &[u8]) -> Vec<LanDiscoveryEndpoint> {
    let Some(header) = DnsHeader::parse(packet) else {
        return Vec::new();
    };

    let mut offset = 12;
    for _ in 0..header.question_count {
        if !skip_dns_question(packet, &mut offset) {
            return Vec::new();
        }
    }

    let mut ptr_instances = HashSet::new();
    let mut srv_records: HashMap<String, (String, u16)> = HashMap::new();
    let mut txt_device_ids = HashMap::new();
    let mut addresses: HashMap<String, Vec<IpAddr>> = HashMap::new();

    for _ in 0..header.record_count() {
        let Some(record) = DnsRecord::parse(packet, &mut offset) else {
            break;
        };
        let record_name = normalize_dns_name(&record.name);
        match record.record_type {
            DNS_TYPE_PTR if record_name == normalize_dns_name(MDNS_SERVICE_TYPE) => {
                let mut data_offset = record.data_start;
                if let Some(instance) = read_dns_name(packet, &mut data_offset) {
                    ptr_instances.insert(normalize_dns_name(&instance));
                }
            }
            DNS_TYPE_SRV => {
                if record.data_len >= 7 {
                    let port = read_u16(packet, record.data_start + 4).unwrap_or_default();
                    let mut data_offset = record.data_start + 6;
                    if let Some(target) = read_dns_name(packet, &mut data_offset) {
                        srv_records.insert(record_name, (normalize_dns_name(&target), port));
                    }
                }
            }
            DNS_TYPE_TXT => {
                if let Some(device_id) = read_txt_device_id(&packet[record.data_range()]) {
                    txt_device_ids.insert(record_name, device_id);
                }
            }
            DNS_TYPE_A if record.data_len == 4 => {
                addresses
                    .entry(record_name)
                    .or_default()
                    .push(IpAddr::V4(Ipv4Addr::new(
                        packet[record.data_start],
                        packet[record.data_start + 1],
                        packet[record.data_start + 2],
                        packet[record.data_start + 3],
                    )));
            }
            DNS_TYPE_AAAA if record.data_len == 16 => {
                let mut octets = [0_u8; 16];
                octets.copy_from_slice(&packet[record.data_range()]);
                addresses
                    .entry(record_name)
                    .or_default()
                    .push(IpAddr::V6(Ipv6Addr::from(octets)));
            }
            _ => {}
        }
    }

    let mut endpoints = Vec::new();
    for instance in ptr_instances {
        let Some((target, port)) = srv_records.get(&instance) else {
            continue;
        };
        let Some(target_addresses) = addresses.get(target) else {
            continue;
        };
        let device_id = txt_device_ids
            .get(&instance)
            .cloned()
            .unwrap_or_else(|| service_instance_device_id(&instance));
        if device_id.is_empty() {
            continue;
        }
        for address in target_addresses {
            endpoints.push(LanDiscoveryEndpoint {
                device_id: device_id.clone(),
                address: SocketAddr::new(*address, *port),
            });
        }
    }

    dedupe_lan_endpoints(endpoints).unwrap_or_default()
}

#[derive(Debug, Clone, Copy)]
struct DnsHeader {
    question_count: u16,
    answer_count: u16,
    authority_count: u16,
    additional_count: u16,
}

impl DnsHeader {
    fn parse(packet: &[u8]) -> Option<Self> {
        Some(Self {
            question_count: read_u16(packet, 4)?,
            answer_count: read_u16(packet, 6)?,
            authority_count: read_u16(packet, 8)?,
            additional_count: read_u16(packet, 10)?,
        })
    }

    const fn record_count(self) -> u16 {
        self.answer_count
            .saturating_add(self.authority_count)
            .saturating_add(self.additional_count)
    }
}

#[derive(Debug)]
struct DnsRecord {
    name: String,
    record_type: u16,
    data_start: usize,
    data_len: usize,
}

impl DnsRecord {
    fn parse(packet: &[u8], offset: &mut usize) -> Option<Self> {
        let name = read_dns_name(packet, offset)?;
        let record_type = read_u16(packet, *offset)?;
        let _class = read_u16(packet, *offset + 2)? & !DNS_CLASS_QU;
        let data_len = usize::from(read_u16(packet, *offset + 8)?);
        let data_start = offset.checked_add(10)?;
        let data_end = data_start.checked_add(data_len)?;
        if data_end > packet.len() {
            return None;
        }
        *offset = data_end;
        Some(Self {
            name,
            record_type,
            data_start,
            data_len,
        })
    }

    fn data_range(&self) -> std::ops::Range<usize> {
        self.data_start..self.data_start + self.data_len
    }
}

fn skip_dns_question(packet: &[u8], offset: &mut usize) -> bool {
    read_dns_name(packet, offset).is_some()
        && offset
            .checked_add(4)
            .filter(|end| *end <= packet.len())
            .is_some_and(|end| {
                *offset = end;
                true
            })
}

fn read_dns_name(packet: &[u8], offset: &mut usize) -> Option<String> {
    let mut labels = Vec::new();
    let mut cursor = *offset;
    let mut jumped = false;
    let mut jumps = 0_u8;

    loop {
        let length = *packet.get(cursor)?;
        if length & 0xC0 == 0xC0 {
            let next = *packet.get(cursor + 1)?;
            let pointer = usize::from((u16::from(length & 0x3F) << 8) | u16::from(next));
            if pointer >= packet.len() || jumps > 12 {
                return None;
            }
            if !jumped {
                *offset = cursor + 2;
            }
            cursor = pointer;
            jumped = true;
            jumps = jumps.saturating_add(1);
            continue;
        }
        if length & 0xC0 != 0 {
            return None;
        }
        if length == 0 {
            if !jumped {
                *offset = cursor + 1;
            }
            break;
        }

        cursor += 1;
        let end = cursor.checked_add(usize::from(length))?;
        let label = std::str::from_utf8(packet.get(cursor..end)?).ok()?;
        labels.push(label.to_string());
        cursor = end;
    }

    Some(labels.join("."))
}

fn append_dns_name(packet: &mut Vec<u8>, name: &str) {
    for label in name.trim_end_matches('.').split('.') {
        packet.push(u8::try_from(label.len()).unwrap_or(0));
        packet.extend_from_slice(label.as_bytes());
    }
    packet.push(0);
}

fn read_txt_device_id(data: &[u8]) -> Option<String> {
    let mut offset = 0;
    while offset < data.len() {
        let length = usize::from(*data.get(offset)?);
        offset += 1;
        let end = offset.checked_add(length)?;
        let entry = std::str::from_utf8(data.get(offset..end)?).ok()?;
        if let Some(device_id) = entry
            .strip_prefix("device_id=")
            .or_else(|| entry.strip_prefix("id="))
            .filter(|value| !value.trim().is_empty())
        {
            return Some(device_id.trim().to_string());
        }
        offset = end;
    }
    None
}

fn service_instance_device_id(instance: &str) -> String {
    instance
        .strip_suffix(&format!(".{}", normalize_dns_name(MDNS_SERVICE_TYPE)))
        .unwrap_or(instance)
        .trim_matches('.')
        .to_string()
}

fn normalize_dns_name(name: &str) -> String {
    name.trim_end_matches('.').to_ascii_lowercase()
}

fn read_u16(packet: &[u8], offset: usize) -> Option<u16> {
    let bytes = packet.get(offset..offset + 2)?;
    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn dedupe_lan_endpoints(
    endpoints: Vec<LanDiscoveryEndpoint>,
) -> Result<Vec<LanDiscoveryEndpoint>, String> {
    let mut seen = HashSet::new();
    Ok(endpoints
        .into_iter()
        .filter(|endpoint| seen.insert((endpoint.device_id.clone(), endpoint.address)))
        .collect())
}

pub fn print_profiles(config: ClientConfig) -> Result<(), String> {
    let client = ControlClient::new(config.server_url.clone());
    let login = login_with_email_code(&client, &config)?;
    let profiles = client.list_profiles(&login.access_token)?;
    println!("user_id: {}", login.user_id);
    for profile in profiles {
        let compiled = compile_device_profile(&profile)?;
        let _hot_path_profile = &compiled.profile;
        println!(
            "{} | {} -> {} | version={} updated_at={} compiled=true",
            compiled.id,
            compiled.source_device_id,
            compiled.target_device_id,
            compiled.version,
            profile.updated_at
        );
    }
    Ok(())
}

pub fn compile_device_profile(profile: &DeviceProfile) -> Result<CompiledDeviceProfile, String> {
    let text = serde_json::to_string(&profile.config).map_err(|error| {
        format!(
            "profile {} config serialization failed: {error}",
            profile.id
        )
    })?;
    let parsed = Profile::from_config_json(&text)
        .map_err(|error| format!("profile {} config is invalid: {error:?}", profile.id))?;
    let compiled = CompiledProfile::compile(&parsed)
        .map_err(|error| format!("profile {} compile failed: {error:?}", profile.id))?;

    Ok(CompiledDeviceProfile {
        id: profile.id.clone(),
        source_device_id: profile.source_device_id.clone(),
        target_device_id: profile.target_device_id.clone(),
        version: profile.version,
        profile: compiled,
    })
}

pub fn upsert_profile_from_file(
    config: ClientConfig,
    source_device_id: String,
    target_device_id: String,
    profile_path: &Path,
) -> Result<(), String> {
    let text = fs::read_to_string(profile_path)
        .map_err(|error| format!("failed to read {}: {error}", profile_path.display()))?;
    let profile_config = serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse {}: {error}", profile_path.display()))?;
    let client = ControlClient::new(config.server_url.clone());
    let login = login_with_email_code(&client, &config)?;
    let profile = client.upsert_profile(
        &login.access_token,
        &UpsertProfileRequest {
            source_device_id,
            target_device_id,
            config: profile_config,
        },
    )?;
    println!(
        "saved profile {} version={} updated_at={}",
        profile.id, profile.version, profile.updated_at
    );
    Ok(())
}

fn post_json<T, R>(
    agent: &ureq::Agent,
    url: &str,
    access_token: Option<&str>,
    request: &T,
) -> Result<R, String>
where
    T: Serialize,
    R: for<'de> Deserialize<'de>,
{
    let mut builder = agent.post(url).header("content-type", "application/json");
    if let Some(token) = access_token {
        builder = builder.header("authorization", &format!("Bearer {token}"));
    }
    let mut response = builder
        .send_json(request)
        .map_err(|error| format!("request failed: {error}"))?;
    response
        .body_mut()
        .read_json()
        .map_err(|error| format!("invalid json response: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kmsync_core::{InputEvent, Key, KeyEvent, KeyState, Modifiers};

    #[test]
    fn mdns_query_requests_kmsync_ptr_records() {
        let query = build_mdns_ptr_query();

        assert_eq!(&query[0..2], &[0, 0]);
        assert_eq!(&query[4..6], &[0, 1]);
        assert!(query
            .windows(b"_kmsync".len())
            .any(|window| window == b"_kmsync"));
        assert!(query.windows(b"_udp".len()).any(|window| window == b"_udp"));
        assert_eq!(&query[query.len() - 4..query.len() - 2], &[0, 12]);
        assert_eq!(&query[query.len() - 2..], &[0x80, 0x01]);
    }

    #[test]
    fn client_config_accepts_optional_email_login_code() {
        let config: ClientConfig = serde_json::from_str(
            r#"{
                "server_url": "http://127.0.0.1:24888",
                "email": "dev@example.com",
                "email_login_code": "ABC12345",
                "device_name": "Development Mac",
                "listen_port": 24800,
                "heartbeat_interval_seconds": 15
            }"#,
        )
        .expect("parse config");

        assert_eq!(config.email_login_code.as_deref(), Some("ABC12345"));
        assert_eq!(
            config.identity_path,
            PathBuf::from("kmsync-device-identity.json")
        );
    }

    #[test]
    fn email_login_code_prefers_config_then_environment() {
        let mut config = ClientConfig {
            server_url: "http://127.0.0.1:24888".to_string(),
            email: "dev@example.com".to_string(),
            email_login_code: Some("CONFIG1".to_string()),
            device_name: "Development Mac".to_string(),
            listen_port: 24_800,
            heartbeat_interval_seconds: 15,
            identity_path: PathBuf::from("identity.json"),
        };

        assert_eq!(email_login_code(&config).as_deref(), Some("CONFIG1"));

        config.email_login_code = None;
        std::env::set_var("KMSYNC_EMAIL_LOGIN_CODE", "ENV12345");
        assert_eq!(email_login_code(&config).as_deref(), Some("ENV12345"));
        std::env::remove_var("KMSYNC_EMAIL_LOGIN_CODE");
    }

    #[test]
    fn email_login_requires_code_after_starting_challenge() {
        let config = ClientConfig {
            server_url: "http://127.0.0.1:24888".to_string(),
            email: "dev@example.com".to_string(),
            email_login_code: None,
            device_name: "Development Mac".to_string(),
            listen_port: 24_800,
            heartbeat_interval_seconds: 15,
            identity_path: PathBuf::from("identity.json"),
        };

        let error = missing_email_login_code_error(
            &config,
            &EmailLoginStartResponse {
                email: "dev@example.com".to_string(),
                expires_at: 123,
            },
        );

        assert!(error.contains("/v1/auth/email/start"));
        assert!(error.contains("expires_at=123"));
        assert!(error.contains("KMSYNC_EMAIL_LOGIN_CODE"));
        assert!(!error.contains("dev-login"));
    }

    #[test]
    fn release_check_url_encodes_device_rollout_identity() {
        let request = ReleaseCheckRequest {
            platform: "windows".to_string(),
            version: "0.1.0".to_string(),
            channel: "stable".to_string(),
            device_id: Some("ed25519:abc+def".to_string()),
        };

        assert_eq!(
            release_check_url("http://127.0.0.1:24888/", &request),
            "http://127.0.0.1:24888/v1/releases/check?platform=windows&version=0.1.0&channel=stable&device_id=ed25519%3Aabc%2Bdef"
        );
    }

    #[test]
    fn release_check_report_shows_auto_update_decision() {
        let report = render_release_check_report(&ReleaseCheckResponse {
            platform: "windows".to_string(),
            channel: "stable".to_string(),
            latest_version: "0.2.0".to_string(),
            min_supported_version: "0.1.0".to_string(),
            update_available: true,
            force_update: false,
            rollout_percent: 25,
            rollout_bucket: Some(13),
            rollout_eligible: true,
            auto_update_action: "download".to_string(),
            download_url: "https://updates.example.invalid/kmsync/windows/0.2.0".to_string(),
            installer_sha256: "a9efb60b6ff3bf8b42fd2506c6f3e8b4c345bd974f4685db963918f6a1a26158"
                .to_string(),
            signature_url:
                "https://updates.example.invalid/kmsync/windows/0.2.0/kmsync-installer.sig"
                    .to_string(),
        });

        assert!(report.contains("auto_update_action=download"));
        assert!(report.contains("rollout_bucket=13"));
        assert!(report.contains("rollout_eligible=true"));
        assert!(report.contains(
            "installer_sha256=a9efb60b6ff3bf8b42fd2506c6f3e8b4c345bd974f4685db963918f6a1a26158"
        ));
        assert!(report.contains("signature_url=https://updates.example.invalid/kmsync/windows/0.2.0/kmsync-installer.sig"));
    }

    #[test]
    fn parses_mdns_response_into_lan_endpoint() {
        let mut packet = mdns_response_header(4);
        append_ptr_record(
            &mut packet,
            "_kmsync._udp.local",
            "windows._kmsync._udp.local",
        );
        append_srv_record(
            &mut packet,
            "windows._kmsync._udp.local",
            24_800,
            "windows.local",
        );
        append_txt_record(
            &mut packet,
            "windows._kmsync._udp.local",
            &["device_id=windows"],
        );
        append_a_record(&mut packet, "windows.local", [192, 168, 1, 20]);

        let endpoints = parse_mdns_lan_endpoints(&packet);

        assert_eq!(
            endpoints,
            vec![LanDiscoveryEndpoint {
                device_id: "windows".to_string(),
                address: "192.168.1.20:24800".parse().expect("socket address"),
            }]
        );
    }

    #[test]
    fn discovers_same_account_lan_devices_from_mdns_and_presence() {
        let devices = vec![
            DeviceWithPresence {
                device: Device {
                    id: "windows".to_string(),
                    name: "Windows".to_string(),
                    os_type: "windows".to_string(),
                    os_version: "11".to_string(),
                    app_version: "0.1.0".to_string(),
                    public_key: valid_test_public_key(),
                    disabled: false,
                },
                presence: Some(Presence {
                    online: true,
                    lan_ips: vec!["10.0.0.5".to_string()],
                    public_ip: "203.0.113.20".to_string(),
                    listen_port: 24_800,
                    nat_type: "unknown".to_string(),
                    last_seen_at: 10,
                }),
            },
            DeviceWithPresence {
                device: Device {
                    id: "mac".to_string(),
                    name: "Mac".to_string(),
                    os_type: "macos".to_string(),
                    os_version: "14".to_string(),
                    app_version: "0.1.0".to_string(),
                    public_key: valid_test_public_key(),
                    disabled: false,
                },
                presence: None,
            },
        ];
        let mdns = vec![
            LanDiscoveryEndpoint {
                device_id: "mac".to_string(),
                address: "10.0.0.9:24800".parse().expect("mac addr"),
            },
            LanDiscoveryEndpoint {
                device_id: "unknown-device".to_string(),
                address: "10.0.0.99:24800".parse().expect("unknown addr"),
            },
        ];

        let discovered = discover_same_account_lan_devices(&devices, &mdns);

        assert_eq!(
            discovered
                .iter()
                .map(|device| device.device.id.as_str())
                .collect::<Vec<_>>(),
            vec!["windows", "mac"]
        );
        assert_eq!(
            discovered[0]
                .candidates
                .iter()
                .map(|candidate| candidate.kind)
                .collect::<Vec<_>>(),
            vec![ConnectionCandidateKind::BackendLan]
        );
        assert_eq!(
            discovered[1]
                .candidates
                .iter()
                .map(|candidate| candidate.kind)
                .collect::<Vec<_>>(),
            vec![ConnectionCandidateKind::MdnsLan]
        );
        assert!(discovered
            .iter()
            .flat_map(|device| &device.candidates)
            .all(|candidate| candidate.device_id != "unknown-device"));
    }

    #[test]
    fn direct_lan_connection_attempts_backend_presence_after_mdns_failure() {
        let candidates = vec![
            ConnectionCandidate {
                device_id: "windows".to_string(),
                kind: ConnectionCandidateKind::MdnsLan,
                address: "10.0.0.9:24800".to_string(),
                priority: ConnectionCandidateKind::MdnsLan.priority(),
            },
            ConnectionCandidate {
                device_id: "windows".to_string(),
                kind: ConnectionCandidateKind::BackendLan,
                address: "10.0.0.5:24800".to_string(),
                priority: ConnectionCandidateKind::BackendLan.priority(),
            },
            ConnectionCandidate {
                device_id: "windows".to_string(),
                kind: ConnectionCandidateKind::Relay,
                address: "relay://relay.kmsync.local:443".to_string(),
                priority: ConnectionCandidateKind::Relay.priority(),
            },
        ];
        let mut attempted = Vec::new();

        let selected = try_direct_lan_connection(&candidates, |candidate, address| {
            attempted.push((candidate.kind, address));
            if candidate.kind == ConnectionCandidateKind::MdnsLan {
                Err("mdns stale".to_string())
            } else {
                Ok(())
            }
        })
        .expect("backend LAN candidate should connect");

        assert_eq!(selected.candidate.kind, ConnectionCandidateKind::BackendLan);
        assert_eq!(
            selected.address,
            "10.0.0.5:24800".parse().expect("backend address")
        );
        assert_eq!(
            attempted,
            vec![
                (
                    ConnectionCandidateKind::MdnsLan,
                    "10.0.0.9:24800".parse().expect("mdns address"),
                ),
                (
                    ConnectionCandidateKind::BackendLan,
                    "10.0.0.5:24800".parse().expect("backend address"),
                ),
            ]
        );
        assert_eq!(selected.failed_attempts.len(), 1);
        assert_eq!(
            selected.failed_attempts[0].candidate.kind,
            ConnectionCandidateKind::MdnsLan
        );
    }

    #[test]
    fn reconnect_state_rediscovers_after_local_lan_ip_change() {
        let mut state = DirectLanReconnectState::default();
        let first_candidates = vec![connection_candidate(
            "windows",
            ConnectionCandidateKind::BackendLan,
            "10.0.0.5:24800",
        )];
        let local_ips = vec!["10.0.0.2".parse().expect("local ip")];

        let initial = state
            .refresh(
                &local_ips,
                &first_candidates,
                false,
                |_candidate, _address| Ok(()),
            )
            .expect("initial connection")
            .expect("initial outcome");

        assert_eq!(initial.reason, DirectLanReconnectReason::InitialConnection);
        assert_eq!(
            initial.attempt.address,
            "10.0.0.5:24800".parse().expect("addr")
        );
        assert_eq!(
            state.current.as_ref().expect("current connection").address,
            "10.0.0.5:24800".parse().expect("addr")
        );
        assert_eq!(state.reconnect_count, 0);

        let unchanged = state
            .refresh(
                &local_ips,
                &first_candidates,
                true,
                |_candidate, _address| panic!("healthy unchanged connection should not redial"),
            )
            .expect("unchanged refresh");
        assert!(unchanged.is_none());

        let changed_local_ips = vec!["192.168.50.10".parse().expect("local ip")];
        let changed_candidates = vec![connection_candidate(
            "windows",
            ConnectionCandidateKind::BackendLan,
            "192.168.50.8:24800",
        )];

        let reconnected = state
            .refresh(
                &changed_local_ips,
                &changed_candidates,
                true,
                |_candidate, _address| Ok(()),
            )
            .expect("reconnect after local ip change")
            .expect("reconnect outcome");

        assert_eq!(
            reconnected.reason,
            DirectLanReconnectReason::LocalNetworkChanged
        );
        assert_eq!(
            reconnected.attempt.address,
            "192.168.50.8:24800".parse().expect("addr")
        );
        assert_eq!(
            state.current.as_ref().expect("current connection").address,
            "192.168.50.8:24800".parse().expect("addr")
        );
        assert_eq!(state.reconnect_count, 1);
    }

    #[test]
    fn reconnect_state_rediscovers_when_remote_candidates_change() {
        let mut state = DirectLanReconnectState::default();
        let local_ips = vec!["10.0.0.2".parse().expect("local ip")];
        let first_candidates = vec![connection_candidate(
            "windows",
            ConnectionCandidateKind::BackendLan,
            "10.0.0.5:24800",
        )];
        state
            .refresh(
                &local_ips,
                &first_candidates,
                false,
                |_candidate, _address| Ok(()),
            )
            .expect("initial connection");

        let woke_device_candidates = vec![connection_candidate(
            "windows",
            ConnectionCandidateKind::MdnsLan,
            "10.0.0.9:24800",
        )];

        let reconnected = state
            .refresh(
                &local_ips,
                &woke_device_candidates,
                true,
                |_candidate, _address| Ok(()),
            )
            .expect("reconnect after target candidate change")
            .expect("reconnect outcome");

        assert_eq!(
            reconnected.reason,
            DirectLanReconnectReason::RemoteCandidatesChanged
        );
        assert_eq!(
            reconnected.attempt.address,
            "10.0.0.9:24800".parse().expect("addr")
        );
        assert_eq!(state.reconnect_count, 1);
    }

    #[test]
    fn reconnect_state_redials_after_connection_loss() {
        let mut state = DirectLanReconnectState::default();
        let local_ips = vec!["10.0.0.2".parse().expect("local ip")];
        let candidates = vec![connection_candidate(
            "windows",
            ConnectionCandidateKind::BackendLan,
            "10.0.0.5:24800",
        )];
        state
            .refresh(
                &local_ips,
                &candidates,
                false,
                |_candidate, _address| Ok(()),
            )
            .expect("initial connection");

        let replacement_candidates = vec![
            connection_candidate(
                "windows",
                ConnectionCandidateKind::MdnsLan,
                "10.0.0.9:24800",
            ),
            connection_candidate(
                "windows",
                ConnectionCandidateKind::BackendLan,
                "10.0.0.6:24800",
            ),
        ];
        let mut attempted = Vec::new();

        let reconnected = state
            .refresh(
                &local_ips,
                &replacement_candidates,
                false,
                |candidate, address| {
                    attempted.push((candidate.kind, address));
                    if candidate.kind == ConnectionCandidateKind::MdnsLan {
                        Err("device was asleep".to_string())
                    } else {
                        Ok(())
                    }
                },
            )
            .expect("reconnect after loss")
            .expect("reconnect outcome");

        assert_eq!(reconnected.reason, DirectLanReconnectReason::ConnectionLost);
        assert_eq!(
            attempted,
            vec![
                (
                    ConnectionCandidateKind::MdnsLan,
                    "10.0.0.9:24800".parse().expect("mdns addr"),
                ),
                (
                    ConnectionCandidateKind::BackendLan,
                    "10.0.0.6:24800".parse().expect("backend addr"),
                ),
            ]
        );
        assert_eq!(reconnected.attempt.failed_attempts.len(), 1);
        assert_eq!(state.reconnect_count, 1);
    }

    #[test]
    fn collects_and_prioritizes_connection_candidates() {
        let devices = vec![DeviceWithPresence {
            device: Device {
                id: "windows".to_string(),
                name: "Windows".to_string(),
                os_type: "windows".to_string(),
                os_version: "11".to_string(),
                app_version: "0.1.0".to_string(),
                public_key: valid_test_public_key(),
                disabled: false,
            },
            presence: Some(Presence {
                online: true,
                lan_ips: vec!["192.168.1.20".to_string(), "192.168.1.21".to_string()],
                public_ip: "203.0.113.20".to_string(),
                listen_port: 24800,
                nat_type: "cone".to_string(),
                last_seen_at: 10,
            }),
        }];
        let mdns = vec![LanDiscoveryEndpoint {
            device_id: "windows".to_string(),
            address: "192.168.1.20:24800".parse().expect("mdns socket addr"),
        }];
        let nat = vec![NatTraversalCandidate {
            device_id: "windows".to_string(),
            address: "203.0.113.20:24800".parse().expect("nat socket addr"),
        }];
        let relay = vec![RelayCandidate {
            device_id: "windows".to_string(),
            relay_url: "relay://us-west.relay.kmsync.local:443".to_string(),
        }];

        let candidates = collect_connection_candidates("windows", &mdns, &devices, &nat, &relay);

        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.kind)
                .collect::<Vec<_>>(),
            vec![
                ConnectionCandidateKind::MdnsLan,
                ConnectionCandidateKind::BackendLan,
                ConnectionCandidateKind::NatTraversal,
                ConnectionCandidateKind::Relay,
            ]
        );
        assert_eq!(candidates[0].address, "192.168.1.20:24800");
        assert_eq!(candidates[1].address, "192.168.1.21:24800");
        assert_eq!(candidates[2].address, "203.0.113.20:24800");
        assert_eq!(
            candidates[3].address,
            "relay://us-west.relay.kmsync.local:443"
        );
    }

    #[test]
    fn backend_lan_candidates_ignore_offline_and_invalid_presence() {
        let devices = vec![
            DeviceWithPresence {
                device: Device {
                    id: "offline".to_string(),
                    name: "Offline".to_string(),
                    os_type: "windows".to_string(),
                    os_version: "11".to_string(),
                    app_version: "0.1.0".to_string(),
                    public_key: valid_test_public_key(),
                    disabled: false,
                },
                presence: Some(Presence {
                    online: false,
                    lan_ips: vec!["192.168.1.30".to_string()],
                    public_ip: "203.0.113.30".to_string(),
                    listen_port: 24800,
                    nat_type: "unknown".to_string(),
                    last_seen_at: 10,
                }),
            },
            DeviceWithPresence {
                device: Device {
                    id: "windows".to_string(),
                    name: "Windows".to_string(),
                    os_type: "windows".to_string(),
                    os_version: "11".to_string(),
                    app_version: "0.1.0".to_string(),
                    public_key: valid_test_public_key(),
                    disabled: false,
                },
                presence: Some(Presence {
                    online: true,
                    lan_ips: vec!["not-an-ip".to_string(), "10.0.0.5".to_string()],
                    public_ip: "203.0.113.20".to_string(),
                    listen_port: 24800,
                    nat_type: "unknown".to_string(),
                    last_seen_at: 10,
                }),
            },
        ];

        let candidates = collect_connection_candidates("windows", &[], &devices, &[], &[]);

        assert_eq!(
            candidates,
            vec![ConnectionCandidate {
                device_id: "windows".to_string(),
                kind: ConnectionCandidateKind::BackendLan,
                address: "10.0.0.5:24800".to_string(),
                priority: ConnectionCandidateKind::BackendLan.priority(),
            }]
        );
    }

    #[test]
    fn verified_target_connection_candidates_require_ed25519_public_key() {
        let devices = vec![DeviceWithPresence {
            device: Device {
                id: "windows".to_string(),
                name: "Windows".to_string(),
                os_type: "windows".to_string(),
                os_version: "11".to_string(),
                app_version: "0.1.0".to_string(),
                public_key: "dev-public-key-placeholder".to_string(),
                disabled: false,
            },
            presence: Some(Presence {
                online: true,
                lan_ips: vec!["10.0.0.5".to_string()],
                public_ip: "203.0.113.20".to_string(),
                listen_port: 24800,
                nat_type: "unknown".to_string(),
                last_seen_at: 10,
            }),
        }];

        let error = verified_target_connection_candidates("windows", &[], &devices, &[], &[])
            .expect_err("placeholder key must be rejected");

        assert!(error.contains("invalid target device public key"));
        assert!(error.contains("windows"));
    }

    #[test]
    fn verified_target_connection_candidates_return_identity_and_lan_candidates() {
        let public_key = format!("ed25519:{}", "a".repeat(64));
        let devices = vec![DeviceWithPresence {
            device: Device {
                id: "windows".to_string(),
                name: "Windows".to_string(),
                os_type: "windows".to_string(),
                os_version: "11".to_string(),
                app_version: "0.1.0".to_string(),
                public_key: public_key.clone(),
                disabled: false,
            },
            presence: Some(Presence {
                online: true,
                lan_ips: vec!["10.0.0.5".to_string()],
                public_ip: "203.0.113.20".to_string(),
                listen_port: 24800,
                nat_type: "unknown".to_string(),
                last_seen_at: 10,
            }),
        }];

        let verified = verified_target_connection_candidates("windows", &[], &devices, &[], &[])
            .expect("verified candidates");

        assert_eq!(verified.identity.device_id, "windows");
        assert_eq!(verified.identity.public_key, public_key);
        assert_eq!(verified.candidates.len(), 1);
        assert_eq!(verified.candidates[0].address, "10.0.0.5:24800");
    }

    #[test]
    fn connection_diagnostic_report_lists_candidates_without_sensitive_payloads() {
        let devices = vec![DeviceWithPresence {
            device: Device {
                id: "windows".to_string(),
                name: "Windows".to_string(),
                os_type: "windows".to_string(),
                os_version: "11".to_string(),
                app_version: "0.1.0".to_string(),
                public_key: valid_test_public_key(),
                disabled: false,
            },
            presence: Some(Presence {
                online: true,
                lan_ips: vec!["192.168.1.20".to_string()],
                public_ip: "203.0.113.20".to_string(),
                listen_port: 24800,
                nat_type: "cone".to_string(),
                last_seen_at: 10,
            }),
        }];
        let mdns = vec![LanDiscoveryEndpoint {
            device_id: "windows".to_string(),
            address: "192.168.1.20:24800".parse().expect("mdns socket addr"),
        }];
        let relay = vec![RelayCandidate {
            device_id: "windows".to_string(),
            relay_url: "relay://relay.kmsync.local:443?token=secret-token".to_string(),
        }];

        let report = render_connection_diagnostic_report("windows", &mdns, &devices, &[], &relay);

        assert!(report.contains("connection diagnostic report"));
        assert!(report.contains("target_device_id=windows"));
        assert!(report.contains("candidate_count=2"));
        assert!(report.contains("MdnsLan priority=400 address=192.168.1.20:24800"));
        assert!(report.contains("Relay priority=100 address=relay://relay.kmsync.local:443"));
        assert!(report.contains("privacy=connection_metadata_only"));
        assert!(!report.contains("secret-token"));
        assert!(!report.contains("secret clipboard"));
        assert!(!report.contains("Key::C"));
    }

    fn connection_candidate(
        device_id: &str,
        kind: ConnectionCandidateKind,
        address: &str,
    ) -> ConnectionCandidate {
        ConnectionCandidate {
            device_id: device_id.to_string(),
            kind,
            address: address.to_string(),
            priority: kind.priority(),
        }
    }

    fn valid_test_public_key() -> String {
        format!("ed25519:{}", "a".repeat(64))
    }

    #[test]
    fn compiles_remote_profile_config_into_hot_path_mapping() {
        let config = serde_json::from_str(include_str!(
            "../../../../configs/mac-to-windows.profile.json"
        ))
        .expect("profile config json");
        let profile = DeviceProfile {
            id: "profile-1".to_string(),
            source_device_id: "mac".to_string(),
            target_device_id: "windows".to_string(),
            config,
            version: 3,
            updated_at: 4,
        };

        let compiled = compile_device_profile(&profile).expect("compile profile");
        let mapped = compiled.profile.transform(InputEvent::Key(KeyEvent {
            key: Key::C,
            state: KeyState::Pressed,
            modifiers: Modifiers::META,
        }));

        assert_eq!(compiled.id, "profile-1");
        assert_eq!(compiled.source_device_id, "mac");
        assert_eq!(compiled.target_device_id, "windows");
        assert_eq!(compiled.version, 3);
        assert_eq!(
            mapped,
            InputEvent::Key(KeyEvent {
                key: Key::C,
                state: KeyState::Pressed,
                modifiers: Modifiers::CONTROL,
            })
        );
    }

    #[test]
    fn device_identity_load_or_generate_persists_generated_key_pair() {
        let path = temp_identity_path();
        let store = RecordingDeviceIdentitySecretStore::default();
        let first =
            DeviceIdentity::load_or_generate_with_store(&path, &store).expect("generate identity");

        assert!(first.public_key.starts_with("ed25519:"));
        assert!(first.private_key.starts_with("ed25519:"));
        assert_ne!(first.public_key, "dev-public-key-placeholder");

        let second =
            DeviceIdentity::load_or_generate_with_store(&path, &store).expect("load identity");
        assert_eq!(second, first);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn device_identity_file_keeps_private_key_in_secret_store() {
        let path = temp_identity_path();
        let store = RecordingDeviceIdentitySecretStore::default();

        let first =
            DeviceIdentity::load_or_generate_with_store(&path, &store).expect("generate identity");
        let text = fs::read_to_string(&path).expect("identity file");

        assert!(first.private_key.starts_with("ed25519:"));
        assert!(!text.contains(&first.private_key));
        assert!(!text.contains("\"private_key\""));
        assert!(text.contains("\"private_key_ref\""));
        assert_eq!(store.stored_private_keys(), vec![first.private_key.clone()]);

        let second =
            DeviceIdentity::load_or_generate_with_store(&path, &store).expect("load identity");
        assert_eq!(second, first);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn device_identity_migrates_legacy_private_key_file_to_secret_store() {
        let path = temp_identity_path();
        let legacy = DeviceIdentity {
            public_key: "ed25519:legacy-public".to_string(),
            private_key: "ed25519:legacy-private".to_string(),
        };
        fs::write(
            &path,
            serde_json::to_string_pretty(&legacy).expect("legacy json"),
        )
        .expect("write legacy identity file");
        let store = RecordingDeviceIdentitySecretStore::default();

        let loaded =
            DeviceIdentity::load_or_generate_with_store(&path, &store).expect("load identity");
        let rewritten = fs::read_to_string(&path).expect("rewritten identity file");

        assert_eq!(loaded, legacy);
        assert_eq!(store.stored_private_keys(), vec![legacy.private_key]);
        assert!(!rewritten.contains("legacy-private"));
        assert!(!rewritten.contains("\"private_key\""));
        assert!(rewritten.contains("\"private_key_ref\""));

        let _ = fs::remove_file(path);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_identity_secret_store_uses_windows_credential_manager() {
        assert_eq!(
            SystemDeviceIdentitySecretStore::kind(),
            DeviceIdentitySecretStoreKind::WindowsCredentialManager
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_identity_secret_store_uses_keychain() {
        assert_eq!(
            SystemDeviceIdentitySecretStore::kind(),
            DeviceIdentitySecretStoreKind::MacosKeychain
        );
    }

    #[test]
    fn register_device_request_uses_generated_identity_public_key() {
        let config = ClientConfig {
            server_url: "http://127.0.0.1:24888".to_string(),
            email: "dev@example.com".to_string(),
            email_login_code: None,
            device_name: "Desktop".to_string(),
            listen_port: 24800,
            heartbeat_interval_seconds: 15,
            identity_path: temp_identity_path(),
        };
        let identity = DeviceIdentity {
            public_key: "ed25519:test-public".to_string(),
            private_key: "ed25519:test-private".to_string(),
        };

        let request = build_register_device_request(&config, &identity);

        assert_eq!(request.public_key, "ed25519:test-public");
        assert_eq!(request.name, "Desktop");
    }

    fn temp_identity_path() -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "kmsync-device-identity-{}-{nanos}.json",
            std::process::id()
        ))
    }

    #[derive(Default)]
    struct RecordingDeviceIdentitySecretStore {
        secrets: std::sync::Mutex<std::collections::HashMap<String, String>>,
    }

    impl RecordingDeviceIdentitySecretStore {
        fn stored_private_keys(&self) -> Vec<String> {
            self.secrets
                .lock()
                .expect("secrets")
                .values()
                .cloned()
                .collect()
        }
    }

    impl DeviceIdentitySecretStore for RecordingDeviceIdentitySecretStore {
        fn store_private_key(
            &self,
            reference: &DeviceIdentitySecretRef,
            private_key: &str,
        ) -> Result<(), String> {
            self.secrets
                .lock()
                .expect("secrets")
                .insert(reference.account.clone(), private_key.to_string());
            Ok(())
        }

        fn load_private_key(&self, reference: &DeviceIdentitySecretRef) -> Result<String, String> {
            self.secrets
                .lock()
                .expect("secrets")
                .get(&reference.account)
                .cloned()
                .ok_or_else(|| format!("missing secret {}", reference.account))
        }
    }

    fn mdns_response_header(answer_count: u16) -> Vec<u8> {
        let mut packet = Vec::new();
        packet.extend_from_slice(&0_u16.to_be_bytes());
        packet.extend_from_slice(&0x8400_u16.to_be_bytes());
        packet.extend_from_slice(&0_u16.to_be_bytes());
        packet.extend_from_slice(&answer_count.to_be_bytes());
        packet.extend_from_slice(&0_u16.to_be_bytes());
        packet.extend_from_slice(&0_u16.to_be_bytes());
        packet
    }

    fn append_ptr_record(packet: &mut Vec<u8>, name: &str, target: &str) {
        append_name(packet, name);
        append_record_header(packet, 12, 1, 120);
        let mut data = Vec::new();
        append_name(&mut data, target);
        append_rdata(packet, &data);
    }

    fn append_srv_record(packet: &mut Vec<u8>, name: &str, port: u16, target: &str) {
        append_name(packet, name);
        append_record_header(packet, 33, 1, 120);
        let mut data = Vec::new();
        data.extend_from_slice(&0_u16.to_be_bytes());
        data.extend_from_slice(&0_u16.to_be_bytes());
        data.extend_from_slice(&port.to_be_bytes());
        append_name(&mut data, target);
        append_rdata(packet, &data);
    }

    fn append_txt_record(packet: &mut Vec<u8>, name: &str, entries: &[&str]) {
        append_name(packet, name);
        append_record_header(packet, 16, 1, 120);
        let mut data = Vec::new();
        for entry in entries {
            data.push(u8::try_from(entry.len()).expect("txt entry length"));
            data.extend_from_slice(entry.as_bytes());
        }
        append_rdata(packet, &data);
    }

    fn append_a_record(packet: &mut Vec<u8>, name: &str, address: [u8; 4]) {
        append_name(packet, name);
        append_record_header(packet, 1, 1, 120);
        append_rdata(packet, &address);
    }

    fn append_record_header(packet: &mut Vec<u8>, record_type: u16, class: u16, ttl: u32) {
        packet.extend_from_slice(&record_type.to_be_bytes());
        packet.extend_from_slice(&class.to_be_bytes());
        packet.extend_from_slice(&ttl.to_be_bytes());
    }

    fn append_rdata(packet: &mut Vec<u8>, data: &[u8]) {
        packet.extend_from_slice(
            &u16::try_from(data.len())
                .expect("rdata length")
                .to_be_bytes(),
        );
        packet.extend_from_slice(data);
    }

    fn append_name(packet: &mut Vec<u8>, name: &str) {
        for label in name.trim_end_matches('.').split('.') {
            packet.push(u8::try_from(label.len()).expect("label length"));
            packet.extend_from_slice(label.as_bytes());
        }
        packet.push(0);
    }
}

fn get_json<R>(agent: &ureq::Agent, url: &str, access_token: &str) -> Result<R, String>
where
    R: for<'de> Deserialize<'de>,
{
    get_json_optional_auth(agent, url, Some(access_token))
}

fn get_json_without_auth<R>(agent: &ureq::Agent, url: &str) -> Result<R, String>
where
    R: for<'de> Deserialize<'de>,
{
    get_json_optional_auth(agent, url, None)
}

fn get_json_optional_auth<R>(
    agent: &ureq::Agent,
    url: &str,
    access_token: Option<&str>,
) -> Result<R, String>
where
    R: for<'de> Deserialize<'de>,
{
    let mut builder = agent.get(url);
    if let Some(token) = access_token {
        builder = builder.header("authorization", &format!("Bearer {token}"));
    }
    let mut response = builder
        .call()
        .map_err(|error| format!("request failed: {error}"))?;
    response
        .body_mut()
        .read_json()
        .map_err(|error| format!("invalid json response: {error}"))
}

fn put_json<T, R>(
    agent: &ureq::Agent,
    url: &str,
    access_token: &str,
    request: &T,
) -> Result<R, String>
where
    T: Serialize,
    R: for<'de> Deserialize<'de>,
{
    let mut response = agent
        .put(url)
        .header("content-type", "application/json")
        .header("authorization", &format!("Bearer {access_token}"))
        .send_json(request)
        .map_err(|error| format!("request failed: {error}"))?;
    response
        .body_mut()
        .read_json()
        .map_err(|error| format!("invalid json response: {error}"))
}

fn discover_lan_ips() -> Vec<String> {
    let mut ips = Vec::new();
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                ips.push(addr.ip().to_string());
            }
        }
    }
    if ips.is_empty() {
        ips.push("127.0.0.1".to_string());
    }
    ips
}
