import { expect, test } from "bun:test";
import {
  canOpenLibraryModel,
  clampLibraryNavigationWidth,
  didLibraryScanComplete,
  LibraryPanel,
  filterAndSortLibraryModels,
  LIBRARY_CATALOG_TIER,
  LIBRARY_VIEW_MODES,
  libraryFolderDepths,
  indexedDirectoryNodes,
  matchesLibraryGroup,
  matchesLibraryMetadataFilters,
  matchesIndexedDirectory,
  nextLibraryNavigationWidth,
  normalizeLibraryTimestamp,
  normalizeCatalogSettings,
  scanStatusMessage,
  ThumbnailRequestGuard,
  thumbnailCacheInvalidations,
} from "./library";

const folder = (id, parent_id = null) => ({ id, parent_id, name: id, model_count: 0 });

test("Library folder depths follow valid parents and stop at missing legacy parents", () => {
  const depths = libraryFolderDepths([
    folder("root"),
    folder("child", "root"),
    folder("grandchild", "child"),
    folder("orphan", "missing"),
  ]);
  expect(Object.fromEntries(depths)).toEqual({ root: 0, child: 1, grandchild: 2, orphan: 0 });
});

test("Library folder depth calculation terminates corrupt parent cycles", () => {
  const depths = libraryFolderDepths([folder("a", "b"), folder("b", "a"), folder("self", "self")]);
  expect(Object.fromEntries(depths)).toEqual({ a: 0, b: 0, self: 0 });
});

const model = (id, overrides = {}) => ({
  id,
  display_name: id,
  description: "",
  folder_id: "root",
  original_name: `${id}.stl`,
  format: "stl",
  bytes: 1,
  content_hash: null,
  storage_path: null,
  metadata_version: 1,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  tags: [],
  source_kind: "managed",
  source_path: null,
  source_root_id: null,
  source_missing: false,
  source_modified_at: null,
  thumbnail_path: null,
  ...overrides,
});

const view = (overrides = {}) => ({
  query: "",
  folderId: null,
  rootId: null,
  source: "all",
  status: "all",
  sort: "name",
  ...overrides,
});

test("catalog search spans roots and source/status filters compose", () => {
  const models = [
    model("managed"),
    model("gear", {
      display_name: "Planetary gear",
      source_kind: "indexed",
      source_path: "mechanisms/gear.stl",
      source_root_id: "root-a",
      tags: ["drive"],
    }),
    model("missing", {
      source_kind: "indexed",
      source_path: "lost/fixture.stl",
      source_root_id: "root-a",
      source_missing: true,
    }),
  ];
  expect(
    filterAndSortLibraryModels(
      models,
      view({ query: "mechanisms", rootId: "root-a", source: "indexed", status: "available" }),
    ).map((item) => item.id),
  ).toEqual(["gear"]);
  expect(
    filterAndSortLibraryModels(models, view({ status: "missing" })).map((item) => item.id),
  ).toEqual(["missing"]);
});

test("group filters distinguish standalone, bundle, and ZIP-member models", () => {
  expect(matchesLibraryGroup(model("plain"), "standalone")).toBe(true);
  expect(matchesLibraryGroup(model("bundle", { bundle_id: "bundle-1" }), "bundle")).toBe(true);
  expect(matchesLibraryGroup(model("zip", { source_archive_member: "parts/a.stl" }), "zip")).toBe(
    true,
  );
  expect(
    matchesLibraryGroup(model("zip", { source_archive_member: "parts/a.stl" }), "standalone"),
  ).toBe(false);
});

test("metadata filters compose print status, designer, stable tag ID, and category", () => {
  const tags = [
    { id: "tag-mech", name: "Mechanical", category: "Type" },
    { id: "tag-favorite", name: "Favorite", category: "Workflow" },
  ];
  const candidate = model("candidate", {
    tags: ["Mechanical", "Favorite"],
    metadata: {
      designer: "Ada",
      print_status: "queued",
      print_count: 0,
    },
  });
  expect(
    matchesLibraryMetadataFilters(
      candidate,
      {
        printStatus: "queued",
        designer: "Ada",
        tagId: "tag-mech",
        category: "Workflow",
      },
      tags,
    ),
  ).toBe(true);
  expect(
    matchesLibraryMetadataFilters(
      candidate,
      { printStatus: "printed", designer: "Ada", tagId: "tag-mech", category: "Workflow" },
      tags,
    ),
  ).toBe(false);
  expect(
    matchesLibraryMetadataFilters(
      candidate,
      { printStatus: "all", designer: "", tagId: "deleted-tag-id", category: "" },
      tags,
    ),
  ).toBe(false);
});

test("directory tree counts descendants and scopes normalized indexed paths", () => {
  const models = [
    model("a", { source_root_id: "root", source_path: "parts/gears/a.stl" }),
    model("b", { source_root_id: "root", source_path: "parts\\b.stl" }),
    model("zip", {
      source_root_id: "root",
      source_path: "archives/pack.zip",
      source_archive_member: "x.stl",
    }),
    model("other", { source_root_id: "other", source_path: "parts/c.stl" }),
  ];
  expect(indexedDirectoryNodes(models, "root")).toEqual([
    { prefix: "archives", name: "archives", depth: 0, count: 1 },
    { prefix: "parts", name: "parts", depth: 0, count: 2 },
    { prefix: "parts/gears", name: "gears", depth: 1, count: 1 },
  ]);
  expect(matchesIndexedDirectory(models[0], "root", "parts")).toBe(true);
  expect(matchesIndexedDirectory(models[2], "root", "parts")).toBe(false);
});

test("parent and license filters compose without confusing no-parent with all", () => {
  const child = model("child", {
    metadata: {
      parent_model_id: "parent",
      model_license: "CC0",
      print_status: "unprinted",
      print_count: 0,
    },
  });
  expect(
    matchesLibraryMetadataFilters(
      child,
      {
        printStatus: "all",
        designer: "",
        tagId: "",
        category: "",
        parentId: "parent",
        license: "CC0",
      },
      [],
    ),
  ).toBe(true);
  expect(
    matchesLibraryMetadataFilters(
      child,
      {
        printStatus: "all",
        designer: "",
        tagId: "",
        category: "",
        parentId: "__none__",
        license: "",
      },
      [],
    ),
  ).toBe(false);
});

test("sidebar resize keyboard behavior remains within desktop bounds", () => {
  expect(clampLibraryNavigationWidth(100)).toBe(200);
  expect(clampLibraryNavigationWidth(900)).toBe(420);
  expect(nextLibraryNavigationWidth(270, "ArrowRight")).toBe(286);
  expect(nextLibraryNavigationWidth(270, "Home")).toBe(200);
  expect(nextLibraryNavigationWidth(270, "End")).toBe(420);
});

test("all catalog views, settings, and metadata tools remain in the Free feature tier", () => {
  expect(LIBRARY_CATALOG_TIER).toBe("free");
  expect(LIBRARY_VIEW_MODES).toEqual(["list", "preview", "detailed"]);
});

test("catalog settings normalize supported render and scan bounds before saving", () => {
  expect(
    normalizeCatalogSettings({
      thumbnail_size: 512,
      thumbnail_background: "#abcdef",
      thumbnail_concurrency: 9,
      scan_batch_size: 3,
    }),
  ).toEqual({
    thumbnail_size: 512,
    thumbnail_background: "#ABCDEF",
    thumbnail_concurrency: 2,
    scan_batch_size: 16,
  });
});

test("catalog sorts by natural name, descending size, and descending date", () => {
  const models = [
    model("part10", { bytes: 10, source_modified_at: 100 }),
    model("part2", { bytes: 20, source_modified_at: 300 }),
    model("part1", { bytes: 30, source_modified_at: 200 }),
  ];
  expect(filterAndSortLibraryModels(models, view()).map((item) => item.id)).toEqual([
    "part1",
    "part2",
    "part10",
  ]);
  expect(filterAndSortLibraryModels(models, view({ sort: "size" })).map((item) => item.id)).toEqual(
    ["part1", "part2", "part10"],
  );
  expect(filterAndSortLibraryModels(models, view({ sort: "date" })).map((item) => item.id)).toEqual(
    ["part2", "part1", "part10"],
  );
});

test("date sorting normalizes native nanoseconds, SQLite seconds, and legacy ISO dates", () => {
  const models = [
    model("managed-seconds", { updated_at: "1767225600" }),
    model("indexed-legacy-seconds", { source_modified_at: 1768435200 }),
    model("legacy-iso", { updated_at: "2026-02-01T00:00:00Z" }),
    model("indexed-nanoseconds", { source_modified_at: 1_772_323_200_000_000_000 }),
  ];
  expect(filterAndSortLibraryModels(models, view({ sort: "date" })).map((item) => item.id)).toEqual(
    ["indexed-nanoseconds", "legacy-iso", "indexed-legacy-seconds", "managed-seconds"],
  );
});

test("root scan timestamps normalize numeric epoch-second strings to a finite date", () => {
  const milliseconds = normalizeLibraryTimestamp("1767225600");
  expect(milliseconds).toBe(1767225600000);
  expect(Number.isFinite(new Date(milliseconds).getTime())).toBe(true);
});

test("missing indexed sources cannot be opened while managed and present sources can", () => {
  expect(canOpenLibraryModel(model("managed"))).toBe(true);
  expect(canOpenLibraryModel(model("present", { source_kind: "indexed" }))).toBe(true);
  expect(canOpenLibraryModel(model("missing", { source_missing: true }))).toBe(false);
});

test("stale thumbnail responses are rejected after a newer request starts", () => {
  const guard = new ThumbnailRequestGuard();
  const first = guard.begin("model");
  const second = guard.begin("model");
  expect(guard.isCurrent("model", first)).toBe(false);
  expect(guard.isCurrent("model", second)).toBe(true);
  guard.invalidate("model");
  expect(guard.isCurrent("model", second)).toBe(false);
});

test("thumbnail cache invalidates content changes, availability changes, and removed rows only", () => {
  const previous = [
    model("content", { content_hash: "old", thumbnail_path: "same.png" }),
    model("available", { content_hash: "same", source_missing: true }),
    model("backing", { content_hash: "same", thumbnail_path: "old.png" }),
    model("metadata", { content_hash: "same", display_name: "Old name" }),
    model("removed", { content_hash: "same" }),
  ];
  const next = [
    model("content", { content_hash: "new", thumbnail_path: "same.png" }),
    model("available", { content_hash: "same", source_missing: false }),
    model("backing", { content_hash: "same", thumbnail_path: "new.png" }),
    model("metadata", { content_hash: "same", display_name: "New name" }),
  ];
  expect(thumbnailCacheInvalidations(previous, next)).toEqual([
    "content",
    "available",
    "backing",
    "removed",
  ]);
});

test("scan state copy distinguishes idle, running, cancelling, and completed", () => {
  const idle = {
    running: false,
    cancelling: false,
    currentRootId: null,
    rootsTotal: 0,
    rootsCompleted: 0,
    filesVisited: 0,
    filesIndexed: 0,
    errors: [],
    startedAt: null,
    finishedAt: null,
    jobKind: null,
    thumbnailsTotal: 0,
    thumbnailsCompleted: 0,
  };
  expect(scanStatusMessage(idle)).toBe("Ready to scan indexed folders.");
  expect(
    scanStatusMessage({
      ...idle,
      running: true,
      rootsTotal: 2,
      rootsCompleted: 1,
      filesVisited: 42,
      filesIndexed: 7,
    }),
  ).toContain("Scanning · 1/2 folders");
  expect(scanStatusMessage({ ...idle, running: true, cancelling: true })).toContain(
    "Stopping scan",
  );
  expect(
    scanStatusMessage({
      ...idle,
      running: true,
      jobKind: "thumbnails",
      thumbnailsTotal: 20,
      thumbnailsCompleted: 7,
    }),
  ).toBe("Generating previews · 7/20 complete");
  expect(
    scanStatusMessage({
      ...idle,
      jobKind: "thumbnails",
      thumbnailsTotal: 20,
      thumbnailsCompleted: 20,
      finishedAt: "now",
    }),
  ).toBe("Preview generation finished · 20/20 complete");
  expect(
    scanStatusMessage({ ...idle, filesIndexed: 3, errors: ["unreadable"], finishedAt: "now" }),
  ).toBe("Scan finished · 3 indexed · 1 issue");
  expect(didLibraryScanComplete(null, idle)).toBe(false);
  expect(didLibraryScanComplete({ ...idle, running: true }, { ...idle, running: true })).toBe(
    false,
  );
  expect(didLibraryScanComplete({ ...idle, running: true }, idle)).toBe(true);
});

test("completed scans between polls invalidate Library data exactly once", () => {
  const idle = { running: false, startedAt: null, finishedAt: null };
  const complete = { running: false, startedAt: "1792400000", finishedAt: "1792400000" };
  expect(didLibraryScanComplete(idle, complete)).toBe(true);
  expect(didLibraryScanComplete(complete, complete)).toBe(false);
  expect(
    didLibraryScanComplete(complete, {
      ...complete,
      startedAt: "1792400010",
      finishedAt: "1792400010",
    }),
  ).toBe(true);
  // An explicitly successful start marks a pending completion even when both
  // scans have the native status clock's same second-resolution timestamps.
  expect(didLibraryScanComplete({ ...complete, running: true }, complete)).toBe(true);
});

function scanPanelFixture(onPollScanStatus) {
  const node = () => ({ removeAttribute() {} });
  const panel = {
    root: { open: true },
    scanPollRevision: 0,
    scanPollInFlight: false,
    previousScanStatus: null,
    onPollScanStatus,
    onScanCompleted() {
      panel.completed += 1;
    },
    completed: 0,
    scanStatus: node(),
    scanButton: node(),
    generateMissingButton: node(),
    regenerateAllButton: node(),
    cancelScanButton: node(),
    onStartScan: async () => {},
  };
  panel.noteStartedJob = () => LibraryPanel.prototype.noteStartedJob.call(panel);
  panel.setScanStatus = (status) => LibraryPanel.prototype.setScanStatus.call(panel, status);
  panel.pollScanStatus = () => LibraryPanel.prototype.pollScanStatus.call(panel);
  return panel;
}
const finishedScan = {
  running: false,
  cancelling: false,
  currentRootId: null,
  rootsTotal: 1,
  rootsCompleted: 1,
  filesVisited: 1,
  filesIndexed: 1,
  errors: [],
  startedAt: "1792400000",
  finishedAt: "1792400000",
  jobKind: "scan",
  thumbnailsTotal: 1,
  thumbnailsCompleted: 1,
};

test("successful fast scan invalidates older in-flight polls and refreshes same-second completion", async () => {
  let resolveOld;
  let calls = 0;
  const old = new Promise((resolve) => {
    resolveOld = resolve;
  });
  const panel = scanPanelFixture(async () => (++calls === 1 ? old : finishedScan));
  panel.previousScanStatus = finishedScan;
  const oldPoll = panel.pollScanStatus();
  await LibraryPanel.prototype.startScan.call(panel, "root");
  resolveOld(finishedScan);
  await oldPoll;
  expect(panel.completed).toBe(0);
  await panel.pollScanStatus();
  expect(panel.completed).toBe(1);
  await panel.pollScanStatus();
  expect(panel.completed).toBe(1);
});

test("failed scan start leaves completion tracking and polling unchanged", async () => {
  let polls = 0;
  const panel = scanPanelFixture(async () => {
    polls += 1;
    return finishedScan;
  });
  panel.previousScanStatus = finishedScan;
  panel.onStartScan = async () => {
    throw new Error("Scan rejected");
  };
  await expect(LibraryPanel.prototype.startScan.call(panel, "root")).rejects.toThrow(
    "Scan rejected",
  );
  expect(panel.previousScanStatus).toBe(finishedScan);
  expect(panel.scanPollRevision).toBe(0);
  expect(panel.completed).toBe(0);
  expect(polls).toBe(0);
});

test("fast thumbnail jobs refresh same-second completion through the actual start handler", async () => {
  const completed = { ...finishedScan, jobKind: "thumbnails" };
  const panel = scanPanelFixture(async () => completed);
  panel.previousScanStatus = completed;
  panel.onStartThumbnailJob = async () => {};
  panel.runAction = async (action) => {
    await action();
  };
  panel.setStatus = () => {};
  await LibraryPanel.prototype.startThumbnailJob.call(panel, true);
  expect(panel.completed).toBe(1);
  await panel.pollScanStatus();
  expect(panel.completed).toBe(1);
});

test("Library replacement rejects a pre-replacement in-flight status reply", async () => {
  let finish;
  const pending = new Promise((resolve) => {
    finish = resolve;
  });
  const panel = scanPanelFixture(async () => pending);
  panel.previousScanStatus = { ...finishedScan, running: true };
  const dismiss = { dismiss() {} };
  Object.assign(panel, {
    metadataEditor: dismiss,
    collectionNameDialog: dismiss,
    tagManager: dismiss,
    printHistory: dismiss,
    selectedModelIds: new Set(),
    clearThumbnailCache() {},
  });
  const request = panel.pollScanStatus();
  LibraryPanel.prototype.invalidateAfterLibraryReplacement.call(panel);
  finish(finishedScan);
  await request;
  expect(panel.completed).toBe(0);
  expect(panel.previousScanStatus).toBe(null);
});
