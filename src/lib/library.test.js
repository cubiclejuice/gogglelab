import { expect, test } from "bun:test";
import {
  canOpenLibraryModel,
  didLibraryScanComplete,
  filterAndSortLibraryModels,
  libraryFolderDepths,
  normalizeLibraryTimestamp,
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
    scanStatusMessage({ ...idle, filesIndexed: 3, errors: ["unreadable"], finishedAt: "now" }),
  ).toBe("Scan finished · 3 indexed · 1 issue");
  expect(didLibraryScanComplete(false, idle)).toBe(false);
  expect(didLibraryScanComplete(true, { running: true })).toBe(false);
  expect(didLibraryScanComplete(true, idle)).toBe(true);
});
