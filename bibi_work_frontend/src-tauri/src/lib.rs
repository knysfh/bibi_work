use std::{
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{Emitter, Manager};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_dialog::DialogExt;
use thiserror::Error;
use url::Url;

const KEYRING_SERVICE: &str = "cn.bibi.work.desktop";
const LOCAL_MOUNT_PATH_KEY_PREFIX: &str = "local-mount-real-path:";
const LOCAL_EXEC_PROTOCOL: &str = "local_executor.v1";
const LOCAL_EXEC_KIND_FILE_IO: &str = "file_io";
const LOCAL_EXEC_DEFAULT_POLL_INTERVAL_MS: u64 = 750;
const LOCAL_EXEC_MAX_LIST_ENTRIES: usize = 500;

#[derive(Debug, Error)]
enum CommandError {
    #[error("failed to open external browser: {0}")]
    Browser(String),
    #[error("OIDC callback error: {0}")]
    Callback(String),
    #[error("OIDC token exchange error: {0}")]
    TokenExchange(String),
    #[error("secure store error: {0}")]
    SecureStore(String),
    #[error("local mount error: {0}")]
    LocalMount(String),
    #[error("local executor error: {0}")]
    LocalExec(String),
}

impl serde::Serialize for CommandError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceInfo {
    device_name: String,
    platform: String,
    arch: String,
    fingerprint: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct OidcLoginRequest {
    authorization_url: String,
    token_endpoint: String,
    client_id: String,
    redirect_uri: String,
    code_verifier: String,
    state: String,
}

#[derive(Clone, Default)]
struct AuthState {
    pending_login: Arc<Mutex<Option<PendingLogin>>>,
}

struct PendingLogin {
    request: OidcLoginRequest,
    sender: mpsc::Sender<Result<TokenSet, CommandError>>,
}

#[derive(Debug, Deserialize)]
struct TokenEndpointResponse {
    access_token: String,
    refresh_token: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenSet {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalExecBridgeStatus {
    state: &'static str,
    detail: &'static str,
}

#[derive(Clone, Default)]
struct LocalExecBridgeState {
    running: Arc<AtomicBool>,
    stop_requested: Arc<AtomicBool>,
}

#[derive(Clone, Default)]
struct LocalMountPathRegistry {
    paths: Arc<Mutex<HashMap<String, String>>>,
    store_path: Arc<Mutex<Option<PathBuf>>>,
}

impl LocalMountPathRegistry {
    fn set_store_path(&self, store_path: PathBuf) -> Result<(), String> {
        let mut path = self
            .store_path
            .lock()
            .map_err(|_| "local mount path store lock poisoned".to_string())?;
        *path = Some(store_path);
        Ok(())
    }

    fn set(&self, local_mount_id: &str, real_path: String) -> Result<(), String> {
        let mut paths = self
            .paths
            .lock()
            .map_err(|_| "local mount path registry lock poisoned".to_string())?;
        paths.insert(local_mount_id.to_string(), real_path);
        Ok(())
    }

    fn get(&self, local_mount_id: &str) -> Result<Option<String>, String> {
        if let Some(real_path) = self.memory_get(local_mount_id)? {
            return Ok(Some(real_path));
        }
        let Some(real_path) = self.disk_get(local_mount_id)? else {
            return Ok(None);
        };
        self.set(local_mount_id, real_path.clone())?;
        Ok(Some(real_path))
    }

    fn set_persisted(&self, local_mount_id: &str, real_path: String) -> Result<(), String> {
        self.disk_set(local_mount_id, &real_path)?;
        self.set(local_mount_id, real_path)
    }

    fn memory_get(&self, local_mount_id: &str) -> Result<Option<String>, String> {
        let paths = self
            .paths
            .lock()
            .map_err(|_| "local mount path registry lock poisoned".to_string())?;
        Ok(paths.get(local_mount_id).cloned())
    }

    fn disk_get(&self, local_mount_id: &str) -> Result<Option<String>, String> {
        Ok(self
            .read_disk_map()?
            .and_then(|paths| paths.get(local_mount_id).cloned()))
    }

    fn disk_set(&self, local_mount_id: &str, real_path: &str) -> Result<(), String> {
        let Some(store_path) = self.current_store_path()? else {
            return Ok(());
        };
        if let Some(parent) = store_path.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let mut paths = self.read_disk_map()?.unwrap_or_default();
        paths.insert(local_mount_id.to_string(), real_path.to_string());
        let content = serde_json::to_string_pretty(&paths).map_err(|err| err.to_string())?;
        let temp_path = store_path.with_extension("json.tmp");
        fs::write(&temp_path, content).map_err(|err| err.to_string())?;
        fs::rename(temp_path, store_path).map_err(|err| err.to_string())?;
        Ok(())
    }

    fn read_disk_map(&self) -> Result<Option<HashMap<String, String>>, String> {
        let Some(store_path) = self.current_store_path()? else {
            return Ok(None);
        };
        match fs::read_to_string(store_path) {
            Ok(content) => serde_json::from_str(&content)
                .map(Some)
                .map_err(|err| err.to_string()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.to_string()),
        }
    }

    fn current_store_path(&self) -> Result<Option<PathBuf>, String> {
        let path = self
            .store_path
            .lock()
            .map_err(|_| "local mount path store lock poisoned".to_string())?;
        Ok(path.clone())
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LocalExecStartBridgeRequest {
    api_base_url: String,
    access_token: String,
    tenant_id: String,
    device_id: String,
    user_agent: Option<String>,
    poll_interval_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct LocalExecWorkItem {
    id: String,
    tenant_id: String,
    command: LocalExecCommand,
    max_output_bytes: i64,
}

#[derive(Debug, Deserialize)]
struct LocalExecCommand {
    protocol: String,
    kind: String,
    local_mount_id: String,
    operation: String,
    virtual_path: String,
    content: Option<String>,
    query: Option<String>,
    max_output_bytes: Option<i64>,
}

#[derive(Debug, Serialize)]
struct LocalExecCompleteRequest {
    tenant_id: String,
    status: String,
    result: Option<Value>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalMountFolderSelection {
    display_name: String,
    real_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalMountRealPathRegistration {
    local_mount_id: String,
    real_path: String,
}

#[tauri::command]
fn auth_open_external_browser(url: String) -> Result<(), CommandError> {
    open::that(url).map_err(|err| CommandError::Browser(err.to_string()))
}

#[tauri::command]
async fn auth_login_with_deep_link(
    state: tauri::State<'_, AuthState>,
    request: OidcLoginRequest,
) -> Result<TokenSet, CommandError> {
    validate_deep_link_redirect_uri(&request.redirect_uri)?;
    eprintln!(
        "bibi auth: starting OIDC login; redirect_uri={}",
        request.redirect_uri
    );
    let (sender, receiver) = mpsc::channel();
    {
        let mut pending = state
            .pending_login
            .lock()
            .map_err(|_| CommandError::Callback("auth state lock poisoned".to_string()))?;
        if pending.is_some() {
            return Err(CommandError::Callback(
                "OIDC login already in progress".to_string(),
            ));
        }
        *pending = Some(PendingLogin {
            request: request.clone(),
            sender,
        });
    }

    if let Err(err) = open::that(&request.authorization_url) {
        clear_pending_login(&state, &request.state);
        return Err(CommandError::Browser(err.to_string()));
    }
    eprintln!("bibi auth: browser opened; waiting for OIDC callback");

    let result = tauri::async_runtime::spawn_blocking(move || {
        receiver.recv_timeout(Duration::from_secs(180))
    })
    .await
    .map_err(|err| CommandError::Callback(err.to_string()))?;
    match result {
        Ok(token_result) => token_result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            clear_pending_login(&state, &request.state);
            Err(CommandError::Callback(
                "timed out waiting for OIDC callback".to_string(),
            ))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(CommandError::Callback(
            "OIDC callback channel closed".to_string(),
        )),
    }
}

#[tauri::command]
async fn secure_store_get(key: String) -> Result<Option<String>, CommandError> {
    tauri::async_runtime::spawn_blocking(move || secure_store_get_blocking(key))
        .await
        .map_err(|err| CommandError::SecureStore(err.to_string()))?
}

fn secure_store_get_blocking(key: String) -> Result<Option<String>, CommandError> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, &key)
        .map_err(|err| CommandError::SecureStore(err.to_string()))?;
    match entry.get_password() {
        Ok(value) => {
            eprintln!(
                "bibi auth: secure store get key={} present value_len={}",
                key,
                value.len()
            );
            Ok(Some(value))
        }
        Err(keyring::Error::NoEntry) => {
            eprintln!("bibi auth: secure store get key={} missing", key);
            Ok(None)
        }
        Err(err) => Err(CommandError::SecureStore(err.to_string())),
    }
}

#[tauri::command]
async fn secure_store_set(key: String, value: String) -> Result<(), CommandError> {
    tauri::async_runtime::spawn_blocking(move || secure_store_set_blocking(key, value))
        .await
        .map_err(|err| CommandError::SecureStore(err.to_string()))?
}

fn secure_store_set_blocking(key: String, value: String) -> Result<(), CommandError> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, &key)
        .map_err(|err| CommandError::SecureStore(err.to_string()))?;
    let value_len = value.len();
    entry
        .set_password(&value)
        .map_err(|err| CommandError::SecureStore(err.to_string()))?;
    eprintln!(
        "bibi auth: secure store set key={} value_len={}",
        key, value_len
    );
    Ok(())
}

#[tauri::command]
async fn secure_store_delete(key: String) -> Result<(), CommandError> {
    tauri::async_runtime::spawn_blocking(move || secure_store_delete_blocking(key))
        .await
        .map_err(|err| CommandError::SecureStore(err.to_string()))?
}

fn secure_store_delete_blocking(key: String) -> Result<(), CommandError> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, &key)
        .map_err(|err| CommandError::SecureStore(err.to_string()))?;
    match entry.delete_credential() {
        Ok(()) => {
            eprintln!("bibi auth: secure store delete key={} removed", key);
            Ok(())
        }
        Err(keyring::Error::NoEntry) => {
            eprintln!("bibi auth: secure store delete key={} missing", key);
            Ok(())
        }
        Err(err) => Err(CommandError::SecureStore(err.to_string())),
    }
}

#[tauri::command]
fn system_get_device_info() -> DeviceInfo {
    let device_name = whoami::fallible::hostname().unwrap_or_else(|_| "unknown-device".to_string());
    let platform = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    let username = whoami::username();
    let fingerprint_source = format!("{device_name}:{platform}:{arch}:{username}");
    let fingerprint = URL_SAFE_NO_PAD.encode(Sha256::digest(fingerprint_source.as_bytes()));

    DeviceInfo {
        device_name,
        platform,
        arch,
        fingerprint,
    }
}

#[tauri::command]
fn local_exec_register_device() -> LocalExecBridgeStatus {
    LocalExecBridgeStatus {
        state: "idle",
        detail: "local executor bridge is available",
    }
}

#[tauri::command]
fn local_exec_start_bridge(
    app: tauri::AppHandle,
    state: tauri::State<'_, LocalExecBridgeState>,
    registry: tauri::State<'_, LocalMountPathRegistry>,
    request: LocalExecStartBridgeRequest,
) -> Result<LocalExecBridgeStatus, CommandError> {
    if request.access_token.trim().is_empty() {
        return Err(CommandError::LocalExec(
            "access token is required to start local executor bridge".to_string(),
        ));
    }
    if request.tenant_id.trim().is_empty() || request.device_id.trim().is_empty() {
        return Err(CommandError::LocalExec(
            "tenant_id and device_id are required to start local executor bridge".to_string(),
        ));
    }
    state.stop_requested.store(false, Ordering::SeqCst);
    if state.running.swap(true, Ordering::SeqCst) {
        return Ok(LocalExecBridgeStatus {
            state: "connected",
            detail: "local executor bridge is already running",
        });
    }

    let bridge_state = state.inner().clone();
    let registry = registry.inner().clone();
    tauri::async_runtime::spawn(async move {
        run_local_exec_bridge(app, bridge_state, registry, request).await;
    });

    Ok(LocalExecBridgeStatus {
        state: "connected",
        detail: "local executor bridge started",
    })
}

#[tauri::command]
fn local_exec_stop_bridge(state: tauri::State<'_, LocalExecBridgeState>) -> LocalExecBridgeStatus {
    state.stop_requested.store(true, Ordering::SeqCst);
    LocalExecBridgeStatus {
        state: "idle",
        detail: "local executor bridge stop requested",
    }
}

#[tauri::command]
async fn local_mount_pick_folder(
    app: tauri::AppHandle,
) -> Result<Option<LocalMountFolderSelection>, CommandError> {
    let (sender, receiver) = mpsc::channel();
    app.dialog()
        .file()
        .set_title("Select local mount folder")
        .pick_folder(move |folder| {
            let _ = sender.send(folder);
        });
    let folder = tauri::async_runtime::spawn_blocking(move || receiver.recv())
        .await
        .map_err(|err| CommandError::LocalMount(err.to_string()))?
        .map_err(|err| CommandError::LocalMount(err.to_string()))?;
    let Some(folder) = folder else {
        return Ok(None);
    };
    let path = folder
        .into_path()
        .map_err(|err| CommandError::LocalMount(err.to_string()))?;
    let metadata =
        std::fs::metadata(&path).map_err(|err| CommandError::LocalMount(err.to_string()))?;
    if !metadata.is_dir() {
        return Err(CommandError::LocalMount(
            "selected path is not a directory".to_string(),
        ));
    }
    let path = path
        .canonicalize()
        .map_err(|err| CommandError::LocalMount(err.to_string()))?;
    Ok(Some(LocalMountFolderSelection {
        display_name: local_folder_display_name(&path),
        real_path: path.to_string_lossy().into_owned(),
    }))
}

#[tauri::command]
async fn local_mount_register_real_path(
    registry: tauri::State<'_, LocalMountPathRegistry>,
    request: LocalMountRealPathRegistration,
) -> Result<(), CommandError> {
    let registry = registry.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        local_mount_register_real_path_blocking(registry, request)
    })
    .await
    .map_err(|err| CommandError::LocalMount(err.to_string()))?
}

fn local_mount_register_real_path_blocking(
    registry: LocalMountPathRegistry,
    request: LocalMountRealPathRegistration,
) -> Result<(), CommandError> {
    let local_mount_id = request.local_mount_id.trim();
    if local_mount_id.is_empty() {
        return Err(CommandError::LocalMount(
            "localMountId is required".to_string(),
        ));
    }
    let real_root =
        canonical_root(&request.real_path).map_err(|err| CommandError::LocalMount(err))?;
    let real_path = real_root.to_string_lossy().into_owned();
    registry
        .set_persisted(local_mount_id, real_path.clone())
        .map_err(CommandError::LocalMount)?;

    let key = format!("{LOCAL_MOUNT_PATH_KEY_PREFIX}{local_mount_id}");
    if let Err(err) = secure_store_set_blocking(key, real_path) {
        eprintln!("bibi local mount: failed to persist real path: {err}");
    }
    Ok(())
}

fn local_folder_display_name(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "Local Folder".to_string())
}

async fn run_local_exec_bridge(
    app: tauri::AppHandle,
    state: LocalExecBridgeState,
    registry: LocalMountPathRegistry,
    request: LocalExecStartBridgeRequest,
) {
    let client = reqwest::Client::new();
    let poll_interval = Duration::from_millis(
        request
            .poll_interval_ms
            .unwrap_or(LOCAL_EXEC_DEFAULT_POLL_INTERVAL_MS)
            .clamp(200, 10_000),
    );
    let api_base_url = request.api_base_url.trim_end_matches('/').to_string();
    while !state.stop_requested.load(Ordering::SeqCst) {
        match fetch_next_local_exec_request(&client, &api_base_url, &request).await {
            Ok(Some(work_item)) => {
                let completion = execute_local_exec_work_item(&work_item, &registry);
                let _ = complete_local_exec_request(
                    &client,
                    &api_base_url,
                    &request,
                    &work_item.id,
                    completion,
                )
                .await;
                let _ = app.emit(
                    "localExec.event",
                    json!({"request_id": work_item.id, "tenant_id": work_item.tenant_id}),
                );
            }
            Ok(None) => {
                std::thread::sleep(poll_interval);
            }
            Err(err) => {
                let _ = app.emit("localExec.event", json!({"error": err}));
                std::thread::sleep(poll_interval);
            }
        }
    }
    state.running.store(false, Ordering::SeqCst);
}

async fn fetch_next_local_exec_request(
    client: &reqwest::Client,
    api_base_url: &str,
    request: &LocalExecStartBridgeRequest,
) -> Result<Option<LocalExecWorkItem>, String> {
    let url = format!(
        "{api_base_url}/local-exec/requests/next?tenant_id={}",
        request.tenant_id
    );
    let response = client
        .get(url)
        .bearer_auth(&request.access_token)
        .header(reqwest::header::USER_AGENT, local_exec_user_agent(request))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if response.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!("poll failed: {}", response.status()));
    }
    let text = response.text().await.map_err(|err| err.to_string())?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str::<Option<LocalExecWorkItem>>(&text).map_err(|err| err.to_string())
}

async fn complete_local_exec_request(
    client: &reqwest::Client,
    api_base_url: &str,
    request: &LocalExecStartBridgeRequest,
    request_id: &str,
    completion: LocalExecCompleteRequest,
) -> Result<(), String> {
    let body = serde_json::to_string(&completion).map_err(|err| err.to_string())?;
    let response = client
        .post(format!(
            "{api_base_url}/local-exec/requests/{request_id}/complete"
        ))
        .bearer_auth(&request.access_token)
        .header(reqwest::header::USER_AGENT, local_exec_user_agent(request))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("complete failed: {}", response.status()))
    }
}

fn local_exec_user_agent(request: &LocalExecStartBridgeRequest) -> String {
    request
        .user_agent
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

fn execute_local_exec_work_item(
    work_item: &LocalExecWorkItem,
    registry: &LocalMountPathRegistry,
) -> LocalExecCompleteRequest {
    let result =
        execute_local_exec_command(&work_item.command, work_item.max_output_bytes, registry);
    match result {
        Ok(value) => LocalExecCompleteRequest {
            tenant_id: work_item.tenant_id.clone(),
            status: "completed".to_string(),
            result: Some(value),
            error: None,
        },
        Err(err) => LocalExecCompleteRequest {
            tenant_id: work_item.tenant_id.clone(),
            status: "failed".to_string(),
            result: None,
            error: Some(err),
        },
    }
}

fn execute_local_exec_command(
    command: &LocalExecCommand,
    max_output_bytes: i64,
    registry: &LocalMountPathRegistry,
) -> Result<Value, String> {
    if command.protocol != LOCAL_EXEC_PROTOCOL || command.kind != LOCAL_EXEC_KIND_FILE_IO {
        return Err("unsupported local executor protocol".to_string());
    }
    let real_root = local_mount_real_path(&command.local_mount_id, registry)?;
    let max_output_bytes = command
        .max_output_bytes
        .unwrap_or(max_output_bytes)
        .clamp(1_024, 8 * 1_048_576) as usize;
    match command.operation.as_str() {
        "read_text" => read_local_text(&real_root, &command.virtual_path, max_output_bytes),
        "write_text" => write_local_text(
            &real_root,
            &command.virtual_path,
            command.content.as_deref().unwrap_or_default(),
        ),
        "list" => list_local_files(&real_root, &command.virtual_path),
        "search" => search_local_files(
            &real_root,
            &command.virtual_path,
            command.query.as_deref().unwrap_or_default(),
            max_output_bytes,
        ),
        _ => Err(format!(
            "unsupported local executor operation: {}",
            command.operation
        )),
    }
}

fn local_mount_real_path(
    local_mount_id: &str,
    registry: &LocalMountPathRegistry,
) -> Result<String, String> {
    if let Some(real_path) = registry.get(local_mount_id)? {
        return Ok(real_path);
    }
    let key = format!("{LOCAL_MOUNT_PATH_KEY_PREFIX}{local_mount_id}");
    let entry = keyring::Entry::new(KEYRING_SERVICE, &key).map_err(|err| err.to_string())?;
    let real_path = entry.get_password().map_err(|err| match err {
        keyring::Error::NoEntry => "local mount real path is not registered".to_string(),
        _ => err.to_string(),
    })?;
    registry.set_persisted(local_mount_id, real_path.clone())?;
    Ok(real_path)
}

fn configure_local_mount_registry(
    app: &tauri::App,
    registry: &LocalMountPathRegistry,
) -> Result<(), String> {
    let app_data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    registry.set_store_path(app_data_dir.join("local-mount-real-paths.json"))
}

fn read_local_text(
    real_root: &str,
    virtual_path: &str,
    max_output_bytes: usize,
) -> Result<Value, String> {
    let path = resolve_existing_local_path(real_root, virtual_path)?;
    let metadata = fs::metadata(&path).map_err(|err| err.to_string())?;
    if !metadata.is_file() {
        return Err("local path is not a file".to_string());
    }
    if metadata.len() as usize > max_output_bytes {
        return Err("local file exceeds max_output_bytes".to_string());
    }
    let content = fs::read_to_string(&path).map_err(|err| err.to_string())?;
    Ok(json!({
        "content": content,
        "inline_content": content,
        "revision": 0,
        "size_bytes": metadata.len(),
        "path": virtual_path
    }))
}

fn write_local_text(real_root: &str, virtual_path: &str, content: &str) -> Result<Value, String> {
    let path = resolve_writable_local_path(real_root, virtual_path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    fs::write(&path, content).map_err(|err| err.to_string())?;
    let metadata = fs::metadata(&path).map_err(|err| err.to_string())?;
    Ok(json!({
        "revision": 0,
        "size_bytes": metadata.len(),
        "path": virtual_path
    }))
}

fn list_local_files(real_root: &str, virtual_path: &str) -> Result<Value, String> {
    let root = canonical_root(real_root)?;
    let target = resolve_existing_local_path_from_root(&root, virtual_path)?;
    let mut files = Vec::new();
    let mut entries = Vec::new();
    collect_local_entries(&root, &target, &mut files, &mut entries)?;
    Ok(json!({"files": files, "entries": entries}))
}

fn search_local_files(
    real_root: &str,
    virtual_path: &str,
    query: &str,
    max_output_bytes: usize,
) -> Result<Value, String> {
    if query.is_empty() {
        return Err("query is required".to_string());
    }
    let root = canonical_root(real_root)?;
    let target = resolve_existing_local_path_from_root(&root, virtual_path)?;
    let mut files = Vec::new();
    let mut entries = Vec::new();
    collect_local_entries(&root, &target, &mut files, &mut entries)?;
    let mut matches = Vec::new();
    for file in files.iter().take(LOCAL_EXEC_MAX_LIST_ENTRIES) {
        let Some(path) = file.get("path").and_then(Value::as_str) else {
            continue;
        };
        let local_path = resolve_existing_local_path_from_root(&root, path)?;
        let metadata = fs::metadata(&local_path).map_err(|err| err.to_string())?;
        if metadata.len() as usize > max_output_bytes {
            continue;
        }
        let Ok(content) = fs::read_to_string(&local_path) else {
            continue;
        };
        for (index, line) in content.lines().enumerate() {
            if line.contains(query) {
                matches.push(json!({
                    "path": path,
                    "line": index + 1,
                    "text": line
                }));
            }
        }
    }
    Ok(json!({"files": files, "entries": entries, "matches": matches}))
}

fn collect_local_entries(
    root: &Path,
    start: &Path,
    files: &mut Vec<Value>,
    entries: &mut Vec<Value>,
) -> Result<(), String> {
    if entries.len() >= LOCAL_EXEC_MAX_LIST_ENTRIES {
        return Ok(());
    }
    let start = start.canonicalize().map_err(|err| err.to_string())?;
    if !start.starts_with(root) {
        return Ok(());
    }
    let metadata = fs::metadata(&start).map_err(|err| err.to_string())?;
    let virtual_path = virtual_path_for_local_path(root, &start)?;
    if metadata.is_file() {
        let file_entry = json!({
            "path": virtual_path,
            "entry_type": "file",
            "depth": virtual_path_depth(&virtual_path),
            "children_count": 0,
            "size_bytes": metadata.len()
        });
        files.push(file_entry.clone());
        entries.push(file_entry);
        return Ok(());
    }
    if metadata.is_dir() {
        let children: Vec<PathBuf> = fs::read_dir(start)
            .map_err(|err| err.to_string())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        entries.push(json!({
            "path": ensure_trailing_slash(&virtual_path),
            "entry_type": "directory",
            "depth": virtual_path_depth(&virtual_path),
            "children_count": children.len()
        }));
        for child in children {
            collect_local_entries(root, &child, files, entries)?;
            if entries.len() >= LOCAL_EXEC_MAX_LIST_ENTRIES {
                break;
            }
        }
    }
    Ok(())
}

fn canonical_root(real_root: &str) -> Result<PathBuf, String> {
    let root = Path::new(real_root)
        .canonicalize()
        .map_err(|err| err.to_string())?;
    if !root.is_dir() {
        return Err("local mount root is not a directory".to_string());
    }
    Ok(root)
}

fn resolve_existing_local_path(real_root: &str, virtual_path: &str) -> Result<PathBuf, String> {
    let root = canonical_root(real_root)?;
    resolve_existing_local_path_from_root(&root, virtual_path)
}

fn resolve_existing_local_path_from_root(
    root: &Path,
    virtual_path: &str,
) -> Result<PathBuf, String> {
    let relative = local_virtual_relative_path(virtual_path)?;
    let path = root
        .join(relative)
        .canonicalize()
        .map_err(|err| err.to_string())?;
    if !path.starts_with(root) {
        return Err("local path escapes the mounted root".to_string());
    }
    Ok(path)
}

fn resolve_writable_local_path(real_root: &str, virtual_path: &str) -> Result<PathBuf, String> {
    let root = canonical_root(real_root)?;
    let relative = local_virtual_relative_path(virtual_path)?;
    let target = root.join(&relative);
    let parent = target
        .parent()
        .ok_or_else(|| "local write path has no parent".to_string())?;
    fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    let parent = parent.canonicalize().map_err(|err| err.to_string())?;
    if !parent.starts_with(&root) {
        return Err("local write path escapes the mounted root".to_string());
    }
    let file_name = target
        .file_name()
        .ok_or_else(|| "local write path must include a file name".to_string())?;
    Ok(parent.join(file_name))
}

fn local_virtual_relative_path(virtual_path: &str) -> Result<PathBuf, String> {
    if !virtual_path.starts_with("/local/main/") || virtual_path.contains('\0') {
        return Err("virtual path must be under /local/main/".to_string());
    }
    let relative = virtual_path.trim_start_matches("/local/main/");
    let mut path = PathBuf::new();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            _ => return Err("virtual path contains unsupported component".to_string()),
        }
    }
    Ok(path)
}

fn virtual_path_for_local_path(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "local path escapes the mounted root".to_string())?;
    let suffix = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if suffix.is_empty() {
        Ok("/local/main/".to_string())
    } else {
        Ok(format!("/local/main/{suffix}"))
    }
}

fn ensure_trailing_slash(path: &str) -> String {
    if path.ends_with('/') {
        path.to_string()
    } else {
        format!("{path}/")
    }
}

fn virtual_path_depth(path: &str) -> i32 {
    path.trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .count() as i32
}

fn validate_deep_link_redirect_uri(redirect_uri: &str) -> Result<(), CommandError> {
    let redirect = Url::parse(redirect_uri)
        .map_err(|err| CommandError::Callback(format!("invalid redirect URI: {err}")))?;
    if !is_auth_callback_url(&redirect) {
        return Err(CommandError::Callback(
            "redirect URI must be bibi-work://auth/callback".to_string(),
        ));
    }
    Ok(())
}

fn handle_deep_link_url(state: AuthState, callback_url: Url) {
    if !is_auth_callback_url(&callback_url) {
        eprintln!(
            "bibi auth: ignored deep link url; scheme={}, host={:?}, path={}",
            callback_url.scheme(),
            callback_url.host_str(),
            callback_url.path()
        );
        return;
    }

    eprintln!("bibi auth: received OIDC callback deep link");
    let callback = parse_oidc_callback(&callback_url);
    let Some(callback_state) = callback.state.as_deref() else {
        eprintln!("bibi auth: callback ignored because state is missing");
        return;
    };
    let pending = {
        let Ok(mut pending) = state.pending_login.lock() else {
            eprintln!("bibi auth: callback ignored because auth state lock is poisoned");
            return;
        };
        if pending
            .as_ref()
            .is_some_and(|item| item.request.state == callback_state)
        {
            pending.take()
        } else {
            None
        }
    };
    let Some(pending) = pending else {
        eprintln!("bibi auth: callback ignored because no matching pending login exists");
        return;
    };
    eprintln!("bibi auth: callback state matched; exchanging authorization code");

    tauri::async_runtime::spawn(async move {
        let result = match callback.error {
            Some(error) => {
                let description = callback.error_description.unwrap_or_default();
                Err(CommandError::Callback(format!(
                    "authorization failed: {error} {description}"
                )))
            }
            None => match callback.code {
                Some(code) => exchange_authorization_code(&pending.request, &code).await,
                None => Err(CommandError::Callback(
                    "callback missing authorization code".to_string(),
                )),
            },
        };
        match &result {
            Ok(_) => eprintln!("bibi auth: token exchange completed"),
            Err(err) => eprintln!("bibi auth: token exchange failed: {err}"),
        }
        let _ = pending.sender.send(result);
    });
}

fn parse_oidc_callback(callback_url: &Url) -> OidcCallbackParams {
    let mut callback = OidcCallbackParams::default();
    for (key, value) in callback_url.query_pairs() {
        match key.as_ref() {
            "code" => callback.code = Some(value.into_owned()),
            "state" => callback.state = Some(value.into_owned()),
            "error" => callback.error = Some(value.into_owned()),
            "error_description" => callback.error_description = Some(value.into_owned()),
            _ => {}
        }
    }
    callback
}

fn is_auth_callback_url(url: &Url) -> bool {
    url.scheme() == "bibi-work" && url.host_str() == Some("auth") && url.path() == "/callback"
}

fn clear_pending_login(state: &AuthState, expected_state: &str) {
    if let Ok(mut pending) = state.pending_login.lock() {
        if pending
            .as_ref()
            .is_some_and(|item| item.request.state == expected_state)
        {
            pending.take();
        }
    }
}

#[derive(Default)]
struct OidcCallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

async fn exchange_authorization_code(
    request: &OidcLoginRequest,
    code: &str,
) -> Result<TokenSet, CommandError> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("client_id", &request.client_id)
        .append_pair("redirect_uri", &request.redirect_uri)
        .append_pair("code_verifier", &request.code_verifier)
        .append_pair("code", code)
        .finish();
    let response = reqwest::Client::new()
        .post(&request.token_endpoint)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body)
        .send()
        .await
        .map_err(|err| CommandError::TokenExchange(err.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(CommandError::TokenExchange(format!(
            "token endpoint returned {status}: {body}"
        )));
    }

    let body = response
        .text()
        .await
        .map_err(|err| CommandError::TokenExchange(err.to_string()))?;
    let token = serde_json::from_str::<TokenEndpointResponse>(&body)
        .map_err(|err| CommandError::TokenExchange(err.to_string()))?;
    if token.access_token.trim().is_empty() {
        return Err(CommandError::TokenExchange(
            "token endpoint did not return access_token".to_string(),
        ));
    }

    Ok(TokenSet {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at: None,
    })
}

pub fn run() {
    let auth_state = AuthState::default();
    let deep_link_auth_state = auth_state.clone();
    let local_exec_state = LocalExecBridgeState::default();
    let local_mount_path_registry = LocalMountPathRegistry::default();
    let setup_local_mount_path_registry = local_mount_path_registry.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(auth_state)
        .manage(local_exec_state)
        .manage(local_mount_path_registry)
        .setup(move |app| {
            if let Err(err) = configure_local_mount_registry(app, &setup_local_mount_path_registry)
            {
                eprintln!("bibi local mount: failed to configure path registry: {err}");
            }

            #[cfg(any(target_os = "linux", target_os = "windows"))]
            if let Err(err) = app.deep_link().register_all() {
                eprintln!("bibi auth: failed to register deep link schemes: {err}");
            }

            let callback_state = deep_link_auth_state.clone();
            let current_callback_state = callback_state.clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    handle_deep_link_url(callback_state.clone(), url);
                }
            });
            if let Some(urls) = app.deep_link().get_current()? {
                for url in urls {
                    handle_deep_link_url(current_callback_state.clone(), url);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            auth_open_external_browser,
            auth_login_with_deep_link,
            secure_store_get,
            secure_store_set,
            secure_store_delete,
            system_get_device_info,
            local_exec_register_device,
            local_exec_start_bridge,
            local_exec_stop_bridge,
            local_mount_pick_folder,
            local_mount_register_real_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running Bibi Work desktop shell");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_mount_real_path_uses_process_registry() {
        let registry = LocalMountPathRegistry::default();
        registry
            .set("mount-1", "/tmp/bibi-local-mount".to_string())
            .unwrap();

        assert_eq!(
            local_mount_real_path("mount-1", &registry).unwrap(),
            "/tmp/bibi-local-mount"
        );
    }

    #[test]
    fn local_mount_real_path_uses_persisted_registry() {
        let test_dir = std::env::temp_dir().join(format!(
            "bibi-local-mount-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mount_dir = test_dir.join("mount");
        fs::create_dir_all(&mount_dir).unwrap();
        let store_path = test_dir.join("local-mount-real-paths.json");
        let real_path = mount_dir
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        let registry = LocalMountPathRegistry::default();
        registry.set_store_path(store_path.clone()).unwrap();
        registry
            .set_persisted("mount-1", real_path.clone())
            .unwrap();

        let reloaded_registry = LocalMountPathRegistry::default();
        reloaded_registry.set_store_path(store_path).unwrap();

        assert_eq!(
            local_mount_real_path("mount-1", &reloaded_registry).unwrap(),
            real_path
        );

        let _ = fs::remove_dir_all(test_dir);
    }
}
