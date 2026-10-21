use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmationPlan {
    pub action: String,
    pub target_ids: Vec<String>,
    pub expected_versions: Vec<u64>,
    pub library_epoch: u64,
    pub summary: String,
}

pub struct ConfirmationStore {
    entries: Mutex<HashMap<String, Entry>>,
    ttl: Duration,
    max_entries: usize,
}

struct Entry {
    principal_id: String,
    request_hash: [u8; 32],
    plan: ConfirmationPlan,
    expires_at: Instant,
}

impl ConfirmationStore {
    pub fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
            max_entries,
        }
    }

    pub fn issue(
        &self,
        principal_id: &str,
        request_body: &[u8],
        plan: ConfirmationPlan,
    ) -> Result<String, String> {
        validate_plan(&plan)?;
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|_, entry| entry.expires_at > Instant::now());
        if entries.len() >= self.max_entries {
            return Err("Confirmation capacity reached".into());
        }
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(|_| "Secure random generation failed")?;
        let token = URL_SAFE_NO_PAD.encode(random);
        entries.insert(
            token.clone(),
            Entry {
                principal_id: principal_id.into(),
                request_hash: Sha256::digest(request_body).into(),
                plan,
                expires_at: Instant::now() + self.ttl,
            },
        );
        Ok(token)
    }

    pub fn consume(
        &self,
        token: &str,
        principal_id: &str,
        request_body: &[u8],
        library_epoch: u64,
    ) -> Result<ConfirmationPlan, String> {
        let mut entries = self.entries.lock().unwrap();
        let Some(entry) = entries.remove(token) else {
            return Err("Confirmation is expired, replayed, or unknown".into());
        };
        let request_hash: [u8; 32] = Sha256::digest(request_body).into();
        if entry.expires_at <= Instant::now()
            || entry.principal_id != principal_id
            || entry.request_hash != request_hash
            || entry.plan.library_epoch != library_epoch
        {
            return Err("Confirmation does not match the request context".into());
        }
        Ok(entry.plan)
    }
}

fn validate_plan(plan: &ConfirmationPlan) -> Result<(), String> {
    if plan.action.is_empty()
        || plan.action.len() > 64
        || plan.target_ids.is_empty()
        || plan.target_ids.len() > 10_000
        || plan.target_ids.len() != plan.expected_versions.len()
        || plan.summary.is_empty()
        || plan.summary.len() > 4096
    {
        return Err("Confirmation plan is invalid".into());
    }
    Ok(())
}
