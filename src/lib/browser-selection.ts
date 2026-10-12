import type { BrowserModel, VersionedTarget } from "./library-client";

export function browserSelectionTargets(models: Iterable<BrowserModel>): VersionedTarget[] {
  return [...models].map((model) => ({
    modelId: model.id,
    expectedVersion: model.metadataVersion,
  }));
}

export function pagesToRestore(loadedModels: number, pageSize: number): number {
  return Math.max(1, Math.ceil(loadedModels / pageSize));
}

export function ensureBulkSelectionLimit(count: number, limit = 10_000): void {
  if (count > limit)
    throw new Error(
      `This filter matches more than the ${limit.toLocaleString()}-model atomic bulk limit. Narrow the filters before selecting all.`,
    );
}
