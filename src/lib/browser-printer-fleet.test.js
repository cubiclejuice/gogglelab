import { expect, test } from "bun:test";
import {
  fleetRefreshNotice,
  filterFleetHistory,
  printerHistoryOptions,
  printerStatusSummary,
  fleetEditedTimestamp,
  reminderCanComplete,
} from "./browser-printer-fleet";

test("history printer choices retain removed printer snapshots without relinking", () => {
  const choices = printerHistoryOptions([{ id: "new", nickname: "Workshop" }], {
    printerId: "old",
    printerNicknameSnapshot: "Workshop",
  });
  expect(choices).toEqual([
    { value: "", label: "No printer" },
    { value: "new", label: "Workshop" },
    { value: "old", label: "Workshop (saved history; printer removed)" },
  ]);
  expect(
    printerHistoryOptions([], { printerId: null, printerNicknameSnapshot: "Old printer" })[0].label,
  ).toBe("Keep saved printer: Old printer");
});

test("fleet partial refresh identifies unavailable collections without erasing successful data", () => {
  expect(fleetRefreshNotice(["maintenance logs"])).toContain("maintenance logs");
  expect(fleetRefreshNotice([])).toBe("");
  expect(filterFleetHistory([{ printerId: null }, { printerId: "a" }], "unlinked")).toEqual([
    { printerId: null },
  ]);
  expect(filterFleetHistory([{ printerId: null }, { printerId: "a" }], "a")).toEqual([
    { printerId: "a" },
  ]);
});

test("printer status labels distinguish stored snapshots and absent checks", () => {
  expect(
    printerStatusSummary({
      connectionStatus: "online",
      jobStatus: "printing",
      lastCheckedAt: "2026-10-19T12:00:00Z",
    }),
  ).toContain("Stored status: online · printing");
  expect(
    printerStatusSummary({
      connectionStatus: "unknown",
      jobStatus: "unknown",
      lastCheckedAt: null,
    }),
  ).toContain("No connection check recorded");
});

test("unchanged date fields preserve source precision and DST-fold identity", () => {
  const original = "2026-10-19T12:34:56.789Z";
  expect(fleetEditedTimestamp(original, "2026-10-19T07:34", "2026-10-19T07:34")).toBe(original);
  const folded = "2026-11-01T07:30:56.789Z";
  expect(fleetEditedTimestamp(folded, "2026-11-01T01:30", "2026-11-01T01:30")).toBe(folded);
  const changed = "2026-10-20T08:45";
  expect(fleetEditedTimestamp(original, "2026-10-19T07:34", changed)).toBe(
    new Date(changed).toISOString(),
  );
});

test("only active reminders offer completion; paused reminders need resume", () => {
  expect(reminderCanComplete("active")).toBe(true);
  expect(reminderCanComplete("paused")).toBe(false);
  expect(reminderCanComplete("completed")).toBe(false);
});
