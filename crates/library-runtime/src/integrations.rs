use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use library_core::{
    ReviewedTag, ReviewedTagStrategy, ReviewedTagTarget, SpoolmanFilamentRecord,
    SpoolmanSpoolRecord,
};
use library_integrations::{
    IntegrationTransport, SuggestedTag, SuggestionInput, SuggestionTagContext,
};
use library_protocol::{
    AiProviderConfig, Capability, IntegrationAiExistingTag, IntegrationAiReviewAction,
    IntegrationAiReviewDecision, IntegrationAiReviewStrategy, IntegrationAiSuggestion,
    IntegrationAiSuggestionBatch, IntegrationAiSuggestionFailure, IntegrationAiSuggestionTarget,
    IntegrationCategorySharing, IntegrationCredentialUpdate, IntegrationEndpointScope,
    IntegrationEngine, IntegrationProviderKind, IntegrationSettings, IntegrationSpoolmanConfig,
    IntegrationSpoolmanPullResult, IntegrationStructuredOutputMode, MutationVersion,
    RequestContext, RuntimeError, RuntimeErrorCode, VersionedTarget,
};
use sha2::{Digest, Sha256};

use crate::integration_state::{
    public_provider, settings_from_inner, stored_provider, validate_identifier, IntegrationInner,
    IntegrationJobKind, IntegrationState, SessionSecret, StoredReview, StoredSpoolman,
    MAX_PROVIDERS, MAX_REVIEWS, MAX_REVIEW_TARGETS, REVIEW_TTL,
};
use crate::{invalid, LibraryRuntime};

#[derive(Clone)]
pub struct RuntimeIntegrationTransport(pub Arc<dyn IntegrationTransport>);

impl Default for RuntimeIntegrationTransport {
    fn default() -> Self {
        Self(Arc::new(library_integrations::DisabledIntegrationTransport))
    }
}

impl IntegrationEngine for LibraryRuntime {
    fn integration_network_enabled(&self, context: &RequestContext) -> bool {
        context.capabilities.contains(&Capability::Integration)
            && self
                .shared_state::<RuntimeIntegrationTransport>()
                .is_some_and(|transport| transport.0.is_enabled())
    }
    fn integration_settings(
        &self,
        context: &RequestContext,
    ) -> Result<IntegrationSettings, RuntimeError> {
        self.require(context, Capability::Integration)?;
        Ok(settings_to_protocol(&integration_state(self)?.settings()))
    }

    fn save_spoolman_config(
        &self,
        context: &RequestContext,
        config: Option<IntegrationSpoolmanConfig>,
    ) -> Result<IntegrationSettings, RuntimeError> {
        self.require(context, Capability::Integration)?;
        self.require(context, Capability::CatalogWrite)?;
        let state = integration_state(self)?;
        let config = config.map(spoolman_to_network);
        if let Some(config) = &config {
            library_integrations::validate_spoolman_config(config).map_err(integration_invalid)?;
        }
        let stored = if let Some(config) = config {
            let _io = self.begin_borrowed_io()?;
            let source = self.mutate_db(|db| db.upsert_spoolman_source(&config.base_url))?;
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
            .map_err(integration_invalid)?;
        inner.spoolman = stored;
        IntegrationState::invalidate_locked(&mut inner);
        Ok(settings_to_protocol(&settings_from_inner(&inner)))
    }

    fn pull_spoolman(
        &self,
        context: &RequestContext,
    ) -> Result<IntegrationSpoolmanPullResult, RuntimeError> {
        self.require(context, Capability::Integration)?;
        self.require(context, Capability::CatalogWrite)?;
        let state = integration_state(self)?;
        let transport = integration_transport(self)?;
        let _job = state
            .begin_job(IntegrationJobKind::Spoolman)
            .map_err(integration_busy)?;
        let (mut stored, mut epoch) = {
            let inner = state.shared.inner.lock().unwrap();
            (
                inner
                    .spoolman
                    .clone()
                    .ok_or_else(|| invalid("Spoolman is not configured"))?,
                inner.epoch,
            )
        };
        let source = {
            let _io = self.begin_borrowed_io()?;
            self.mutate_db(|db| db.upsert_spoolman_source(&stored.config.base_url))?
        };
        if source.id != stored.source_id {
            let mut inner = state.shared.inner.lock().unwrap();
            if inner.epoch != epoch
                || inner.spoolman.as_ref().map(|value| &value.config) != Some(&stored.config)
            {
                return Err(stale_integration("Spoolman configuration changed"));
            }
            stored.source_id = source.id;
            state
                .persist_values(Some(stored.clone()), &inner.providers)
                .map_err(integration_invalid)?;
            inner.spoolman = Some(stored.clone());
            IntegrationState::invalidate_locked(&mut inner);
            epoch = inner.epoch;
        }
        let library_epoch = self.library_epoch();
        let snapshot = transport
            .0
            .pull_spoolman(&stored.config)
            .map_err(integration_invalid)?;
        let completed_at = crate::format_system_time(SystemTime::now())?;
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
        let _io = self.begin_borrowed_io()?;
        if self.library_epoch() != library_epoch {
            return Err(stale_integration(
                "Library changed while Spoolman was running",
            ));
        }
        let inner = state.shared.inner.lock().unwrap();
        if inner.epoch != epoch
            || inner
                .spoolman
                .as_ref()
                .map(|value| (&value.source_id, &value.config))
                != Some((&stored.source_id, &stored.config))
        {
            return Err(stale_integration("Spoolman configuration changed"));
        }
        let summary = self.mutate_db(|db| {
            db.sync_spoolman_snapshot(&stored.source_id, &filaments, &spools, &completed_at)
        })?;
        Ok(IntegrationSpoolmanPullResult {
            filaments_received: filaments.len(),
            spools_received: spools.len(),
            filaments_upserted: filaments.len(),
            spools_upserted: spools.len(),
            removed_locally: summary.missing,
            last_successful_pull_at: summary.completed_at,
        })
    }

    fn save_ai_provider(
        &self,
        context: &RequestContext,
        config: AiProviderConfig,
        credential: IntegrationCredentialUpdate,
    ) -> Result<AiProviderConfig, RuntimeError> {
        self.require(context, Capability::Integration)?;
        let state = integration_state(self)?;
        let public = provider_to_state(&config);
        let stored = stored_provider(&public).map_err(integration_invalid)?;
        let replacement = match credential.action.as_str() {
            "keep" | "clear" if credential.secret.is_none() => None,
            "setSession" => Some(
                SessionSecret::new(
                    credential
                        .secret
                        .ok_or_else(|| invalid("Session credential is missing"))?,
                )
                .map_err(integration_invalid)?,
            ),
            _ => return Err(invalid("Credential update is invalid")),
        };
        let mut inner = state.shared.inner.lock().unwrap();
        let existing = inner
            .providers
            .iter()
            .position(|value| value.id == stored.id);
        if existing.is_none() && inner.providers.len() >= MAX_PROVIDERS {
            return Err(invalid("AI provider limit reached"));
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
            .map_err(integration_invalid)?;
        match credential.action.as_str() {
            "keep" => {}
            "clear" => {
                inner.credentials.remove(&stored.id);
            }
            "setSession" => {
                inner
                    .credentials
                    .insert(stored.id.clone(), replacement.unwrap());
            }
            _ => unreachable!(),
        }
        inner.providers = providers;
        IntegrationState::invalidate_locked(&mut inner);
        Ok(provider_to_protocol(&public_provider(
            &stored,
            inner.credentials.contains_key(&stored.id),
        )))
    }

    fn delete_ai_provider(
        &self,
        context: &RequestContext,
        provider_id: &str,
    ) -> Result<(), RuntimeError> {
        self.require(context, Capability::Integration)?;
        validate_identifier(provider_id, "Provider").map_err(integration_invalid)?;
        let state = integration_state(self)?;
        let mut inner = state.shared.inner.lock().unwrap();
        let mut providers = inner.providers.clone();
        let before = providers.len();
        providers.retain(|provider| provider.id != provider_id);
        if providers.len() == before {
            return Err(RuntimeError {
                code: RuntimeErrorCode::NotFound,
                message: "AI provider was not found".into(),
                retryable: false,
                current_version: None,
            });
        }
        state
            .persist_values(inner.spoolman.clone(), &providers)
            .map_err(integration_invalid)?;
        inner.providers = providers;
        inner.credentials.remove(provider_id);
        IntegrationState::invalidate_locked(&mut inner);
        Ok(())
    }

    fn suggest_ai_tags(
        &self,
        context: &RequestContext,
        provider_id: &str,
        targets: Vec<VersionedTarget>,
    ) -> Result<IntegrationAiSuggestionBatch, RuntimeError> {
        suggest_ai_tags(self, context, provider_id, targets)
    }

    fn apply_ai_review(
        &self,
        context: &RequestContext,
        review_token: &str,
        decisions: Vec<IntegrationAiReviewDecision>,
        strategy: IntegrationAiReviewStrategy,
    ) -> Result<Vec<MutationVersion>, RuntimeError> {
        apply_ai_review(self, context, review_token, decisions, strategy)
    }

    fn discard_ai_review(
        &self,
        context: &RequestContext,
        review_token: &str,
    ) -> Result<(), RuntimeError> {
        self.require(context, Capability::Integration)?;
        let state = integration_state(self)?;
        let mut inner = state.shared.inner.lock().unwrap();
        let index = inner
            .reviews
            .iter()
            .position(|review| review.token == review_token)
            .ok_or_else(|| stale_integration("AI review token is expired, replayed, or unknown"))?;
        if inner.reviews[index].principal_id != context.principal_id {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "AI review belongs to another principal".into(),
                retryable: false,
                current_version: None,
            });
        }
        inner.reviews.remove(index);
        Ok(())
    }
}

fn suggest_ai_tags(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    provider_id: &str,
    targets: Vec<VersionedTarget>,
) -> Result<IntegrationAiSuggestionBatch, RuntimeError> {
    runtime.require(context, Capability::Integration)?;
    runtime.require(context, Capability::CatalogRead)?;
    validate_identifier(provider_id, "Provider").map_err(integration_invalid)?;
    validate_targets(&targets)?;
    let state = integration_state(runtime)?;
    let transport = integration_transport(runtime)?;
    let _job = state
        .begin_job(IntegrationJobKind::Ai)
        .map_err(integration_busy)?;
    let (provider, credential, epoch) = {
        let inner = state.shared.inner.lock().unwrap();
        let provider = inner
            .providers
            .iter()
            .find(|value| value.id == provider_id)
            .cloned()
            .ok_or_else(|| invalid("AI provider was not found"))?;
        let credential = inner
            .credentials
            .get(provider_id)
            .map(SessionSecret::temporary_copy);
        (provider, credential, inner.epoch)
    };
    let library_epoch = runtime.library_epoch();
    let _io = runtime.begin_borrowed_io()?;
    let taxonomy = runtime.read_db(|db| {
        db.list_tags().map(|tags| {
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
    let mut inputs = Vec::new();
    let mut failures = Vec::new();
    for target in &targets {
        let model = runtime
            .read_db(|db| db.get_model(&target.model_id))?
            .ok_or_else(|| invalid("AI target model was not found"))?;
        if model.metadata_version != target.expected_version {
            return Err(stale_integration("AI target metadata version changed"));
        }
        match active_preview(runtime, &model.id) {
            Ok(Some(preview_data_url)) => {
                let existing_tags: Vec<IntegrationAiExistingTag> = model
                    .tags
                    .iter()
                    .map(|name| IntegrationAiExistingTag {
                        name: name.clone(),
                        category: taxonomy.get(&name.to_lowercase()).cloned().flatten(),
                    })
                    .collect();
                inputs.push((target.clone(), model, existing_tags, preview_data_url));
            }
            Ok(None) => failures.push(IntegrationAiSuggestionFailure {
                model_id: model.id,
                code: "previewUnavailable".into(),
                message: "A bounded preview is unavailable for this model".into(),
            }),
            Err(_) => failures.push(IntegrationAiSuggestionFailure {
                model_id: model.id,
                code: "previewError".into(),
                message: "A bounded preview could not be prepared for this model".into(),
            }),
        }
    }
    drop(_io);
    let queue = Arc::new(Mutex::new(VecDeque::from(inputs)));
    let output = Arc::new(Mutex::new(Vec::new()));
    std::thread::scope(|scope| {
        for _ in 0..usize::from(provider.config.concurrency) {
            let queue = Arc::clone(&queue);
            let output = Arc::clone(&output);
            let transport = Arc::clone(&transport.0);
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
                let result = transport
                    .suggest_tags(&provider.config, credential, &input)
                    .and_then(|mut tags| {
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
            Ok(tags) => review_targets.push(IntegrationAiSuggestionTarget {
                model_id: target.model_id,
                expected_version: target.expected_version,
                display_name: model.display_name,
                existing_tags,
                suggestions: suggestion_dtos(&model.id, tags),
            }),
            Err(_) => failures.push(IntegrationAiSuggestionFailure {
                model_id: target.model_id,
                code: "providerError".into(),
                message: "The provider did not return valid tag suggestions".into(),
            }),
        }
    }
    if runtime.library_epoch() != library_epoch {
        return Err(stale_integration(
            "Library changed while suggestions were running",
        ));
    }
    for target in &review_targets {
        let model = runtime.read_db(|db| db.get_model(&target.model_id))?;
        if model.as_ref().map(|model| model.metadata_version) != Some(target.expected_version) {
            return Err(stale_integration(
                "Model metadata changed while suggestions were running",
            ));
        }
    }
    let mut inner = state.shared.inner.lock().unwrap();
    if inner.epoch != epoch
        || !inner
            .providers
            .iter()
            .any(|value| value.id == provider_id && value.config == provider.config)
    {
        return Err(stale_integration("AI provider configuration changed"));
    }
    prune_reviews(&mut inner);
    let token = new_review_token(&mut inner, provider_id);
    inner.reviews.push_back(StoredReview {
        token: token.clone(),
        principal_id: context.principal_id.clone(),
        epoch,
        library_generation: library_epoch,
        created_at: SystemTime::now(),
        category_mode: provider.config.category_mode.clone(),
        known_tags: taxonomy,
        targets: review_targets.iter().map(target_to_state).collect(),
        applying: false,
    });
    while inner.reviews.len() > MAX_REVIEWS {
        inner.reviews.pop_front();
    }
    Ok(IntegrationAiSuggestionBatch {
        review_token: token,
        provider_id: provider_id.into(),
        model: provider.config.model,
        category_sharing: category_sharing_to_protocol(provider.config.category_sharing),
        targets: review_targets,
        failures,
    })
}

fn apply_ai_review(
    runtime: &LibraryRuntime,
    context: &RequestContext,
    review_token: &str,
    decisions: Vec<IntegrationAiReviewDecision>,
    strategy: IntegrationAiReviewStrategy,
) -> Result<Vec<MutationVersion>, RuntimeError> {
    runtime.require(context, Capability::Integration)?;
    runtime.require(context, Capability::CatalogWrite)?;
    let _io = runtime.begin_borrowed_io()?;
    let state = integration_state(runtime)?;
    let mut inner = state.shared.inner.lock().unwrap();
    prune_reviews(&mut inner);
    let index = inner
        .reviews
        .iter()
        .position(|review| review.token == review_token)
        .ok_or_else(|| stale_integration("AI review token is expired, replayed, or unknown"))?;
    let review = &inner.reviews[index];
    if review.principal_id != context.principal_id {
        return Err(RuntimeError {
            code: RuntimeErrorCode::Forbidden,
            message: "AI review belongs to another principal".into(),
            retryable: false,
            current_version: None,
        });
    }
    if review.epoch != inner.epoch || review.library_generation != runtime.library_epoch() {
        inner.reviews.remove(index);
        return Err(stale_integration("AI review became stale"));
    }
    if review.applying {
        return Err(integration_busy("AI review is already being applied"));
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
    let result = runtime.mutate_db(|db| {
        let current_tags = db
            .list_tags()?
            .into_iter()
            .map(|tag| (tag.name.to_lowercase(), tag.category))
            .collect::<HashMap<_, _>>();
        if updates.iter().flat_map(|target| &target.tags).any(|tag| {
            let key = tag.name.to_lowercase();
            known_tags.contains_key(&key) && current_tags.get(&key) != known_tags.get(&key)
        }) {
            return Err(library_core::LibraryError::InvalidInput(
                "A known tag changed while the review was open".into(),
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
                return Err(library_core::LibraryError::InvalidInput(
                    "The existing tag taxonomy changed while the review was open".into(),
                ));
            }
        }
        for (model_id, expected_version) in &all_targets {
            let model = db
                .get_model(model_id)?
                .ok_or_else(|| library_core::LibraryError::ModelNotFound(model_id.clone()))?;
            if model.metadata_version != *expected_version {
                return Err(library_core::LibraryError::MetadataConflict {
                    model_id: model_id.clone(),
                    expected: *expected_version,
                    actual: model.metadata_version,
                });
            }
        }
        if !updates.is_empty() {
            db.apply_reviewed_tags(&updates, reviewed_strategy(strategy))?;
        }
        all_targets
            .iter()
            .map(|(model_id, _)| {
                let model = db
                    .get_model(model_id)?
                    .ok_or_else(|| library_core::LibraryError::ModelNotFound(model_id.clone()))?;
                Ok(MutationVersion {
                    model_id: model.id,
                    metadata_version: model.metadata_version,
                })
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

fn integration_state(runtime: &LibraryRuntime) -> Result<Arc<IntegrationState>, RuntimeError> {
    runtime
        .shared_state::<IntegrationState>()
        .ok_or_else(|| invalid("Integration state is unavailable"))
}

fn integration_transport(
    runtime: &LibraryRuntime,
) -> Result<Arc<RuntimeIntegrationTransport>, RuntimeError> {
    runtime
        .shared_state::<RuntimeIntegrationTransport>()
        .ok_or_else(|| RuntimeError {
            code: RuntimeErrorCode::Unsupported,
            message: "Integration network transport is not enabled".into(),
            retryable: false,
            current_version: None,
        })
}

fn active_preview(
    runtime: &LibraryRuntime,
    model_id: &str,
) -> Result<Option<String>, RuntimeError> {
    let item = runtime
        .read_db(|db| db.list_thumbnail_gallery(model_id))?
        .into_iter()
        .find(|item| item.is_active);
    item.map(|item| {
        crate::thumbnails::png_data_uri_with_hash(
            &runtime.root.join(item.relative_path),
            &item.content_hash,
        )
        .map_err(|_| invalid("Thumbnail preview is unavailable"))
    })
    .transpose()
}

fn validate_targets(targets: &[VersionedTarget]) -> Result<(), RuntimeError> {
    if targets.is_empty() || targets.len() > MAX_REVIEW_TARGETS {
        return Err(invalid("AI target count is outside its limit"));
    }
    let mut ids = HashSet::new();
    if targets.iter().any(|target| {
        validate_identifier(&target.model_id, "Model").is_err() || !ids.insert(&target.model_id)
    }) {
        return Err(invalid("AI targets contain invalid or duplicate models"));
    }
    Ok(())
}

fn validate_category_policy(
    config: &library_integrations::ProviderConfig,
    allowed: &HashSet<String>,
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
                    .is_some_and(|category| !allowed.contains(category))
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
        if let Some(category) = taxonomy.get(&tag.name.to_lowercase()) {
            if tag.category.is_some() && tag.category.as_ref() != category.as_ref() {
                return Err("Provider tag category conflicts with the existing taxonomy".into());
            }
            tag.category = category.clone();
        }
    }
    Ok(())
}

fn suggestion_dtos(model_id: &str, tags: Vec<SuggestedTag>) -> Vec<IntegrationAiSuggestion> {
    tags.into_iter()
        .enumerate()
        .map(|(index, tag)| IntegrationAiSuggestion {
            suggestion_id: format!("{model_id}:{index}"),
            name: tag.name,
            category: tag.category,
            confidence: tag.confidence,
        })
        .collect()
}

fn target_to_state(
    target: &IntegrationAiSuggestionTarget,
) -> crate::integration_state::AiSuggestionTarget {
    crate::integration_state::AiSuggestionTarget {
        model_id: target.model_id.clone(),
        expected_version: target.expected_version,
        display_name: target.display_name.clone(),
        existing_tags: target
            .existing_tags
            .iter()
            .map(|tag| crate::integration_state::AiExistingTag {
                name: tag.name.clone(),
                category: tag.category.clone(),
            })
            .collect(),
        suggestions: target
            .suggestions
            .iter()
            .map(|suggestion| crate::integration_state::AiSuggestion {
                suggestion_id: suggestion.suggestion_id.clone(),
                name: suggestion.name.clone(),
                category: suggestion.category.clone(),
                confidence: suggestion.confidence,
            })
            .collect(),
    }
}

fn reviewed_updates(
    review: &StoredReview,
    decisions: &[IntegrationAiReviewDecision],
) -> Result<Vec<ReviewedTagTarget>, RuntimeError> {
    if decisions.len() != review.targets.len() {
        return Err(invalid("Every review target requires an explicit decision"));
    }
    let mut ids = HashSet::new();
    let mut updates = Vec::new();
    for decision in decisions {
        if !ids.insert(decision.model_id.as_str()) {
            return Err(invalid("AI review contains duplicate decisions"));
        }
        let target = review
            .targets
            .iter()
            .find(|target| target.model_id == decision.model_id)
            .ok_or_else(|| invalid("AI review targets an unrelated model"))?;
        if decision.action == IntegrationAiReviewAction::Reject {
            if !decision.accepted_suggestion_ids.is_empty() {
                return Err(invalid("Rejected targets cannot accept suggestions"));
            }
            continue;
        }
        let mut accepted = HashSet::new();
        let mut tags = Vec::new();
        for id in &decision.accepted_suggestion_ids {
            if !accepted.insert(id) {
                return Err(invalid("AI review repeats a suggestion"));
            }
            let suggestion = target
                .suggestions
                .iter()
                .find(|suggestion| suggestion.suggestion_id == *id)
                .ok_or_else(|| invalid("AI review accepts an unknown suggestion"))?;
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

fn reviewed_strategy(strategy: IntegrationAiReviewStrategy) -> ReviewedTagStrategy {
    match strategy {
        IntegrationAiReviewStrategy::Append => ReviewedTagStrategy::Append,
        IntegrationAiReviewStrategy::Replace => ReviewedTagStrategy::Replace,
        IntegrationAiReviewStrategy::Merge => ReviewedTagStrategy::Merge,
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

fn integration_invalid(message: impl Into<String>) -> RuntimeError {
    invalid(&message.into())
}

fn integration_busy(message: impl Into<String>) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Busy,
        message: message.into(),
        retryable: true,
        current_version: None,
    }
}

fn stale_integration(message: impl Into<String>) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Stale,
        message: message.into(),
        retryable: false,
        current_version: None,
    }
}

fn settings_to_protocol(
    value: &crate::integration_state::IntegrationSettings,
) -> IntegrationSettings {
    IntegrationSettings {
        spoolman: value.spoolman.as_ref().map(spoolman_to_protocol),
        ai_providers: value
            .ai_providers
            .iter()
            .map(provider_to_protocol)
            .collect(),
    }
}

fn spoolman_to_protocol(value: &library_integrations::SpoolmanConfig) -> IntegrationSpoolmanConfig {
    IntegrationSpoolmanConfig {
        base_url: value.base_url.clone(),
        endpoint_scope: scope_to_protocol(value.endpoint_scope),
        page_size: value.page_size,
        timeout_seconds: value.timeout_seconds,
    }
}

fn spoolman_to_network(value: IntegrationSpoolmanConfig) -> library_integrations::SpoolmanConfig {
    library_integrations::SpoolmanConfig {
        base_url: value.base_url,
        endpoint_scope: scope_to_network(value.endpoint_scope),
        page_size: value.page_size,
        timeout_seconds: value.timeout_seconds,
    }
}

fn provider_to_protocol(
    value: &crate::integration_state::AiProviderPublicConfig,
) -> AiProviderConfig {
    AiProviderConfig {
        id: value.id.clone(),
        label: value.label.clone(),
        kind: kind_to_protocol(value.kind),
        base_url: value.base_url.clone(),
        endpoint_scope: scope_to_protocol(value.endpoint_scope),
        model: value.model.clone(),
        credential_storage: value.credential_storage.clone(),
        has_credential: value.has_credential,
        max_tags: value.max_tags,
        detail: value.detail.clone(),
        category_mode: value.category_mode.clone(),
        concurrency: value.concurrency,
        timeout_seconds: value.timeout_seconds,
        structured_output: output_to_protocol(value.structured_output),
        category_sharing: category_sharing_to_protocol(value.category_sharing),
    }
}

fn provider_to_state(value: &AiProviderConfig) -> crate::integration_state::AiProviderPublicConfig {
    crate::integration_state::AiProviderPublicConfig {
        id: value.id.clone(),
        label: value.label.clone(),
        kind: kind_to_network(value.kind),
        base_url: value.base_url.clone(),
        endpoint_scope: scope_to_network(value.endpoint_scope),
        model: value.model.clone(),
        credential_storage: value.credential_storage.clone(),
        has_credential: value.has_credential,
        max_tags: value.max_tags,
        detail: value.detail.clone(),
        category_mode: value.category_mode.clone(),
        concurrency: value.concurrency,
        timeout_seconds: value.timeout_seconds,
        structured_output: output_to_network(value.structured_output),
        category_sharing: category_sharing_to_network(value.category_sharing),
    }
}

fn scope_to_protocol(value: library_integrations::EndpointScope) -> IntegrationEndpointScope {
    match value {
        library_integrations::EndpointScope::Local => IntegrationEndpointScope::Local,
        library_integrations::EndpointScope::Remote => IntegrationEndpointScope::Remote,
    }
}
fn scope_to_network(value: IntegrationEndpointScope) -> library_integrations::EndpointScope {
    match value {
        IntegrationEndpointScope::Local => library_integrations::EndpointScope::Local,
        IntegrationEndpointScope::Remote => library_integrations::EndpointScope::Remote,
    }
}
fn kind_to_protocol(value: library_integrations::ProviderKind) -> IntegrationProviderKind {
    match value {
        library_integrations::ProviderKind::OpenaiCompatible => {
            IntegrationProviderKind::OpenaiCompatible
        }
        library_integrations::ProviderKind::Anthropic => IntegrationProviderKind::Anthropic,
        library_integrations::ProviderKind::Gemini => IntegrationProviderKind::Gemini,
    }
}
fn kind_to_network(value: IntegrationProviderKind) -> library_integrations::ProviderKind {
    match value {
        IntegrationProviderKind::OpenaiCompatible => {
            library_integrations::ProviderKind::OpenaiCompatible
        }
        IntegrationProviderKind::Anthropic => library_integrations::ProviderKind::Anthropic,
        IntegrationProviderKind::Gemini => library_integrations::ProviderKind::Gemini,
    }
}
fn output_to_protocol(
    value: library_integrations::StructuredOutputMode,
) -> IntegrationStructuredOutputMode {
    match value {
        library_integrations::StructuredOutputMode::JsonSchema => {
            IntegrationStructuredOutputMode::JsonSchema
        }
        library_integrations::StructuredOutputMode::JsonObject => {
            IntegrationStructuredOutputMode::JsonObject
        }
        library_integrations::StructuredOutputMode::PlainJson => {
            IntegrationStructuredOutputMode::PlainJson
        }
    }
}
fn output_to_network(
    value: IntegrationStructuredOutputMode,
) -> library_integrations::StructuredOutputMode {
    match value {
        IntegrationStructuredOutputMode::JsonSchema => {
            library_integrations::StructuredOutputMode::JsonSchema
        }
        IntegrationStructuredOutputMode::JsonObject => {
            library_integrations::StructuredOutputMode::JsonObject
        }
        IntegrationStructuredOutputMode::PlainJson => {
            library_integrations::StructuredOutputMode::PlainJson
        }
    }
}
fn category_sharing_to_protocol(
    value: library_integrations::CategorySharing,
) -> IntegrationCategorySharing {
    match value {
        library_integrations::CategorySharing::Local => IntegrationCategorySharing::Local,
        library_integrations::CategorySharing::Provider => IntegrationCategorySharing::Provider,
    }
}
fn category_sharing_to_network(
    value: IntegrationCategorySharing,
) -> library_integrations::CategorySharing {
    match value {
        IntegrationCategorySharing::Local => library_integrations::CategorySharing::Local,
        IntegrationCategorySharing::Provider => library_integrations::CategorySharing::Provider,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use library_integrations::{ProviderConfig, SpoolmanConfig, SpoolmanSnapshot, SuggestedTag};
    use library_protocol::{
        IntegrationAiReviewStrategy, IntegrationCategorySharing, IntegrationEndpointScope,
        IntegrationProviderKind, IntegrationStructuredOutputMode,
    };

    use super::*;

    #[derive(Default)]
    struct MockTransport {
        pulls: AtomicUsize,
    }

    impl IntegrationTransport for MockTransport {
        fn pull_spoolman(&self, _config: &SpoolmanConfig) -> Result<SpoolmanSnapshot, String> {
            self.pulls.fetch_add(1, Ordering::SeqCst);
            Ok(SpoolmanSnapshot {
                filaments: Vec::new(),
                spools: Vec::new(),
            })
        }

        fn suggest_tags(
            &self,
            _config: &ProviderConfig,
            _credential: Option<&str>,
            _input: &SuggestionInput,
        ) -> Result<Vec<SuggestedTag>, String> {
            Ok(vec![SuggestedTag {
                name: "Fixture".into(),
                category: None,
                confidence: Some(0.9),
            }])
        }
    }

    fn context(principal_id: &str) -> RequestContext {
        RequestContext {
            principal_id: principal_id.into(),
            capabilities: [
                Capability::CatalogRead,
                Capability::CatalogWrite,
                Capability::Integration,
            ]
            .into_iter()
            .collect(),
            request_id: format!("request-{principal_id}"),
        }
    }

    #[test]
    fn mock_transport_pulls_atomically_and_credentials_are_never_returned() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("integrations.json");
        let runtime = LibraryRuntime::open(crate::RuntimeConfig::desktop(
            temp.path().join("library"),
            settings.clone(),
        ))
        .unwrap();
        let state = Arc::new(IntegrationState::load(settings).unwrap());
        let transport = Arc::new(MockTransport::default());
        runtime.install_shared_state(Arc::clone(&state)).unwrap();
        runtime
            .install_shared_state(Arc::new(RuntimeIntegrationTransport(transport.clone())))
            .unwrap();
        let context = context("fixture");
        runtime
            .save_spoolman_config(
                &context,
                Some(IntegrationSpoolmanConfig {
                    base_url: "http://127.0.0.1:3746/".into(),
                    endpoint_scope: IntegrationEndpointScope::Local,
                    page_size: 50,
                    timeout_seconds: 5,
                }),
            )
            .unwrap();
        runtime.pull_spoolman(&context).unwrap();
        assert_eq!(transport.pulls.load(Ordering::SeqCst), 1);
        let saved = runtime
            .save_ai_provider(
                &context,
                AiProviderConfig {
                    id: "local".into(),
                    label: "Local".into(),
                    kind: IntegrationProviderKind::OpenaiCompatible,
                    base_url: Some("http://127.0.0.1:11434/".into()),
                    endpoint_scope: IntegrationEndpointScope::Local,
                    model: "fixture".into(),
                    credential_storage: "session".into(),
                    has_credential: false,
                    max_tags: 5,
                    detail: "brief".into(),
                    category_mode: "none".into(),
                    concurrency: 1,
                    timeout_seconds: 5,
                    structured_output: IntegrationStructuredOutputMode::PlainJson,
                    category_sharing: IntegrationCategorySharing::Local,
                },
                IntegrationCredentialUpdate {
                    action: "setSession".into(),
                    secret: Some("top-secret".into()),
                },
            )
            .unwrap();
        assert!(saved.has_credential);
        let encoded =
            serde_json::to_string(&runtime.integration_settings(&context).unwrap()).unwrap();
        assert!(!encoded.contains("top-secret"));
    }

    #[test]
    fn review_tokens_are_principal_bound_without_consuming_cross_principal_attempts() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("integrations.json");
        let runtime = LibraryRuntime::open(crate::RuntimeConfig::desktop(
            temp.path().join("library"),
            settings.clone(),
        ))
        .unwrap();
        let state = Arc::new(IntegrationState::load(settings).unwrap());
        runtime.install_shared_state(Arc::clone(&state)).unwrap();
        state
            .shared
            .inner
            .lock()
            .unwrap()
            .reviews
            .push_back(StoredReview {
                token: "review".into(),
                principal_id: "owner".into(),
                epoch: 1,
                library_generation: runtime.library_epoch(),
                created_at: SystemTime::now(),
                category_mode: "none".into(),
                known_tags: HashMap::new(),
                targets: Vec::new(),
                applying: false,
            });
        let error = runtime
            .apply_ai_review(
                &context("other"),
                "review",
                Vec::new(),
                IntegrationAiReviewStrategy::Append,
            )
            .unwrap_err();
        assert_eq!(error.code, RuntimeErrorCode::Forbidden);
        assert!(state
            .shared
            .inner
            .lock()
            .unwrap()
            .reviews
            .iter()
            .any(|review| review.token == "review"));
        let error = runtime
            .discard_ai_review(&context("other"), "review")
            .unwrap_err();
        assert_eq!(error.code, RuntimeErrorCode::Forbidden);
        assert_eq!(state.shared.inner.lock().unwrap().reviews.len(), 1);
    }
}
