use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc};
use tower_http::cors::CorsLayer;
use uuid::Uuid;

const PRESENCE_TTL_SECONDS: u64 = 60;
const RELAY_FRAME_MAGIC: &[u8; 4] = b"KMR1";
const RELAY_TARGET_DEVICE_ID_LEN: usize = 36;
const RELAY_CLIENT_FRAME_HEADER_LEN: usize = RELAY_FRAME_MAGIC.len() + RELAY_TARGET_DEVICE_ID_LEN;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let config_path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/kmsync/kmsync-server.json"));
    let config = ServerConfig::load(&config_path).expect("load server config");
    let bind = config.bind;
    let state = AppState::load(Some(config.data_path)).expect("load state");

    let app = build_app(state);

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .expect("bind server");
    println!("kmsync-server listening on http://{bind}");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
    .expect("serve");
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerConfig {
    bind: SocketAddr,
    data_path: PathBuf,
}

impl ServerConfig {
    fn load(path: &FsPath) -> Result<Self, std::io::Error> {
        let text = fs::read_to_string(path)?;
        serde_json::from_str(&text).map_err(std::io::Error::other)
    }
}

fn build_app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/releases/check", get(check_release))
        .route("/v1/relay/token", post(issue_relay_token))
        .route("/v1/relay/ws", get(relay_ws))
        .route("/v1/signal/connect.request", post(signal_connect_request))
        .route("/v1/signal/connect.accept", post(signal_connect_accept))
        .route("/v1/signal/connect.reject", post(signal_connect_reject))
        .route("/v1/signal/candidate.add", post(signal_candidate_add))
        .route("/v1/signal/session.close", post(signal_session_close))
        .route("/v1/auth/email/start", post(start_email_login))
        .route("/v1/auth/email/verify", post(verify_email_login))
        .route("/v1/auth/refresh", post(refresh_access_token))
        .route("/v1/auth/logout", post(logout))
        .route("/v1/events/ws", get(events_ws))
        .route("/v1/devices/register", post(register_device))
        .route("/v1/devices", get(list_devices))
        .route("/v1/topology", get(get_topology).put(upsert_topology))
        .route(
            "/v1/devices/{device_id}",
            patch(update_device).delete(delete_device),
        )
        .route(
            "/v1/devices/{device_id}/reauthorize",
            post(reauthorize_device),
        )
        .route("/v1/devices/{device_id}/heartbeat", post(heartbeat))
        .route("/v1/profiles/changes", get(list_profile_changes))
        .route("/v1/profiles/rollback", post(rollback_profile))
        .route("/v1/profiles", get(list_profiles).put(upsert_profile))
        .layer(CorsLayer::very_permissive())
        .with_state(state)
}

#[derive(Clone)]
struct AppState {
    inner: Arc<Mutex<Store>>,
    persistence: StorePersistence,
    events: broadcast::Sender<ServerEvent>,
    relay: RelayHub,
}

#[derive(Clone, Default)]
struct RelayHub {
    inner: Arc<Mutex<HashMap<Uuid, RelayPeer>>>,
}

struct RelayPeer {
    connection_id: Uuid,
    tx: mpsc::UnboundedSender<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelayRouteError {
    TargetOffline,
    TargetReceiverClosed,
}

impl RelayHub {
    fn register(&self, device_id: Uuid, connection_id: Uuid) -> mpsc::UnboundedReceiver<Vec<u8>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(device_id, RelayPeer { connection_id, tx });
        rx
    }

    fn unregister_if_current(&self, device_id: Uuid, connection_id: Uuid) {
        let mut peers = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if peers
            .get(&device_id)
            .is_some_and(|peer| peer.connection_id == connection_id)
        {
            peers.remove(&device_id);
        }
    }

    fn send_frame(&self, target_device_id: Uuid, frame: Vec<u8>) -> Result<(), RelayRouteError> {
        let tx = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&target_device_id)
            .map(|peer| peer.tx.clone())
            .ok_or(RelayRouteError::TargetOffline)?;
        tx.send(frame)
            .map_err(|_| RelayRouteError::TargetReceiverClosed)
    }
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct Store {
    users: HashMap<Uuid, User>,
    #[serde(default)]
    email_login_codes: HashMap<String, EmailLoginChallenge>,
    sessions: HashMap<String, Uuid>,
    refresh_tokens: HashMap<String, Uuid>,
    relay_tokens: HashMap<String, RelayTokenRecord>,
    signal_sessions: HashMap<Uuid, SignalSession>,
    devices: HashMap<Uuid, Device>,
    presence: HashMap<Uuid, Presence>,
    #[serde(default)]
    topologies: HashMap<Uuid, Topology>,
    profiles: HashMap<String, DeviceProfile>,
    profile_history: HashMap<String, Vec<DeviceProfile>>,
    profile_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct User {
    id: Uuid,
    email: String,
    created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EmailLoginChallenge {
    code: String,
    expires_at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DeviceRole {
    Master,
    #[default]
    Client,
}

const fn default_version() -> u64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Device {
    id: Uuid,
    user_id: Uuid,
    name: String,
    #[serde(default)]
    role: DeviceRole,
    os_type: String,
    os_version: String,
    app_version: String,
    public_key: String,
    #[serde(default)]
    disabled: bool,
    created_at: u64,
    #[serde(default)]
    updated_at: u64,
    #[serde(default = "default_version")]
    device_version: u64,
    #[serde(default = "default_version")]
    name_version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Presence {
    device_id: Uuid,
    user_id: Uuid,
    online: bool,
    lan_ips: Vec<String>,
    public_ip: String,
    listen_port: u16,
    nat_type: String,
    last_seen_at: u64,
    #[serde(default)]
    expires_at: u64,
    #[serde(default = "default_version")]
    presence_version: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct TopologyLayout {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    left: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    right: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    top: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bottom: Option<Uuid>,
}

impl TopologyLayout {
    fn device_ids(&self) -> impl Iterator<Item = Uuid> {
        [self.left, self.right, self.top, self.bottom]
            .into_iter()
            .flatten()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Topology {
    user_id: Uuid,
    master_device_id: Option<Uuid>,
    layout: TopologyLayout,
    topology_version: u64,
    updated_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct DeviceProfile {
    id: String,
    user_id: Uuid,
    source_device_id: Uuid,
    target_device_id: Uuid,
    config: Value,
    version: u64,
    #[serde(default)]
    revision: u64,
    updated_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerEvent {
    DevicePresenceChanged {
        #[serde(skip_serializing)]
        user_id: Uuid,
        device_id: Uuid,
        online: bool,
        presence_version: u64,
    },
    DeviceChanged {
        #[serde(skip_serializing)]
        user_id: Uuid,
        device_id: Uuid,
        device_version: u64,
        name_version: u64,
    },
    TopologyChanged {
        #[serde(skip_serializing)]
        user_id: Uuid,
        topology_version: u64,
    },
    ProfileChanged {
        #[serde(skip_serializing)]
        user_id: Uuid,
        profile: DeviceProfile,
    },
    SignalSessionChanged {
        #[serde(skip_serializing)]
        user_id: Uuid,
        session: SignalSession,
    },
}

impl ServerEvent {
    const fn user_id(&self) -> Uuid {
        match self {
            Self::DevicePresenceChanged { user_id, .. } => *user_id,
            Self::DeviceChanged { user_id, .. } => *user_id,
            Self::TopologyChanged { user_id, .. } => *user_id,
            Self::ProfileChanged { user_id, .. } => *user_id,
            Self::SignalSessionChanged { user_id, .. } => *user_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelayTokenRecord {
    token: String,
    user_id: Uuid,
    source_device_id: Uuid,
    target_device_id: Uuid,
    region: String,
    relay_url: String,
    expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SignalCandidate {
    transport: String,
    address: String,
    priority: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SignalSession {
    id: Uuid,
    user_id: Uuid,
    source_device_id: Uuid,
    target_device_id: Uuid,
    status: String,
    candidates: Vec<SignalCandidate>,
    created_at: u64,
    closed_at: Option<u64>,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

#[derive(Debug, Serialize)]
struct HealthBody {
    ok: bool,
}

async fn health() -> Json<HealthBody> {
    Json(HealthBody { ok: true })
}

#[derive(Debug, Deserialize)]
struct ReleaseCheckQuery {
    platform: String,
    version: String,
    channel: Option<String>,
    device_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReleaseCheckResponse {
    platform: String,
    channel: String,
    latest_version: String,
    min_supported_version: String,
    update_available: bool,
    force_update: bool,
    rollout_percent: u8,
    rollout_bucket: Option<u8>,
    rollout_eligible: bool,
    auto_update_action: AutoUpdateAction,
    download_url: String,
    installer_sha256: String,
    signature_url: String,
}

async fn check_release(
    Query(query): Query<ReleaseCheckQuery>,
) -> Result<Json<ReleaseCheckResponse>, ApiError> {
    let channel = query.channel.unwrap_or_else(|| "stable".to_string());
    let policy = release_policy(&query.platform, &channel)?;
    let current = parse_version(&query.version)?;
    let latest = parse_version(policy.latest_version)?;
    let min_supported = parse_version(policy.min_supported_version)?;
    let update_available = current < latest;
    let force_update = current < min_supported;
    let rollout_bucket = query
        .device_id
        .as_deref()
        .map(|device_id| stable_rollout_bucket(&query.platform, &channel, device_id));
    let rollout_eligible = update_available
        && (force_update || rollout_bucket.is_some_and(|bucket| bucket < policy.rollout_percent));
    let auto_update_action = if !update_available {
        AutoUpdateAction::None
    } else if force_update {
        AutoUpdateAction::Force
    } else if rollout_eligible {
        AutoUpdateAction::Download
    } else {
        AutoUpdateAction::WaitForRollout
    };

    Ok(Json(ReleaseCheckResponse {
        platform: query.platform,
        channel,
        latest_version: policy.latest_version.to_string(),
        min_supported_version: policy.min_supported_version.to_string(),
        update_available,
        force_update,
        rollout_percent: policy.rollout_percent,
        rollout_bucket,
        rollout_eligible,
        auto_update_action,
        download_url: policy.download_url.to_string(),
        installer_sha256: policy.installer_sha256.to_string(),
        signature_url: policy.signature_url.to_string(),
    }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum AutoUpdateAction {
    None,
    Download,
    WaitForRollout,
    Force,
}

#[derive(Debug, Clone, Copy)]
struct ReleasePolicy {
    latest_version: &'static str,
    min_supported_version: &'static str,
    rollout_percent: u8,
    download_url: &'static str,
    installer_sha256: &'static str,
    signature_url: &'static str,
}

fn release_policy(platform: &str, channel: &str) -> Result<ReleasePolicy, ApiError> {
    match (platform, channel) {
        ("windows", "stable") => Ok(ReleasePolicy {
            latest_version: "0.2.0",
            min_supported_version: "0.1.0",
            rollout_percent: 25,
            download_url: "https://updates.example.invalid/kmsync/windows/0.2.0",
            installer_sha256: "a9efb60b6ff3bf8b42fd2506c6f3e8b4c345bd974f4685db963918f6a1a26158",
            signature_url:
                "https://updates.example.invalid/kmsync/windows/0.2.0/kmsync-installer.sig",
        }),
        ("macos", "stable") => Ok(ReleasePolicy {
            latest_version: "0.2.0",
            min_supported_version: "0.1.0",
            rollout_percent: 25,
            download_url: "https://updates.example.invalid/kmsync/macos/0.2.0",
            installer_sha256: "4fb9b8b4a8bffcf4ce9c14d9f89b205c41a2b35d4e0a41dd137501aee4b96d07",
            signature_url: "https://updates.example.invalid/kmsync/macos/0.2.0/kmsync.dmg.sig",
        }),
        ("linux", "stable") => Ok(ReleasePolicy {
            latest_version: "0.2.0",
            min_supported_version: "0.1.0",
            rollout_percent: 10,
            download_url: "https://updates.example.invalid/kmsync/linux/0.2.0",
            installer_sha256: "d98259bc527271f0c85f728e42e34795e72d1fce734cb589641bd7a2f59e0518",
            signature_url: "https://updates.example.invalid/kmsync/linux/0.2.0/kmsync.tar.gz.sig",
        }),
        (_, _) => Err(ApiError::bad_request(
            "unsupported release platform or channel",
        )),
    }
}

fn stable_rollout_bucket(platform: &str, channel: &str, device_id: &str) -> u8 {
    const FNV_OFFSET: u32 = 2_166_136_261;
    const FNV_PRIME: u32 = 16_777_619;

    let mut hash = FNV_OFFSET;
    for byte in platform
        .bytes()
        .chain([b':'])
        .chain(channel.bytes())
        .chain([b':'])
        .chain(device_id.bytes())
    {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    u8::try_from(hash % 100).expect("rollout bucket is always < 100")
}

fn parse_version(version: &str) -> Result<(u64, u64, u64), ApiError> {
    let mut parts = version.split('.');
    let major = parse_version_part(parts.next(), version)?;
    let minor = parse_version_part(parts.next(), version)?;
    let patch = parse_version_part(parts.next(), version)?;
    if parts.next().is_some() {
        return Err(ApiError::bad_request("invalid version"));
    }
    Ok((major, minor, patch))
}

fn parse_version_part(part: Option<&str>, original: &str) -> Result<u64, ApiError> {
    part.filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("invalid version"))?
        .parse()
        .map_err(|_| ApiError::bad_request(&format!("invalid version '{original}'")))
}

#[derive(Debug, Deserialize)]
struct RelayTokenRequest {
    source_device_id: Uuid,
    target_device_id: Uuid,
    preferred_region: Option<String>,
}

#[derive(Debug, Serialize)]
struct RelayTokenResponse {
    relay_token: String,
    relay_url: String,
    region: String,
    expires_at: u64,
}

async fn issue_relay_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RelayTokenRequest>,
) -> Result<Json<RelayTokenResponse>, ApiError> {
    let user_id = authorize_or_default(&state, &headers)?;
    let mut store = state.lock()?;
    assert_device_owner(&store, user_id, request.source_device_id)?;
    assert_device_owner(&store, user_id, request.target_device_id)?;

    let region = schedule_relay_region(request.preferred_region.as_deref());
    let relay_url = relay_url_for_region(region);
    let expires_at = now_seconds().saturating_add(600);
    let relay_token = format!("relay-{user_id}-{}", Uuid::new_v4());
    store.relay_tokens.insert(
        relay_token.clone(),
        RelayTokenRecord {
            token: relay_token.clone(),
            user_id,
            source_device_id: request.source_device_id,
            target_device_id: request.target_device_id,
            region: region.to_string(),
            relay_url: relay_url.to_string(),
            expires_at,
        },
    );
    state.save_locked(&store)?;

    Ok(Json(RelayTokenResponse {
        relay_token,
        relay_url: relay_url.to_string(),
        region: region.to_string(),
        expires_at,
    }))
}

fn schedule_relay_region(preferred_region: Option<&str>) -> &'static str {
    match preferred_region {
        Some("us-west") => "us-west",
        Some("eu-central") => "eu-central",
        Some("ap-southeast") => "ap-southeast",
        _ => "us-west",
    }
}

fn relay_url_for_region(region: &str) -> &'static str {
    match region {
        "eu-central" => "relay://eu-central.relay.kmsync.local:443",
        "ap-southeast" => "relay://ap-southeast.relay.kmsync.local:443",
        _ => "relay://us-west.relay.kmsync.local:443",
    }
}

#[derive(Debug, Deserialize)]
struct SignalConnectRequest {
    source_device_id: Uuid,
    target_device_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct SignalSessionRequest {
    session_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct SignalCandidateRequest {
    session_id: Uuid,
    candidate: SignalCandidate,
}

async fn signal_connect_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SignalConnectRequest>,
) -> Result<Json<SignalSession>, ApiError> {
    let user_id = authorize_or_default(&state, &headers)?;
    let mut store = state.lock()?;
    assert_device_owner(&store, user_id, request.source_device_id)?;
    assert_device_owner(&store, user_id, request.target_device_id)?;

    let session = SignalSession {
        id: Uuid::new_v4(),
        user_id,
        source_device_id: request.source_device_id,
        target_device_id: request.target_device_id,
        status: "requested".to_string(),
        candidates: Vec::new(),
        created_at: now_seconds(),
        closed_at: None,
    };
    store.signal_sessions.insert(session.id, session.clone());
    state.save_locked(&store)?;
    state.publish_event(ServerEvent::SignalSessionChanged {
        user_id,
        session: session.clone(),
    });
    Ok(Json(session))
}

async fn signal_connect_accept(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SignalSessionRequest>,
) -> Result<Json<SignalSession>, ApiError> {
    update_signal_session_status(state, headers, request.session_id, "accepted")
}

async fn signal_connect_reject(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SignalSessionRequest>,
) -> Result<Json<SignalSession>, ApiError> {
    update_signal_session_status(state, headers, request.session_id, "rejected")
}

async fn signal_session_close(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SignalSessionRequest>,
) -> Result<Json<SignalSession>, ApiError> {
    update_signal_session_status(state, headers, request.session_id, "closed")
}

fn update_signal_session_status(
    state: AppState,
    headers: HeaderMap,
    session_id: Uuid,
    status: &str,
) -> Result<Json<SignalSession>, ApiError> {
    let user_id = authorize_or_default(&state, &headers)?;
    let mut store = state.lock()?;
    let Some(session) = store.signal_sessions.get_mut(&session_id) else {
        return Err(ApiError::not_found("signal session not found"));
    };
    if session.user_id != user_id {
        return Err(ApiError::not_found("signal session not found"));
    }
    session.status = status.to_string();
    if status == "closed" {
        session.closed_at = Some(now_seconds());
    }
    let session = session.clone();
    state.save_locked(&store)?;
    state.publish_event(ServerEvent::SignalSessionChanged {
        user_id,
        session: session.clone(),
    });
    Ok(Json(session))
}

async fn signal_candidate_add(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SignalCandidateRequest>,
) -> Result<Json<SignalSession>, ApiError> {
    let user_id = authorize_or_default(&state, &headers)?;
    let mut store = state.lock()?;
    let Some(session) = store.signal_sessions.get_mut(&request.session_id) else {
        return Err(ApiError::not_found("signal session not found"));
    };
    if session.user_id != user_id {
        return Err(ApiError::not_found("signal session not found"));
    }
    if request.candidate.transport.trim().is_empty() || request.candidate.address.trim().is_empty()
    {
        return Err(ApiError::bad_request(
            "candidate transport and address are required",
        ));
    }
    session.candidates.push(request.candidate);
    session
        .candidates
        .sort_by(|left, right| right.priority.cmp(&left.priority));
    let session = session.clone();
    state.save_locked(&store)?;
    state.publish_event(ServerEvent::SignalSessionChanged {
        user_id,
        session: session.clone(),
    });
    Ok(Json(session))
}

const EMAIL_LOGIN_CODE_TTL_SECONDS: u64 = 10 * 60;

#[derive(Debug, Deserialize)]
struct EmailLoginStartRequest {
    email: String,
}

#[derive(Debug, Serialize)]
struct EmailLoginStartResponse {
    email: String,
    expires_at: u64,
    #[cfg(test)]
    code: String,
}

#[derive(Debug, Deserialize)]
struct EmailLoginVerifyRequest {
    email: String,
    code: String,
}

#[derive(Debug, Serialize)]
struct LoginResponse {
    user_id: Uuid,
    access_token: String,
    refresh_token: String,
}

async fn start_email_login(
    State(state): State<AppState>,
    Json(request): Json<EmailLoginStartRequest>,
) -> Result<Json<EmailLoginStartResponse>, ApiError> {
    let email = normalize_email(&request.email)?;
    let code = new_email_login_code();
    let expires_at = now_seconds().saturating_add(EMAIL_LOGIN_CODE_TTL_SECONDS);

    let mut store = state.lock()?;
    store.email_login_codes.insert(
        email.clone(),
        EmailLoginChallenge {
            code: code.clone(),
            expires_at,
        },
    );
    state.save_locked(&store)?;

    Ok(Json(EmailLoginStartResponse {
        email,
        expires_at,
        #[cfg(test)]
        code,
    }))
}

async fn verify_email_login(
    State(state): State<AppState>,
    Json(request): Json<EmailLoginVerifyRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    let email = normalize_email(&request.email)?;
    let code = request.code.trim();

    let mut store = state.lock()?;
    let challenge = store
        .email_login_codes
        .get(&email)
        .cloned()
        .ok_or_else(|| ApiError::unauthorized("invalid email code"))?;
    if challenge.expires_at < now_seconds() {
        store.email_login_codes.remove(&email);
        state.save_locked(&store)?;
        return Err(ApiError::unauthorized("email code expired"));
    }
    if !challenge.code.eq_ignore_ascii_case(code) {
        return Err(ApiError::unauthorized("invalid email code"));
    }

    store.email_login_codes.remove(&email);
    let user_id = get_or_create_user(&mut store, &email);

    let token = new_access_token(user_id);
    let refresh_token = new_refresh_token(user_id);
    store.sessions.insert(token.clone(), user_id);
    store.refresh_tokens.insert(refresh_token.clone(), user_id);
    state.save_locked(&store)?;

    Ok(Json(LoginResponse {
        user_id,
        access_token: token,
        refresh_token,
    }))
}

#[derive(Debug, Deserialize)]
struct RefreshRequest {
    refresh_token: String,
}

#[derive(Debug, Serialize)]
struct RefreshResponse {
    access_token: String,
}

async fn refresh_access_token(
    State(state): State<AppState>,
    Json(request): Json<RefreshRequest>,
) -> Result<Json<RefreshResponse>, ApiError> {
    let mut store = state.lock()?;
    let user_id = store
        .refresh_tokens
        .get(&request.refresh_token)
        .copied()
        .ok_or_else(|| ApiError::unauthorized("invalid refresh token"))?;
    let access_token = new_access_token(user_id);
    store.sessions.insert(access_token.clone(), user_id);
    state.save_locked(&store)?;
    Ok(Json(RefreshResponse { access_token }))
}

#[derive(Debug, Deserialize)]
struct LogoutRequest {
    refresh_token: Option<String>,
}

#[derive(Debug, Serialize)]
struct LogoutResponse {
    logged_out: bool,
}

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<LogoutRequest>,
) -> Result<Json<LogoutResponse>, ApiError> {
    let token = bearer_token(&headers)?.to_string();
    let mut store = state.lock()?;
    if store.sessions.remove(&token).is_none() {
        return Err(ApiError::unauthorized("invalid token"));
    }
    if let Some(refresh_token) = request.refresh_token {
        store.refresh_tokens.remove(&refresh_token);
    }
    state.save_locked(&store)?;
    Ok(Json(LogoutResponse { logged_out: true }))
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    access_token: Option<String>,
}

async fn events_ws(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<EventsQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let user_id = match query.access_token {
        Some(token) => authorize_access_token(&state, &token)?,
        None => authorize_or_default(&state, &headers)?,
    };
    let events = state.subscribe_events();
    Ok(ws.on_upgrade(move |socket| stream_events(socket, events, user_id)))
}

async fn stream_events(
    mut socket: WebSocket,
    mut events: broadcast::Receiver<ServerEvent>,
    user_id: Uuid,
) {
    loop {
        match events.recv().await {
            Ok(event) if event.user_id() == user_id => {
                let Ok(text) = serde_json::to_string(&event) else {
                    break;
                };
                if socket.send(Message::from(text)).await.is_err() {
                    break;
                }
            }
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RelayWsQuery {
    device_id: Uuid,
    access_token: Option<String>,
    mode: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelayWsMode {
    Rx,
    Tx,
}

impl RelayWsMode {
    fn parse(value: Option<&str>) -> Result<Self, ApiError> {
        match value.unwrap_or("rx") {
            "rx" => Ok(Self::Rx),
            "tx" => Ok(Self::Tx),
            _ => Err(ApiError::bad_request("relay mode must be rx or tx")),
        }
    }
}

async fn relay_ws(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<RelayWsQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let user_id = match query.access_token {
        Some(token) => authorize_access_token(&state, &token)?,
        None => authorize_or_default(&state, &headers)?,
    };
    let mode = RelayWsMode::parse(query.mode.as_deref())?;
    assert_relay_device_enabled(&state, user_id, query.device_id)?;

    let connection_id = Uuid::new_v4();
    let outbound = match mode {
        RelayWsMode::Rx => Some(state.relay.register(query.device_id, connection_id)),
        RelayWsMode::Tx => None,
    };
    Ok(ws.on_upgrade(move |socket| {
        stream_relay(
            socket,
            state,
            user_id,
            query.device_id,
            connection_id,
            outbound,
        )
    }))
}

async fn stream_relay(
    mut socket: WebSocket,
    state: AppState,
    user_id: Uuid,
    source_device_id: Uuid,
    connection_id: Uuid,
    mut outbound: Option<mpsc::UnboundedReceiver<Vec<u8>>>,
) {
    loop {
        if let Some(outbound) = outbound.as_mut() {
            tokio::select! {
                Some(payload) = outbound.recv() => {
                    if socket.send(Message::Binary(payload.into())).await.is_err() {
                        break;
                    }
                }
                incoming = socket.recv() => {
                    if !handle_relay_socket_message(incoming, &mut socket, &state, user_id, source_device_id).await {
                        break;
                    }
                }
            }
        } else if !handle_relay_socket_message(
            socket.recv().await,
            &mut socket,
            &state,
            user_id,
            source_device_id,
        )
        .await
        {
            break;
        }
    }

    state
        .relay
        .unregister_if_current(source_device_id, connection_id);
}

async fn handle_relay_socket_message(
    incoming: Option<Result<Message, axum::Error>>,
    socket: &mut WebSocket,
    state: &AppState,
    user_id: Uuid,
    source_device_id: Uuid,
) -> bool {
    match incoming {
        Some(Ok(Message::Binary(payload))) => {
            if let Err(error) =
                route_relay_client_frame(state, user_id, source_device_id, payload.as_ref())
            {
                let _ = socket
                    .send(Message::Text(format!("relay_error={error}").into()))
                    .await;
            }
            true
        }
        Some(Ok(Message::Ping(payload))) => socket.send(Message::Pong(payload)).await.is_ok(),
        Some(Ok(Message::Close(_))) | None => false,
        Some(Ok(Message::Text(_) | Message::Pong(_))) => true,
        Some(Err(_)) => false,
    }
}

fn route_relay_client_frame(
    state: &AppState,
    user_id: Uuid,
    source_device_id: Uuid,
    payload: &[u8],
) -> Result<(), String> {
    assert_relay_device_enabled(state, user_id, source_device_id).map_err(|error| error.message)?;
    let (target_device_id, frame) = parse_relay_client_frame(payload)?;
    assert_relay_device_enabled(state, user_id, target_device_id).map_err(|error| error.message)?;
    state
        .relay
        .send_frame(target_device_id, frame.to_vec())
        .map_err(|error| format!("relay route failed: {error:?}"))
}

fn parse_relay_client_frame(payload: &[u8]) -> Result<(Uuid, &[u8]), String> {
    if payload.len() <= RELAY_CLIENT_FRAME_HEADER_LEN {
        return Err("relay frame is too short".to_string());
    }
    if &payload[..RELAY_FRAME_MAGIC.len()] != RELAY_FRAME_MAGIC {
        return Err("relay frame has invalid magic".to_string());
    }
    let target_id_start = RELAY_FRAME_MAGIC.len();
    let target_id_end = target_id_start + RELAY_TARGET_DEVICE_ID_LEN;
    let target_device_id = std::str::from_utf8(&payload[target_id_start..target_id_end])
        .map_err(|error| format!("relay target device id is not utf8: {error}"))?;
    let target_device_id = Uuid::parse_str(target_device_id)
        .map_err(|error| format!("relay target device id is not a uuid: {error}"))?;
    Ok((target_device_id, &payload[target_id_end..]))
}

fn assert_relay_device_enabled(
    state: &AppState,
    user_id: Uuid,
    device_id: Uuid,
) -> Result<(), ApiError> {
    let store = state.lock()?;
    let Some(device) = store.devices.get(&device_id) else {
        return Err(ApiError::not_found("device not found"));
    };
    if device.user_id != user_id {
        return Err(ApiError::not_found("device not found"));
    }
    if device.disabled {
        return Err(ApiError::forbidden("device disabled"));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct RegisterDeviceRequest {
    device_id: Option<Uuid>,
    name: String,
    role: Option<DeviceRole>,
    os_type: String,
    os_version: String,
    app_version: String,
    public_key: String,
}

#[derive(Debug, Serialize)]
struct RegisterDeviceResponse {
    device_id: Uuid,
}

async fn register_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RegisterDeviceRequest>,
) -> Result<Json<RegisterDeviceResponse>, ApiError> {
    let user_id = authorize_or_default(&state, &headers)?;
    if request.name.trim().is_empty() {
        return Err(ApiError::bad_request("device name is required"));
    }

    let mut store = state.lock()?;
    let now = now_seconds();
    let requested_device_id = request.device_id.unwrap_or_else(Uuid::new_v4);
    let device_id = if store.devices.contains_key(&requested_device_id) {
        requested_device_id
    } else {
        store
            .devices
            .values()
            .find(|device| device.user_id == user_id && device.public_key == request.public_key)
            .map_or(requested_device_id, |device| device.id)
    };
    if let Some(device) = store.devices.get_mut(&device_id) {
        if device.user_id != user_id {
            return Err(ApiError::conflict(
                "device id already belongs to another user",
            ));
        }
        if device.public_key != request.public_key {
            return Err(ApiError::conflict("device public key mismatch"));
        }

        let role = request.role.unwrap_or_else(|| device.role.clone());
        let changed = device.role != role
            || device.os_type != request.os_type
            || device.os_version != request.os_version
            || device.app_version != request.app_version;
        device.role = role;
        device.os_type = request.os_type;
        device.os_version = request.os_version;
        device.app_version = request.app_version;
        if changed {
            device.updated_at = now;
            device.device_version = device.device_version.saturating_add(1);
        }
    } else {
        let device = Device {
            id: device_id,
            user_id,
            name: request.name,
            role: request.role.unwrap_or_default(),
            os_type: request.os_type,
            os_version: request.os_version,
            app_version: request.app_version,
            public_key: request.public_key,
            disabled: false,
            created_at: now,
            updated_at: now,
            device_version: 1,
            name_version: 1,
        };
        store.devices.insert(device_id, device);
    }
    state.save_locked(&store)?;
    Ok(Json(RegisterDeviceResponse { device_id }))
}

#[derive(Debug, Serialize)]
struct DeviceWithPresence {
    device: Device,
    presence: Option<Presence>,
}

async fn list_devices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<DeviceWithPresence>>, ApiError> {
    let user_id = authorize_or_default(&state, &headers)?;
    let store = state.lock()?;
    let now = now_seconds();
    let devices = store
        .devices
        .values()
        .filter(|device| device.user_id == user_id)
        .map(|device| DeviceWithPresence {
            device: device.clone(),
            presence: presence_for_device_list(store.presence.get(&device.id), now),
        })
        .collect();
    Ok(Json(devices))
}

fn presence_for_device_list(presence: Option<&Presence>, now: u64) -> Option<Presence> {
    let mut presence = presence.cloned()?;
    if !presence_is_online_at(&presence, now) {
        presence.online = false;
    }
    Some(presence)
}

fn presence_is_online_at(presence: &Presence, now: u64) -> bool {
    presence.online && now <= presence_expires_at(presence)
}

fn presence_expires_at(presence: &Presence) -> u64 {
    if presence.expires_at == 0 {
        presence.last_seen_at.saturating_add(PRESENCE_TTL_SECONDS)
    } else {
        presence.expires_at
    }
}

#[derive(Debug, Deserialize)]
struct UpdateDeviceRequest {
    name: Option<String>,
    disabled: Option<bool>,
}

async fn update_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(device_id): AxumPath<Uuid>,
    Json(request): Json<UpdateDeviceRequest>,
) -> Result<Json<Device>, ApiError> {
    let user_id = authorize_or_default(&state, &headers)?;
    let mut store = state.lock()?;
    let Some(device) = store.devices.get_mut(&device_id) else {
        return Err(ApiError::not_found("device not found"));
    };
    if device.user_id != user_id {
        return Err(ApiError::not_found("device not found"));
    }

    let mut changed = false;
    let mut publish_device_changed = false;
    if let Some(name) = request.name {
        if name.trim().is_empty() {
            return Err(ApiError::bad_request("device name is required"));
        }
        if device.name != name {
            device.name = name;
            device.name_version = device.name_version.saturating_add(1);
            changed = true;
            publish_device_changed = true;
        }
    }
    if let Some(disabled) = request.disabled {
        if device.disabled != disabled {
            device.disabled = disabled;
            changed = true;
            publish_device_changed = true;
        }
    }
    if changed {
        device.updated_at = now_seconds();
        device.device_version = device.device_version.saturating_add(1);
    }
    let device = device.clone();
    state.save_locked(&store)?;
    if publish_device_changed {
        state.publish_event(ServerEvent::DeviceChanged {
            user_id,
            device_id,
            device_version: device.device_version,
            name_version: device.name_version,
        });
    }
    Ok(Json(device))
}

#[derive(Debug, Deserialize)]
struct ReauthorizeDeviceRequest {
    public_key: String,
}

async fn reauthorize_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(device_id): AxumPath<Uuid>,
    Json(request): Json<ReauthorizeDeviceRequest>,
) -> Result<Json<Device>, ApiError> {
    let user_id = authorize_or_default(&state, &headers)?;
    if request.public_key.trim().is_empty() {
        return Err(ApiError::bad_request("public key is required"));
    }

    let mut store = state.lock()?;
    let Some(device) = store.devices.get_mut(&device_id) else {
        return Err(ApiError::not_found("device not found"));
    };
    if device.user_id != user_id {
        return Err(ApiError::not_found("device not found"));
    }

    device.public_key = request.public_key;
    device.disabled = false;
    device.updated_at = now_seconds();
    device.device_version = device.device_version.saturating_add(1);
    let device = device.clone();
    state.save_locked(&store)?;
    state.publish_event(ServerEvent::DeviceChanged {
        user_id,
        device_id,
        device_version: device.device_version,
        name_version: device.name_version,
    });
    Ok(Json(device))
}

#[derive(Debug, Serialize)]
struct DeleteDeviceResponse {
    deleted: bool,
}

async fn delete_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(device_id): AxumPath<Uuid>,
) -> Result<Json<DeleteDeviceResponse>, ApiError> {
    let user_id = authorize_or_default(&state, &headers)?;
    let mut store = state.lock()?;
    assert_device_owner(&store, user_id, device_id)?;
    let presence_version = store
        .presence
        .get(&device_id)
        .map_or(1, |presence| presence.presence_version.saturating_add(1));
    store.devices.remove(&device_id);
    store.presence.remove(&device_id);
    store.topologies.values_mut().for_each(|topology| {
        if topology.master_device_id == Some(device_id) {
            topology.master_device_id = None;
            topology.topology_version = topology.topology_version.saturating_add(1);
            topology.updated_at = now_seconds();
        }
        if topology.layout.left == Some(device_id) {
            topology.layout.left = None;
        }
        if topology.layout.right == Some(device_id) {
            topology.layout.right = None;
        }
        if topology.layout.top == Some(device_id) {
            topology.layout.top = None;
        }
        if topology.layout.bottom == Some(device_id) {
            topology.layout.bottom = None;
        }
    });
    store.profiles.retain(|_, profile| {
        profile.source_device_id != device_id && profile.target_device_id != device_id
    });
    state.save_locked(&store)?;
    state.publish_event(ServerEvent::DevicePresenceChanged {
        user_id,
        device_id,
        online: false,
        presence_version,
    });
    Ok(Json(DeleteDeviceResponse { deleted: true }))
}

#[derive(Debug, Deserialize)]
struct HeartbeatRequest {
    lan_ips: Vec<String>,
    listen_port: u16,
    nat_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct HeartbeatResponse {
    online: bool,
    last_seen_at: u64,
    presence_version: u64,
}

async fn heartbeat(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    AxumPath(device_id): AxumPath<Uuid>,
    Json(request): Json<HeartbeatRequest>,
) -> Result<Json<HeartbeatResponse>, ApiError> {
    let user_id = authorize_or_default(&state, &headers)?;
    let mut store = state.lock()?;
    let Some(device) = store.devices.get(&device_id) else {
        return Err(ApiError::not_found("device not found"));
    };
    if device.user_id != user_id {
        return Err(ApiError::not_found("device not found"));
    }
    if device.disabled {
        return Err(ApiError::forbidden("device is disabled"));
    }

    let last_seen_at = now_seconds();
    let nat_type = request.nat_type.unwrap_or_else(|| "unknown".to_string());
    let previous_presence = store.presence.get(&device_id);
    let previous_online =
        previous_presence.is_some_and(|presence| presence_is_online_at(presence, last_seen_at));
    let presence_version = store.presence.get(&device_id).map_or(1, |presence| {
        if previous_online
            && presence.lan_ips == request.lan_ips
            && presence.listen_port == request.listen_port
            && presence.nat_type == nat_type
        {
            presence.presence_version
        } else {
            presence.presence_version.saturating_add(1)
        }
    });
    let should_publish = store.presence.get(&device_id).is_none_or(|presence| {
        !presence_is_online_at(presence, last_seen_at)
            || presence.lan_ips != request.lan_ips
            || presence.listen_port != request.listen_port
            || presence.nat_type != nat_type
    });
    let presence = Presence {
        device_id,
        user_id,
        online: true,
        lan_ips: request.lan_ips,
        public_ip: addr.ip().to_string(),
        listen_port: request.listen_port,
        nat_type,
        last_seen_at,
        expires_at: last_seen_at.saturating_add(PRESENCE_TTL_SECONDS),
        presence_version,
    };
    store.presence.insert(device_id, presence);
    state.save_locked(&store)?;
    if should_publish {
        state.publish_event(ServerEvent::DevicePresenceChanged {
            user_id,
            device_id,
            online: true,
            presence_version,
        });
    }

    Ok(Json(HeartbeatResponse {
        online: true,
        last_seen_at,
        presence_version,
    }))
}

#[derive(Debug, Deserialize)]
struct UpsertTopologyRequest {
    master_device_id: Option<Uuid>,
    #[serde(default)]
    layout: TopologyLayout,
}

async fn get_topology(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Topology>, ApiError> {
    let user_id = authorize_or_default(&state, &headers)?;
    let store = state.lock()?;
    Ok(Json(
        store
            .topologies
            .get(&user_id)
            .cloned()
            .unwrap_or_else(|| Topology {
                user_id,
                master_device_id: None,
                layout: TopologyLayout::default(),
                topology_version: 0,
                updated_at: 0,
            }),
    ))
}

async fn upsert_topology(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpsertTopologyRequest>,
) -> Result<Json<Topology>, ApiError> {
    let user_id = authorize_or_default(&state, &headers)?;
    let mut store = state.lock()?;
    if let Some(master_device_id) = request.master_device_id {
        assert_device_owner(&store, user_id, master_device_id)?;
    }
    for device_id in request.layout.device_ids() {
        assert_device_owner(&store, user_id, device_id)?;
    }

    let now = now_seconds();
    let topology_version = store
        .topologies
        .get(&user_id)
        .map_or(1, |topology| topology.topology_version.saturating_add(1));
    let topology = Topology {
        user_id,
        master_device_id: request.master_device_id,
        layout: request.layout,
        topology_version,
        updated_at: now,
    };
    store.topologies.insert(user_id, topology.clone());
    state.save_locked(&store)?;
    state.publish_event(ServerEvent::TopologyChanged {
        user_id,
        topology_version,
    });
    Ok(Json(topology))
}

#[derive(Debug, Deserialize)]
struct UpsertProfileRequest {
    source_device_id: Uuid,
    target_device_id: Uuid,
    expected_version: Option<u64>,
    config: Value,
}

async fn list_profiles(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<DeviceProfile>>, ApiError> {
    let user_id = authorize_or_default(&state, &headers)?;
    let store = state.lock()?;
    let profiles = store
        .profiles
        .values()
        .filter(|profile| profile.user_id == user_id)
        .cloned()
        .collect();
    Ok(Json(profiles))
}

#[derive(Debug, Deserialize)]
struct ProfileChangesQuery {
    since_revision: Option<u64>,
}

#[derive(Debug, Serialize)]
struct ProfileChangesResponse {
    revision: u64,
    profiles: Vec<DeviceProfile>,
}

async fn list_profile_changes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ProfileChangesQuery>,
) -> Result<Json<ProfileChangesResponse>, ApiError> {
    let user_id = authorize_or_default(&state, &headers)?;
    let since_revision = query.since_revision.unwrap_or(0);
    let store = state.lock()?;
    let mut profiles = store
        .profiles
        .values()
        .filter(|profile| profile.user_id == user_id && profile.revision > since_revision)
        .cloned()
        .collect::<Vec<_>>();
    profiles.sort_by_key(|profile| profile.revision);
    Ok(Json(ProfileChangesResponse {
        revision: store.profile_revision,
        profiles,
    }))
}

async fn upsert_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpsertProfileRequest>,
) -> Result<Json<DeviceProfile>, ApiError> {
    let user_id = authorize_or_default(&state, &headers)?;
    let mut store = state.lock()?;
    assert_device_owner(&store, user_id, request.source_device_id)?;
    assert_device_owner(&store, user_id, request.target_device_id)?;

    let id = profile_id(user_id, request.source_device_id, request.target_device_id);
    let existing = store.profiles.get(&id).cloned();
    if let (Some(expected), Some(existing)) = (request.expected_version, existing.as_ref()) {
        if expected != existing.version {
            return Err(ApiError::conflict("profile version conflict"));
        }
    }
    let version = store
        .profiles
        .get(&id)
        .map_or(1, |profile| profile.version.saturating_add(1));
    if let Some(existing) = existing {
        store
            .profile_history
            .entry(id.clone())
            .or_default()
            .push(existing);
    }
    store.profile_revision = store.profile_revision.saturating_add(1);
    let profile = DeviceProfile {
        id: id.clone(),
        user_id,
        source_device_id: request.source_device_id,
        target_device_id: request.target_device_id,
        config: request.config,
        version,
        revision: store.profile_revision,
        updated_at: now_seconds(),
    };
    store.profiles.insert(id, profile.clone());
    state.save_locked(&store)?;
    state.publish_event(ServerEvent::ProfileChanged {
        user_id,
        profile: profile.clone(),
    });
    Ok(Json(profile))
}

#[derive(Debug, Deserialize)]
struct RollbackProfileRequest {
    profile_id: String,
    target_version: u64,
}

async fn rollback_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RollbackProfileRequest>,
) -> Result<Json<DeviceProfile>, ApiError> {
    let user_id = authorize_or_default(&state, &headers)?;
    let mut store = state.lock()?;
    let current = store
        .profiles
        .get(&request.profile_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("profile not found"))?;
    if current.user_id != user_id {
        return Err(ApiError::not_found("profile not found"));
    }

    let target = profile_version(&store, &request.profile_id, request.target_version)
        .ok_or_else(|| ApiError::not_found("profile version not found"))?;
    if target.user_id != user_id {
        return Err(ApiError::not_found("profile version not found"));
    }

    store
        .profile_history
        .entry(request.profile_id.clone())
        .or_default()
        .push(current.clone());
    store.profile_revision = store.profile_revision.saturating_add(1);
    let profile = DeviceProfile {
        id: current.id.clone(),
        user_id: current.user_id,
        source_device_id: current.source_device_id,
        target_device_id: current.target_device_id,
        config: target.config,
        version: current.version.saturating_add(1),
        revision: store.profile_revision,
        updated_at: now_seconds(),
    };
    store
        .profiles
        .insert(request.profile_id.clone(), profile.clone());
    state.save_locked(&store)?;
    state.publish_event(ServerEvent::ProfileChanged {
        user_id,
        profile: profile.clone(),
    });
    Ok(Json(profile))
}

fn profile_version(store: &Store, profile_id: &str, version: u64) -> Option<DeviceProfile> {
    store
        .profiles
        .get(profile_id)
        .filter(|profile| profile.version == version)
        .cloned()
        .or_else(|| {
            store
                .profile_history
                .get(profile_id)?
                .iter()
                .find(|profile| profile.version == version)
                .cloned()
        })
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<Uuid, ApiError> {
    let token = bearer_token(headers)?;
    authorize_access_token(state, token)
}

fn authorize_or_default(state: &AppState, headers: &HeaderMap) -> Result<Uuid, ApiError> {
    if headers.contains_key("authorization") {
        authorize(state, headers)
    } else {
        Ok(default_user_id())
    }
}

fn authorize_access_token(state: &AppState, token: &str) -> Result<Uuid, ApiError> {
    state
        .lock()?
        .sessions
        .get(token)
        .copied()
        .ok_or_else(|| ApiError::unauthorized("invalid token"))
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, ApiError> {
    let Some(value) = headers.get("authorization") else {
        return Err(ApiError::unauthorized("missing authorization header"));
    };
    let Ok(value) = value.to_str() else {
        return Err(ApiError::unauthorized("invalid authorization header"));
    };
    value
        .strip_prefix("Bearer ")
        .ok_or_else(|| ApiError::unauthorized("expected bearer token"))
}

fn default_user_id() -> Uuid {
    Uuid::from_u128(1)
}

fn normalize_email(email: &str) -> Result<String, ApiError> {
    let email = email.trim().to_ascii_lowercase();
    let Some((local, domain)) = email.split_once('@') else {
        return Err(ApiError::bad_request("valid email is required"));
    };
    if local.is_empty() || domain.is_empty() || domain.contains('@') {
        return Err(ApiError::bad_request("valid email is required"));
    }
    Ok(email)
}

fn new_email_login_code() -> String {
    Uuid::new_v4().simple().to_string()[..8].to_ascii_uppercase()
}

fn get_or_create_user(store: &mut Store, email: &str) -> Uuid {
    if let Some(user_id) = store
        .users
        .values()
        .find(|user| user.email == email)
        .map(|user| user.id)
    {
        return user_id;
    }

    let id = Uuid::new_v4();
    store.users.insert(
        id,
        User {
            id,
            email: email.to_string(),
            created_at: now_seconds(),
        },
    );
    id
}

fn new_access_token(user_id: Uuid) -> String {
    format!("auth-{user_id}-{}", Uuid::new_v4())
}

fn new_refresh_token(user_id: Uuid) -> String {
    format!("refresh-{user_id}-{}", Uuid::new_v4())
}

fn assert_device_owner(store: &Store, user_id: Uuid, device_id: Uuid) -> Result<(), ApiError> {
    match store.devices.get(&device_id) {
        Some(device) if device.user_id == user_id => Ok(()),
        _ => Err(ApiError::not_found("device not found")),
    }
}

fn profile_id(user_id: Uuid, source_device_id: Uuid, target_device_id: Uuid) -> String {
    format!("{user_id}:{source_device_id}:{target_device_id}")
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[derive(Clone)]
enum StorePersistence {
    InMemory,
    JsonFile(PathBuf),
}

impl StorePersistence {
    fn from_options(data_path: Option<PathBuf>) -> Self {
        match data_path {
            Some(path) => Self::JsonFile(path),
            None => Self::InMemory,
        }
    }

    fn load(&self) -> Result<Store, std::io::Error> {
        match self {
            Self::InMemory => Ok(Store::default()),
            Self::JsonFile(path) => load_store(path),
        }
    }

    fn save(&self, store: &Store) -> Result<(), std::io::Error> {
        match self {
            Self::InMemory => Ok(()),
            Self::JsonFile(path) => save_store(path, store),
        }
    }
}

impl AppState {
    fn load(data_path: Option<PathBuf>) -> Result<Self, std::io::Error> {
        let persistence = StorePersistence::from_options(data_path);
        let store = persistence.load()?;
        Ok(Self {
            inner: Arc::new(Mutex::new(store)),
            persistence,
            events: broadcast::channel(256).0,
            relay: RelayHub::default(),
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Store>, ApiError> {
        self.inner
            .lock()
            .map_err(|_| ApiError::internal("store lock poisoned"))
    }

    fn save_locked(&self, store: &Store) -> Result<(), ApiError> {
        self.persistence
            .save(store)
            .map_err(|error| ApiError::internal(&error.to_string()))
    }

    fn subscribe_events(&self) -> broadcast::Receiver<ServerEvent> {
        self.events.subscribe()
    }

    fn publish_event(&self, event: ServerEvent) {
        let _ = self.events.send(event);
    }
}

fn load_store(path: &FsPath) -> Result<Store, std::io::Error> {
    if !path.exists() {
        return Ok(Store::default());
    }
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(std::io::Error::other)
}

fn save_store(path: &FsPath, store: &Store) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("tmp");
    let text = serde_json::to_string_pretty(store).map_err(std::io::Error::other)?;
    fs::write(&tmp_path, text)?;
    fs::rename(tmp_path, path)
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: &str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.to_string(),
        }
    }

    fn unauthorized(message: &str) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.to_string(),
        }
    }

    fn forbidden(message: &str) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.to_string(),
        }
    }

    fn conflict(message: &str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.to_string(),
        }
    }

    fn not_found(message: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.to_string(),
        }
    }

    fn internal(message: &str) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.to_string(),
        }
    }
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(ErrorBody {
                error: self.message,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::extract::ConnectInfo;
    use axum::http::{Method, Request};
    use serde_json::json;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tower::util::ServiceExt;

    fn test_app() -> Router {
        build_app(AppState::load(None).expect("in-memory state"))
    }

    #[test]
    fn relay_client_frame_parser_extracts_target_uuid_and_payload() {
        let target = Uuid::parse_str("22222222-2222-4222-8222-222222222222").expect("uuid");
        let mut payload = Vec::new();
        payload.extend_from_slice(RELAY_FRAME_MAGIC);
        payload.extend_from_slice(target.to_string().as_bytes());
        payload.extend_from_slice(b"encoded-protocol-frame");

        let (parsed_target, frame) =
            parse_relay_client_frame(&payload).expect("relay client frame");

        assert_eq!(parsed_target, target);
        assert_eq!(frame, b"encoded-protocol-frame");
    }

    #[tokio::test]
    async fn relay_hub_routes_payload_to_registered_target_and_unregisters_stale_connection() {
        let hub = RelayHub::default();
        let target = Uuid::parse_str("22222222-2222-4222-8222-222222222222").expect("uuid");
        let connection_id =
            Uuid::parse_str("33333333-3333-4333-8333-333333333333").expect("connection uuid");
        let mut rx = hub.register(target, connection_id);

        hub.send_frame(target, b"frame".to_vec())
            .expect("route relay frame");

        assert_eq!(rx.recv().await, Some(b"frame".to_vec()));
        hub.unregister_if_current(
            target,
            Uuid::parse_str("44444444-4444-4444-8444-444444444444").expect("stale uuid"),
        );
        assert!(hub.send_frame(target, b"still-online".to_vec()).is_ok());
        hub.unregister_if_current(target, connection_id);
        assert_eq!(
            hub.send_frame(target, b"offline".to_vec()),
            Err(RelayRouteError::TargetOffline)
        );
    }

    async fn json_request(
        app: Router,
        method: Method,
        uri: String,
        token: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        let request = builder.body(Body::from(body.to_string())).expect("request");
        let response = app.oneshot(request).await.expect("response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json body")
        };
        (status, body)
    }

    async fn login(app: Router) -> String {
        let body = login_with_body(app).await;
        body["access_token"].as_str().expect("token").to_string()
    }

    async fn login_with_body(app: Router) -> Value {
        login_with_email(app, "tester@example.com").await
    }

    async fn login_with_email(app: Router, email: &str) -> Value {
        let (start_status, start_body) = json_request(
            app.clone(),
            Method::POST,
            "/v1/auth/email/start".to_string(),
            None,
            json!({ "email": email }),
        )
        .await;
        assert_eq!(start_status, StatusCode::OK);
        let code = start_body["code"].as_str().expect("test code");

        let (status, body) = json_request(
            app,
            Method::POST,
            "/v1/auth/email/verify".to_string(),
            None,
            json!({ "email": email, "code": code }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        body
    }

    async fn register_device(app: Router, token: &str, name: &str) -> String {
        let (status, body) = json_request(
            app,
            Method::POST,
            "/v1/devices/register".to_string(),
            Some(token),
            json!({
                "name": name,
                "os_type": "windows",
                "os_version": "11",
                "app_version": "0.1.0",
                "public_key": format!("dev-key-{name}")
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        body["device_id"].as_str().expect("device id").to_string()
    }

    async fn send_heartbeat(
        app: Router,
        token: &str,
        device_id: &str,
        lan_ips: Vec<&str>,
        listen_port: u16,
        nat_type: &str,
        remote_addr: &str,
    ) -> (StatusCode, Value) {
        let mut heartbeat = Request::builder()
            .method(Method::POST)
            .uri(format!("/v1/devices/{device_id}/heartbeat"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(
                json!({
                    "lan_ips": lan_ips,
                    "listen_port": listen_port,
                    "nat_type": nat_type
                })
                .to_string(),
            ))
            .expect("heartbeat request");
        heartbeat.extensions_mut().insert(ConnectInfo(
            remote_addr.parse::<SocketAddr>().expect("remote addr"),
        ));
        let response = app.oneshot(heartbeat).await.expect("heartbeat response");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("heartbeat bytes");
        let body = serde_json::from_slice(&bytes).expect("heartbeat body");
        (status, body)
    }

    #[tokio::test]
    async fn stable_device_registration_is_idempotent() {
        let app = test_app();
        let token = login(app.clone()).await;
        let device_id = "11111111-1111-4111-8111-111111111111";

        let (first_status, first_body) = json_request(
            app.clone(),
            Method::POST,
            "/v1/devices/register".to_string(),
            Some(&token),
            json!({
                "device_id": device_id,
                "name": "first-name",
                "role": "master",
                "os_type": "windows",
                "os_version": "11",
                "app_version": "0.1.0",
                "public_key": "ed25519:first"
            }),
        )
        .await;
        assert_eq!(first_status, StatusCode::OK);
        assert_eq!(first_body["device_id"], device_id);

        let (second_status, second_body) = json_request(
            app.clone(),
            Method::POST,
            "/v1/devices/register".to_string(),
            Some(&token),
            json!({
                "device_id": device_id,
                "name": "local-name-should-not-overwrite-server-name",
                "role": "master",
                "os_type": "windows",
                "os_version": "11",
                "app_version": "0.2.0",
                "public_key": "ed25519:first"
            }),
        )
        .await;
        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(second_body["device_id"], device_id);

        let (list_status, devices) = json_request(
            app,
            Method::GET,
            "/v1/devices".to_string(),
            Some(&token),
            Value::Null,
        )
        .await;
        assert_eq!(list_status, StatusCode::OK);
        assert_eq!(devices.as_array().expect("devices").len(), 1);
        assert_eq!(devices[0]["device"]["id"], device_id);
        assert_eq!(devices[0]["device"]["name"], "first-name");
        assert_eq!(devices[0]["device"]["app_version"], "0.2.0");
        assert_eq!(devices[0]["device"]["role"], "master");
    }

    #[tokio::test]
    async fn stable_device_registration_rejects_public_key_mismatch() {
        let app = test_app();
        let token = login(app.clone()).await;
        let device_id = "22222222-2222-4222-8222-222222222222";

        let (first_status, _) = json_request(
            app.clone(),
            Method::POST,
            "/v1/devices/register".to_string(),
            Some(&token),
            json!({
                "device_id": device_id,
                "name": "desktop",
                "os_type": "windows",
                "os_version": "11",
                "app_version": "0.1.0",
                "public_key": "ed25519:original"
            }),
        )
        .await;
        assert_eq!(first_status, StatusCode::OK);

        let (conflict_status, conflict) = json_request(
            app,
            Method::POST,
            "/v1/devices/register".to_string(),
            Some(&token),
            json!({
                "device_id": device_id,
                "name": "desktop",
                "os_type": "windows",
                "os_version": "11",
                "app_version": "0.1.0",
                "public_key": "ed25519:changed"
            }),
        )
        .await;
        assert_eq!(conflict_status, StatusCode::CONFLICT);
        assert_eq!(conflict["error"], "device public key mismatch");
    }

    #[tokio::test]
    async fn stable_device_registration_reuses_public_key_when_device_id_changes() {
        let app = test_app();
        let token = login(app.clone()).await;
        let first_device_id = "33333333-3333-4333-8333-333333333333";
        let regenerated_device_id = "44444444-4444-4444-8444-444444444444";

        let (first_status, first_body) = json_request(
            app.clone(),
            Method::POST,
            "/v1/devices/register".to_string(),
            Some(&token),
            json!({
                "device_id": first_device_id,
                "name": "Office PC",
                "role": "client",
                "os_type": "windows",
                "os_version": "11",
                "app_version": "0.1.0",
                "public_key": "ed25519:stable-device-key"
            }),
        )
        .await;
        assert_eq!(first_status, StatusCode::OK);
        assert_eq!(first_body["device_id"], first_device_id);

        let (second_status, second_body) = json_request(
            app.clone(),
            Method::POST,
            "/v1/devices/register".to_string(),
            Some(&token),
            json!({
                "device_id": regenerated_device_id,
                "name": "Fresh Install Name",
                "role": "client",
                "os_type": "windows",
                "os_version": "11",
                "app_version": "0.2.0",
                "public_key": "ed25519:stable-device-key"
            }),
        )
        .await;
        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(second_body["device_id"], first_device_id);

        let (list_status, devices) = json_request(
            app,
            Method::GET,
            "/v1/devices".to_string(),
            Some(&token),
            Value::Null,
        )
        .await;
        assert_eq!(list_status, StatusCode::OK);
        assert_eq!(devices.as_array().expect("devices").len(), 1);
        assert_eq!(devices[0]["device"]["id"], first_device_id);
        assert_eq!(devices[0]["device"]["name"], "Office PC");
        assert_eq!(devices[0]["device"]["app_version"], "0.2.0");
    }

    #[tokio::test]
    async fn heartbeat_versions_only_change_when_candidates_change() {
        let app = test_app();
        let token = login(app.clone()).await;
        let device_id = register_device(app.clone(), &token, "desktop").await;

        let (first_status, first) = send_heartbeat(
            app.clone(),
            &token,
            &device_id,
            vec!["192.168.1.10"],
            24_800,
            "open",
            "127.0.0.1:40000",
        )
        .await;
        assert_eq!(first_status, StatusCode::OK);
        assert_eq!(first["presence_version"], 1);

        let (second_status, second) = send_heartbeat(
            app.clone(),
            &token,
            &device_id,
            vec!["192.168.1.10"],
            24_800,
            "open",
            "127.0.0.1:40001",
        )
        .await;
        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(second["presence_version"], 1);

        let (third_status, third) = send_heartbeat(
            app,
            &token,
            &device_id,
            vec!["10.0.0.8", "192.168.50.20"],
            24_999,
            "symmetric",
            "203.0.113.44:41000",
        )
        .await;
        assert_eq!(third_status, StatusCode::OK);
        assert_eq!(third["presence_version"], 2);
    }

    #[tokio::test]
    async fn renaming_device_pushes_device_changed_event() {
        let state = AppState::load(None).expect("in-memory state");
        let app = build_app(state.clone());
        let login = login_with_body(app.clone()).await;
        let token = login["access_token"].as_str().expect("token").to_string();
        let user_id = Uuid::parse_str(login["user_id"].as_str().expect("user id")).expect("uuid");
        let device_id = register_device(app.clone(), &token, "desktop").await;
        let device_uuid = Uuid::parse_str(&device_id).expect("device uuid");
        let mut events = state.subscribe_events();

        let (rename_status, renamed) = json_request(
            app,
            Method::PATCH,
            format!("/v1/devices/{device_id}"),
            Some(&token),
            json!({ "name": "desk-mini" }),
        )
        .await;
        assert_eq!(rename_status, StatusCode::OK);
        assert_eq!(renamed["name"], "desk-mini");
        assert_eq!(renamed["name_version"], 2);

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .expect("device changed event")
            .expect("event");
        assert_eq!(
            event,
            ServerEvent::DeviceChanged {
                user_id,
                device_id: device_uuid,
                device_version: 2,
                name_version: 2,
            }
        );
    }

    #[tokio::test]
    async fn topology_api_persists_master_layout_and_publishes_change() {
        let state = AppState::load(None).expect("in-memory state");
        let app = build_app(state.clone());
        let login = login_with_body(app.clone()).await;
        let token = login["access_token"].as_str().expect("token").to_string();
        let user_id = Uuid::parse_str(login["user_id"].as_str().expect("user id")).expect("uuid");
        let master_id = register_device(app.clone(), &token, "master").await;
        let right_id = register_device(app.clone(), &token, "right").await;
        let mut events = state.subscribe_events();

        let (put_status, topology) = json_request(
            app.clone(),
            Method::PUT,
            "/v1/topology".to_string(),
            Some(&token),
            json!({
                "master_device_id": master_id,
                "layout": {
                    "right": right_id
                }
            }),
        )
        .await;
        assert_eq!(put_status, StatusCode::OK);
        assert_eq!(topology["master_device_id"], master_id);
        assert_eq!(topology["layout"]["right"], right_id);
        assert_eq!(topology["topology_version"], 1);

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .expect("topology event")
            .expect("event");
        assert_eq!(
            event,
            ServerEvent::TopologyChanged {
                user_id,
                topology_version: 1,
            }
        );

        let (get_status, saved) = json_request(
            app,
            Method::GET,
            "/v1/topology".to_string(),
            Some(&token),
            Value::Null,
        )
        .await;
        assert_eq!(get_status, StatusCode::OK);
        assert_eq!(saved["master_device_id"], master_id);
        assert_eq!(saved["layout"]["right"], right_id);
        assert_eq!(saved["topology_version"], 1);
    }

    #[tokio::test]
    async fn device_api_flow_registers_heartbeat_and_lists_presence() {
        let app = test_app();
        let token = login(app.clone()).await;
        let device_id = register_device(app.clone(), &token, "desktop").await;

        let mut heartbeat = Request::builder()
            .method(Method::POST)
            .uri(format!("/v1/devices/{device_id}/heartbeat"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(
                json!({
                    "lan_ips": ["192.168.1.10"],
                    "listen_port": 24800,
                    "nat_type": "open"
                })
                .to_string(),
            ))
            .expect("heartbeat request");
        heartbeat.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:40000".parse::<SocketAddr>().expect("addr"),
        ));

        let heartbeat_response = app
            .clone()
            .oneshot(heartbeat)
            .await
            .expect("heartbeat response");
        assert_eq!(heartbeat_response.status(), StatusCode::OK);

        let (status, body) = json_request(
            app,
            Method::GET,
            "/v1/devices".to_string(),
            Some(&token),
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.as_array().expect("devices").len(), 1);
        assert_eq!(body[0]["device"]["name"], "desktop");
        assert_eq!(body[0]["presence"]["lan_ips"][0], "192.168.1.10");
        assert_eq!(body[0]["presence"]["public_ip"], "127.0.0.1");
    }

    #[tokio::test]
    async fn device_api_flow_accepts_no_auth_for_mvp_desktop_inventory() {
        let app = test_app();
        let (register_status, register_body) = json_request(
            app.clone(),
            Method::POST,
            "/v1/devices/register".to_string(),
            None,
            json!({
                "device_id": "11111111-1111-4111-8111-111111111111",
                "name": "desktop",
                "role": "master",
                "os_type": "macos",
                "os_version": "14",
                "app_version": "0.1.0",
                "public_key": "dev-key"
            }),
        )
        .await;
        assert_eq!(register_status, StatusCode::OK);
        let device_id = register_body["device_id"].as_str().expect("device id");

        let mut heartbeat = Request::builder()
            .method(Method::POST)
            .uri(format!("/v1/devices/{device_id}/heartbeat"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "lan_ips": ["192.168.1.10"],
                    "listen_port": 24800,
                    "nat_type": "open"
                })
                .to_string(),
            ))
            .expect("heartbeat request");
        heartbeat.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:40000".parse::<SocketAddr>().expect("addr"),
        ));
        let heartbeat_response = app
            .clone()
            .oneshot(heartbeat)
            .await
            .expect("heartbeat response");
        assert_eq!(heartbeat_response.status(), StatusCode::OK);

        let (list_status, devices) = json_request(
            app,
            Method::GET,
            "/v1/devices".to_string(),
            None,
            Value::Null,
        )
        .await;
        assert_eq!(list_status, StatusCode::OK);
        assert_eq!(devices.as_array().expect("devices").len(), 1);
        assert_eq!(devices[0]["device"]["name"], "desktop");
        assert_eq!(devices[0]["presence"]["lan_ips"][0], "192.168.1.10");
    }

    #[tokio::test]
    async fn heartbeat_refreshes_existing_device_connection_details() {
        let app = test_app();
        let token = login(app.clone()).await;
        let device_id = register_device(app.clone(), &token, "desktop").await;

        let mut first_heartbeat = Request::builder()
            .method(Method::POST)
            .uri(format!("/v1/devices/{device_id}/heartbeat"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(
                json!({
                    "lan_ips": ["192.168.1.10"],
                    "listen_port": 24800,
                    "nat_type": "open"
                })
                .to_string(),
            ))
            .expect("heartbeat request");
        first_heartbeat.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:40000".parse::<SocketAddr>().expect("addr"),
        ));

        let first_response = app
            .clone()
            .oneshot(first_heartbeat)
            .await
            .expect("first heartbeat response");
        assert_eq!(first_response.status(), StatusCode::OK);

        let mut second_heartbeat = Request::builder()
            .method(Method::POST)
            .uri(format!("/v1/devices/{device_id}/heartbeat"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(
                json!({
                    "lan_ips": ["10.0.0.8", "192.168.50.20"],
                    "listen_port": 24999,
                    "nat_type": "symmetric"
                })
                .to_string(),
            ))
            .expect("heartbeat request");
        second_heartbeat.extensions_mut().insert(ConnectInfo(
            "203.0.113.44:41000".parse::<SocketAddr>().expect("addr"),
        ));

        let second_response = app
            .clone()
            .oneshot(second_heartbeat)
            .await
            .expect("second heartbeat response");
        assert_eq!(second_response.status(), StatusCode::OK);

        let (status, devices) = json_request(
            app,
            Method::GET,
            "/v1/devices".to_string(),
            Some(&token),
            Value::Null,
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(devices.as_array().expect("devices").len(), 1);
        assert_eq!(devices[0]["device"]["id"], device_id);
        assert_eq!(devices[0]["presence"]["lan_ips"][0], "10.0.0.8");
        assert_eq!(devices[0]["presence"]["lan_ips"][1], "192.168.50.20");
        assert_eq!(devices[0]["presence"]["public_ip"], "203.0.113.44");
        assert_eq!(devices[0]["presence"]["listen_port"], 24999);
        assert_eq!(devices[0]["presence"]["nat_type"], "symmetric");
    }

    #[tokio::test]
    async fn device_list_marks_expired_presence_offline() {
        let state = AppState::load(None).expect("in-memory state");
        let app = build_app(state.clone());
        let token = login(app.clone()).await;
        let device_id = register_device(app.clone(), &token, "desktop").await;
        let (heartbeat_status, _) = send_heartbeat(
            app.clone(),
            &token,
            &device_id,
            vec!["192.168.1.10"],
            24800,
            "open",
            "127.0.0.1:40000",
        )
        .await;
        assert_eq!(heartbeat_status, StatusCode::OK);

        let expired_at = now_seconds().saturating_sub(1);
        {
            let mut store = state.lock().expect("store");
            let presence = store
                .presence
                .get_mut(&Uuid::parse_str(&device_id).expect("device uuid"))
                .expect("presence");
            presence.online = true;
            presence.last_seen_at = expired_at.saturating_sub(60);
            presence.expires_at = expired_at;
        }

        let (list_status, devices) = json_request(
            app,
            Method::GET,
            "/v1/devices".to_string(),
            Some(&token),
            Value::Null,
        )
        .await;

        assert_eq!(list_status, StatusCode::OK);
        assert_eq!(devices.as_array().expect("devices").len(), 1);
        assert_eq!(devices[0]["presence"]["online"], false);
        assert_eq!(devices[0]["presence"]["lan_ips"][0], "192.168.1.10");
    }

    #[tokio::test]
    async fn reconnect_after_expiry_publishes_online_presence_change() {
        let state = AppState::load(None).expect("in-memory state");
        let app = build_app(state.clone());
        let token = login(app.clone()).await;
        let device_id = register_device(app.clone(), &token, "desktop").await;
        let (heartbeat_status, first) = send_heartbeat(
            app.clone(),
            &token,
            &device_id,
            vec!["192.168.1.10"],
            24800,
            "open",
            "127.0.0.1:40000",
        )
        .await;
        assert_eq!(heartbeat_status, StatusCode::OK);
        assert_eq!(first["presence_version"], 1);

        let expired_at = now_seconds().saturating_sub(1);
        {
            let mut store = state.lock().expect("store");
            let presence = store
                .presence
                .get_mut(&Uuid::parse_str(&device_id).expect("device uuid"))
                .expect("presence");
            presence.online = true;
            presence.last_seen_at = expired_at.saturating_sub(60);
            presence.expires_at = expired_at;
        }

        let mut events = state.subscribe_events();
        let (reconnect_status, reconnect) = send_heartbeat(
            app,
            &token,
            &device_id,
            vec!["192.168.1.10"],
            24800,
            "open",
            "127.0.0.1:40000",
        )
        .await;

        assert_eq!(reconnect_status, StatusCode::OK);
        assert_eq!(reconnect["presence_version"], 2);
        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("presence event")
            .expect("event");
        match event {
            ServerEvent::DevicePresenceChanged {
                device_id: changed_device_id,
                online,
                presence_version,
                ..
            } => {
                assert_eq!(changed_device_id.to_string(), device_id);
                assert!(online);
                assert_eq!(presence_version, 2);
            }
            other => panic!("expected presence change event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn email_login_replaces_dev_login_with_verified_code_flow() {
        let app = test_app();

        let (start_status, start_body) = json_request(
            app.clone(),
            Method::POST,
            "/v1/auth/email/start".to_string(),
            None,
            json!({ "email": " Tester@Example.COM " }),
        )
        .await;
        assert_eq!(start_status, StatusCode::OK);
        assert_eq!(start_body["email"], "tester@example.com");
        let code = start_body["code"].as_str().expect("test code");

        let (wrong_status, wrong_body) = json_request(
            app.clone(),
            Method::POST,
            "/v1/auth/email/verify".to_string(),
            None,
            json!({ "email": "tester@example.com", "code": "WRONG" }),
        )
        .await;
        assert_eq!(wrong_status, StatusCode::UNAUTHORIZED);
        assert_eq!(wrong_body["error"], "invalid email code");

        let (verify_status, verify_body) = json_request(
            app.clone(),
            Method::POST,
            "/v1/auth/email/verify".to_string(),
            None,
            json!({ "email": "tester@example.com", "code": code }),
        )
        .await;
        assert_eq!(verify_status, StatusCode::OK);
        assert!(verify_body["access_token"]
            .as_str()
            .expect("access token")
            .starts_with("auth-"));
        assert!(verify_body["refresh_token"]
            .as_str()
            .expect("refresh token")
            .starts_with("refresh-"));

        let (dev_status, _) = json_request(
            app,
            Method::POST,
            "/v1/auth/dev-login".to_string(),
            None,
            json!({ "email": "tester@example.com" }),
        )
        .await;
        assert_eq!(dev_status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn device_presence_changes_are_pushed_to_subscribers() {
        let state = AppState::load(None).expect("in-memory state");
        let app = build_app(state.clone());
        let login = login_with_body(app.clone()).await;
        let token = login["access_token"].as_str().expect("token").to_string();
        let user_id = Uuid::parse_str(login["user_id"].as_str().expect("user id")).expect("uuid");
        let device_id = register_device(app.clone(), &token, "desktop").await;
        let device_uuid = Uuid::parse_str(&device_id).expect("device uuid");
        let mut events = state.subscribe_events();

        let mut heartbeat = Request::builder()
            .method(Method::POST)
            .uri(format!("/v1/devices/{device_id}/heartbeat"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(
                json!({
                    "lan_ips": ["192.168.1.10"],
                    "listen_port": 24800,
                    "nat_type": "open"
                })
                .to_string(),
            ))
            .expect("heartbeat request");
        heartbeat.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:40000".parse::<SocketAddr>().expect("addr"),
        ));
        let heartbeat_response = app
            .clone()
            .oneshot(heartbeat)
            .await
            .expect("heartbeat response");
        assert_eq!(heartbeat_response.status(), StatusCode::OK);

        let online_event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .expect("online event")
            .expect("event");
        assert_eq!(
            online_event,
            ServerEvent::DevicePresenceChanged {
                user_id,
                device_id: device_uuid,
                online: true,
                presence_version: 1,
            }
        );

        let (delete_status, _) = json_request(
            app,
            Method::DELETE,
            format!("/v1/devices/{device_id}"),
            Some(&token),
            Value::Null,
        )
        .await;
        assert_eq!(delete_status, StatusCode::OK);

        let offline_event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .expect("offline event")
            .expect("event");
        assert_eq!(
            offline_event,
            ServerEvent::DevicePresenceChanged {
                user_id,
                device_id: device_uuid,
                online: false,
                presence_version: 2,
            }
        );
    }

    #[tokio::test]
    async fn device_registration_persists_public_key_and_binds_it_to_user() {
        let app = test_app();
        let alice = login_with_email(app.clone(), "alice@example.com").await;
        let bob = login_with_email(app.clone(), "bob@example.com").await;
        let alice_token = alice["access_token"].as_str().expect("alice token");
        let bob_token = bob["access_token"].as_str().expect("bob token");

        let (register_status, _) = json_request(
            app.clone(),
            Method::POST,
            "/v1/devices/register".to_string(),
            Some(alice_token),
            json!({
                "name": "alice-desktop",
                "os_type": "windows",
                "os_version": "11",
                "app_version": "0.1.0",
                "public_key": "alice-public-key"
            }),
        )
        .await;
        assert_eq!(register_status, StatusCode::OK);

        let (alice_status, alice_devices) = json_request(
            app.clone(),
            Method::GET,
            "/v1/devices".to_string(),
            Some(alice_token),
            Value::Null,
        )
        .await;
        let (bob_status, bob_devices) = json_request(
            app,
            Method::GET,
            "/v1/devices".to_string(),
            Some(bob_token),
            Value::Null,
        )
        .await;

        assert_eq!(alice_status, StatusCode::OK);
        assert_eq!(alice_devices.as_array().expect("alice devices").len(), 1);
        assert_eq!(alice_devices[0]["device"]["public_key"], "alice-public-key");
        assert_eq!(bob_status, StatusCode::OK);
        assert!(bob_devices.as_array().expect("bob devices").is_empty());
    }

    #[tokio::test]
    async fn profile_api_flow_upserts_and_increments_versions() {
        let app = test_app();
        let token = login(app.clone()).await;
        let source = register_device(app.clone(), &token, "source").await;
        let target = register_device(app.clone(), &token, "target").await;

        let body = json!({
            "source_device_id": source,
            "target_device_id": target,
            "config": {
                "source_os": "macos",
                "target_os": "windows",
                "preset": "keep_mac_habit",
                "modifier_mapping": { "left_meta": "left_control" },
                "scroll": { "vertical_multiplier": -1.0, "horizontal_multiplier": -1.0 },
                "pointer": { "speed_multiplier": 1.0 }
            }
        });

        let (first_status, first_body) = json_request(
            app.clone(),
            Method::PUT,
            "/v1/profiles".to_string(),
            Some(&token),
            body.clone(),
        )
        .await;
        let (second_status, second_body) = json_request(
            app.clone(),
            Method::PUT,
            "/v1/profiles".to_string(),
            Some(&token),
            body,
        )
        .await;
        let (list_status, list_body) = json_request(
            app,
            Method::GET,
            "/v1/profiles".to_string(),
            Some(&token),
            Value::Null,
        )
        .await;

        assert_eq!(first_status, StatusCode::OK);
        assert_eq!(first_body["version"], 1);
        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(second_body["version"], 2);
        assert_eq!(list_status, StatusCode::OK);
        assert_eq!(list_body.as_array().expect("profiles").len(), 1);
        assert_eq!(list_body[0]["version"], 2);
    }

    #[tokio::test]
    async fn profile_changes_api_returns_profiles_after_revision_cursor() {
        let app = test_app();
        let token = login(app.clone()).await;
        let source = register_device(app.clone(), &token, "source").await;
        let target = register_device(app.clone(), &token, "target").await;

        let body = json!({
            "source_device_id": source,
            "target_device_id": target,
            "config": {
                "source_os": "macos",
                "target_os": "windows",
                "preset": "keep_mac_habit",
                "modifier_mapping": { "left_meta": "left_control" },
                "scroll": { "vertical_multiplier": -1.0, "horizontal_multiplier": -1.0 },
                "pointer": { "speed_multiplier": 1.0 }
            }
        });

        let (first_status, first_body) = json_request(
            app.clone(),
            Method::PUT,
            "/v1/profiles".to_string(),
            Some(&token),
            body.clone(),
        )
        .await;
        assert_eq!(first_status, StatusCode::OK);
        let first_revision = first_body["revision"].as_u64().expect("revision");

        let (initial_status, initial_changes) = json_request(
            app.clone(),
            Method::GET,
            "/v1/profiles/changes?since_revision=0".to_string(),
            Some(&token),
            Value::Null,
        )
        .await;
        assert_eq!(initial_status, StatusCode::OK);
        assert_eq!(initial_changes["revision"], first_revision);
        assert_eq!(
            initial_changes["profiles"]
                .as_array()
                .expect("profiles")
                .len(),
            1
        );

        let (second_status, second_body) = json_request(
            app.clone(),
            Method::PUT,
            "/v1/profiles".to_string(),
            Some(&token),
            body,
        )
        .await;
        assert_eq!(second_status, StatusCode::OK);
        let second_revision = second_body["revision"].as_u64().expect("revision");

        let (delta_status, delta_changes) = json_request(
            app,
            Method::GET,
            format!("/v1/profiles/changes?since_revision={first_revision}"),
            Some(&token),
            Value::Null,
        )
        .await;
        assert_eq!(delta_status, StatusCode::OK);
        assert_eq!(delta_changes["revision"], second_revision);
        assert_eq!(
            delta_changes["profiles"]
                .as_array()
                .expect("profiles")
                .len(),
            1
        );
        assert_eq!(delta_changes["profiles"][0]["version"], 2);
    }

    #[tokio::test]
    async fn profile_upsert_pushes_config_change_event_to_subscribers() {
        let state = AppState::load(None).expect("in-memory state");
        let app = build_app(state.clone());
        let login = login_with_body(app.clone()).await;
        let token = login["access_token"].as_str().expect("token").to_string();
        let user_id = Uuid::parse_str(login["user_id"].as_str().expect("user id")).expect("uuid");
        let source = register_device(app.clone(), &token, "source").await;
        let target = register_device(app.clone(), &token, "target").await;
        let mut events = state.subscribe_events();

        let (status, profile) = json_request(
            app,
            Method::PUT,
            "/v1/profiles".to_string(),
            Some(&token),
            json!({
                "source_device_id": source,
                "target_device_id": target,
                "config": {
                    "source_os": "macos",
                    "target_os": "windows",
                    "preset": "keep_mac_habit",
                    "modifier_mapping": { "left_meta": "left_control" },
                    "scroll": { "vertical_multiplier": -1.0, "horizontal_multiplier": -1.0 },
                    "pointer": { "speed_multiplier": 1.0 }
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .expect("profile change event")
            .expect("event");
        assert_eq!(
            event,
            ServerEvent::ProfileChanged {
                user_id,
                profile: serde_json::from_value(profile).expect("profile json")
            }
        );
    }

    #[tokio::test]
    async fn profile_upsert_detects_version_conflict_and_rolls_back() {
        let app = test_app();
        let token = login(app.clone()).await;
        let source = register_device(app.clone(), &token, "source").await;
        let target = register_device(app.clone(), &token, "target").await;
        let base_config = json!({
            "source_os": "macos",
            "target_os": "windows",
            "preset": "keep_mac_habit",
            "modifier_mapping": { "left_meta": "left_control" },
            "scroll": { "vertical_multiplier": -1.0, "horizontal_multiplier": -1.0 },
            "pointer": { "speed_multiplier": 1.0 }
        });
        let updated_config = json!({
            "source_os": "macos",
            "target_os": "windows",
            "preset": "custom",
            "modifier_mapping": { "left_meta": "left_control", "right_meta": "right_control" },
            "scroll": { "vertical_multiplier": -1.0, "horizontal_multiplier": -1.0 },
            "pointer": { "speed_multiplier": 1.25 }
        });

        let (first_status, first_profile) = json_request(
            app.clone(),
            Method::PUT,
            "/v1/profiles".to_string(),
            Some(&token),
            json!({
                "source_device_id": source,
                "target_device_id": target,
                "config": base_config
            }),
        )
        .await;
        assert_eq!(first_status, StatusCode::OK);
        let profile_id = first_profile["id"].as_str().expect("profile id");

        let (second_status, second_profile) = json_request(
            app.clone(),
            Method::PUT,
            "/v1/profiles".to_string(),
            Some(&token),
            json!({
                "source_device_id": source,
                "target_device_id": target,
                "expected_version": 1,
                "config": updated_config
            }),
        )
        .await;
        assert_eq!(second_status, StatusCode::OK);
        assert_eq!(second_profile["version"], 2);

        let (conflict_status, conflict_body) = json_request(
            app.clone(),
            Method::PUT,
            "/v1/profiles".to_string(),
            Some(&token),
            json!({
                "source_device_id": source,
                "target_device_id": target,
                "expected_version": 1,
                "config": {
                    "source_os": "macos",
                    "target_os": "windows",
                    "preset": "custom",
                    "modifier_mapping": {},
                    "scroll": { "vertical_multiplier": 1.0, "horizontal_multiplier": 1.0 },
                    "pointer": { "speed_multiplier": 1.0 }
                }
            }),
        )
        .await;
        assert_eq!(conflict_status, StatusCode::CONFLICT);
        assert_eq!(conflict_body["error"], "profile version conflict");

        let (rollback_status, rollback_profile) = json_request(
            app,
            Method::POST,
            "/v1/profiles/rollback".to_string(),
            Some(&token),
            json!({
                "profile_id": profile_id,
                "target_version": 1
            }),
        )
        .await;
        assert_eq!(rollback_status, StatusCode::OK);
        assert_eq!(rollback_profile["version"], 3);
        assert_eq!(rollback_profile["config"]["preset"], "keep_mac_habit");
        assert_eq!(
            rollback_profile["config"]["pointer"]["speed_multiplier"],
            1.0
        );
    }

    #[tokio::test]
    async fn version_check_reports_update_policy_and_rollout() {
        let app = test_app();

        let (old_status, old_policy) = json_request(
            app.clone(),
            Method::GET,
            "/v1/releases/check?platform=windows&version=0.1.0&channel=stable&device_id=windows-device".to_string(),
            None,
            Value::Null,
        )
        .await;
        assert_eq!(old_status, StatusCode::OK);
        assert_eq!(old_policy["latest_version"], "0.2.0");
        assert_eq!(old_policy["min_supported_version"], "0.1.0");
        assert_eq!(old_policy["update_available"], true);
        assert_eq!(old_policy["force_update"], false);
        assert_eq!(old_policy["rollout_percent"], 25);
        assert_eq!(old_policy["rollout_bucket"], 13);
        assert_eq!(old_policy["rollout_eligible"], true);
        assert_eq!(old_policy["auto_update_action"], "download");
        assert_eq!(
            old_policy["download_url"],
            "https://updates.example.invalid/kmsync/windows/0.2.0"
        );
        assert_eq!(
            old_policy["installer_sha256"],
            "a9efb60b6ff3bf8b42fd2506c6f3e8b4c345bd974f4685db963918f6a1a26158"
        );
        assert_eq!(
            old_policy["signature_url"],
            "https://updates.example.invalid/kmsync/windows/0.2.0/kmsync-installer.sig"
        );

        let (held_status, held_policy) = json_request(
            app.clone(),
            Method::GET,
            "/v1/releases/check?platform=windows&version=0.1.0&channel=stable&device_id=rollout-device-beta".to_string(),
            None,
            Value::Null,
        )
        .await;
        assert_eq!(held_status, StatusCode::OK);
        assert_eq!(held_policy["rollout_bucket"], 86);
        assert_eq!(held_policy["rollout_eligible"], false);
        assert_eq!(held_policy["auto_update_action"], "wait_for_rollout");

        let (unsupported_status, unsupported_policy) = json_request(
            app.clone(),
            Method::GET,
            "/v1/releases/check?platform=windows&version=0.0.9&channel=stable&device_id=rollout-device-beta".to_string(),
            None,
            Value::Null,
        )
        .await;
        assert_eq!(unsupported_status, StatusCode::OK);
        assert_eq!(unsupported_policy["force_update"], true);
        assert_eq!(unsupported_policy["rollout_eligible"], true);
        assert_eq!(unsupported_policy["auto_update_action"], "force");

        let (current_status, current_policy) = json_request(
            app,
            Method::GET,
            "/v1/releases/check?platform=windows&version=0.2.0&channel=stable&device_id=windows-device".to_string(),
            None,
            Value::Null,
        )
        .await;
        assert_eq!(current_status, StatusCode::OK);
        assert_eq!(current_policy["update_available"], false);
        assert_eq!(current_policy["auto_update_action"], "none");
    }

    #[tokio::test]
    async fn relay_token_api_issues_token_for_owned_devices_and_schedules_region() {
        let app = test_app();
        let token = login(app.clone()).await;
        let source = register_device(app.clone(), &token, "source").await;
        let target = register_device(app.clone(), &token, "target").await;

        let (status, relay) = json_request(
            app.clone(),
            Method::POST,
            "/v1/relay/token".to_string(),
            Some(&token),
            json!({
                "source_device_id": source,
                "target_device_id": target,
                "preferred_region": "us-west"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(relay["region"], "us-west");
        assert_eq!(relay["relay_url"], "relay://us-west.relay.kmsync.local:443");
        assert!(relay["expires_at"].as_u64().expect("expiry") > 0);
        assert!(relay["relay_token"]
            .as_str()
            .expect("token")
            .starts_with("relay-"));

        let other_login = login_with_email(app.clone(), "other@example.com").await;
        let other_token = other_login["access_token"].as_str().expect("other token");
        let (forbidden_status, forbidden) = json_request(
            app,
            Method::POST,
            "/v1/relay/token".to_string(),
            Some(other_token),
            json!({
                "source_device_id": source,
                "target_device_id": target
            }),
        )
        .await;

        assert_eq!(forbidden_status, StatusCode::NOT_FOUND);
        assert_eq!(forbidden["error"], "device not found");
    }

    #[tokio::test]
    async fn signaling_flow_requests_accepts_adds_candidates_rejects_and_closes_sessions() {
        let app = test_app();
        let token = login(app.clone()).await;
        let source = register_device(app.clone(), &token, "source").await;
        let target = register_device(app.clone(), &token, "target").await;

        let (request_status, requested) = json_request(
            app.clone(),
            Method::POST,
            "/v1/signal/connect.request".to_string(),
            Some(&token),
            json!({
                "source_device_id": source,
                "target_device_id": target
            }),
        )
        .await;
        assert_eq!(request_status, StatusCode::OK);
        assert_eq!(requested["status"], "requested");
        assert!(requested.get("input_event").is_none());
        assert!(requested.get("clipboard").is_none());
        let session_id = requested["id"].as_str().expect("session id");

        let (candidate_status, candidate_session) = json_request(
            app.clone(),
            Method::POST,
            "/v1/signal/candidate.add".to_string(),
            Some(&token),
            json!({
                "session_id": session_id,
                "candidate": {
                    "transport": "lan",
                    "address": "192.168.1.10:24800",
                    "priority": 100
                }
            }),
        )
        .await;
        assert_eq!(candidate_status, StatusCode::OK);
        assert_eq!(
            candidate_session["candidates"][0]["address"],
            "192.168.1.10:24800"
        );

        let (accept_status, accepted) = json_request(
            app.clone(),
            Method::POST,
            "/v1/signal/connect.accept".to_string(),
            Some(&token),
            json!({ "session_id": session_id }),
        )
        .await;
        assert_eq!(accept_status, StatusCode::OK);
        assert_eq!(accepted["status"], "accepted");

        let (close_status, closed) = json_request(
            app.clone(),
            Method::POST,
            "/v1/signal/session.close".to_string(),
            Some(&token),
            json!({ "session_id": session_id }),
        )
        .await;
        assert_eq!(close_status, StatusCode::OK);
        assert_eq!(closed["status"], "closed");

        let (second_status, second) = json_request(
            app.clone(),
            Method::POST,
            "/v1/signal/connect.request".to_string(),
            Some(&token),
            json!({
                "source_device_id": source,
                "target_device_id": target
            }),
        )
        .await;
        assert_eq!(second_status, StatusCode::OK);
        let second_session_id = second["id"].as_str().expect("second session id");

        let (reject_status, rejected) = json_request(
            app,
            Method::POST,
            "/v1/signal/connect.reject".to_string(),
            Some(&token),
            json!({ "session_id": second_session_id }),
        )
        .await;
        assert_eq!(reject_status, StatusCode::OK);
        assert_eq!(rejected["status"], "rejected");
    }

    #[tokio::test]
    async fn signaling_changes_are_pushed_to_event_stream_subscribers() {
        let state = AppState::load(None).expect("in-memory state");
        let app = build_app(state.clone());
        let login = login_with_body(app.clone()).await;
        let token = login["access_token"].as_str().expect("token").to_string();
        let user_id = Uuid::parse_str(login["user_id"].as_str().expect("user id")).expect("uuid");
        let source = register_device(app.clone(), &token, "source").await;
        let target = register_device(app.clone(), &token, "target").await;
        let mut events = state.subscribe_events();

        let (request_status, requested) = json_request(
            app.clone(),
            Method::POST,
            "/v1/signal/connect.request".to_string(),
            Some(&token),
            json!({
                "source_device_id": source,
                "target_device_id": target
            }),
        )
        .await;
        assert_eq!(request_status, StatusCode::OK);
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
                .await
                .expect("requested signal event")
                .expect("event"),
            ServerEvent::SignalSessionChanged {
                user_id,
                session: serde_json::from_value(requested.clone()).expect("signal session")
            }
        );
        let session_id = requested["id"].as_str().expect("session id");

        let (candidate_status, candidate_session) = json_request(
            app.clone(),
            Method::POST,
            "/v1/signal/candidate.add".to_string(),
            Some(&token),
            json!({
                "session_id": session_id,
                "candidate": {
                    "transport": "lan",
                    "address": "192.168.1.10:24800",
                    "priority": 100
                }
            }),
        )
        .await;
        assert_eq!(candidate_status, StatusCode::OK);
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
                .await
                .expect("candidate signal event")
                .expect("event"),
            ServerEvent::SignalSessionChanged {
                user_id,
                session: serde_json::from_value(candidate_session.clone())
                    .expect("candidate session")
            }
        );

        let (accept_status, accepted) = json_request(
            app,
            Method::POST,
            "/v1/signal/connect.accept".to_string(),
            Some(&token),
            json!({ "session_id": session_id }),
        )
        .await;
        assert_eq!(accept_status, StatusCode::OK);
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
                .await
                .expect("accepted signal event")
                .expect("event"),
            ServerEvent::SignalSessionChanged {
                user_id,
                session: serde_json::from_value(accepted).expect("accepted session")
            }
        );
    }

    #[tokio::test]
    async fn auth_flow_refreshes_access_token_and_logs_out_session() {
        let app = test_app();
        let login = login_with_body(app.clone()).await;
        let access_token = login["access_token"].as_str().expect("access token");
        let refresh_token = login["refresh_token"].as_str().expect("refresh token");

        let (refresh_status, refresh_body) = json_request(
            app.clone(),
            Method::POST,
            "/v1/auth/refresh".to_string(),
            None,
            json!({ "refresh_token": refresh_token }),
        )
        .await;
        assert_eq!(refresh_status, StatusCode::OK);
        let refreshed_access = refresh_body["access_token"]
            .as_str()
            .expect("refreshed access token");
        assert_ne!(refreshed_access, access_token);

        let (logout_status, logout_body) = json_request(
            app.clone(),
            Method::POST,
            "/v1/auth/logout".to_string(),
            Some(access_token),
            json!({ "refresh_token": refresh_token }),
        )
        .await;
        assert_eq!(logout_status, StatusCode::OK);
        assert_eq!(logout_body["logged_out"], true);

        let (old_status, _) = json_request(
            app.clone(),
            Method::GET,
            "/v1/devices".to_string(),
            Some(access_token),
            Value::Null,
        )
        .await;
        let (new_status, new_devices) = json_request(
            app,
            Method::GET,
            "/v1/devices".to_string(),
            Some(refreshed_access),
            Value::Null,
        )
        .await;
        assert_eq!(old_status, StatusCode::UNAUTHORIZED);
        assert_eq!(new_status, StatusCode::OK);
        assert!(new_devices.as_array().expect("devices").is_empty());
    }

    #[tokio::test]
    async fn local_ui_cors_preflight_allows_json_api_requests() {
        let response = test_app()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/v1/auth/email/start")
                    .header("origin", "http://127.0.0.1:24900")
                    .header("access-control-request-method", "POST")
                    .header("access-control-request-headers", "content-type")
                    .body(Body::empty())
                    .expect("preflight request"),
            )
            .await
            .expect("preflight response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            "http://127.0.0.1:24900"
        );
        assert!(response.headers()["access-control-allow-methods"]
            .to_str()
            .expect("methods")
            .contains("POST"));
    }

    #[test]
    fn local_file_persistence_is_selected_from_data_path() {
        let backend = StorePersistence::from_options(Some(PathBuf::from("server-state.json")));

        assert!(matches!(backend, StorePersistence::JsonFile(_)));
    }

    #[test]
    fn server_config_rejects_database_and_redis_options() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("kmsync-server-obsolete-{suffix}.json"));
        fs::write(
            &path,
            r#"{
                "bind": "0.0.0.0:24888",
                "data_path": "/var/lib/kmsync/server-state.json",
                "mysql_url": "mysql://kmsync:CHANGE_ME@127.0.0.1:3306/kmsync",
                "redis_url": "redis://127.0.0.1:6379"
            }"#,
        )
        .expect("write config");

        let error = ServerConfig::load(&path).expect_err("obsolete config keys are rejected");

        assert!(error.to_string().contains("unknown field"));
        fs::remove_file(path).expect("remove config");
    }

    #[test]
    fn server_config_loads_file_only_runtime_options_from_json_file() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("kmsync-server-{suffix}.json"));
        fs::write(
            &path,
            r#"{
                "bind": "0.0.0.0:24888",
                "data_path": "/var/lib/kmsync/server-state.json"
            }"#,
        )
        .expect("write config");

        let config = ServerConfig::load(&path).expect("load config");

        assert_eq!(config.bind, "0.0.0.0:24888".parse().expect("bind"));
        assert_eq!(
            config.data_path,
            PathBuf::from("/var/lib/kmsync/server-state.json")
        );

        fs::remove_file(path).expect("remove config");
    }

    #[tokio::test]
    async fn local_file_persists_devices_sessions_and_presence_across_restarts() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let state_path = std::env::temp_dir().join(format!("kmsync-state-{suffix}.json"));
        let first_app = build_app(AppState::load(Some(state_path.clone())).expect("state"));
        let token = login(first_app.clone()).await;
        let device_id = register_device(first_app.clone(), &token, "desktop").await;

        let mut heartbeat = Request::builder()
            .method(Method::POST)
            .uri(format!("/v1/devices/{device_id}/heartbeat"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(
                json!({
                    "lan_ips": ["172.16.0.8"],
                    "listen_port": 24888,
                    "nat_type": "open"
                })
                .to_string(),
            ))
            .expect("heartbeat request");
        heartbeat.extensions_mut().insert(ConnectInfo(
            "198.51.100.21:42000".parse::<SocketAddr>().expect("addr"),
        ));
        assert_eq!(
            first_app
                .oneshot(heartbeat)
                .await
                .expect("heartbeat response")
                .status(),
            StatusCode::OK
        );

        let restarted_app = build_app(AppState::load(Some(state_path.clone())).expect("state"));
        let (status, devices) = json_request(
            restarted_app,
            Method::GET,
            "/v1/devices".to_string(),
            Some(&token),
            Value::Null,
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(devices.as_array().expect("devices").len(), 1);
        assert_eq!(devices[0]["device"]["id"], device_id);
        assert_eq!(devices[0]["presence"]["lan_ips"][0], "172.16.0.8");
        assert_eq!(devices[0]["presence"]["public_ip"], "198.51.100.21");
        assert_eq!(devices[0]["presence"]["listen_port"], 24888);
        assert_eq!(devices[0]["presence"]["nat_type"], "open");

        fs::remove_file(state_path).expect("remove state file");
    }

    #[tokio::test]
    async fn device_management_flow_renames_disables_reauthorizes_and_unbinds() {
        let app = test_app();
        let token = login(app.clone()).await;
        let device_id = register_device(app.clone(), &token, "desktop").await;

        let (rename_status, renamed) = json_request(
            app.clone(),
            Method::PATCH,
            format!("/v1/devices/{device_id}"),
            Some(&token),
            json!({ "name": "desk-mini", "disabled": true }),
        )
        .await;
        assert_eq!(rename_status, StatusCode::OK);
        assert_eq!(renamed["name"], "desk-mini");
        assert_eq!(renamed["disabled"], true);

        let mut heartbeat = Request::builder()
            .method(Method::POST)
            .uri(format!("/v1/devices/{device_id}/heartbeat"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(
                json!({
                    "lan_ips": ["192.168.1.10"],
                    "listen_port": 24800
                })
                .to_string(),
            ))
            .expect("heartbeat request");
        heartbeat.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:40000".parse::<SocketAddr>().expect("addr"),
        ));
        let disabled_heartbeat = app
            .clone()
            .oneshot(heartbeat)
            .await
            .expect("heartbeat response");
        assert_eq!(disabled_heartbeat.status(), StatusCode::FORBIDDEN);

        let (reauth_status, reauthorized) = json_request(
            app.clone(),
            Method::POST,
            format!("/v1/devices/{device_id}/reauthorize"),
            Some(&token),
            json!({ "public_key": "new-key" }),
        )
        .await;
        assert_eq!(reauth_status, StatusCode::OK);
        assert_eq!(reauthorized["public_key"], "new-key");
        assert_eq!(reauthorized["disabled"], false);

        let (delete_status, deleted) = json_request(
            app.clone(),
            Method::DELETE,
            format!("/v1/devices/{device_id}"),
            Some(&token),
            Value::Null,
        )
        .await;
        assert_eq!(delete_status, StatusCode::OK);
        assert_eq!(deleted["deleted"], true);

        let (list_status, devices) = json_request(
            app,
            Method::GET,
            "/v1/devices".to_string(),
            Some(&token),
            Value::Null,
        )
        .await;
        assert_eq!(list_status, StatusCode::OK);
        assert!(devices.as_array().expect("devices").is_empty());
    }
}
