use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use library_core::{
    FilamentSyncSummary, Model, ReviewedTag, ReviewedTagStrategy, ReviewedTagTarget,
    SpoolmanFilamentRecord, SpoolmanSpoolRecord, VersionedTarget,
};
use library_integrations::{
    pull_spoolman, suggest_tags, validate_spoolman_config, ProviderConfig, SpoolmanConfig,
    SuggestedTag, SuggestionInput, SuggestionTagContext,
};
use library_runtime::integration_state::{
    public_provider, settings_from_inner, stored_provider, validate_identifier, IntegrationInner,
    IntegrationJobKind, SessionSecret, StoredReview, StoredSpoolman, MAX_PROVIDERS, MAX_REVIEWS,
    MAX_REVIEW_TARGETS, REVIEW_TTL,
};

#[cfg(test)]
use library_integrations::{CategorySharing, EndpointScope, ProviderKind, StructuredOutputMode};
pub use library_runtime::integration_state::{
    AiExistingTag, AiProviderPublicConfig, AiReviewAction, AiReviewDecision, AiReviewStrategy,
    AiSuggestion, AiSuggestionBatch, AiSuggestionFailure, AiSuggestionTarget, CredentialUpdate,
    IntegrationSettings, IntegrationState, SpoolmanPullResult,
};
use sha2::{Digest, Sha256};
use tauri::State;

use crate::extension_api::AppError;
use crate::library_commands::{get_thumbnail, library_error, LibraryState};

#[tauri::command]
pub fn integration_get_settings(state: State<'_, IntegrationState>) -> IntegrationSettings {
    state.settings()
}

#[tauri::command]
pub fn integration_save_spoolman_config(
    state: State<'_, IntegrationState>,
    library: State<'_, LibraryState>,
    config: Option<SpoolmanConfig>,
) -> Result<IntegrationSettings, AppError> {
    if let Some(config) = &config {
        validate_spoolman_config(config).map_err(integration_error)?;
    }
    let stored = if let Some(config) = config {
        let _io = library.begin_io()?;
        let source = library.with_db(|db| {
            db.upsert_spoolman_source(&config.base_url)
                .map_err(library_error)
        })?;
        Some(StoredSpoolman {
            source_id: source.id,
            config,
        })
    } else {
        None
    };
    let mut inner = state.shared.inner.lock().unwrap();
    state
        .persist_values(stored.clone(), &inner.providers)
        .map_err(integration_error)?;
    inner.spoolman = stored;
    IntegrationState::invalidate_locked(&mut inner);
    Ok(settings_from_inner(&inner))
}

#[tauri::command]
pub async fn spoolman_pull(
    state: State<'_, IntegrationState>,
    library: State<'_, LibraryState>,
) -> Result<SpoolmanPullResult, AppError> {
    let state = state.inner().clone();
    let library = library.inner().clone();
    let admission = state
        .begin_job(IntegrationJobKind::Spoolman)
        .map_err(integration_error)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _admission = admission;
        spoolman_pull_impl(&state, &library)
    })
    .await
    .map_err(|_| integration_error("Spoolman worker failed"))?
}

fn spoolman_pull_impl(
    state: &IntegrationState,
    library: &LibraryState,
) -> Result<SpoolmanPullResult, AppError> {
    let (mut stored, mut epoch) = {
        let inner = state.shared.inner.lock().unwrap();
        (
            inner
                .spoolman
                .clone()
                .ok_or_else(|| integration_error("Spoolman is not configured"))?,
            inner.epoch,
        )
    };
    let source = {
        let _io = library.begin_io()?;
        library.with_db(|db| {
            db.upsert_spoolman_source(&stored.config.base_url)
                .map_err(library_error)
        })?
    };
    if source.id != stored.source_id {
        let mut inner = state.shared.inner.lock().unwrap();
        if inner.epoch != epoch
            || inner.spoolman.as_ref().map(|value| &value.config) != Some(&stored.config)
        {
            return Err(integration_error(
                "Spoolman configuration changed before the pull started",
            ));
        }
        stored.source_id = source.id;
        state
            .persist_values(Some(stored.clone()), &inner.providers)
            .map_err(integration_error)?;
        inner.spoolman = Some(stored.clone());
        IntegrationState::invalidate_locked(&mut inner);
        epoch = inner.epoch;
    }
    let generation = library.management_generation();
    let snapshot = pull_spoolman(&stored.config).map_err(integration_error)?;
    let completed_at = now_rfc3339().map_err(integration_error)?;
    let filaments = snapshot
        .filaments
        .iter()
        .map(|value| SpoolmanFilamentRecord {
            external_id: value.external_id.clone(),
            brand: value.brand.clone(),
            material: value.material.clone(),
            color: value.color.clone(),
            name: value.name.clone(),
            notes: value.notes.clone(),
        })
        .collect::<Vec<_>>();
    let spools = snapshot
        .spools
        .iter()
        .map(|value| SpoolmanSpoolRecord {
            external_id: value.external_id.clone(),
            filament_external_id: value.filament_external_id.clone(),
            brand: value.brand.clone(),
            material: value.material.clone(),
            color: value.color.clone(),
            name: value.name.clone(),
            notes: value.notes.clone(),
            archived: value.archived,
        })
        .collect::<Vec<_>>();
    let _io = library.begin_io()?;
    ensure_generation(library, generation)?;
    let inner = state.shared.inner.lock().unwrap();
    if inner.epoch != epoch
        || inner
            .spoolman
            .as_ref()
            .map(|value| (&value.source_id, &value.config))
            != Some((&stored.source_id, &stored.config))
    {
        return Err(integration_error(
            "Spoolman configuration changed while the pull was running",
        ));
    }
    let summary = library.with_db(|db| {
        db.sync_spoolman_snapshot(&stored.source_id, &filaments, &spools, &completed_at)
            .map_err(library_error)
    })?;
    Ok(spoolman_result(&summary, filaments.len(), spools.len()))
}

#[tauri::command]
pub fn integration_save_ai_provider(
    state: State<'_, IntegrationState>,
    config: AiProviderPublicConfig,
    credential_update: CredentialUpdate,
) -> Result<AiProviderPublicConfig, AppError> {
    let stored = stored_provider(&config).map_err(integration_error)?;
    let replacement_secret = match credential_update.action.as_str() {
        "keep" if credential_update.secret.is_none() => None,
        "clear" if credential_update.secret.is_none() => None,
        "setSession" => Some(
            SessionSecret::new(
                credential_update
                    .secret
                    .ok_or_else(|| integration_error("Session credential is missing"))?,
            )
            .map_err(integration_error)?,
        ),
        _ => return Err(integration_error("Credential update is invalid")),
    };
    let mut inner = state.shared.inner.lock().unwrap();
    let existing = inner
        .providers
        .iter()
        .position(|value| value.id == stored.id);
    if existing.is_none() && inner.providers.len() >= MAX_PROVIDERS {
        return Err(integration_error("AI provider limit reached"));
    }
    let mut providers = inner.providers.clone();
    if let Some(index) = existing {
        providers[index] = stored.clone();
    } else {
        providers.push(stored.clone());
    }
    providers.sort_by(|a, b| a.id.cmp(&b.id));
    state
        .persist_values(inner.spoolman.clone(), &providers)
        .map_err(integration_error)?;
    match credential_update.action.as_str() {
        "keep" => {}
        "clear" => {
            inner.credentials.remove(&stored.id);
        }
        "setSession" => {
            inner
                .credentials
                .insert(stored.id.clone(), replacement_secret.unwrap());
        }
        _ => unreachable!(),
    }
    inner.providers = providers;
    IntegrationState::invalidate_locked(&mut inner);
    Ok(public_provider(
        &stored,
        inner.credentials.contains_key(&stored.id),
    ))
}

#[tauri::command]
pub fn integration_delete_ai_provider(
    state: State<'_, IntegrationState>,
    provider_id: String,
) -> Result<(), AppError> {
    validate_identifier(&provider_id, "Provider").map_err(integration_error)?;
    let mut inner = state.shared.inner.lock().unwrap();
    let mut providers = inner.providers.clone();
    let before = providers.len();
    providers.retain(|provider| provider.id != provider_id);
    if providers.len() == before {
        return Err(integration_error("AI provider not found"));
    }
    state
        .persist_values(inner.spoolman.clone(), &providers)
        .map_err(integration_error)?;
    inner.providers = providers;
    inner.credentials.remove(&provider_id);
    IntegrationState::invalidate_locked(&mut inner);
    Ok(())
}

#[tauri::command]
pub async fn ai_suggest_tags(
    state: State<'_, IntegrationState>,
    library: State<'_, LibraryState>,
    provider_id: String,
    targets: Vec<VersionedTarget>,
) -> Result<AiSuggestionBatch, AppError> {
    let state = state.inner().clone();
    let library = library.inner().clone();
    let admission = state
        .begin_job(IntegrationJobKind::Ai)
        .map_err(integration_error)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _admission = admission;
        ai_suggest_tags_impl(&state, &library, provider_id, targets)
    })
    .await
    .map_err(|_| integration_error("AI suggestion worker failed"))?
}

fn ai_suggest_tags_impl(
    state: &IntegrationState,
    library: &LibraryState,
    provider_id: String,
    targets: Vec<VersionedTarget>,
) -> Result<AiSuggestionBatch, AppError> {
    validate_identifier(&provider_id, "Provider").map_err(integration_error)?;
    validate_targets(&targets)?;
    let (provider, credential, epoch) = {
        let inner = state.shared.inner.lock().unwrap();
        let provider = inner
            .providers
            .iter()
            .find(|value| value.id == provider_id)
            .cloned()
            .ok_or_else(|| integration_error("AI provider not found"))?;
        let credential = inner
            .credentials
            .get(&provider_id)
            .map(SessionSecret::temporary_copy);
        (provider, credential, inner.epoch)
    };
    let generation = library.management_generation();
    let mut inputs = Vec::new();
    let mut failures = Vec::new();
    let input_io = library.begin_io()?;
    let taxonomy = library.with_db(|db| {
        db.list_tags().map_err(library_error).map(|tags| {
            tags.into_iter()
                .map(|tag| (tag.name.to_lowercase(), tag.category))
                .collect::<HashMap<_, _>>()
        })
    })?;
    let allowed_categories = taxonomy
        .values()
        .filter_map(Clone::clone)
        .collect::<HashSet<_>>();
    let mut existing_categories = allowed_categories.iter().cloned().collect::<Vec<_>>();
    existing_categories.sort();
    for target in &targets {
        let model = library.with_db(|db| db.get_model(&target.model_id).map_err(library_error))?;
        let Some(model) = model else {
            return Err(integration_error("AI target model not found"));
        };
        if model.metadata_version != target.expected_version {
            return Err(integration_error("AI target metadata version changed"));
        }
        match get_thumbnail(library, &model.id, false) {
            Ok(Some(preview_data_url)) => {
                let existing_tags: Vec<AiExistingTag> = model
                    .tags
                    .iter()
                    .map(|name| AiExistingTag {
                        name: name.clone(),
                        category: taxonomy.get(&name.to_lowercase()).cloned().flatten(),
                    })
                    .collect();
                inputs.push((target.clone(), model, existing_tags, preview_data_url));
            }
            Ok(None) => failures.push(AiSuggestionFailure {
                model_id: model.id,
                code: "previewUnavailable".into(),
                message: "A bounded preview is unavailable for this model".into(),
            }),
            Err(_) => failures.push(AiSuggestionFailure {
                model_id: model.id,
                code: "previewError".into(),
                message: "A bounded preview could not be prepared for this model".into(),
            }),
        }
    }
    drop(input_io);
    let queue = Arc::new(Mutex::new(VecDeque::from(inputs)));
    let output = Arc::new(Mutex::new(Vec::new()));
    std::thread::scope(|scope| {
        for _ in 0..usize::from(provider.config.concurrency) {
            let queue = Arc::clone(&queue);
            let output = Arc::clone(&output);
            let provider = &provider;
            let credential = credential.as_ref().map(|value| value.as_str());
            let allowed_categories = &allowed_categories;
            let taxonomy = &taxonomy;
            let existing_categories = &existing_categories;
            scope.spawn(move || loop {
                let Some((target, model, existing_tags, preview_data_url)) =
                    queue.lock().unwrap().pop_front()
                else {
                    break;
                };
                let input = SuggestionInput {
                    model_id: model.id.clone(),
                    display_name: model.display_name.clone(),
                    existing_tags: model.tags.clone(),
                    assigned_tags: existing_tags
                        .iter()
                        .map(|tag| SuggestionTagContext {
                            name: tag.name.clone(),
                            category: tag.category.clone(),
                        })
                        .collect(),
                    existing_categories: existing_categories.clone(),
                    preview_data_url,
                };
                let result =
                    suggest_tags(&provider.config, credential, &input).and_then(|mut tags| {
                        validate_category_policy(&provider.config, allowed_categories, &tags)?;
                        canonicalize_known_tags(taxonomy, &mut tags)?;
                        Ok(tags)
                    });
                output
                    .lock()
                    .unwrap()
                    .push((target, model, existing_tags, result));
            });
        }
    });
    let mut suggestions = output.lock().unwrap().drain(..).collect::<Vec<_>>();
    suggestions.sort_by_key(|(target, _, _, _)| {
        targets
            .iter()
            .position(|value| value.model_id == target.model_id)
            .unwrap_or(usize::MAX)
    });
    let mut review_targets = Vec::new();
    for (target, model, existing_tags, result) in suggestions {
        match result {
            Ok(tags) => review_targets.push(AiSuggestionTarget {
                model_id: target.model_id,
                expected_version: target.expected_version,
                display_name: model.display_name,
                existing_tags,
                suggestions: suggestion_dtos(&model.id, tags),
            }),
            Err(_) => failures.push(AiSuggestionFailure {
                model_id: target.model_id,
                code: "providerError".into(),
                message: "The provider did not return valid tag suggestions".into(),
            }),
        }
    }
    let validation_io = library.begin_io()?;
    ensure_generation(library, generation)?;
    for target in &review_targets {
        let model = library.with_db(|db| db.get_model(&target.model_id).map_err(library_error))?;
        if model.as_ref().map(|model| model.metadata_version) != Some(target.expected_version) {
            return Err(integration_error(
                "Model metadata changed while suggestions were running",
            ));
        }
    }
    drop(validation_io);
    let mut inner = state.shared.inner.lock().unwrap();
    if inner.epoch != epoch
        || !inner
            .providers
            .iter()
            .any(|value| value.id == provider_id && value.config == provider.config)
    {
        return Err(integration_error(
            "AI provider configuration changed while suggestions were running",
        ));
    }
    prune_reviews(&mut inner);
    let token = new_review_token(&mut inner, &provider_id);
    inner.reviews.push_back(StoredReview {
        token: token.clone(),
        epoch,
        library_generation: generation,
        created_at: SystemTime::now(),
        category_mode: provider.config.category_mode.clone(),
        known_tags: taxonomy,
        targets: review_targets.clone(),
        applying: false,
    });
    while inner.reviews.len() > MAX_REVIEWS {
        inner.reviews.pop_front();
    }
    Ok(AiSuggestionBatch {
        review_token: token,
        provider_id,
        model: provider.config.model,
        category_sharing: provider.config.category_sharing,
        targets: review_targets,
        failures,
    })
}

#[tauri::command]
pub fn ai_apply_tag_review(
    state: State<'_, IntegrationState>,
    library: State<'_, LibraryState>,
    review_token: String,
    decisions: Vec<AiReviewDecision>,
    strategy: AiReviewStrategy,
) -> Result<Vec<Model>, AppError> {
    apply_review_impl(&state, &library, review_token, decisions, strategy)
}

fn apply_review_impl(
    state: &IntegrationState,
    library: &LibraryState,
    review_token: String,
    decisions: Vec<AiReviewDecision>,
    strategy: AiReviewStrategy,
) -> Result<Vec<Model>, AppError> {
    let _io = library.begin_io()?;
    let mut inner = state.shared.inner.lock().unwrap();
    prune_reviews(&mut inner);
    let index = inner
        .reviews
        .iter()
        .position(|review| review.token == review_token)
        .ok_or_else(|| integration_error("AI review token is expired, replayed, or unknown"))?;
    let review = &inner.reviews[index];
    if review.epoch != inner.epoch || review.library_generation != library.management_generation() {
        inner.reviews.remove(index);
        return Err(integration_error("AI review became stale"));
    }
    if review.applying {
        return Err(integration_error("AI review is already being applied"));
    }
    let updates = reviewed_updates(review, &decisions)?;
    let category_mode = review.category_mode.clone();
    let known_tags = review.known_tags.clone();
    let all_targets = review
        .targets
        .iter()
        .map(|target| (target.model_id.clone(), target.expected_version))
        .collect::<Vec<_>>();
    inner.reviews[index].applying = true;
    drop(inner);
    let result = library.with_db(|db| {
        let current_tags = db
            .list_tags()
            .map_err(library_error)?
            .into_iter()
            .map(|tag| (tag.name.to_lowercase(), tag.category))
            .collect::<HashMap<_, _>>();
        if updates.iter().flat_map(|target| &target.tags).any(|tag| {
            let key = tag.name.to_lowercase();
            known_tags.contains_key(&key) && current_tags.get(&key) != known_tags.get(&key)
        }) {
            return Err(integration_error(
                "A known tag changed while the review was open",
            ));
        }
        if category_mode == "existingOnly" {
            let categories = current_tags
                .values()
                .filter_map(Clone::clone)
                .collect::<HashSet<_>>();
            if updates.iter().flat_map(|target| &target.tags).any(|tag| {
                tag.category
                    .as_ref()
                    .is_some_and(|category| !categories.contains(category))
            }) {
                return Err(integration_error(
                    "The existing tag taxonomy changed while the review was open",
                ));
            }
        }
        for (model_id, expected_version) in &all_targets {
            let model = db
                .get_model(model_id)
                .map_err(library_error)?
                .ok_or_else(|| integration_error("AI review target model not found"))?;
            if model.metadata_version != *expected_version {
                return Err(integration_error(
                    "AI review target metadata version changed",
                ));
            }
        }
        if !updates.is_empty() {
            db.apply_reviewed_tags(&updates, reviewed_strategy(strategy))
                .map_err(library_error)?;
        }
        all_targets
            .iter()
            .map(|(model_id, _)| {
                db.get_model(model_id)
                    .map_err(library_error)?
                    .ok_or_else(|| integration_error("AI review target model not found"))
            })
            .collect()
    });
    let mut inner = state.shared.inner.lock().unwrap();
    if let Some(index) = inner
        .reviews
        .iter()
        .position(|review| review.token == review_token)
    {
        if result.is_ok() {
            inner.reviews.remove(index);
        } else {
            inner.reviews[index].applying = false;
        }
    }
    result
}

#[tauri::command]
pub fn ai_discard_tag_review(
    state: State<'_, IntegrationState>,
    review_token: String,
) -> Result<(), AppError> {
    let mut inner = state.shared.inner.lock().unwrap();
    let index = inner
        .reviews
        .iter()
        .position(|review| review.token == review_token)
        .ok_or_else(|| integration_error("AI review token is expired, replayed, or unknown"))?;
    inner.reviews.remove(index);
    Ok(())
}

fn validate_targets(targets: &[VersionedTarget]) -> Result<(), AppError> {
    if targets.is_empty() || targets.len() > MAX_REVIEW_TARGETS {
        return Err(integration_error("AI target count is outside its limit"));
    }
    let mut ids = HashSet::new();
    if targets.iter().any(|target| {
        validate_identifier(&target.model_id, "Model").is_err() || !ids.insert(&target.model_id)
    }) {
        return Err(integration_error(
            "AI targets contain invalid or duplicate models",
        ));
    }
    Ok(())
}

fn suggestion_dtos(model_id: &str, tags: Vec<SuggestedTag>) -> Vec<AiSuggestion> {
    tags.into_iter()
        .enumerate()
        .map(|(index, tag)| AiSuggestion {
            suggestion_id: format!("{model_id}:{index}"),
            name: tag.name,
            category: tag.category,
            confidence: tag.confidence,
        })
        .collect()
}

fn validate_category_policy(
    config: &ProviderConfig,
    allowed_categories: &HashSet<String>,
    tags: &[SuggestedTag],
) -> Result<(), String> {
    match config.category_mode.as_str() {
        "none" if tags.iter().any(|tag| tag.category.is_some()) => {
            Err("Provider returned categories while categories are disabled".into())
        }
        "existingOnly"
            if tags.iter().any(|tag| {
                tag.category
                    .as_ref()
                    .is_some_and(|category| !allowed_categories.contains(category))
            }) =>
        {
            Err("Provider returned a category outside the existing taxonomy".into())
        }
        "none" | "existingOnly" | "allowNew" => Ok(()),
        _ => Err("AI category policy is invalid".into()),
    }
}

fn canonicalize_known_tags(
    taxonomy: &HashMap<String, Option<String>>,
    tags: &mut [SuggestedTag],
) -> Result<(), String> {
    for tag in tags {
        let key = tag.name.to_lowercase();
        if let Some(actual_category) = taxonomy.get(&key) {
            if tag.category.is_some() && tag.category.as_ref() != actual_category.as_ref() {
                return Err("Provider tag category conflicts with the existing taxonomy".into());
            }
            tag.category = actual_category.clone();
        }
    }
    Ok(())
}

fn reviewed_updates(
    review: &StoredReview,
    decisions: &[AiReviewDecision],
) -> Result<Vec<ReviewedTagTarget>, AppError> {
    if decisions.len() != review.targets.len() {
        return Err(integration_error(
            "Every review target requires an explicit accept or reject decision",
        ));
    }
    let mut decision_ids = HashSet::new();
    let mut updates = Vec::with_capacity(review.targets.len());
    if !matches!(
        review.category_mode.as_str(),
        "none" | "existingOnly" | "allowNew"
    ) {
        return Err(integration_error("AI review category policy is invalid"));
    }
    for decision in decisions {
        if !decision_ids.insert(decision.model_id.as_str()) {
            return Err(integration_error("AI review contains duplicate decisions"));
        }
        let target = review
            .targets
            .iter()
            .find(|target| target.model_id == decision.model_id)
            .ok_or_else(|| integration_error("AI review decision targets an unrelated model"))?;
        if matches!(decision.action, AiReviewAction::Reject) {
            if !decision.accepted_suggestion_ids.is_empty() {
                return Err(integration_error(
                    "Rejected AI review targets cannot accept suggestions",
                ));
            }
            continue;
        }
        let mut accepted_ids = HashSet::new();
        let mut tags = Vec::new();
        for suggestion_id in &decision.accepted_suggestion_ids {
            if !accepted_ids.insert(suggestion_id) {
                return Err(integration_error("AI review repeats a suggestion"));
            }
            let suggestion = target
                .suggestions
                .iter()
                .find(|suggestion| suggestion.suggestion_id == *suggestion_id)
                .ok_or_else(|| integration_error("AI review accepts an unknown suggestion"))?;
            tags.push(ReviewedTag {
                name: suggestion.name.clone(),
                category: suggestion.category.clone(),
            });
        }
        updates.push(ReviewedTagTarget {
            model_id: target.model_id.clone(),
            expected_version: target.expected_version,
            tags,
        });
    }
    Ok(updates)
}

fn reviewed_strategy(value: AiReviewStrategy) -> ReviewedTagStrategy {
    match value {
        AiReviewStrategy::Append => ReviewedTagStrategy::Append,
        AiReviewStrategy::Replace => ReviewedTagStrategy::Replace,
        AiReviewStrategy::Merge => ReviewedTagStrategy::Merge,
    }
}

fn prune_reviews(inner: &mut IntegrationInner) {
    inner.reviews.retain(|review| {
        review
            .created_at
            .elapsed()
            .is_ok_and(|elapsed| elapsed <= REVIEW_TTL)
    });
}

fn new_review_token(inner: &mut IntegrationInner, provider_id: &str) -> String {
    inner.sequence = inner.sequence.wrapping_add(1);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hash = Sha256::new();
    hash.update(nanos.to_le_bytes());
    hash.update(inner.sequence.to_le_bytes());
    hash.update(inner.epoch.to_le_bytes());
    hash.update(provider_id.as_bytes());
    format!("{:x}", hash.finalize())
}

fn spoolman_result(
    summary: &FilamentSyncSummary,
    filaments: usize,
    spools: usize,
) -> SpoolmanPullResult {
    SpoolmanPullResult {
        filaments_received: filaments,
        spools_received: spools,
        filaments_upserted: filaments,
        spools_upserted: spools,
        removed_locally: summary.missing,
        last_successful_pull_at: summary.completed_at.clone(),
    }
}

fn ensure_generation(library: &LibraryState, expected: u64) -> Result<(), AppError> {
    if library.management_generation() != expected {
        return Err(integration_error(
            "The Library changed while the integration request was running",
        ));
    }
    Ok(())
}

fn now_rfc3339() -> Result<String, String> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| "Current time could not be formatted".into())
}

fn integration_error(message: impl Into<String>) -> AppError {
    AppError::Library {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use library_core::{MetadataPatch, TagMode};
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use tempfile::TempDir;

    fn provider() -> AiProviderPublicConfig {
        AiProviderPublicConfig {
            id: "local".into(),
            label: "Local model".into(),
            kind: ProviderKind::OpenaiCompatible,
            base_url: Some("http://127.0.0.1:11434/v1/".into()),
            endpoint_scope: EndpointScope::Local,
            model: "vision".into(),
            credential_storage: "none".into(),
            has_credential: false,
            max_tags: 5,
            detail: "brief".into(),
            category_mode: "allowNew".into(),
            concurrency: 1,
            timeout_seconds: 30,
            structured_output: StructuredOutputMode::PlainJson,
            category_sharing: CategorySharing::Local,
        }
    }

    fn mock_server(responses: Vec<String>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 4096];
                let header_end = loop {
                    let read = stream.read(&mut buffer).unwrap();
                    if read == 0 {
                        break request.len();
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if let Some(position) = request.windows(4).position(|part| part == b"\r\n\r\n")
                    {
                        break position + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                while request.len().saturating_sub(header_end) < content_length {
                    let read = stream.read(&mut buffer).unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (format!("http://{address}/"), worker)
    }

    fn json_response(body: &str, extra_headers: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn provider_response(tag: &str) -> String {
        let content =
            serde_json::json!({"tags":[{"name":tag,"category":null,"confidence":0.9}]}).to_string();
        json_response(
            &serde_json::json!({"choices":[{"finish_reason":"stop","message":{"content":content}}]}).to_string(),
            "",
        )
    }

    fn ascii_stl(name: &str) -> String {
        format!("solid {name}\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid {name}\n")
    }

    fn integration_fixture(
        base_url: String,
        models: usize,
    ) -> (TempDir, IntegrationState, LibraryState, Vec<Model>) {
        let temp = TempDir::new().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let state = IntegrationState::load(root.join("config/integrations.json")).unwrap();
        let library = LibraryState::new(root.join("library"));
        let mut public = provider();
        public.base_url = Some(base_url);
        let stored = stored_provider(&public).unwrap();
        state.shared.inner.lock().unwrap().providers.push(stored);
        let mut imported = Vec::new();
        for index in 0..models {
            let source = root.join(format!("source-{index}.stl"));
            fs::write(&source, ascii_stl(&format!("part-{index}"))).unwrap();
            let model = library
                .with_db(|db| {
                    db.import_file(&source, None, None)
                        .map(|result| result.model)
                        .map_err(library_error)
                })
                .unwrap();
            imported.push(model);
        }
        (temp, state, library, imported)
    }

    #[test]
    fn persisted_settings_never_contain_session_credentials() {
        let temp = TempDir::new().unwrap();
        let canonical_temp = fs::canonicalize(temp.path()).unwrap();
        let path = canonical_temp.join("integrations.json");
        let state = IntegrationState::load(path.clone()).unwrap();
        let stored = stored_provider(&provider()).unwrap();
        let mut inner = state.shared.inner.lock().unwrap();
        inner.providers.push(stored);
        inner.credentials.insert(
            "local".into(),
            SessionSecret::new("top-secret".into()).unwrap(),
        );
        state
            .persist_values(inner.spoolman.clone(), &inner.providers)
            .unwrap();
        drop(inner);
        let bytes = fs::read(path).unwrap();
        assert!(!String::from_utf8(bytes).unwrap().contains("top-secret"));
        let reloaded = IntegrationState::load(canonical_temp.join("integrations.json")).unwrap();
        assert!(!reloaded.settings().ai_providers[0].has_credential);
    }

    #[test]
    fn explicit_decisions_cannot_cross_review_targets_or_suggestions() {
        let review = StoredReview {
            token: "token".into(),
            epoch: 1,
            library_generation: 1,
            created_at: SystemTime::now(),
            category_mode: "existingOnly".into(),
            known_tags: HashMap::new(),
            targets: vec![AiSuggestionTarget {
                model_id: "model_a".into(),
                expected_version: 3,
                display_name: "A".into(),
                existing_tags: Vec::new(),
                suggestions: vec![AiSuggestion {
                    suggestion_id: "model_a:0".into(),
                    name: "gear".into(),
                    category: Some("type".into()),
                    confidence: Some(0.9),
                }],
            }],
            applying: false,
        };
        assert!(reviewed_updates(&review, &[]).is_err());
        assert!(reviewed_updates(
            &review,
            &[AiReviewDecision {
                model_id: "model_b".into(),
                action: AiReviewAction::Reject,
                accepted_suggestion_ids: vec![],
            }]
        )
        .is_err());
        assert!(reviewed_updates(
            &review,
            &[AiReviewDecision {
                model_id: "model_a".into(),
                action: AiReviewAction::Accept,
                accepted_suggestion_ids: vec!["model_a:9".into()],
            }]
        )
        .is_err());
        assert!(reviewed_updates(
            &review,
            &[AiReviewDecision {
                model_id: "model_a".into(),
                action: AiReviewAction::Reject,
                accepted_suggestion_ids: vec![],
            }]
        )
        .unwrap()
        .is_empty());
        let accepted_clear = reviewed_updates(
            &review,
            &[AiReviewDecision {
                model_id: "model_a".into(),
                action: AiReviewAction::Accept,
                accepted_suggestion_ids: vec![],
            }],
        )
        .unwrap();
        assert_eq!(accepted_clear.len(), 1);
        assert!(accepted_clear[0].tags.is_empty());
    }

    #[test]
    fn category_policy_rejects_new_or_disabled_categories() {
        let mut config = stored_provider(&provider()).unwrap().config;
        let tags = vec![SuggestedTag {
            name: "gear".into(),
            category: Some("type".into()),
            confidence: None,
        }];
        config.category_mode = "existingOnly".into();
        assert!(validate_category_policy(&config, &HashSet::new(), &tags).is_err());
        assert!(
            validate_category_policy(&config, &HashSet::from(["type".to_string()]), &tags).is_ok()
        );
        config.category_mode = "none".into();
        assert!(
            validate_category_policy(&config, &HashSet::from(["type".to_string()]), &tags).is_err()
        );
    }

    #[test]
    fn concurrent_integration_jobs_are_refused_and_raii_releases_admission() {
        let temp = TempDir::new().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let state = IntegrationState::load(root.join("integrations.json")).unwrap();
        let ai = state.begin_job(IntegrationJobKind::Ai).unwrap();
        assert!(state.begin_job(IntegrationJobKind::Ai).is_err());
        let spoolman = state.begin_job(IntegrationJobKind::Spoolman).unwrap();
        assert!(state.begin_job(IntegrationJobKind::Spoolman).is_err());
        drop(ai);
        drop(spoolman);
        assert!(state.begin_job(IntegrationJobKind::Ai).is_ok());
        assert!(state.begin_job(IntegrationJobKind::Spoolman).is_ok());
    }

    #[test]
    fn localhost_preview_review_reject_is_noop_and_replay_is_blocked() {
        let (base_url, worker) = mock_server(vec![provider_response("gear")]);
        let (_temp, state, library, models) = integration_fixture(base_url, 1);
        let batch = ai_suggest_tags_impl(
            &state,
            &library,
            "local".into(),
            vec![VersionedTarget {
                model_id: models[0].id.clone(),
                expected_version: models[0].metadata_version,
            }],
        )
        .unwrap();
        assert_eq!(batch.targets.len(), 1);
        assert_eq!(batch.targets[0].suggestions[0].name, "gear");
        let review_token = batch.review_token.clone();
        let decisions = vec![AiReviewDecision {
            model_id: models[0].id.clone(),
            action: AiReviewAction::Reject,
            accepted_suggestion_ids: Vec::new(),
        }];
        let applied = apply_review_impl(
            &state,
            &library,
            review_token.clone(),
            decisions,
            AiReviewStrategy::Replace,
        )
        .unwrap();
        let current = &applied[0];
        assert_eq!(current.metadata_version, models[0].metadata_version);
        assert!(apply_review_impl(
            &state,
            &library,
            review_token,
            vec![AiReviewDecision {
                model_id: models[0].id.clone(),
                action: AiReviewAction::Reject,
                accepted_suggestion_ids: Vec::new(),
            }],
            AiReviewStrategy::Replace,
        )
        .is_err());
        assert!(state.shared.inner.lock().unwrap().reviews.is_empty());
        worker.join().unwrap();
    }

    #[test]
    fn mixed_preview_failure_continues_valid_models() {
        let (base_url, worker) = mock_server(vec![provider_response("printable")]);
        let (_temp, state, library, models) = integration_fixture(base_url, 2);
        let bad_path = library
            .root()
            .join(models[1].storage_path.as_ref().unwrap());
        fs::remove_file(bad_path).unwrap();
        let batch = ai_suggest_tags_impl(
            &state,
            &library,
            "local".into(),
            models
                .iter()
                .map(|model| VersionedTarget {
                    model_id: model.id.clone(),
                    expected_version: model.metadata_version,
                })
                .collect(),
        )
        .unwrap();
        assert_eq!(batch.targets.len(), 1);
        assert_eq!(batch.failures.len(), 1);
        assert_eq!(batch.failures[0].model_id, models[1].id);
        worker.join().unwrap();
    }

    #[test]
    fn model_edit_after_provider_request_prevents_review_creation() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/", listener.local_addr().unwrap());
        let (_temp, state, library, models) = integration_fixture(base_url, 1);
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            accepted_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            stream
                .write_all(provider_response("gear").as_bytes())
                .unwrap();
        });
        let state_worker = state.clone();
        let library_worker = library.clone();
        let target = VersionedTarget {
            model_id: models[0].id.clone(),
            expected_version: models[0].metadata_version,
        };
        let request = thread::spawn(move || {
            ai_suggest_tags_impl(&state_worker, &library_worker, "local".into(), vec![target])
        });
        accepted_rx.recv().unwrap();
        library
            .with_db(|db| {
                db.update_model_metadata(
                    &models[0].id,
                    models[0].metadata_version,
                    &MetadataPatch {
                        notes: Some("changed".into()),
                        ..MetadataPatch::default()
                    },
                )
                .map(|_| ())
                .map_err(library_error)
            })
            .unwrap();
        release_tx.send(()).unwrap();
        assert!(request.join().unwrap().is_err());
        assert!(state.shared.inner.lock().unwrap().reviews.is_empty());
        server.join().unwrap();
    }

    #[test]
    fn provider_replacement_during_request_discards_results() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/", listener.local_addr().unwrap());
        let (_temp, state, library, models) = integration_fixture(base_url, 1);
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            accepted_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            stream
                .write_all(provider_response("gear").as_bytes())
                .unwrap();
        });
        let request_state = state.clone();
        let request_library = library.clone();
        let target = VersionedTarget {
            model_id: models[0].id.clone(),
            expected_version: models[0].metadata_version,
        };
        let request = thread::spawn(move || {
            ai_suggest_tags_impl(
                &request_state,
                &request_library,
                "local".into(),
                vec![target],
            )
        });
        accepted_rx.recv().unwrap();
        {
            let mut inner = state.shared.inner.lock().unwrap();
            inner.providers[0].config.model = "replacement".into();
            IntegrationState::invalidate_locked(&mut inner);
        }
        release_tx.send(()).unwrap();
        assert!(request.join().unwrap().is_err());
        assert!(state.shared.inner.lock().unwrap().reviews.is_empty());
        server.join().unwrap();
    }

    #[test]
    fn maintenance_generation_change_during_request_discards_results() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/", listener.local_addr().unwrap());
        let (_temp, state, library, models) = integration_fixture(base_url, 1);
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            accepted_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            stream
                .write_all(provider_response("gear").as_bytes())
                .unwrap();
        });
        let request_state = state.clone();
        let request_library = library.clone();
        let target = VersionedTarget {
            model_id: models[0].id.clone(),
            expected_version: models[0].metadata_version,
        };
        let request = thread::spawn(move || {
            ai_suggest_tags_impl(
                &request_state,
                &request_library,
                "local".into(),
                vec![target],
            )
        });
        accepted_rx.recv().unwrap();
        let guard = library.begin_maintenance().unwrap();
        guard.invalidate();
        drop(guard);
        release_tx.send(()).unwrap();
        assert!(request.join().unwrap().is_err());
        assert!(state.shared.inner.lock().unwrap().reviews.is_empty());
        server.join().unwrap();
    }

    #[test]
    fn apply_conflict_retains_review_for_retry() {
        let (base_url, worker) = mock_server(vec![provider_response("gear")]);
        let (_temp, state, library, models) = integration_fixture(base_url, 1);
        let batch = ai_suggest_tags_impl(
            &state,
            &library,
            "local".into(),
            vec![VersionedTarget {
                model_id: models[0].id.clone(),
                expected_version: models[0].metadata_version,
            }],
        )
        .unwrap();
        library
            .with_db(|db| {
                db.update_model_metadata(
                    &models[0].id,
                    models[0].metadata_version,
                    &MetadataPatch {
                        notes: Some("changed after review".into()),
                        ..MetadataPatch::default()
                    },
                )
                .map(|_| ())
                .map_err(library_error)
            })
            .unwrap();
        let result = apply_review_impl(
            &state,
            &library,
            batch.review_token.clone(),
            vec![AiReviewDecision {
                model_id: models[0].id.clone(),
                action: AiReviewAction::Accept,
                accepted_suggestion_ids: vec![batch.targets[0].suggestions[0]
                    .suggestion_id
                    .clone()],
            }],
            AiReviewStrategy::Append,
        );
        assert!(result.is_err());
        let inner = state.shared.inner.lock().unwrap();
        let retained = inner
            .reviews
            .iter()
            .find(|review| review.token == batch.review_token)
            .unwrap();
        assert!(!retained.applying);
        worker.join().unwrap();
    }

    #[test]
    fn existing_only_review_revalidates_taxonomy_at_apply() {
        let content =
            serde_json::json!({"tags":[{"name":"gear","category":"type","confidence":0.9}]})
                .to_string();
        let response = json_response(
            &serde_json::json!({"choices":[{"finish_reason":"stop","message":{"content":content}}]}).to_string(),
            "",
        );
        let (base_url, worker) = mock_server(vec![response]);
        let (_temp, state, library, models) = integration_fixture(base_url, 1);
        let tag = library
            .with_db(|db| {
                db.create_tag("fixture", Some("type"))
                    .map_err(library_error)
            })
            .unwrap();
        state.shared.inner.lock().unwrap().providers[0]
            .config
            .category_mode = "existingOnly".into();
        let batch = ai_suggest_tags_impl(
            &state,
            &library,
            "local".into(),
            vec![VersionedTarget {
                model_id: models[0].id.clone(),
                expected_version: models[0].metadata_version,
            }],
        )
        .unwrap();
        library
            .with_db(|db| db.delete_tag(&tag.id).map_err(library_error))
            .unwrap();
        assert!(apply_review_impl(
            &state,
            &library,
            batch.review_token.clone(),
            vec![AiReviewDecision {
                model_id: models[0].id.clone(),
                action: AiReviewAction::Accept,
                accepted_suggestion_ids: vec![batch.targets[0].suggestions[0]
                    .suggestion_id
                    .clone()],
            }],
            AiReviewStrategy::Merge,
        )
        .is_err());
        assert!(state
            .shared
            .inner
            .lock()
            .unwrap()
            .reviews
            .iter()
            .any(|review| review.token == batch.review_token));
        worker.join().unwrap();
    }

    #[test]
    fn known_tag_category_is_canonical_in_preview_and_merge_result() {
        let (base_url, worker) = mock_server(vec![provider_response("gear")]);
        let (_temp, state, library, models) = integration_fixture(base_url, 1);
        let (gear, bolt) = library
            .with_db(|db| {
                let gear = db.create_tag("gear", Some("type")).map_err(library_error)?;
                let bolt = db.create_tag("bolt", Some("type")).map_err(library_error)?;
                Ok((gear, bolt))
            })
            .unwrap();
        let model = library
            .with_db(|db| {
                db.update_model_metadata(
                    &models[0].id,
                    models[0].metadata_version,
                    &MetadataPatch {
                        tags: Some(vec![bolt.id]),
                        tag_mode: Some(TagMode::Replace),
                        ..MetadataPatch::default()
                    },
                )
                .map_err(library_error)
            })
            .unwrap();
        state.shared.inner.lock().unwrap().providers[0]
            .config
            .category_mode = "none".into();
        let batch = ai_suggest_tags_impl(
            &state,
            &library,
            "local".into(),
            vec![VersionedTarget {
                model_id: model.id.clone(),
                expected_version: model.metadata_version,
            }],
        )
        .unwrap();
        assert_eq!(
            batch.targets[0].existing_tags[0].category.as_deref(),
            Some("type")
        );
        assert_eq!(
            batch.targets[0].suggestions[0].category.as_deref(),
            Some("type")
        );
        let applied = apply_review_impl(
            &state,
            &library,
            batch.review_token,
            vec![AiReviewDecision {
                model_id: model.id,
                action: AiReviewAction::Accept,
                accepted_suggestion_ids: vec![batch.targets[0].suggestions[0]
                    .suggestion_id
                    .clone()],
            }],
            AiReviewStrategy::Merge,
        )
        .unwrap();
        assert_eq!(applied[0].tags, vec![gear.name]);
        worker.join().unwrap();
    }

    #[test]
    fn spoolman_wrapper_commits_complete_snapshot_and_keeps_it_on_failed_pull() {
        let filaments = r#"[{"id":1,"name":"Black","material":"PLA","vendor":{"name":"Acme"}}]"#;
        let spools = r#"[{"id":8,"archived":true,"filament":{"id":1,"name":"Black","material":"PLA","vendor":{"name":"Acme"}}}]"#;
        let (base_url, worker) = mock_server(vec![
            json_response(filaments, "x-total-count: 1\r\n"),
            json_response(spools, "x-total-count: 1\r\n"),
            json_response(filaments, "x-total-count: 1\r\n"),
            json_response(
                r#"[{"id":8,"archived":"yes","filament":{"id":1,"name":"Black","material":"PLA","vendor":{"name":"Acme"}}}]"#,
                "x-total-count: 1\r\n",
            ),
        ]);
        let temp = TempDir::new().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let library = LibraryState::new(root.join("library"));
        let source = library
            .with_db(|db| db.upsert_spoolman_source(&base_url).map_err(library_error))
            .unwrap();
        let state = IntegrationState::load(root.join("config/integrations.json")).unwrap();
        state.shared.inner.lock().unwrap().spoolman = Some(StoredSpoolman {
            source_id: "stale-restored-source".into(),
            config: SpoolmanConfig {
                base_url,
                endpoint_scope: EndpointScope::Local,
                page_size: 100,
                timeout_seconds: 5,
            },
        });
        let first = spoolman_pull_impl(&state, &library).unwrap();
        assert_eq!(first.filaments_received, 1);
        assert_eq!(first.spools_received, 1);
        assert_eq!(
            state
                .shared
                .inner
                .lock()
                .unwrap()
                .spoolman
                .as_ref()
                .unwrap()
                .source_id,
            source.id
        );
        assert!(spoolman_pull_impl(&state, &library).is_err());
        let retained = library
            .with_db(|db| db.list_filaments(true).map_err(library_error))
            .unwrap();
        assert_eq!(retained.len(), 2);
        assert!(retained
            .iter()
            .any(|entry| entry.archived && !entry.source_missing));
        worker.join().unwrap();
    }
}
