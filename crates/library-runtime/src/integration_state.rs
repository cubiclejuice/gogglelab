use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use library_integrations::{
    validate_provider_config, validate_spoolman_config, CategorySharing, EndpointScope,
    ProviderConfig, ProviderKind, SpoolmanConfig, StructuredOutputMode,
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const SETTINGS_VERSION: u32 = 1;
const MAX_SETTINGS_BYTES: u64 = 256 * 1024;
pub const MAX_PROVIDERS: usize = 16;
pub const MAX_REVIEW_TARGETS: usize = 50;
pub const MAX_REVIEWS: usize = 16;
pub const REVIEW_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiProviderPublicConfig {
    pub id: String,
    pub label: String,
    pub kind: ProviderKind,
    pub base_url: Option<String>,
    pub endpoint_scope: EndpointScope,
    pub model: String,
    pub credential_storage: String,
    pub has_credential: bool,
    pub max_tags: u8,
    pub detail: String,
    pub category_mode: String,
    pub concurrency: u8,
    pub timeout_seconds: u16,
    pub structured_output: StructuredOutputMode,
    #[serde(default)]
    pub category_sharing: CategorySharing,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationSettings {
    pub spoolman: Option<SpoolmanConfig>,
    pub ai_providers: Vec<AiProviderPublicConfig>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpoolmanPullResult {
    pub filaments_received: usize,
    pub spools_received: usize,
    pub filaments_upserted: usize,
    pub spools_upserted: usize,
    pub removed_locally: usize,
    pub last_successful_pull_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialUpdate {
    pub action: String,
    pub secret: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSuggestion {
    pub suggestion_id: String,
    pub name: String,
    pub category: Option<String>,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiExistingTag {
    pub name: String,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSuggestionTarget {
    pub model_id: String,
    pub expected_version: u64,
    pub display_name: String,
    pub existing_tags: Vec<AiExistingTag>,
    pub suggestions: Vec<AiSuggestion>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSuggestionFailure {
    pub model_id: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSuggestionBatch {
    pub review_token: String,
    pub provider_id: String,
    pub model: String,
    pub category_sharing: CategorySharing,
    pub targets: Vec<AiSuggestionTarget>,
    pub failures: Vec<AiSuggestionFailure>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiReviewDecision {
    pub model_id: String,
    pub action: AiReviewAction,
    pub accepted_suggestion_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiReviewAction {
    Accept,
    Reject,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiReviewStrategy {
    Append,
    Replace,
    Merge,
}

#[derive(Clone)]
pub struct IntegrationState {
    pub shared: Arc<IntegrationShared>,
}

pub struct IntegrationShared {
    pub settings_path: PathBuf,
    pub inner: Mutex<IntegrationInner>,
}

pub struct IntegrationInner {
    pub spoolman: Option<StoredSpoolman>,
    pub providers: Vec<StoredProvider>,
    pub credentials: HashMap<String, SessionSecret>,
    pub reviews: VecDeque<StoredReview>,
    pub epoch: u64,
    pub sequence: u64,
    pub ai_request_active: bool,
    pub spoolman_pull_active: bool,
}

#[derive(Clone, Copy)]
pub enum IntegrationJobKind {
    Ai,
    Spoolman,
}

pub struct IntegrationJobGuard {
    state: IntegrationState,
    kind: IntegrationJobKind,
}

impl Drop for IntegrationJobGuard {
    fn drop(&mut self) {
        let mut inner = self.state.shared.inner.lock().unwrap();
        match self.kind {
            IntegrationJobKind::Ai => inner.ai_request_active = false,
            IntegrationJobKind::Spoolman => inner.spoolman_pull_active = false,
        }
    }
}

pub struct SessionSecret(pub Zeroizing<String>);

impl SessionSecret {
    pub fn new(value: String) -> Result<Self, String> {
        if value.is_empty() || value.len() > 16 * 1024 || value.bytes().any(|byte| byte == 0) {
            return Err("Session credential is invalid".into());
        }
        Ok(Self(Zeroizing::new(value)))
    }

    pub fn temporary_copy(&self) -> Zeroizing<String> {
        Zeroizing::new(self.0.to_string())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSpoolman {
    pub source_id: String,
    pub config: SpoolmanConfig,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredProvider {
    pub id: String,
    pub label: String,
    pub config: ProviderConfig,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsFile {
    version: u32,
    spoolman: Option<StoredSpoolman>,
    providers: Vec<StoredProvider>,
}

pub struct StoredReview {
    pub token: String,
    pub epoch: u64,
    pub library_generation: u64,
    pub created_at: SystemTime,
    pub category_mode: String,
    pub known_tags: HashMap<String, Option<String>>,
    pub targets: Vec<AiSuggestionTarget>,
    pub applying: bool,
}

impl IntegrationState {
    pub fn load(settings_path: PathBuf) -> Result<Self, String> {
        let persisted = load_settings(&settings_path)?;
        for provider in &persisted.providers {
            validate_stored_provider(provider)?;
        }
        if persisted.providers.len() > MAX_PROVIDERS {
            return Err("Integration settings contain too many providers".into());
        }
        if let Some(spoolman) = &persisted.spoolman {
            validate_spoolman_config(&spoolman.config)?;
            validate_identifier(&spoolman.source_id, "Spoolman source")?;
        }
        Ok(Self {
            shared: Arc::new(IntegrationShared {
                settings_path,
                inner: Mutex::new(IntegrationInner {
                    spoolman: persisted.spoolman,
                    providers: persisted.providers,
                    credentials: HashMap::new(),
                    reviews: VecDeque::new(),
                    epoch: 1,
                    sequence: 0,
                    ai_request_active: false,
                    spoolman_pull_active: false,
                }),
            }),
        })
    }

    pub fn settings(&self) -> IntegrationSettings {
        let inner = self.shared.inner.lock().unwrap();
        settings_from_inner(&inner)
    }

    pub fn invalidate_for_library_epoch(&self) {
        let mut inner = self.shared.inner.lock().unwrap();
        Self::invalidate_locked(&mut inner);
    }

    pub fn persist_values(
        &self,
        spoolman: Option<StoredSpoolman>,
        providers: &[StoredProvider],
    ) -> Result<(), String> {
        persist_settings(
            &self.shared.settings_path,
            &SettingsFile {
                version: SETTINGS_VERSION,
                spoolman,
                providers: providers.to_vec(),
            },
        )
    }

    pub fn invalidate_locked(inner: &mut IntegrationInner) {
        inner.epoch = inner.epoch.wrapping_add(1).max(1);
        inner.reviews.clear();
    }

    pub fn begin_job(&self, kind: IntegrationJobKind) -> Result<IntegrationJobGuard, String> {
        let mut inner = self.shared.inner.lock().unwrap();
        let active = match kind {
            IntegrationJobKind::Ai => &mut inner.ai_request_active,
            IntegrationJobKind::Spoolman => &mut inner.spoolman_pull_active,
        };
        if *active {
            return Err(match kind {
                IntegrationJobKind::Ai => "An AI suggestion request is already running".into(),
                IntegrationJobKind::Spoolman => "A Spoolman pull is already running".into(),
            });
        }
        *active = true;
        Ok(IntegrationJobGuard {
            state: self.clone(),
            kind,
        })
    }
}

pub fn public_provider(provider: &StoredProvider, has_credential: bool) -> AiProviderPublicConfig {
    AiProviderPublicConfig {
        id: provider.id.clone(),
        label: provider.label.clone(),
        kind: provider.config.kind,
        base_url: Some(provider.config.base_url.clone()),
        endpoint_scope: provider.config.endpoint_scope,
        model: provider.config.model.clone(),
        credential_storage: if has_credential { "session" } else { "none" }.into(),
        has_credential,
        max_tags: provider.config.max_tags,
        detail: provider.config.detail.clone(),
        category_mode: provider.config.category_mode.clone(),
        concurrency: provider.config.concurrency,
        timeout_seconds: provider.config.timeout_seconds,
        structured_output: provider.config.structured_output,
        category_sharing: provider.config.category_sharing,
    }
}

pub fn settings_from_inner(inner: &IntegrationInner) -> IntegrationSettings {
    IntegrationSettings {
        spoolman: inner.spoolman.as_ref().map(|value| value.config.clone()),
        ai_providers: inner
            .providers
            .iter()
            .map(|provider| public_provider(provider, inner.credentials.contains_key(&provider.id)))
            .collect(),
    }
}

pub fn stored_provider(config: &AiProviderPublicConfig) -> Result<StoredProvider, String> {
    validate_identifier(&config.id, "Provider")?;
    if config.label.trim().is_empty()
        || config.label.len() > 120
        || config.label.chars().any(char::is_control)
        || !matches!(config.credential_storage.as_str(), "none" | "session")
    {
        return Err("AI provider display configuration is invalid".into());
    }
    let base_url = config
        .base_url
        .clone()
        .unwrap_or_else(|| match config.kind {
            ProviderKind::Anthropic => "https://api.anthropic.com/".into(),
            ProviderKind::Gemini => "https://generativelanguage.googleapis.com/".into(),
            ProviderKind::OpenaiCompatible => String::new(),
        });
    let stored = StoredProvider {
        id: config.id.clone(),
        label: config.label.trim().to_owned(),
        config: ProviderConfig {
            kind: config.kind,
            base_url,
            endpoint_scope: config.endpoint_scope,
            model: config.model.clone(),
            max_tags: config.max_tags,
            detail: config.detail.clone(),
            category_mode: config.category_mode.clone(),
            concurrency: config.concurrency,
            timeout_seconds: config.timeout_seconds,
            structured_output: config.structured_output,
            category_sharing: config.category_sharing,
        },
    };
    validate_stored_provider(&stored)?;
    Ok(stored)
}

pub fn validate_stored_provider(provider: &StoredProvider) -> Result<(), String> {
    validate_identifier(&provider.id, "Provider")?;
    if provider.label.is_empty()
        || provider.label.len() > 120
        || provider.label.chars().any(char::is_control)
    {
        return Err("AI provider label is invalid".into());
    }
    validate_provider_config(&provider.config)
}

pub fn validate_identifier(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!("{label} identifier is invalid"));
    }
    Ok(())
}

fn load_settings(path: &Path) -> Result<SettingsFile, String> {
    validate_config_ancestors(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SettingsFile {
                version: SETTINGS_VERSION,
                spoolman: None,
                providers: Vec::new(),
            });
        }
        Err(_) => return Err("Integration settings could not be inspected".into()),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_SETTINGS_BYTES
    {
        return Err("Integration settings file is unsafe or oversized".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| "Integration settings could not be opened")?
        .take(MAX_SETTINGS_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Integration settings could not be read")?;
    if bytes.len() as u64 > MAX_SETTINGS_BYTES {
        return Err("Integration settings file is oversized".into());
    }
    let settings: SettingsFile =
        serde_json::from_slice(&bytes).map_err(|_| "Integration settings are malformed")?;
    if settings.version != SETTINGS_VERSION {
        return Err("Integration settings version is unsupported".into());
    }
    Ok(settings)
}

fn persist_settings(path: &Path, settings: &SettingsFile) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|_| "Integration settings could not be encoded")?;
    if bytes.len() as u64 > MAX_SETTINGS_BYTES {
        return Err("Integration settings exceed their size limit".into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "Integration settings path has no parent".to_string())?;
    validate_config_ancestors(path)?;
    fs::create_dir_all(parent)
        .map_err(|_| "Integration settings directory could not be created")?;
    validate_config_ancestors(path)?;
    if fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err("Integration settings file is unsafe".into());
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = path.with_extension(format!("tmp-{}-{nonce}", std::process::id()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|_| "Integration settings temporary file could not be created")?;
    let result = (|| {
        file.write_all(&bytes)
            .map_err(|_| "Integration settings could not be written".to_string())?;
        file.sync_all()
            .map_err(|_| "Integration settings could not be synchronized".to_string())?;
        validate_config_ancestors(path)?;
        fs::rename(&temporary, path)
            .map_err(|_| "Integration settings could not be published".to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn validate_config_ancestors(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("Integration settings path must be absolute".into());
    }
    let mut ancestors = path.ancestors().skip(1).collect::<Vec<_>>();
    ancestors.reverse();
    for ancestor in ancestors {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err("Integration settings directory ancestry is unsafe".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("Integration settings directory could not be inspected".into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_settings_default_to_local_category_sharing_and_never_persist_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("integrations.json");
        fs::write(
            &path,
            r#"{
              "version":1,
              "spoolman":null,
              "providers":[{
                "id":"local","label":"Local","config":{
                  "kind":"openaiCompatible","baseUrl":"http://127.0.0.1:11434/",
                  "endpointScope":"local","model":"vision","maxTags":5,
                  "detail":"brief","categoryMode":"allowNew","concurrency":1,
                  "timeoutSeconds":30,"structuredOutput":"plainJson"
                }
              }]
            }"#,
        )
        .unwrap();
        let state = IntegrationState::load(path.clone()).unwrap();
        assert_eq!(
            state.settings().ai_providers[0].category_sharing,
            CategorySharing::Local
        );
        let mut inner = state.shared.inner.lock().unwrap();
        inner.credentials.insert(
            "local".into(),
            SessionSecret::new("never-persist-this".into()).unwrap(),
        );
        state
            .persist_values(inner.spoolman.clone(), &inner.providers)
            .unwrap();
        drop(inner);
        let bytes = fs::read(path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("never-persist-this"));
    }
}
