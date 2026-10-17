import { expect, test } from "bun:test";
import {
  MAX_SLICER_PROFILE_SUGGESTIONS,
  SlicerProfileDraftManager,
  mergeSlicerProfileDrafts,
  reviewSlicerProfileSuggestions,
  slicerDefaultOptions,
} from "./settings";

const builtIn = {
  id: "vendor.slicer",
  bundle_id: "vendor.slicer",
  display_name: "Vendor Slicer",
  app_path: "/Applications/Vendor Slicer.app",
  is_custom: false,
  available: true,
};

const savedCustom = {
  id: "custom-1",
  bundle_id: "custom-1",
  display_name: "My Slicer",
  app_path: "/Applications/My Slicer.app",
  is_custom: true,
  available: true,
};

test("slicer draft merge deduplicates IDs and platform-equivalent executable paths", () => {
  const merged = mergeSlicerProfileDrafts(
    [
      {
        id: "custom-1",
        name: "Saved",
        path: "C:\\Apps\\Slicer\\slicer.exe",
        trust: false,
      },
    ],
    [
      { id: "custom-1", name: "Duplicate ID", path: "C:\\Apps\\Other.exe" },
      { id: null, name: "Same executable", path: "c:/apps/slicer/slicer.EXE" },
      { id: null, name: "New executable", path: "C:\\Apps\\New.exe" },
    ],
  );

  expect(merged).toEqual([
    {
      id: "custom-1",
      name: "Saved",
      path: "C:\\Apps\\Slicer\\slicer.exe",
      trust: false,
    },
    { id: null, name: "New executable", path: "C:\\Apps\\New.exe", trust: false },
  ]);
});

test("auto-detect results are deduplicated and bounded before review", () => {
  const candidates = Array.from({ length: MAX_SLICER_PROFILE_SUGGESTIONS + 3 }, (_, index) => ({
    id: null,
    name: `Candidate ${index}`,
    path: `/Applications/Candidate ${index}.app`,
  }));
  const reviewed = reviewSlicerProfileSuggestions(
    [
      ...candidates,
      { id: null, name: "Already saved", path: savedCustom.app_path },
      { id: null, name: "Duplicate candidate", path: candidates[0].path },
    ],
    [{ id: savedCustom.id, name: savedCustom.display_name, path: savedCustom.app_path }],
    [builtIn],
  );

  expect(reviewed).toHaveLength(MAX_SLICER_PROFILE_SUGGESTIONS);
  expect(reviewed.every((profile) => profile.id === null)).toBe(true);
  expect(reviewed.every((profile) => profile.trust === false)).toBe(true);
  expect(new Set(reviewed.map((profile) => profile.path)).size).toBe(reviewed.length);
});

test("unavailable profiles stay visible but cannot be selected as the default", () => {
  const unavailable = { ...savedCustom, available: false };
  const options = slicerDefaultOptions([builtIn, unavailable], unavailable.id);

  expect(options).toEqual([
    { value: "", label: "No default slicer", disabled: false },
    { value: builtIn.id, label: "Vendor Slicer", disabled: false },
    { value: unavailable.id, label: "My Slicer — unavailable", disabled: true },
  ]);
  expect(slicerDefaultOptions([builtIn], "removed-profile").at(-1)).toEqual({
    value: "removed-profile",
    label: "Saved slicer profile — unavailable",
    disabled: true,
  });
});

test("cancelled local picker leaves drafts unchanged and reports no error", async () => {
  const manager = new SlicerProfileDraftManager([builtIn]);
  const added = await manager.addFromPicker(async () => null);

  expect(added).toBe(false);
  expect(manager.drafts).toEqual([]);
  expect(manager.dirty).toBe(false);
  expect(manager.error).toBe("");
});

test("picker failures preserve existing drafts and announce an error", async () => {
  const manager = new SlicerProfileDraftManager([builtIn, savedCustom]);
  manager.setName("saved:custom-1", "Renamed Slicer");
  const added = await manager.addFromPicker(async () => {
    throw new Error("permission denied");
  });

  expect(added).toBe(false);
  expect(manager.drafts).toEqual([
    { id: "custom-1", name: "Renamed Slicer", path: savedCustom.app_path, trust: false },
  ]);
  expect(manager.dirty).toBe(true);
  expect(manager.error).toContain("permission denied");
});

test("detected profiles remain review suggestions until explicitly staged", async () => {
  const manager = new SlicerProfileDraftManager([builtIn]);
  const detected = await manager.detectSuggestions(async () => [
    { id: null, name: "Suggested Slicer", path: "/Applications/Suggested.app" },
  ]);

  expect(detected).toBe(true);
  expect(manager.drafts).toEqual([]);
  expect(manager.suggestions).toEqual([
    {
      id: null,
      name: "Suggested Slicer",
      path: "/Applications/Suggested.app",
      trust: false,
    },
  ]);
  manager.selectSuggestion(0, true);
  expect(manager.stageSelectedSuggestions()).toBe(1);
  expect(manager.drafts).toEqual([
    {
      id: null,
      name: "Suggested Slicer",
      path: "/Applications/Suggested.app",
      trust: false,
    },
  ]);
  expect(manager.requiresTrust("draft-1")).toBe(true);
  expect(manager.canSave).toBe(false);
  expect(manager.entries[0].needsPathReselect).toBe(true);

  expect(await manager.changePath("draft-1", async () => "/Applications/Suggested.app")).toBe(true);
  expect(manager.entries[0].needsPathReselect).toBe(false);
  expect(manager.canSave).toBe(false);
  manager.setTrust("draft-1", true);
  expect(manager.drafts[0].trust).toBe(true);
  expect(manager.canSave).toBe(true);
});

test("cancelling local re-selection keeps an auto-detected suggestion untrusted", async () => {
  const manager = new SlicerProfileDraftManager([builtIn]);
  await manager.detectSuggestions(async () => [
    { id: null, name: "Suggested Slicer", path: "/Applications/Suggested.app", trust: false },
  ]);
  manager.selectSuggestion(0, true);
  manager.stageSelectedSuggestions();

  expect(await manager.changePath("draft-1", async () => null)).toBe(false);
  expect(manager.entries[0].needsPathReselect).toBe(true);
  manager.setTrust("draft-1", true);
  expect(manager.drafts[0].trust).toBe(false);
  expect(manager.canSave).toBe(false);
});

test("detection failures keep the draft list and surface an accessible error message", async () => {
  const manager = new SlicerProfileDraftManager([builtIn, savedCustom]);
  const detected = await manager.detectSuggestions(async () => {
    throw new Error("scanner unavailable");
  });

  expect(detected).toBe(false);
  expect(manager.drafts).toEqual([
    { id: "custom-1", name: "My Slicer", path: savedCustom.app_path, trust: false },
  ]);
  expect(manager.error).toContain("scanner unavailable");
});

test("profile save failure preserves drafts and trust state for retry", async () => {
  const manager = new SlicerProfileDraftManager([builtIn]);
  await manager.addFromPicker(async () => "/Applications/Custom.app");
  manager.setTrust("draft-1", true);

  const saved = await manager.save(async () => {
    throw new Error("settings file is read-only");
  });

  expect(saved).toBe(false);
  expect(manager.drafts).toEqual([
    { id: null, name: "Custom", path: "/Applications/Custom.app", trust: true },
  ]);
  expect(manager.dirty).toBe(true);
  expect(manager.canSave).toBe(true);
  expect(manager.error).toContain("settings file is read-only");
});

test("saving a custom executable requires explicit trust confirmation", async () => {
  const manager = new SlicerProfileDraftManager([builtIn]);
  await manager.addFromPicker(async () => "/Applications/Custom.app");
  let called = false;

  expect(manager.canSave).toBe(false);
  expect(await manager.save(async () => (called = true))).toBe(false);
  expect(called).toBe(false);
  manager.setTrust("draft-1", true);
  expect(manager.canSave).toBe(true);
});

test("unavailable saved profiles require local reselection and explicit trust", async () => {
  const unavailable = { ...savedCustom, available: false };
  const manager = new SlicerProfileDraftManager([builtIn, unavailable]);
  manager.setName("saved:custom-1", "Renamed while unavailable");
  const sent = [];

  expect(manager.drafts[0].trust).toBe(false);
  expect(manager.canSave).toBe(false);
  expect(await manager.save(async (profiles) => sent.push(profiles))).toBe(false);
  expect(sent).toEqual([]);

  await manager.changePath("saved:custom-1", async () => "/Applications/Recovered.app");
  expect(manager.canSave).toBe(false);
  manager.setTrust("saved:custom-1", true);
  expect(manager.drafts[0].trust).toBe(true);
  expect(await manager.save(async (profiles) => sent.push(profiles))).toBe(true);
  expect(sent[0][0].trust).toBe(true);
});

test("unchanged available custom profiles submit trust false when their name is edited", async () => {
  const manager = new SlicerProfileDraftManager([builtIn, savedCustom]);
  manager.setName("saved:custom-1", "Renamed Slicer");
  let saved;

  expect(await manager.save(async (profiles) => (saved = profiles))).toBe(true);
  expect(saved).toEqual([
    {
      id: "custom-1",
      name: "Renamed Slicer",
      path: savedCustom.app_path,
      trust: false,
    },
  ]);
});

test("default can be cleared and persistence failures restore the prior selection", async () => {
  const manager = new SlicerProfileDraftManager([builtIn, savedCustom]);
  manager.syncDefaultFromModel(savedCustom.id);
  const requested = [];

  const cleared = await manager.setDefault(null, async (id) => requested.push(id));
  expect(cleared).toBe(true);
  expect(requested).toEqual([null]);
  expect(manager.defaultId).toBe(null);
  expect(manager.status).toBe("Default slicer cleared.");

  const changed = await manager.setDefault(builtIn.id, async () => {
    throw new Error("could not save default");
  });
  expect(changed).toBe(false);
  expect(manager.defaultId).toBe(null);
  expect(manager.error).toContain("could not save default");
});

test("unsaved profile edits survive settings rerenders and saved drafts reconcile by path/name", async () => {
  const manager = new SlicerProfileDraftManager([builtIn, savedCustom]);
  manager.setName("saved:custom-1", "Unsaved rename");
  manager.sync([builtIn, { ...savedCustom, display_name: "My Slicer updated by parent" }]);
  expect(manager.drafts[0].name).toBe("Unsaved rename");

  manager.setName("saved:custom-1", "Saved name");
  expect(await manager.save(async () => {})).toBe(true);
  manager.sync([
    builtIn,
    { ...savedCustom, display_name: "Saved name", app_path: savedCustom.app_path },
  ]);
  expect(manager.dirty).toBe(false);
  expect(manager.drafts).toEqual([
    { id: "custom-1", name: "Saved name", path: savedCustom.app_path, trust: false },
  ]);
});
