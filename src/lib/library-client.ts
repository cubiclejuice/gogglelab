export interface HostCapabilities {
  apiVersion: "1";
  hostMode: "desktop" | "headless";
  libraryEpoch: string;
  catalogRevision: string;
  features: {
    revealSource: boolean;
    trashSource: boolean;
    launchSlicer: boolean;
    downloadSource: boolean;
    downloadBundle: boolean;
    ai: boolean;
    spoolman: boolean;
    mcp: boolean;
    captureInbox: boolean;
  };
  limits: {
    pageSizeMax: number;
    thumbnailUploadBytes: number;
    modelUploadBytes: number;
    activePreviewsMax: number;
  };
}

export interface BrowserModel {
  id: string;
  displayName: string;
  folderId: string;
  metadataVersion: string;
  rating: number;
  favorite: boolean;
  bytes: string;
  format: string | null;
  contentHash: string | null;
  tags: string[];
  filamentIds: string[];
  sourceKind: string;
  availability: "available" | "missing" | "bookmark" | "removed";
  catalogKind: string;
  thumbnailId: string | null;
  designer: string | null;
  sourceUrl: string | null;
  notes: string | null;
  modelLicense: string | null;
  printStatus: string;
  bundleId: string | null;
  sourceRootId: string | null;
  archiveMember: boolean;
  parentModelId: string | null;
  createdAt: string;
  updatedAt: string;
  lastPrintedAt: string | null;
  printCount: string;
}

export interface CursorPage<T> {
  items: T[];
  nextCursor: string | null;
  snapshotRevision: string;
}

export interface BrowserModelQuery {
  limit?: number;
  cursor?: string;
  query?: string;
  folderId?: string;
  sort?: "name" | "size" | "date";
  direction?: "asc" | "desc";
  tagId?: string;
  category?: string;
  designer?: string;
  modelLicense?: string;
  printStatus?: PrintStatus;
  everPrinted?: boolean;
  bundleId?: string;
  rootId?: string;
  sourceDirectory?: string;
  filamentId?: string;
  sourceMissing?: boolean;
  duplicatesOnly?: boolean;
  sourceKind?: "managed" | "indexed";
  group?: "standalone" | "bundle" | "archive";
  thumbnailMode?: "missing" | "default" | "custom";
  minimumRating?: number;
  favorite?: boolean;
  signal?: AbortSignal;
}

export interface ThumbnailDescriptor {
  thumbnailId: string;
  modelId: string;
  ordinal: number;
  active: boolean;
  custom: boolean;
  mime: string;
  bytes: string;
  sha256: string;
}

export interface ContentLease {
  id: string;
  kind: "thumbnail" | "model" | "geometry" | "bundleArchive";
  fileName: string;
  mime: string;
  bytes: string;
  sha256: string;
  libraryEpoch: string;
  expiresAt: string;
}
export interface BrowserGeometryLease {
  content: ContentLease;
  packetFormat: "stl2";
  packetVersion: 2;
  modelFormat: string;
  triangleCount: number;
  plates: Array<{ id: number; name: string; triangleStart: number; triangleCount: number }>;
  parts: Array<{
    id: string;
    name: string;
    triangleStart: number;
    triangleCount: number;
    materialId: string | null;
    filamentSlot: number | null;
  }>;
  materials: Array<{ id: string; name: string | null; color: string | null; source: string }>;
  embeddedFilaments: Array<{
    slot: number;
    name: string | null;
    material: string | null;
    color: string;
  }>;
  partWarning: string | null;
  plateWarning: string | null;
}
export interface EmbeddedMetadataProposal {
  modelId: string;
  sourceHash: string;
  current: {
    designer: string | null;
    sourceUrl: string | null;
    notes: string | null;
    modelLicense: string | null;
  };
  embedded: {
    designer: string | null;
    sourceUrl: string | null;
    notes: string | null;
    modelLicense: string | null;
  };
  embeddedFilaments: BrowserGeometryLease["embeddedFilaments"];
}
export interface DuplicateGroup {
  contentHash: string;
  models: BrowserModel[];
}

export type PrintStatus = "unprinted" | "want" | "queued" | "printing" | "printed" | "failed";
export interface VersionedTarget {
  modelId: string;
  expectedVersion: string;
}
export interface BrowserMetadataPatch {
  designer?: string;
  sourceUrl?: string;
  notes?: string;
  modelLicense?: string;
  printStatus?: PrintStatus;
  tagIds?: string[];
  tagMode?: "replace" | "merge";
  folderId?: string;
  parentModelId?: string;
  rating?: number;
  favorite?: boolean;
}
export interface MutationVersion {
  modelId: string;
  metadataVersion: string;
}
export interface BrowserTag {
  id: string;
  name: string;
  category: string | null;
}
export interface BrowserFolder {
  id: string;
  name: string;
  parentId: string | null;
  modelCount: string;
}
export interface BrowserFilamentInput {
  brand: string;
  material: string;
  color: string;
  name: string;
  notes: string;
}
export interface BrowserFilament extends BrowserFilamentInput {
  id: string;
  version: string;
  origin: string;
  resourceKind: string;
  sourceId: string | null;
  externalId: string | null;
  parentFilamentId: string | null;
  archived: boolean;
  sourceMissing: boolean;
}
export interface BrowserPrintAttemptInput {
  printedAt: string;
  quantity: number;
  notes: string;
  filamentIds: string[];
  outcome: "printed" | "failed";
}
export interface BrowserPrintAttempt extends BrowserPrintAttemptInput {
  id: string;
  modelId: string;
  createdAt: string;
  updatedAt: string;
}

export interface CaptureEnvelope {
  version: 1;
  itemId: string;
  createdAt: string;
  page: {
    url: string;
    title: string;
    designer?: string | null;
    license?: string | null;
    notes?: string | null;
    parentExternalId?: string | null;
  };
  downloads: Array<{
    captureId: string;
    fileName: string;
    relativeInboxPath?: string | null;
    sha256?: string | null;
    bytes?: number | null;
  }>;
}

export interface UploadToken {
  id: string;
  itemId: string;
  captureId: string;
  expiresAt: string;
  libraryEpoch: string;
}

export interface CaptureImportResult {
  itemId: string;
  status: "imported" | "partial" | "failed" | "alreadyImported";
  pageModelId?: string | null;
  downloads: Array<{
    captureId: string;
    status: "imported" | "failed" | "alreadyImported" | "alreadyImportedRemoved" | "needsFile";
    modelId?: string | null;
    errorCode?: string | null;
  }>;
}
export interface BrowserRootHandle {
  id: string;
  catalogRootId: string | null;
  label: string;
  access: "readOnly" | "writable" | "backupOnly" | "inbox";
}
export interface BrowserScanStatus {
  running: boolean;
  cancelling: boolean;
  jobKind: string | null;
  currentRootId: string | null;
  rootsTotal: number;
  rootsCompleted: number;
  filesVisited: number;
  filesIndexed: number;
  errors: string[];
  thumbnailsTotal: number;
  thumbnailsCompleted: number;
}
export interface CaptureSummary {
  itemId: string;
  createdAt: string;
  status: CaptureImportResult["status"];
  pageModelId: string | null;
  title: string;
  url: string;
  downloadCount: number;
  failedCount: number;
  needsFileCount: number;
}
export interface BrowserBundlePreview {
  token: string;
  bundleId: string;
  memberCount: number;
  previewedCount: number;
  omittedCount: number;
  triangleCount: number;
  layout: "combined" | "grid";
  members: Array<{
    modelId: string;
    displayName: string;
    triangleCount: number;
    offset: [number, number, number];
  }>;
  notice: string | null;
  geometryBytes: string;
  geometrySha256: string;
  libraryEpoch: string;
}
export interface CatalogFacets {
  designers: string[];
  licenses: string[];
  categories: string[];
  parents: Array<{ id: string; label: string }>;
  folderIds: string[];
  rootIds: string[];
}
export interface IndexRootSettings {
  handleId: string;
  catalogRootId: string | null;
  label: string;
  access: "readOnly" | "writable" | "backupOnly" | "inbox";
  enabled: boolean;
  lastScannedAt: string | null;
  scanIntervalMinutes: number;
  maxFileBytes: string;
  scanZip: boolean;
}
export interface BrowserCatalogSettings {
  thumbnailSize: 128 | 256 | 512;
  thumbnailBackground: string;
  thumbnailConcurrency: 1 | 2;
  scanBatchSize: number;
}
export type ManagementAction =
  | { kind: "recoverableRemove"; targets: VersionedTarget[] }
  | { kind: "restoreRemoval"; snapshotId: string }
  | { kind: "createBackup"; destinationHandleId: string; fileName: string }
  | {
      kind: "restoreBackup";
      sourceHandleId: string;
      fileName: string;
      rootMappings: Array<{
        backupCatalogRootId: string;
        operatorHandleId: string;
        enabled: boolean;
      }>;
    }
  | {
      kind: "exportSelection";
      targets: VersionedTarget[];
      destinationHandleId: string;
      fileName: string;
    }
  | { kind: "quarantineLooseSource"; target: VersionedTarget }
  | { kind: "undoQuarantine"; snapshotId: string }
  | {
      kind: "moveLooseSource";
      target: VersionedTarget;
      destinationHandleId: string;
      fileName: string;
    }
  | { kind: "purgeCatalog"; backupDestinationHandleId: string; backupFileName: string }
  | { kind: "cleanupOrphans" };
export interface ActionPlan {
  token: string;
  actionKind: string;
  expiresAt: string;
  libraryEpoch: string;
  catalogRevision: string;
  targets: Array<{ modelId: string; metadataVersion: string; contentHash: string | null }>;
  impact: string[];
}
export type ManagementResult =
  | { kind: "removed"; snapshotId: string; removedIds: string[] }
  | { kind: "restoredRemoval"; models: MutationVersion[] }
  | {
      kind: "backupCreated";
      destinationHandleId: string;
      fileName: string;
      files: number;
      bytes: string;
      sha256: string;
    }
  | { kind: "backupRestored"; models: number; libraryEpoch: string }
  | {
      kind: "selectionExported";
      destinationHandleId: string;
      fileName: string;
      models: number;
      bytes: string;
    }
  | { kind: "quarantined"; snapshotId: string; modelId: string }
  | { kind: "quarantineUndone"; model: MutationVersion }
  | { kind: "moved"; model: MutationVersion }
  | {
      kind: "purged";
      backupDestinationHandleId: string;
      backupFileName: string;
      libraryEpoch: string;
    }
  | { kind: "cleanupCompleted"; files: string; bytes: string };
export interface SpoolmanConfig {
  baseUrl: string;
  endpointScope: "local" | "remote";
  pageSize: number;
  timeoutSeconds: number;
}
export interface AiProviderConfig {
  id: string;
  label: string;
  kind: "openaiCompatible" | "anthropic" | "gemini";
  baseUrl: string | null;
  endpointScope: "local" | "remote";
  model: string;
  credentialStorage: "none" | "session";
  hasCredential: boolean;
  maxTags: number;
  detail: "brief" | "balanced" | "detailed";
  categoryMode: "none" | "existingOnly" | "allowNew";
  concurrency: number;
  timeoutSeconds: number;
  structuredOutput: "jsonSchema" | "jsonObject" | "plainJson";
  categorySharing: "local" | "provider";
}
export interface IntegrationSettings {
  spoolman: SpoolmanConfig | null;
  aiProviders: AiProviderConfig[];
}
export interface AiSuggestionBatch {
  reviewToken: string;
  providerId: string;
  model: string;
  categorySharing: "local" | "provider";
  targets: Array<{
    modelId: string;
    expectedVersion: string;
    displayName: string;
    existingTags: Array<{ name: string; category: string | null }>;
    suggestions: Array<{
      suggestionId: string;
      name: string;
      category: string | null;
      confidence: number | null;
    }>;
  }>;
  failures: Array<{ modelId: string; code: string; message: string }>;
}

export interface LibraryEvent {
  sequence: string;
  libraryEpoch: string;
  catalogRevision: string;
  type: "catalog.changed" | "library.reset" | "job.changed" | "server.shutdown";
  [key: string]: unknown;
}

export class LibraryHttpError extends Error {
  constructor(
    message: string,
    readonly code: string,
    readonly requestId: string,
    readonly retryable: boolean,
    readonly currentVersion?: string,
  ) {
    super(message);
  }
}

export function formatWireInteger(value: string): string {
  if (!/^(0|[1-9]\d*)$/.test(value)) return value;
  return BigInt(value).toLocaleString();
}

const LOCAL_PLAINTEXT_HOSTS = new Set(["localhost", "127.0.0.1", "[::1]"]);

/**
 * Validates the server URL before a bearer credential can be accepted.
 * Plain HTTP is intentionally limited to the loopback names browsers expose;
 * private-network and public hosts must terminate TLS.
 */
export function admitBrowserServerUrl(value: string | URL): URL {
  const url = new URL(value);
  if (url.username || url.password) {
    throw new LibraryHttpError(
      "Server URLs cannot contain a username or password",
      "unsafeServerUrl",
      "local",
      false,
    );
  }
  if (url.search || url.hash) {
    throw new LibraryHttpError(
      "Server URLs cannot contain query parameters or fragments",
      "unsafeServerUrl",
      "local",
      false,
    );
  }
  if (url.protocol === "https:") return url;
  if (url.protocol === "http:" && LOCAL_PLAINTEXT_HOSTS.has(url.hostname)) return url;
  throw new LibraryHttpError(
    "This server requires HTTPS before an access key can be entered. Configure TLS on the proxy, or open it from localhost on the server computer.",
    "tlsRequired",
    "local",
    false,
  );
}

export interface LibraryClient {
  openApi(signal?: AbortSignal): Promise<{ paths: Record<string, unknown> }>;
  capabilities(signal?: AbortSignal): Promise<HostCapabilities>;
  status(signal?: AbortSignal): Promise<Record<string, unknown>>;
  catalogFacets(signal?: AbortSignal): Promise<CatalogFacets>;
  rootSettings(signal?: AbortSignal): Promise<IndexRootSettings[]>;
  updateRootSettings(
    handleId: string,
    settings: Pick<
      IndexRootSettings,
      "enabled" | "scanIntervalMinutes" | "maxFileBytes" | "scanZip"
    >,
    signal?: AbortSignal,
  ): Promise<IndexRootSettings>;
  catalogSettings(signal?: AbortSignal): Promise<BrowserCatalogSettings>;
  updateCatalogSettings(
    settings: BrowserCatalogSettings,
    signal?: AbortSignal,
  ): Promise<BrowserCatalogSettings>;
  planManagementAction(action: ManagementAction, signal?: AbortSignal): Promise<ActionPlan>;
  executeManagementAction(token: string, signal?: AbortSignal): Promise<ManagementResult>;
  integrationSettings(signal?: AbortSignal): Promise<IntegrationSettings>;
  saveSpoolmanConfig(
    config: SpoolmanConfig | null,
    signal?: AbortSignal,
  ): Promise<IntegrationSettings>;
  pullSpoolman(signal?: AbortSignal): Promise<Record<string, unknown>>;
  saveAiProvider(
    config: AiProviderConfig,
    credentialUpdate: { action: "keep" | "clear" | "setSession"; secret?: string },
    signal?: AbortSignal,
  ): Promise<AiProviderConfig>;
  deleteAiProvider(providerId: string, signal?: AbortSignal): Promise<void>;
  suggestAiTags(
    providerId: string,
    targets: VersionedTarget[],
    signal?: AbortSignal,
  ): Promise<AiSuggestionBatch>;
  applyAiReview(
    reviewToken: string,
    decisions: Array<{
      modelId: string;
      action: "accept" | "reject";
      acceptedSuggestionIds: string[];
    }>,
    strategy: "append" | "replace" | "merge",
    signal?: AbortSignal,
  ): Promise<MutationVersion[]>;
  discardAiReview(reviewToken: string, signal?: AbortSignal): Promise<void>;
  listRoots(signal?: AbortSignal): Promise<BrowserRootHandle[]>;
  scanStatus(signal?: AbortSignal): Promise<BrowserScanStatus>;
  startScan(rootId?: string | null, signal?: AbortSignal): Promise<BrowserScanStatus>;
  cancelScan(signal?: AbortSignal): Promise<BrowserScanStatus>;
  startThumbnailJob(
    modelIds: string[],
    force: boolean,
    signal?: AbortSignal,
  ): Promise<BrowserScanStatus>;
  listModels(options?: BrowserModelQuery): Promise<CursorPage<BrowserModel>>;
  getModel(modelId: string, signal?: AbortSignal): Promise<BrowserModel>;
  listThumbnails(modelId: string, signal?: AbortSignal): Promise<ThumbnailDescriptor[]>;
  addThumbnail(
    modelId: string,
    expectedVersion: string,
    file: File,
    signal?: AbortSignal,
  ): Promise<string>;
  setActiveThumbnail(
    modelId: string,
    expectedVersion: string,
    thumbnailId: string,
    signal?: AbortSignal,
  ): Promise<string>;
  deleteThumbnail(
    modelId: string,
    expectedVersion: string,
    thumbnailId: string,
    signal?: AbortSignal,
  ): Promise<string>;
  issueThumbnailMedia(
    modelId: string,
    thumbnailId: string,
    signal?: AbortSignal,
  ): Promise<ContentLease>;
  issueModelDownload(
    modelId: string,
    expectedHash?: string | null,
    signal?: AbortSignal,
  ): Promise<ContentLease>;
  readContent(leaseId: string, mime: string, signal?: AbortSignal): Promise<Blob>;
  updateMetadata(
    modelId: string,
    expectedVersion: string,
    patch: BrowserMetadataPatch,
    signal?: AbortSignal,
  ): Promise<MutationVersion>;
  bulkUpdateMetadata(
    targets: VersionedTarget[],
    patch: BrowserMetadataPatch,
    signal?: AbortSignal,
  ): Promise<MutationVersion[]>;
  listTags(signal?: AbortSignal): Promise<BrowserTag[]>;
  createTag(name: string, category: string | null, signal?: AbortSignal): Promise<BrowserTag>;
  updateTag(
    tagId: string,
    name: string,
    category: string | null,
    signal?: AbortSignal,
  ): Promise<BrowserTag>;
  deleteTag(tagId: string, signal?: AbortSignal): Promise<void>;
  listFolders(signal?: AbortSignal): Promise<BrowserFolder[]>;
  createFolder(
    name: string,
    parentId?: string | null,
    signal?: AbortSignal,
  ): Promise<BrowserFolder>;
  renameFolder(folderId: string, name: string, signal?: AbortSignal): Promise<BrowserFolder>;
  deleteFolder(folderId: string, signal?: AbortSignal): Promise<void>;
  listFilaments(includeUnavailable?: boolean, signal?: AbortSignal): Promise<BrowserFilament[]>;
  createFilament(input: BrowserFilamentInput, signal?: AbortSignal): Promise<BrowserFilament>;
  updateFilament(
    id: string,
    expectedVersion: string,
    input: BrowserFilamentInput,
    signal?: AbortSignal,
  ): Promise<BrowserFilament>;
  deleteFilament(
    id: string,
    expectedVersion: string,
    signal?: AbortSignal,
  ): Promise<BrowserFilament | null>;
  assignFilaments(
    assignments: Array<{ target: VersionedTarget; filamentIds: string[] }>,
    signal?: AbortSignal,
  ): Promise<MutationVersion[]>;
  listHistory(modelId: string, signal?: AbortSignal): Promise<BrowserPrintAttempt[]>;
  createHistory(
    modelId: string,
    input: BrowserPrintAttemptInput,
    signal?: AbortSignal,
  ): Promise<BrowserPrintAttempt>;
  updateHistory(
    attemptId: string,
    input: BrowserPrintAttemptInput,
    signal?: AbortSignal,
  ): Promise<BrowserPrintAttempt>;
  deleteHistory(attemptId: string, signal?: AbortSignal): Promise<void>;
  createHistoryBatch(
    idempotencyKey: string,
    items: Array<{ modelId: string; input: BrowserPrintAttemptInput }>,
    signal?: AbortSignal,
  ): Promise<BrowserPrintAttempt[]>;
  events(signal: AbortSignal, lastEventId?: string): AsyncGenerator<LibraryEvent>;
  stageCaptureUpload(
    intent: {
      itemId: string;
      captureId: string;
      fileName: string;
      bytes: string;
      sha256: string;
    },
    body: Blob,
    signal?: AbortSignal,
  ): Promise<UploadToken>;
  importCapture(
    envelope: CaptureEnvelope,
    uploads: UploadToken[],
    signal?: AbortSignal,
  ): Promise<CaptureImportResult>;
  listCaptures(options?: {
    limit?: number;
    cursor?: string;
    signal?: AbortSignal;
  }): Promise<CursorPage<CaptureSummary>>;
  prepareBundle(bundleId: string, signal?: AbortSignal): Promise<BrowserBundlePreview>;
  readBundleGeometry(token: string, signal?: AbortSignal): Promise<ArrayBuffer>;
  closeBundle(token: string, signal?: AbortSignal): Promise<void>;
  issueBundleDownload(token: string, signal?: AbortSignal): Promise<ContentLease>;
  issueModelGeometry(
    modelId: string,
    expectedHash?: string | null,
    plateId?: number | null,
    signal?: AbortSignal,
  ): Promise<BrowserGeometryLease>;
  contentResponse(leaseId: string, signal?: AbortSignal): Promise<Response>;
  duplicateGroups(modelIds: string[], signal?: AbortSignal): Promise<DuplicateGroup[]>;
  embeddedMetadata(
    modelId: string,
    expectedHash?: string | null,
    signal?: AbortSignal,
  ): Promise<EmbeddedMetadataProposal>;
  applyEmbeddedMetadata(
    modelId: string,
    expectedVersion: string,
    expectedSourceHash: string,
    selection: { designer: boolean; sourceUrl: boolean; notes: boolean; modelLicense: boolean },
    signal?: AbortSignal,
  ): Promise<MutationVersion>;
}

export class HttpLibraryClient implements LibraryClient {
  private token = "";
  private readonly baseUrl: URL | null;
  readonly endpointSecurityError: LibraryHttpError | null;

  constructor(baseUrl = window.location.origin) {
    try {
      this.baseUrl = admitBrowserServerUrl(baseUrl);
      this.endpointSecurityError = null;
    } catch (error) {
      this.baseUrl = null;
      this.endpointSecurityError =
        error instanceof LibraryHttpError
          ? error
          : new LibraryHttpError("The server URL is invalid", "unsafeServerUrl", "local", false);
    }
  }

  setToken(token: string): void {
    if (this.endpointSecurityError) throw this.endpointSecurityError;
    this.token = token.trim();
  }

  clearToken(): void {
    this.token = "";
  }

  openApi(signal?: AbortSignal): Promise<{ paths: Record<string, unknown> }> {
    return this.json("/api/v1/openapi.json", { signal });
  }

  capabilities(signal?: AbortSignal): Promise<HostCapabilities> {
    return this.json("/api/v1/capabilities", { signal });
  }

  status(signal?: AbortSignal): Promise<Record<string, unknown>> {
    return this.json("/api/v1/status", { signal });
  }
  catalogFacets(signal?: AbortSignal): Promise<CatalogFacets> {
    return this.json("/api/v1/catalog/facets", { signal });
  }
  rootSettings(signal?: AbortSignal): Promise<IndexRootSettings[]> {
    return this.json("/api/v1/settings/roots", { signal });
  }
  updateRootSettings(
    handleId: string,
    settings: Pick<
      IndexRootSettings,
      "enabled" | "scanIntervalMinutes" | "maxFileBytes" | "scanZip"
    >,
    signal?: AbortSignal,
  ): Promise<IndexRootSettings> {
    return this.json(`/api/v1/settings/roots/${segment(handleId)}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(settings),
      signal,
    });
  }
  catalogSettings(signal?: AbortSignal): Promise<BrowserCatalogSettings> {
    return this.json("/api/v1/settings/catalog", { signal });
  }
  updateCatalogSettings(
    settings: BrowserCatalogSettings,
    signal?: AbortSignal,
  ): Promise<BrowserCatalogSettings> {
    return this.json("/api/v1/settings/catalog", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(settings),
      signal,
    });
  }
  planManagementAction(action: ManagementAction, signal?: AbortSignal): Promise<ActionPlan> {
    return this.json("/api/v1/management/actions/plan", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(action),
      signal,
    });
  }
  executeManagementAction(token: string, signal?: AbortSignal): Promise<ManagementResult> {
    return this.json("/api/v1/management/actions/execute", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ token }),
      signal,
    });
  }
  integrationSettings(signal?: AbortSignal): Promise<IntegrationSettings> {
    return this.json("/api/v1/integrations/settings", { signal });
  }
  saveSpoolmanConfig(
    config: SpoolmanConfig | null,
    signal?: AbortSignal,
  ): Promise<IntegrationSettings> {
    return this.json("/api/v1/integrations/spoolman", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ config }),
      signal,
    });
  }
  pullSpoolman(signal?: AbortSignal): Promise<Record<string, unknown>> {
    return this.json("/api/v1/integrations/spoolman/pull", { method: "POST", signal });
  }
  saveAiProvider(
    config: AiProviderConfig,
    credentialUpdate: { action: "keep" | "clear" | "setSession"; secret?: string },
    signal?: AbortSignal,
  ): Promise<AiProviderConfig> {
    return this.json(`/api/v1/integrations/ai/providers/${segment(config.id)}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ config, credentialUpdate }),
      signal,
    });
  }
  deleteAiProvider(providerId: string, signal?: AbortSignal): Promise<void> {
    return this.json(`/api/v1/integrations/ai/providers/${segment(providerId)}`, {
      method: "DELETE",
      signal,
    });
  }
  suggestAiTags(
    providerId: string,
    targets: VersionedTarget[],
    signal?: AbortSignal,
  ): Promise<AiSuggestionBatch> {
    return this.json("/api/v1/integrations/ai/suggestions", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ providerId, targets }),
      signal,
    });
  }
  applyAiReview(
    reviewToken: string,
    decisions: Array<{
      modelId: string;
      action: "accept" | "reject";
      acceptedSuggestionIds: string[];
    }>,
    strategy: "append" | "replace" | "merge",
    signal?: AbortSignal,
  ): Promise<MutationVersion[]> {
    return this.json(`/api/v1/integrations/ai/reviews/${segment(reviewToken)}/apply`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ decisions, strategy }),
      signal,
    });
  }
  discardAiReview(reviewToken: string, signal?: AbortSignal): Promise<void> {
    return this.json(`/api/v1/integrations/ai/reviews/${segment(reviewToken)}`, {
      method: "DELETE",
      signal,
    });
  }
  listRoots(signal?: AbortSignal): Promise<BrowserRootHandle[]> {
    return this.json("/api/v1/roots", { signal });
  }
  scanStatus(signal?: AbortSignal): Promise<BrowserScanStatus> {
    return this.json("/api/v1/scan", { signal });
  }
  startScan(rootId: string | null = null, signal?: AbortSignal): Promise<BrowserScanStatus> {
    return this.json("/api/v1/scan", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ rootId }),
      signal,
    });
  }
  cancelScan(signal?: AbortSignal): Promise<BrowserScanStatus> {
    return this.json("/api/v1/scan", { method: "DELETE", signal });
  }
  startThumbnailJob(
    modelIds: string[],
    force: boolean,
    signal?: AbortSignal,
  ): Promise<BrowserScanStatus> {
    return this.json("/api/v1/thumbnails/jobs", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ modelIds, force }),
      signal,
    });
  }

  listModels(options: BrowserModelQuery = {}): Promise<CursorPage<BrowserModel>> {
    const query = new URLSearchParams({ limit: String(options.limit ?? 50) });
    if (options.cursor) query.set("cursor", options.cursor);
    if (options.query) query.set("query", options.query);
    if (options.folderId) query.set("folderId", options.folderId);
    if (options.sort) query.set("sort", options.sort);
    if (options.direction) query.set("direction", options.direction);
    if (options.tagId) query.set("tagId", options.tagId);
    if (options.category) query.set("category", options.category);
    if (options.designer) query.set("designer", options.designer);
    if (options.modelLicense) query.set("modelLicense", options.modelLicense);
    if (options.printStatus) query.set("printStatus", options.printStatus);
    if (options.everPrinted !== undefined) query.set("everPrinted", String(options.everPrinted));
    if (options.bundleId) query.set("bundleId", options.bundleId);
    if (options.rootId) query.set("rootId", options.rootId);
    if (options.sourceDirectory) query.set("sourceDirectory", options.sourceDirectory);
    if (options.minimumRating !== undefined)
      query.set("minimumRating", String(options.minimumRating));
    if (options.favorite !== undefined) query.set("favorite", String(options.favorite));
    if (options.filamentId) query.set("filamentId", options.filamentId);
    if (options.sourceMissing !== undefined)
      query.set("sourceMissing", String(options.sourceMissing));
    if (options.duplicatesOnly !== undefined)
      query.set("duplicatesOnly", String(options.duplicatesOnly));
    if (options.sourceKind) query.set("sourceKind", options.sourceKind);
    if (options.group) query.set("group", options.group);
    if (options.thumbnailMode) query.set("thumbnailMode", options.thumbnailMode);
    return this.json(`/api/v1/models?${query}`, { signal: options.signal });
  }
  getModel(modelId: string, signal?: AbortSignal): Promise<BrowserModel> {
    return this.json(`/api/v1/models/${segment(modelId)}`, { signal });
  }

  listThumbnails(modelId: string, signal?: AbortSignal): Promise<ThumbnailDescriptor[]> {
    return this.json(`/api/v1/models/${segment(modelId)}/thumbnails`, { signal });
  }

  async addThumbnail(
    modelId: string,
    expectedVersion: string,
    file: File,
    signal?: AbortSignal,
  ): Promise<string> {
    const form = new FormData();
    form.set("expectedVersion", expectedVersion);
    form.set("file", file, file.name);
    const result = await this.json<{ metadataVersion: string }>(
      `/api/v1/models/${segment(modelId)}/thumbnails`,
      { method: "POST", body: form, signal },
    );
    return result.metadataVersion;
  }

  async setActiveThumbnail(
    modelId: string,
    expectedVersion: string,
    thumbnailId: string,
    signal?: AbortSignal,
  ): Promise<string> {
    const result = await this.versionMutation(
      `/api/v1/models/${segment(modelId)}/thumbnails/${segment(thumbnailId)}/active`,
      "PATCH",
      expectedVersion,
      signal,
    );
    return result.metadataVersion;
  }

  async deleteThumbnail(
    modelId: string,
    expectedVersion: string,
    thumbnailId: string,
    signal?: AbortSignal,
  ): Promise<string> {
    const result = await this.versionMutation(
      `/api/v1/models/${segment(modelId)}/thumbnails/${segment(thumbnailId)}`,
      "DELETE",
      expectedVersion,
      signal,
    );
    return result.metadataVersion;
  }

  issueThumbnailMedia(
    modelId: string,
    thumbnailId: string,
    signal?: AbortSignal,
  ): Promise<ContentLease> {
    return this.json(
      `/api/v1/models/${segment(modelId)}/thumbnails/${segment(thumbnailId)}/media-lease`,
      { method: "POST", signal },
    );
  }

  issueModelDownload(
    modelId: string,
    expectedHash?: string | null,
    signal?: AbortSignal,
  ): Promise<ContentLease> {
    return this.json(`/api/v1/models/${segment(modelId)}/download-lease`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ expectedHash: expectedHash || null }),
      signal,
    });
  }

  async readContent(leaseId: string, mime: string, signal?: AbortSignal): Promise<Blob> {
    const response = await this.fetch(`/api/v1/content/${segment(leaseId)}`, { signal });
    return new Blob([await response.arrayBuffer()], { type: mime });
  }

  updateMetadata(
    modelId: string,
    expectedVersion: string,
    patch: BrowserMetadataPatch,
    signal?: AbortSignal,
  ): Promise<MutationVersion> {
    return this.json(`/api/v1/models/${segment(modelId)}/metadata`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ expectedVersion, patch }),
      signal,
    });
  }

  bulkUpdateMetadata(
    targets: VersionedTarget[],
    patch: BrowserMetadataPatch,
    signal?: AbortSignal,
  ): Promise<MutationVersion[]> {
    return this.json("/api/v1/models/metadata/bulk", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ targets, patch }),
      signal,
    });
  }

  listTags(signal?: AbortSignal): Promise<BrowserTag[]> {
    return this.json("/api/v1/tags", { signal });
  }

  createTag(name: string, category: string | null, signal?: AbortSignal): Promise<BrowserTag> {
    return this.json("/api/v1/tags", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name, category }),
      signal,
    });
  }

  updateTag(
    tagId: string,
    name: string,
    category: string | null,
    signal?: AbortSignal,
  ): Promise<BrowserTag> {
    return this.json(`/api/v1/tags/${segment(tagId)}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name, category }),
      signal,
    });
  }

  deleteTag(tagId: string, signal?: AbortSignal): Promise<void> {
    return this.json(`/api/v1/tags/${segment(tagId)}`, { method: "DELETE", signal });
  }

  listFolders(signal?: AbortSignal): Promise<BrowserFolder[]> {
    return this.json("/api/v1/folders", { signal });
  }
  createFolder(
    name: string,
    parentId: string | null = null,
    signal?: AbortSignal,
  ): Promise<BrowserFolder> {
    return this.json("/api/v1/folders", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name, parentId }),
      signal,
    });
  }
  renameFolder(folderId: string, name: string, signal?: AbortSignal): Promise<BrowserFolder> {
    return this.json(`/api/v1/folders/${segment(folderId)}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name }),
      signal,
    });
  }
  deleteFolder(folderId: string, signal?: AbortSignal): Promise<void> {
    return this.json(`/api/v1/folders/${segment(folderId)}`, { method: "DELETE", signal });
  }
  listFilaments(includeUnavailable = true, signal?: AbortSignal): Promise<BrowserFilament[]> {
    return this.json(`/api/v1/filaments?includeUnavailable=${includeUnavailable}`, { signal });
  }
  createFilament(input: BrowserFilamentInput, signal?: AbortSignal): Promise<BrowserFilament> {
    return this.json("/api/v1/filaments", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(input),
      signal,
    });
  }
  updateFilament(
    id: string,
    expectedVersion: string,
    input: BrowserFilamentInput,
    signal?: AbortSignal,
  ): Promise<BrowserFilament> {
    return this.json(`/api/v1/filaments/${segment(id)}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ expectedVersion, input }),
      signal,
    });
  }
  deleteFilament(
    id: string,
    expectedVersion: string,
    signal?: AbortSignal,
  ): Promise<BrowserFilament | null> {
    return this.json(`/api/v1/filaments/${segment(id)}`, {
      method: "DELETE",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ expectedVersion }),
      signal,
    });
  }
  assignFilaments(
    assignments: Array<{ target: VersionedTarget; filamentIds: string[] }>,
    signal?: AbortSignal,
  ): Promise<MutationVersion[]> {
    return this.json("/api/v1/models/filaments/bulk", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ assignments }),
      signal,
    });
  }
  listHistory(modelId: string, signal?: AbortSignal): Promise<BrowserPrintAttempt[]> {
    return this.json(`/api/v1/models/${segment(modelId)}/history`, { signal });
  }
  createHistory(
    modelId: string,
    input: BrowserPrintAttemptInput,
    signal?: AbortSignal,
  ): Promise<BrowserPrintAttempt> {
    return this.json(`/api/v1/models/${segment(modelId)}/history`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(input),
      signal,
    });
  }
  updateHistory(
    attemptId: string,
    input: BrowserPrintAttemptInput,
    signal?: AbortSignal,
  ): Promise<BrowserPrintAttempt> {
    return this.json(`/api/v1/history/${segment(attemptId)}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(input),
      signal,
    });
  }
  deleteHistory(attemptId: string, signal?: AbortSignal): Promise<void> {
    return this.json(`/api/v1/history/${segment(attemptId)}`, { method: "DELETE", signal });
  }
  createHistoryBatch(
    idempotencyKey: string,
    items: Array<{ modelId: string; input: BrowserPrintAttemptInput }>,
    signal?: AbortSignal,
  ): Promise<BrowserPrintAttempt[]> {
    return this.json("/api/v1/history/bulk", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ idempotencyKey, items }),
      signal,
    });
  }

  async *events(signal: AbortSignal, lastEventId?: string): AsyncGenerator<LibraryEvent> {
    const headers: Record<string, string> = { Accept: "text/event-stream" };
    if (lastEventId) headers["Last-Event-ID"] = lastEventId;
    const response = await this.fetch("/api/v1/events", {
      signal,
      headers,
    });
    if (!response.body) throw new Error("Event stream has no response body");
    const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
    let pending = "";
    let eventType = "message";
    let data = "";
    while (!signal.aborted) {
      const { value, done } = await reader.read();
      if (done) break;
      pending += value;
      const lines = pending.split(/\r?\n/);
      pending = lines.pop() ?? "";
      for (const line of lines) {
        if (line.startsWith("event:")) eventType = line.slice(6).trim();
        else if (line.startsWith("data:")) data += line.slice(5).trim();
        else if (!line && data) {
          yield { ...JSON.parse(data), type: eventType } as LibraryEvent;
          eventType = "message";
          data = "";
        }
      }
    }
  }

  async stageCaptureUpload(
    intent: { itemId: string; captureId: string; fileName: string; bytes: string; sha256: string },
    body: Blob,
    signal?: AbortSignal,
  ): Promise<UploadToken> {
    const form = new FormData();
    form.set("intent", new Blob([JSON.stringify(intent)], { type: "application/json" }));
    form.set("file", body, intent.fileName);
    return this.json("/api/v1/captures/uploads", { method: "POST", body: form, signal });
  }

  importCapture(
    envelope: CaptureEnvelope,
    uploads: UploadToken[],
    signal?: AbortSignal,
  ): Promise<CaptureImportResult> {
    return this.json("/api/v1/captures/import", {
      method: "POST",
      body: JSON.stringify({ envelope, uploads }),
      headers: { "Content-Type": "application/json" },
      signal,
    });
  }
  listCaptures(
    options: { limit?: number; cursor?: string; signal?: AbortSignal } = {},
  ): Promise<CursorPage<CaptureSummary>> {
    const query = new URLSearchParams({ limit: String(options.limit ?? 50) });
    if (options.cursor) query.set("cursor", options.cursor);
    return this.json(`/api/v1/captures?${query}`, { signal: options.signal });
  }
  prepareBundle(bundleId: string, signal?: AbortSignal): Promise<BrowserBundlePreview> {
    return this.json(`/api/v1/bundles/${segment(bundleId)}/preview`, { method: "POST", signal });
  }
  async readBundleGeometry(token: string, signal?: AbortSignal): Promise<ArrayBuffer> {
    const response = await this.fetch(`/api/v1/bundle-previews/${segment(token)}/geometry`, {
      signal,
    });
    return response.arrayBuffer();
  }
  closeBundle(token: string, signal?: AbortSignal): Promise<void> {
    return this.json(`/api/v1/bundle-previews/${segment(token)}`, {
      method: "DELETE",
      signal,
    });
  }
  issueBundleDownload(token: string, signal?: AbortSignal): Promise<ContentLease> {
    return this.json(`/api/v1/bundle-previews/${segment(token)}/download-lease`, {
      method: "POST",
      signal,
    });
  }
  issueModelGeometry(
    modelId: string,
    expectedHash?: string | null,
    plateId?: number | null,
    signal?: AbortSignal,
  ): Promise<BrowserGeometryLease> {
    return this.json(`/api/v1/models/${segment(modelId)}/geometry-lease`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ expectedHash: expectedHash || null, plateId: plateId ?? null }),
      signal,
    });
  }
  contentResponse(leaseId: string, signal?: AbortSignal): Promise<Response> {
    return this.fetch(`/api/v1/content/${segment(leaseId)}`, { signal });
  }
  duplicateGroups(modelIds: string[], signal?: AbortSignal): Promise<DuplicateGroup[]> {
    return this.json("/api/v1/duplicates/query", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ modelIds }),
      signal,
    });
  }
  embeddedMetadata(
    modelId: string,
    expectedHash?: string | null,
    signal?: AbortSignal,
  ): Promise<EmbeddedMetadataProposal> {
    return this.json(`/api/v1/models/${segment(modelId)}/embedded-metadata/proposal`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ expectedHash: expectedHash || null }),
      signal,
    });
  }
  applyEmbeddedMetadata(
    modelId: string,
    expectedVersion: string,
    expectedSourceHash: string,
    selection: { designer: boolean; sourceUrl: boolean; notes: boolean; modelLicense: boolean },
    signal?: AbortSignal,
  ): Promise<MutationVersion> {
    return this.json(`/api/v1/models/${segment(modelId)}/embedded-metadata/apply`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ expectedVersion, expectedSourceHash, selection }),
      signal,
    });
  }

  private async json<T>(path: string, init: RequestInit = {}): Promise<T> {
    const response = await this.fetch(path, init);
    if (response.status === 204) return undefined as T;
    return (await response.json()) as T;
  }

  private versionMutation(
    path: string,
    method: "PATCH" | "DELETE",
    expectedVersion: string,
    signal?: AbortSignal,
  ): Promise<{ metadataVersion: string }> {
    return this.json(path, {
      method,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ expectedVersion }),
      signal,
    });
  }

  private async fetch(path: string, init: RequestInit): Promise<Response> {
    if (this.endpointSecurityError || !this.baseUrl) {
      throw (
        this.endpointSecurityError ??
        new LibraryHttpError("The server URL is invalid", "unsafeServerUrl", "local", false)
      );
    }
    if (!this.token)
      throw new LibraryHttpError(
        "Authentication token is required",
        "unauthorized",
        "local",
        false,
      );
    const headers = new Headers(init.headers);
    headers.set("Authorization", `Bearer ${this.token}`);
    const response = await fetch(new URL(path, this.baseUrl), {
      ...init,
      headers,
      credentials: "omit",
      referrerPolicy: "no-referrer",
      cache: "no-store",
      redirect: "error",
    });
    if (response.ok) return response;
    const body = (await response.json().catch(() => null)) as {
      error?: {
        code?: string;
        message?: string;
        requestId?: string;
        retryable?: boolean;
        currentVersion?: string;
      };
    } | null;
    throw new LibraryHttpError(
      body?.error?.message ?? `Request failed with HTTP ${response.status}`,
      body?.error?.code ?? "httpError",
      body?.error?.requestId ?? response.headers.get("x-request-id") ?? "unknown",
      body?.error?.retryable ?? false,
      body?.error?.currentVersion,
    );
  }
}

function segment(value: string): string {
  return encodeURIComponent(value);
}
