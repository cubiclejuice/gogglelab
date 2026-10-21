import { describe, expect, test } from "bun:test";
import {
  buildPrinterDataImportPayload,
  createPrinterMappingOptions,
  describeSlicerImportReview,
  describeUnlinkedPrinterHistory,
  describeImportedFields,
  paginatePrinterImportProfiles,
  readRawColorwayValue,
} from "./recovery";
import {
  applyPrintventoryAssignments,
  applyPrintventoryPartAssignments,
  parsePrintventoryExport,
} from "./printventory-import";
import { SlicerImportReviewState } from "./slicer-import";

describe("readRawColorwayValue", () => {
  test("returns the exact stored string without parsing or normalizing it", () => {
    const value = " {not JSON}\ncolorway record  ";
    const storage = { getItem: (key) => (key === "gogglelab.colorways.v1" ? value : null) };

    expect(readRawColorwayValue(storage)).toBe(value);
  });

  test("returns null when the key is absent or storage is unavailable", () => {
    expect(readRawColorwayValue({ getItem: () => null })).toBeNull();
    expect(
      readRawColorwayValue({
        getItem: () => {
          throw new Error("storage blocked");
        },
      }),
    ).toBeNull();
  });
});

describe("Printventory slicer review notice", () => {
  test("explains explicit local reselection and skip choices for nonempty slicers", () => {
    expect(
      describeSlicerImportReview({ models: [], slicers: [{ name: "Slicer", path: "/hint" }] }),
    ).toContain("local app selection and trust");
    expect(describeSlicerImportReview({ models: [], slicers: [] })).toBe("");
    expect(describeSlicerImportReview({ models: [] })).toBe("");
  });
});

describe("Printventory import preview", () => {
  test("shows the supported status, rating, favorite, categorized tags, and history fields", () => {
    const preview = describeImportedFields({
      print_status: "queued",
      rating: 4,
      favorite: true,
      gogglelabTags: [{ name: "Frame", category: "Assembly" }],
      printEvents: [{ outcome: "printed" }],
    });

    expect(preview).toContain("Print status: queued");
    expect(preview).toContain("Rating: 4");
    expect(preview).toContain("Favorite: Yes");
    expect(preview).toContain("Tags and categories:");
    expect(preview).toContain("Print history: 1 event");
    expect(preview).not.toContain("Other model fields not imported");
  });
});

describe("Printventory printer import review", () => {
  test("describes retained unlinked history counts without calling them skipped", () => {
    expect(describeUnlinkedPrinterHistory(1, 2)).toBe(
      "1 maintenance log and 2 reminders will be retained without a linked printer.",
    );
    expect(describeUnlinkedPrinterHistory(0, 0)).toBe("");
  });

  const document = {
    models: [],
    printers: [
      {
        externalRef: "foreign-printer-1",
        input: {
          nickname: "Imported P1",
          make: "Maker",
          model: "M1",
          printerType: "FDM",
          firmware: null,
          host: "192.168.50.10",
          webUrl: "http://192.168.50.10",
          bedMm: [220, 220, 250],
          notes: "",
        },
        endpointReviewRequired: true,
        createdAt: "2026-10-01T00:00:00.000Z",
        updatedAt: "2026-10-02T00:00:00.000Z",
      },
    ],
    maintenanceLogs: [
      {
        externalRef: "foreign-log-1",
        printerExternalRef: "foreign-printer-1",
        printerNickname: "P1 before rename",
        input: {
          title: "Replace nozzle",
          performedAt: "2026-10-12T00:00:00.000Z",
          notes: "",
        },
      },
    ],
    reminders: [
      {
        externalRef: "foreign-reminder-1",
        printerExternalRef: "foreign-printer-1",
        printerNickname: "P1 before rename",
        input: {
          title: "Clean plate",
          notes: "",
          dueAt: "2026-10-20T00:00:00.000Z",
          intervalDays: 30,
        },
        status: "paused",
        createdAt: "2026-10-01T00:00:00.000Z",
        updatedAt: "2026-10-02T00:00:00.000Z",
      },
    ],
  };

  test("uses only the user's local mapping choice and preserves imported snapshots", () => {
    const result = buildPrinterDataImportPayload(
      document,
      new Map([["foreign-printer-1", "local:printer-local-1"]]),
      "2026-10-14T00:00:00.000Z",
    );

    expect(result.pendingExternalRefs).toEqual([]);
    expect(result.input.printers[0]).toMatchObject({
      externalRef: "foreign-printer-1",
      mapToLocalPrinterId: "printer-local-1",
    });
    expect(result.input.printers[0].input.host).toBe("192.168.50.10");
    expect("endpointApproved" in result.input.printers[0]).toBe(false);
    expect(result.input.printers[0]).not.toHaveProperty("createdAt");
    expect(result.input.printers[0]).not.toHaveProperty("updatedAt");
    expect(result.input.maintenanceLogs[0]).toMatchObject({
      externalRef: "foreign-log-1",
      printerExternalRef: "foreign-printer-1",
      printerNickname: "P1 before rename",
    });
    expect(result.input.reminders[0]).toMatchObject({
      externalRef: "foreign-reminder-1",
      printerExternalRef: "foreign-printer-1",
      printerNickname: "P1 before rename",
      status: "paused",
      createdAt: "2026-10-01T00:00:00.000Z",
      updatedAt: "2026-10-02T00:00:00.000Z",
    });
  });

  test("reviews printer and history IDs emitted by GoggleLab's own JSON export", () => {
    const exported = parsePrintventoryExport(
      JSON.stringify({
        models: [],
        printers: [
          {
            gogglelab_printer_id: "printer-external-1",
            nickname: "Workshop",
            manufacturer: "Maker",
            model: "M1",
            host: "user:secret@192.168.1.25",
            web_url: "http://user:secret@192.168.1.25/",
            source_kind: "managed",
            endpoint_approved: true,
            connection_status: "online",
            job_status: "printing",
            created_at: "2026-10-09T19:00:00-05:00",
            updated_at: "2026-10-14T12:30:45.123Z",
          },
        ],
        printer_maintenance_logs: [
          {
            gogglelab_maintenance_log_id: "maintenance-external-1",
            gogglelab_printer_id: "printer-external-1",
            printer_nickname: "Workshop before rename",
            title: "Clean nozzle",
            performed_at: "2026-10-14T00:00:00.000Z",
            notes: "Synthetic maintenance",
            created_at: "2026-10-14T00:00:00.000Z",
          },
        ],
        printer_maintenance_reminders: [
          {
            gogglelab_printer_reminder_id: "reminder-external-1",
            gogglelab_printer_id: "printer-external-1",
            printer_nickname: "Workshop before rename",
            title: "Inspect belts",
            notes: "Synthetic reminder",
            due_at: "2026-11-14T00:00:00.000Z",
            interval_days: 30,
            status: "active",
            created_at: "2026-10-14T00:00:00.000Z",
            updated_at: "2026-10-14T00:00:00.000Z",
          },
        ],
      }),
    );
    const reviewed = buildPrinterDataImportPayload(
      exported,
      new Map([["printer-external-1", "create"]]),
      "2026-10-14T00:00:00.000Z",
    );

    expect(reviewed.pendingExternalRefs).toEqual([]);
    expect(reviewed.input.printers[0]).toMatchObject({
      externalRef: "printer-external-1",
      createdAt: "2026-10-10T00:00:00Z",
      updatedAt: "2026-10-14T12:30:45.123Z",
      input: { host: null, webUrl: null },
    });
    expect(reviewed.input.printers[0]).not.toHaveProperty("sourceKind");
    expect(reviewed.input.printers[0]).not.toHaveProperty("endpointApproved");
    expect(reviewed.input.printers[0]).not.toHaveProperty("connectionStatus");
    expect(reviewed.input.printers[0]).not.toHaveProperty("jobStatus");
    expect(reviewed.input.maintenanceLogs[0]).toMatchObject({
      externalRef: "maintenance-external-1",
      printerExternalRef: "printer-external-1",
      printerNickname: "Workshop before rename",
    });
    expect(reviewed.input.reminders[0]).toMatchObject({
      externalRef: "reminder-external-1",
      printerExternalRef: "printer-external-1",
      printerNickname: "Workshop before rename",
    });
  });

  test("routes exported printer, parts, slicer, maintenance, reminder, and print history through review and apply", async () => {
    const fixture = await Bun.file(
      new URL("../../tests/fixtures/printventory-printer-history.json", import.meta.url),
    ).text();
    const document = parsePrintventoryExport(fixture);
    const slicerReview = new SlicerImportReviewState(document.slicers);
    expect(slicerReview.ready).toBe(false);
    slicerReview.choose(0, "create");
    slicerReview.selectPath(0, "/synthetic/local/FixtureSlicer");
    slicerReview.setTrust(0, true);
    expect(slicerReview.drafts).toEqual([
      { id: null, name: "Round-trip slicer", path: "/synthetic/local/FixtureSlicer", trust: true },
    ]);
    const reviewed = buildPrinterDataImportPayload(
      document,
      new Map([["printer-external-1", "create"]]),
      "2026-10-14T00:00:00.000Z",
    );
    const importedPartInputs = [];
    const importedParts = await applyPrintventoryPartAssignments(
      document.parts,
      document.parts.map((part) => ({ externalRef: part.externalRef, target: "create" })),
      [],
      {
        createPartStock: async (input) => {
          importedPartInputs.push(input);
          return { id: "local-part-1", ...input, metadataVersion: 1 };
        },
        updatePartStock: async () => {
          throw new Error("unexpected part update");
        },
      },
    );
    const model = {
      id: "target-model",
      display_name: "Gear",
      original_name: "gear.stl",
      source_path: "/library/gear.stl",
      source_kind: "indexed",
      metadata_version: 1,
      tags: [],
      filament_ids: [],
    };
    const importedAttempts = [];
    const printerIdByExternalRef = new Map(
      reviewed.input.printers.map(({ externalRef }) => [externalRef, "local-printer-1"]),
    );
    let normalCreateCalled = false;
    const result = await applyPrintventoryAssignments(
      document.models,
      [{ entryIndex: 0, targetModelId: model.id }],
      [model],
      [],
      [],
      {
        createTag: async () => {
          throw new Error("unexpected tag creation");
        },
        updateTag: async () => {
          throw new Error("unexpected tag update");
        },
        updateMetadata: async () => {
          throw new Error("unexpected metadata update");
        },
        setModelFilaments: async () => {
          throw new Error("unexpected filament assignment");
        },
        listPrintAttempts: async () => [],
        createPrintAttempt: async () => {
          normalCreateCalled = true;
          throw new Error("historical usage must not deduct stock again");
        },
        importPrintAttempt: async () => {
          throw new Error("single event should use the atomic batch import");
        },
        importPrintAttempts: async (_modelId, attempts) => {
          importedAttempts.push(...attempts);
          return {
            addedCount: attempts.length,
            skippedCount: 0,
            metadataVersion: model.metadata_version + 1,
          };
        },
      },
      printerIdByExternalRef,
      document.parts,
      importedParts.partIdByExternalRef,
    );

    expect(reviewed.input.printers).toHaveLength(1);
    expect(reviewed.input.maintenanceLogs[0].externalRef).toBe("maintenance-external-1");
    expect(reviewed.input.reminders[0].externalRef).toBe("reminder-external-1");
    expect(importedPartInputs).toEqual([
      {
        name: "M3 heat-set insert",
        category: "hardware",
        quantity: 10,
        unit: "pcs",
        notes: "brass",
        lowStock: 2,
      },
    ]);
    expect(importedParts.partIdByExternalRef.get("part-external-1")).toBe("local-part-1");
    expect(result.rows[0].mappedFields).toContain("Print history");
    expect(importedAttempts).toHaveLength(1);
    expect(importedAttempts[0].createdAt).toBe("2026-10-14T13:00:00Z");
    expect(importedAttempts[0].printerId).toBe("local-printer-1");
    expect(importedAttempts[0].printerName).toBe("Round-trip printer");
    expect(importedAttempts[0].input.parts).toEqual([{ id: "local-part-1", quantity: 2 }]);
    expect(normalCreateCalled).toBe(false);
  });

  test("keeps history unlinked when a printer profile is skipped", () => {
    const created = buildPrinterDataImportPayload(
      document,
      new Map([["foreign-printer-1", "create"]]),
      "2026-10-14T00:00:00.000Z",
    );
    expect(created.input.printers[0].externalRef).toBe("foreign-printer-1");
    expect(created.input.printers[0].mapToLocalPrinterId).toBeUndefined();

    const skipped = buildPrinterDataImportPayload(
      document,
      new Map([["foreign-printer-1", "skip"]]),
      "2026-10-14T00:00:00.000Z",
    );
    expect(skipped.input.printers).toEqual([]);
    expect(skipped.input.maintenanceLogs).toEqual([
      {
        externalRef: "foreign-log-1",
        printerExternalRef: "foreign-printer-1",
        printerNickname: "P1 before rename",
        input: {
          title: "Replace nozzle",
          performedAt: "2026-10-12T00:00:00.000Z",
          notes: "",
        },
      },
    ]);
    expect(skipped.input.reminders).toEqual([
      {
        externalRef: "foreign-reminder-1",
        printerExternalRef: "foreign-printer-1",
        printerNickname: "P1 before rename",
        input: {
          title: "Clean plate",
          notes: "",
          dueAt: "2026-10-20T00:00:00.000Z",
          intervalDays: 30,
        },
        status: "paused",
        createdAt: "2026-10-01T00:00:00.000Z",
        updatedAt: "2026-10-02T00:00:00.000Z",
      },
    ]);
    expect(skipped.unlinkedMaintenanceLogs).toBe(1);
    expect(skipped.unlinkedReminders).toBe(1);
  });

  test("keeps history with null printer references and preserves its snapshots", () => {
    const result = buildPrinterDataImportPayload(
      {
        models: [],
        printers: [],
        maintenanceLogs: [
          {
            externalRef: "orphan-log",
            printerExternalRef: null,
            printerNickname: "Retired P1",
            input: {
              title: "Replace nozzle",
              performedAt: "2026-10-12T00:00:00.000Z",
              notes: "Preserve the service note",
            },
            createdAt: "2026-10-11T00:00:00.000Z",
          },
        ],
        reminders: [
          {
            externalRef: "orphan-reminder",
            printerExternalRef: null,
            printerNickname: "Retired P1",
            input: {
              title: "Inspect belts",
              notes: "Preserve the reminder note",
              dueAt: "2026-11-14T00:00:00.000Z",
              intervalDays: 30,
            },
            status: "completed",
            lastCompletedAt: "2026-10-10T00:00:00.000Z",
            createdAt: "2026-10-01T00:00:00.000Z",
            updatedAt: "2026-10-10T00:00:00.000Z",
          },
        ],
      },
      new Map(),
      "2026-10-16T00:00:00.000Z",
    );

    expect(result.input.maintenanceLogs).toEqual([
      {
        externalRef: "orphan-log",
        printerExternalRef: null,
        printerNickname: "Retired P1",
        input: {
          title: "Replace nozzle",
          performedAt: "2026-10-12T00:00:00.000Z",
          notes: "Preserve the service note",
        },
        createdAt: "2026-10-11T00:00:00.000Z",
      },
    ]);
    expect(result.input.reminders).toEqual([
      {
        externalRef: "orphan-reminder",
        printerExternalRef: null,
        printerNickname: "Retired P1",
        input: {
          title: "Inspect belts",
          notes: "Preserve the reminder note",
          dueAt: "2026-11-14T00:00:00.000Z",
          intervalDays: 30,
        },
        status: "completed",
        lastCompletedAt: "2026-10-10T00:00:00.000Z",
        createdAt: "2026-10-01T00:00:00.000Z",
        updatedAt: "2026-10-10T00:00:00.000Z",
      },
    ]);
    expect(result.unlinkedMaintenanceLogs).toBe(1);
    expect(result.unlinkedReminders).toBe(1);
  });

  test("keeps printer import disabled until every external profile has a reviewed choice", () => {
    const result = buildPrinterDataImportPayload(document, new Map(), "2026-10-14T00:00:00.000Z");
    expect(result.pendingExternalRefs).toEqual(["foreign-printer-1"]);
    expect(result.input.printers).toEqual([]);
  });

  test("paginates 5,000 imported profiles and keeps local mapping choices bounded", () => {
    const importedProfiles = Array.from({ length: 5_000 }, (_, index) => ({
      externalRef: `external-${index}`,
    }));
    const page = paginatePrinterImportProfiles(importedProfiles, 99);
    const localPrinters = Array.from({ length: 5_000 }, (_, index) => ({
      id: `local-${index}`,
      nickname: `Workshop ${index}`,
      make: index === 4_999 ? "Special Maker" : null,
      model: index === 4_999 ? "P-4999" : null,
    }));
    const mappings = createPrinterMappingOptions(localPrinters, "P-4999", "local-10");

    expect(page).toMatchObject({ page: 99, pageCount: 100, start: 4_950, end: 5_000 });
    expect(page.items).toHaveLength(50);
    expect(mappings.matchCount).toBe(1);
    expect(mappings.options).toHaveLength(2);
    expect(mappings.options.map((printer) => printer.id)).toContain("local-4999");
    expect(mappings.options.map((printer) => printer.id)).toContain("local-10");
  });
});
