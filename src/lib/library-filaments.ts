import type { AiSuggestionBatch, Filament, LibraryModel, VersionedModelTarget } from "./ipc";

export const AI_DISCLOSURE =
  "The selected provider receives a normalized preview image, display name, and existing tag names. Tag categories stay local. File paths, notes, source URLs, print history, and image provenance are not shared.";

export function filamentLabel(filament: Filament): string {
  const identity =
    filament.resource_kind === "spool" && filament.external_id
      ? `Spool #${filament.external_id}`
      : filament.name || filament.material || filament.id;
  return [identity, filament.brand, filament.material].filter(Boolean).join(" · ");
}

export function filamentReferenceLabel(id: string, catalog: Filament[]): string {
  const filament = catalog.find((entry) => entry.id === id);
  return filament ? filamentLabel(filament) : `Unknown filament (${id})`;
}

export function assignmentIdsPreservingUnknown(
  existingIds: string[],
  catalog: Filament[],
  selectedKnownIds: string[],
  removedUnknownIds: ReadonlySet<string>,
): string[] {
  const selectable = new Set(
    catalog.filter((entry) => !entry.archived && !entry.source_missing).map((entry) => entry.id),
  );
  const preservedUnavailable = existingIds.filter(
    (id) => !selectable.has(id) && !removedUnknownIds.has(id),
  );
  return [...new Set([...selectedKnownIds, ...preservedUnavailable])];
}

export function matchesFilamentFilter(
  model: LibraryModel,
  selectedIds: ReadonlySet<string>,
  unassigned: boolean,
): boolean {
  const assigned = model.filament_ids ?? [];
  if (unassigned && assigned.length === 0) return true;
  if (!selectedIds.size) return !unassigned;
  return assigned.some((id) => selectedIds.has(id));
}

export function aiTargets(models: LibraryModel[]): VersionedModelTarget[] {
  return models.map((model) => ({ model_id: model.id, expected_version: model.metadata_version }));
}

export function filamentAssignmentBatch(
  updates: Array<{ target: VersionedModelTarget; filamentIds: string[] }>,
): Array<{ target: VersionedModelTarget; filament_ids: string[] }> {
  return updates.map((update) => ({
    target: { ...update.target },
    filament_ids: [...update.filamentIds],
  }));
}

export function defaultAiDecisions(batch: AiSuggestionBatch): Array<{
  modelId: string;
  acceptedSuggestionIds: string[];
}> {
  return batch.targets.map((target) => ({ modelId: target.modelId, acceptedSuggestionIds: [] }));
}

export function buildAiReviewDecisions(
  batch: AiSuggestionBatch,
  acceptedByModel: ReadonlyMap<string, string[]>,
  actions: ReadonlyMap<string, "accept" | "reject">,
): Array<{
  modelId: string;
  action: "accept" | "reject";
  acceptedSuggestionIds: string[];
}> {
  return batch.targets.map((target) => {
    const action = actions.get(target.modelId);
    if (!action) throw new Error(`Model ${target.displayName} needs an explicit accept or reject.`);
    const accepted = acceptedByModel.get(target.modelId) ?? [];
    return {
      modelId: target.modelId,
      action,
      acceptedSuggestionIds: action === "reject" ? [] : accepted,
    };
  });
}

export function aiAfterTags(
  existing: Array<{ name: string; category: string | null }>,
  suggestions: Array<{ suggestionId: string; name: string; category: string | null }>,
  acceptedIds: ReadonlySet<string>,
  strategy: "append" | "replace" | "merge",
): Array<{ name: string; category: string | null }> {
  const accepted = suggestions.filter((suggestion) => acceptedIds.has(suggestion.suggestionId));
  let combined: Array<{ name: string; category: string | null }>;
  if (strategy === "replace") combined = accepted;
  else if (strategy === "append") combined = [...existing, ...accepted];
  else {
    const represented = new Set(
      accepted
        .map((tag) => tag.category)
        .filter((category): category is string => Boolean(category)),
    );
    combined = [
      ...existing.filter((tag) => !tag.category || !represented.has(tag.category)),
      ...accepted,
    ];
  }
  const seen = new Set<string>();
  return combined.filter((tag) => {
    const key = `${tag.category ?? ""}\0${tag.name}`.toLocaleLowerCase();
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}
