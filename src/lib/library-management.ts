import type { LibraryModel, VersionedRemovalTarget } from "./ipc";

export interface BundleGroup {
  bundleId: string;
  models: LibraryModel[];
}

export function explicitScopeIds(models: LibraryModel[]): string[] {
  return models.filter((model) => !model.source_missing).map((model) => model.id);
}

export function removalTargets(models: LibraryModel[]): VersionedRemovalTarget[] {
  return models.map((model) => ({ modelId: model.id, expectedVersion: model.metadata_version }));
}

export function chooseRouletteModel(
  models: LibraryModel[],
  random: () => number = Math.random,
): LibraryModel | null {
  const available = models.filter((model) => !model.source_missing);
  if (!available.length) return null;
  const index = Math.min(available.length - 1, Math.floor(random() * available.length));
  return available[index];
}

export function partitionBundleGroups(models: LibraryModel[]): {
  groups: BundleGroup[];
  singles: LibraryModel[];
} {
  const byBundle = new Map<string, LibraryModel[]>();
  for (const model of models) {
    if (model.bundle_id) {
      const group = byBundle.get(model.bundle_id) ?? [];
      group.push(model);
      byBundle.set(model.bundle_id, group);
    }
  }
  const groupedIds = new Set<string>();
  const groups = [...byBundle]
    .filter(([, members]) => members.length >= 2)
    .map(([bundleId, members]) => {
      for (const member of members) groupedIds.add(member.id);
      return { bundleId, models: members };
    });
  return { groups, singles: models.filter((model) => !groupedIds.has(model.id)) };
}

export function bundlePreviewCapacity(
  models: LibraryModel[],
  capacity = 32,
): {
  visible: LibraryModel[];
  omitted: number;
} {
  return { visible: models.slice(0, capacity), omitted: Math.max(0, models.length - capacity) };
}
