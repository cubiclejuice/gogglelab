import { describe, expect, test } from "bun:test";
import {
  applyPrintventoryAssignments,
  applyPrintventoryPartAssignments,
  buildPrintventoryFieldMapping,
  buildPrintventoryImportPlan,
  canApplyPrintventoryImport,
  createPrintventoryTargetSearch,
  assertPrintventoryExportBounds,
  MAX_PRINTVENTORY_JSON_BYTES,
  MAX_PRINTVENTORY_MODEL_ENTRIES,
  MAX_PRINTVENTORY_EVENT_PARTS,
  MAX_PRINTVENTORY_PART_ENTRIES,
  missingPrintventoryTagNames,
  parsePrintventoryExport,
  printventoryTargetLabel,
  readPrintventoryExportFile,
  validatePrintventoryPartAssignments,
  validatePrintventoryAssignments,
} from "./printventory-import";

const printventoryFile = (overrides = {}) => ({
  filePath: "/private/other-app/Models/gear.stl",
  fileName: "gear.stl",
  designer: "Ada Maker",
  source: "https://example.test/models/42",
  notes: "Imported note",
  printed: true,
  parentModel: null,
  license: "CC BY 4.0",
  tags: ["mechanical", "gear"],
  filaments: [],
  ...overrides,
});

const libraryModel = (id, fields = {}) => ({
  id,
  display_name: "Gear",
  original_name: "gear.stl",
  source_path: "/private/other-app/Models/gear.stl",
  source_kind: "indexed",
  metadata_version: 4,
  tags: [],
  filament_ids: [],
  ...fields,
});

describe("Printventory JSON compatibility", () => {
  test("imports only bounded slicer names and opaque path hints without trusting JSON fields", () => {
    const document = parsePrintventoryExport(
      JSON.stringify({
        models: [],
        slicers: [
          {
            name: " Custom Slicer ",
            path: "C:\\Apps\\Slicer.exe",
            trust: true,
            identity: { file_id: "forged" },
            args: ["--run"],
          },
        ],
      }),
    );
    expect(document.slicers).toEqual([{ name: "Custom Slicer", path: "C:\\Apps\\Slicer.exe" }]);
    for (const slicers of [
      "malformed",
      [null],
      [{ name: "", path: "/app" }],
      [{ name: "Slicer", path: "" }],
      [{ name: "x".repeat(129), path: "/app" }],
      [{ name: "Slicer", path: "/".repeat(4097) }],
      [{ name: "Slicer", path: "/app\n" }],
      Array.from({ length: 65 }, (_, i) => ({ name: `Slicer ${i}`, path: `/apps/${i}` })),
    ]) {
      expect(() => parsePrintventoryExport(JSON.stringify({ models: [], slicers }))).toThrow(
        /slicer/i,
      );
    }
    expect(parsePrintventoryExport('{"models":[]}').slicers).toBeUndefined();
  });

  test("requires an export object with a models array and preserves model metadata", () => {
    const document = parsePrintventoryExport(
      JSON.stringify({
        version: 1,
        exportDate: "2026-10-13T12:00:00Z",
        printers: [],
        parts: [],
        slicers: [],
        models: [printventoryFile()],
      }),
    );

    expect(document.models).toHaveLength(1);
    expect(document.models[0].filePath).toBe("/private/other-app/Models/gear.stl");
    expect(() => parsePrintventoryExport('{"models":{}}')).toThrow(/models array/i);
    expect(() => parsePrintventoryExport("[]")).toThrow(/export object/i);
  });

  test("allows a reviewed slicer-only document to reach apply without a model assignment", () => {
    expect(
      canApplyPrintventoryImport(true, false, false, [], new Map(), {
        slicers: [{ name: "Slicer", path: "/hint" }],
      }),
    ).toBe(true);
    expect(canApplyPrintventoryImport(true, false, false, [], new Map(), { slicers: [] })).toBe(
      false,
    );
  });

  test("parses bounded top-level stock parts while keeping old exports compatible", () => {
    const document = parsePrintventoryExport(
      JSON.stringify({
        models: [],
        parts: [
          {
            name: "M3 bolt",
            category: "Hardware",
            quantity: 40,
            unit: "pcs",
            notes: "Imported stock",
            low_stock: 8,
            id: "part-42",
          },
          { name: "Washer", quantity: 12, unit: "pcs", lowStock: 3, notes: "" },
        ],
      }),
    );
    expect(document.parts).toEqual([
      {
        externalRef: "part-42",
        sourceId: "part-42",
        input: {
          name: "M3 bolt",
          category: "Hardware",
          quantity: 40,
          unit: "pcs",
          notes: "Imported stock",
          lowStock: 8,
        },
      },
      {
        externalRef: "row:2",
        input: {
          name: "Washer",
          category: null,
          quantity: 12,
          unit: "pcs",
          notes: "",
          lowStock: 3,
        },
      },
    ]);
    expect(parsePrintventoryExport('{"models":[]}').parts).toBeUndefined();
    expect(() =>
      parsePrintventoryExport(
        JSON.stringify({ models: [], parts: Array(MAX_PRINTVENTORY_PART_ENTRIES + 1).fill({}) }),
      ),
    ).toThrow(/parts import limit/i);
    expect(() =>
      parsePrintventoryExport(
        JSON.stringify({ models: [], parts: [{ name: "", quantity: -1, unit: "pcs" }] }),
      ),
    ).toThrow(/part entry/i);
  });

  test("applies only reviewed part stock choices and returns their local IDs for event mapping", async () => {
    const imported = [
      {
        externalRef: "row:1",
        input: {
          name: "M3 bolt",
          category: "Hardware",
          quantity: 40,
          unit: "pcs",
          notes: "",
          lowStock: 8,
        },
      },
    ];
    const created = [];
    const result = await applyPrintventoryPartAssignments(
      imported,
      [{ externalRef: "row:1", target: "create" }],
      [],
      {
        createPartStock: async (input) => {
          created.push(input);
          return { id: "local-bolt", ...input, metadataVersion: 1 };
        },
        updatePartStock: async () => {
          throw new Error("unexpected stock update");
        },
      },
    );

    expect(created).toEqual([imported[0].input]);
    expect(result.partIdByExternalRef.get("row:1")).toBe("local-bolt");
    expect(result.createdCount).toBe(1);
    expect(result.skippedCount).toBe(0);
  });

  test("requires a distinct reviewed local target for each imported part", () => {
    const imported = [
      {
        externalRef: "row:1",
        input: { name: "Bolt", category: null, quantity: 2, unit: "pcs", notes: "", lowStock: 0 },
      },
      {
        externalRef: "row:2",
        input: { name: "Nut", category: null, quantity: 3, unit: "pcs", notes: "", lowStock: 0 },
      },
    ];
    expect(
      validatePrintventoryPartAssignments(imported, [
        { externalRef: "row:1", target: "local:hardware" },
        { externalRef: "row:2", target: "local:hardware" },
      ]).valid,
    ).toBe(false);
    expect(
      validatePrintventoryPartAssignments(imported, [
        { externalRef: "row:1", target: "create" },
        { externalRef: "row:2", target: "skip" },
      ]).valid,
    ).toBe(true);
  });

  test("rejects JSON files and export model lists above explicit bounds", () => {
    expect(() => assertPrintventoryExportBounds(MAX_PRINTVENTORY_JSON_BYTES + 1, 0)).toThrow(
      /25 MiB/i,
    );
    expect(() =>
      parsePrintventoryExport(
        JSON.stringify({ models: Array(MAX_PRINTVENTORY_MODEL_ENTRIES + 1).fill({}) }),
      ),
    ).toThrow(/5,000 model entries/i);
  });

  test("bounds every optional printer and printer-history collection", () => {
    const oversizedCollections = [
      {
        printers: Array.from({ length: 5_001 }, (_, index) => ({
          id: `printer-${index + 1}`,
          nickname: "Printer",
        })),
      },
      {
        printer_maintenance_logs: Array(50_001).fill({
          title: "Service",
          performedAt: "2026-10-10",
        }),
      },
      {
        printer_maintenance_reminders: Array(10_001).fill({
          title: "Clean nozzle",
          dueAt: "2026-10-10",
        }),
      },
    ];

    for (const collection of oversizedCollections) {
      expect(() => parsePrintventoryExport(JSON.stringify({ models: [], ...collection }))).toThrow(
        /limit/i,
      );
    }
  });

  test("accepts printer data beyond the former 1,000-profile cap within the catalog bound", () => {
    const printers = Array.from({ length: 1_001 }, (_, index) => ({
      id: `printer-${index + 1}`,
      nickname: `Printer ${index + 1}`,
    }));
    const document = parsePrintventoryExport(JSON.stringify({ models: [], printers }));

    expect(document.printers).toHaveLength(1_001);
  });

  test("accepts history beyond the former 10,000-row export cap within the bounded round-trip limits", () => {
    const maintenanceLogs = Array.from({ length: 10_001 }, (_, index) => ({
      id: `log-${index + 1}`,
      gogglelab_printer_id: "external-printer-1",
      printer_nickname: "Workshop",
      title: "Service",
      performedAt: "2026-10-10",
    }));
    const reminders = Array.from({ length: 10_000 }, (_, index) => ({
      id: `reminder-${index + 1}`,
      gogglelab_printer_id: "external-printer-1",
      printer_nickname: "Workshop",
      title: "Clean nozzle",
      dueAt: "2026-10-10",
    }));
    const document = parsePrintventoryExport(
      JSON.stringify({
        models: [],
        printer_maintenance_logs: maintenanceLogs,
        printer_maintenance_reminders: reminders,
      }),
    );

    expect(document.maintenanceLogs).toHaveLength(10_001);
    expect(document.reminders).toHaveLength(10_000);
    expect(document.printers).toHaveLength(1);
  });

  test("normalizes only allowlisted printer fields and marks endpoint data for review", () => {
    const document = parsePrintventoryExport(
      JSON.stringify({
        models: [],
        printers: [
          {
            id: "foreign-printer-1",
            nickname: "Workshop",
            manufacturer: "Maker",
            model: "M1",
            printer_type: "FDM",
            firmware_type: "Klipper",
            host: "192.168.1.50",
            web_url: "http://192.168.1.50",
            bed_mm: [220, 220, 250],
            notes: "Imported for review",
            created_at: "2026-10-09T19:00:00-05:00",
            updated_at: "2026-10-14T12:30:45.123Z",
          },
        ],
      }),
    );

    expect(document.printers).toEqual([
      {
        externalRef: "foreign-printer-1",
        input: {
          nickname: "Workshop",
          make: "Maker",
          model: "M1",
          printerType: "FDM",
          firmware: "Klipper",
          host: "192.168.1.50",
          webUrl: "http://192.168.1.50",
          bedMm: [220, 220, 250],
          notes: "Imported for review",
        },
        endpointReviewRequired: true,
        createdAt: "2026-10-10T00:00:00Z",
        updatedAt: "2026-10-14T12:30:45.123Z",
      },
    ]);
  });

  test("parses maintenance identifiers emitted by GoggleLab's own printer export", () => {
    const document = parsePrintventoryExport(
      JSON.stringify({
        models: [],
        printers: [
          {
            gogglelab_printer_id: "printer-local-1",
            nickname: "Workshop",
          },
        ],
        printer_maintenance_logs: [
          {
            gogglelab_maintenance_log_id: "maintenance-1",
            gogglelab_printer_id: "printer-local-1",
            printer_nickname: "Workshop",
            title: "Clean nozzle",
            performed_at: "2026-10-14T00:00:00.000Z",
            notes: "Synthetic maintenance row",
            created_at: "2026-10-14T00:00:00.000Z",
          },
          {
            gogglelab_printer_maintenance_log_id: "maintenance-legacy-1",
            gogglelab_printer_id: "printer-local-1",
            title: "Inspect nozzle",
            performed_at: "2026-10-13T00:00:00.000Z",
          },
        ],
        printer_maintenance_reminders: [
          {
            gogglelab_printer_reminder_id: "reminder-1",
            gogglelab_printer_id: "printer-local-1",
            printer_nickname: "Workshop",
            title: "Inspect belts",
            notes: "Synthetic reminder row",
            due_at: "2026-11-14T00:00:00.000Z",
            interval_days: 30,
            status: "active",
            created_at: "2026-10-14T00:00:00.000Z",
            updated_at: "2026-10-14T00:00:00.000Z",
          },
          {
            gogglelab_printer_maintenance_reminder_id: "reminder-legacy-1",
            gogglelab_printer_id: "printer-local-1",
            title: "Inspect nozzle",
            due_at: "2026-11-13T00:00:00.000Z",
            status: "paused",
          },
        ],
      }),
    );

    expect(document.maintenanceLogs[0]).toMatchObject({
      externalRef: "maintenance-1",
      printerExternalRef: "printer-local-1",
      printerNickname: "Workshop",
      input: { title: "Clean nozzle", notes: "Synthetic maintenance row" },
    });
    expect(document.maintenanceLogs[1].externalRef).toBe("maintenance-legacy-1");
    expect(document.reminders[0]).toMatchObject({
      externalRef: "reminder-1",
      printerExternalRef: "printer-local-1",
      printerNickname: "Workshop",
      input: { title: "Inspect belts", intervalDays: 30 },
    });
    expect(document.reminders[1].externalRef).toBe("reminder-legacy-1");
  });

  test("drops imported printer credentials, endpoint approval, and runtime status", () => {
    const document = parsePrintventoryExport(
      JSON.stringify({
        models: [],
        printers: [
          {
            id: "foreign-printer-2",
            nickname: "Printer",
            bambu_access_code: "secret-code",
            bambu_serial_number: "secret-serial",
            prusa_api_key: "secret-key",
            password: "secret-password",
            endpoint_approved: true,
            endpointApproved: true,
            connection_status: "online",
            job_status: "printing",
          },
        ],
      }),
    );

    expect(document.printers[0]).toEqual({
      externalRef: "foreign-printer-2",
      input: {
        nickname: "Printer",
        make: null,
        model: null,
        printerType: null,
        firmware: null,
        host: null,
        webUrl: null,
        bedMm: null,
        notes: "",
      },
      endpointReviewRequired: false,
    });
    expect(JSON.stringify(document.printers)).not.toMatch(/secret|approved|online|printing/i);
  });

  test("drops credentials embedded in imported host and web URL values", () => {
    const document = parsePrintventoryExport(
      JSON.stringify({
        models: [],
        printers: [
          {
            id: 23,
            nickname: "Printer",
            host: "192.168.1.23:7125?password=host-secret",
            web_url: "https://admin:web-secret@printer.example.test",
          },
        ],
      }),
    );

    expect(document.printers[0].input.host).toBeNull();
    expect(document.printers[0].input.webUrl).toBeNull();
    expect(document.printers[0].endpointReviewRequired).toBe(false);
    expect(JSON.stringify(document.printers)).not.toMatch(/host-secret|web-secret|operator|admin/i);
  });

  test("does not require endpoint review when every imported endpoint is dropped", () => {
    const document = parsePrintventoryExport(
      JSON.stringify({
        models: [],
        printers: [
          {
            id: "foreign-printer-dropped-endpoints",
            nickname: "Printer",
            host: "printer.example.test?password=secret",
            web_url: "ftp://printer.example.test",
          },
        ],
      }),
    );

    expect(document.printers[0].input.host).toBeNull();
    expect(document.printers[0].input.webUrl).toBeNull();
    expect(document.printers[0].endpointReviewRequired).toBe(false);
  });

  test("drops printer web URL paths that may contain embedded credentials", () => {
    const document = parsePrintventoryExport(
      JSON.stringify({
        models: [],
        printers: [
          {
            id: 24,
            nickname: "Printer",
            web_url: "https://printer.example.test/proxy/access-token-123",
          },
        ],
      }),
    );

    expect(document.printers[0].input.webUrl).toBe("https://printer.example.test");
    expect(document.printers[0].endpointReviewRequired).toBe(true);
    expect(JSON.stringify(document.printers)).not.toContain("access-token-123");
  });

  test("keeps printer IDs as external references in printer, maintenance, and reminder records", () => {
    const document = parsePrintventoryExport(
      JSON.stringify({
        models: [],
        printers: [{ id: "foreign-id", gogglelab_printer_id: "gogglelab-id", nickname: "Printer" }],
        printer_maintenance_logs: [
          {
            id: "log-id",
            gogglelab_printer_id: "gogglelab-id",
            printer_id: "foreign-id",
            printerNickname: " Printer snapshot ",
            title: "Nozzle replacement",
            performedAt: "2026-10-10T12:00:00Z",
            notes: "Service note",
          },
        ],
        printer_maintenance_reminders: [
          {
            id: "reminder-id",
            printer_id: "foreign-id",
            printerNickname: " Printer snapshot ",
            title: "Clean nozzle",
            dueAt: "2026-10-11",
            intervalDays: 90,
            status: "active",
            lastCompletedAt: null,
          },
        ],
      }),
    );

    expect(document.printers[0].externalRef).toBe("gogglelab-id");
    expect(document.printers[0]).not.toHaveProperty("id");
    expect(document.maintenanceLogs[0]).toMatchObject({
      externalRef: "log-id",
      printerExternalRef: "gogglelab-id",
      input: { title: "Nozzle replacement", performedAt: "2026-10-10T12:00:00Z" },
    });
    expect(document.reminders[0]).toMatchObject({
      externalRef: "reminder-id",
      printerExternalRef: "foreign-id",
      printerNickname: "Printer snapshot",
      input: { title: "Clean nozzle", dueAt: "2026-10-11T00:00:00.000Z", intervalDays: 90 },
    });
    expect(document.maintenanceLogs[0]).not.toHaveProperty("printerId");
    expect(document.reminders[0]).not.toHaveProperty("printerId");
    expect(document).not.toHaveProperty("printer_maintenance_logs");
    expect(document).not.toHaveProperty("printer_maintenance_reminders");
  });

  test("normalizes safe numeric Printventory IDs to string external references", () => {
    const document = parsePrintventoryExport(
      JSON.stringify({
        models: [],
        printers: [{ id: 23, nickname: "Printer" }],
        printer_maintenance_logs: [
          {
            id: 42,
            printer_id: 23,
            title: "Service",
            performedAt: "2026-10-10",
          },
        ],
        printer_maintenance_reminders: [
          {
            id: 55,
            printer_id: 23,
            title: "Clean nozzle",
            dueAt: "2026-10-11",
          },
        ],
      }),
    );

    expect(document.printers[0].externalRef).toBe("23");
    expect(document.maintenanceLogs[0].externalRef).toBe("42");
    expect(document.maintenanceLogs[0].printerExternalRef).toBe("23");
    expect(document.reminders[0].externalRef).toBe("55");
    expect(document.reminders[0].printerExternalRef).toBe("23");
  });

  test("rejects duplicate root printer external references", () => {
    expect(() =>
      parsePrintventoryExport(
        JSON.stringify({
          models: [],
          printers: [
            { id: "first-id", gogglelab_printer_id: "same-ref", nickname: "Printer A" },
            { id: "same-ref", nickname: "Printer B" },
          ],
        }),
      ),
    ).toThrow(/duplicate printer external reference/i);
  });

  test("synthesizes one endpoint-free printer candidate from missing maintenance snapshots", () => {
    const document = parsePrintventoryExport(
      JSON.stringify({
        models: [],
        printer_maintenance_logs: [
          {
            id: "log-id",
            printer_id: "missing-printer",
            printerNickname: "Snapshot printer",
            title: "Replace nozzle",
            performedAt: "2026-10-10",
          },
        ],
        printer_maintenance_reminders: [
          {
            id: "reminder-id",
            printer_id: "missing-printer",
            printerNickname: "Snapshot printer",
            title: "Clean nozzle",
            dueAt: "2026-10-11",
          },
        ],
      }),
    );

    expect(document.printers).toEqual([
      {
        externalRef: "missing-printer",
        input: {
          nickname: "Snapshot printer",
          make: null,
          model: null,
          printerType: null,
          firmware: null,
          host: null,
          webUrl: null,
          bedMm: null,
          notes: "",
        },
        endpointReviewRequired: false,
      },
    ]);
  });

  test("bounds printer candidates synthesized from orphaned history references", () => {
    const printer_maintenance_logs = Array.from({ length: 5_001 }, (_, index) => ({
      id: `log-${index + 1}`,
      printer_id: `external-printer-${index + 1}`,
      printerNickname: `Printer ${index + 1}`,
      title: "Service",
      performedAt: "2026-10-10",
    }));

    expect(() =>
      parsePrintventoryExport(JSON.stringify({ models: [], printer_maintenance_logs })),
    ).toThrow(/printer profile limit/i);
  });

  test("rejects malformed printer IDs, strings, bed dimensions, dates, intervals, and statuses", () => {
    const invalidDocuments = [
      { printers: [{ id: "bad\u0000id", nickname: "Printer" }] },
      { printers: [{ id: 1.5, nickname: "Printer" }] },
      { printers: [{ id: Number.MAX_SAFE_INTEGER + 1, nickname: "Printer" }] },
      { printers: [{ id: "p-1", nickname: "x".repeat(1_000) }] },
      { printers: [{ id: "p-1", bed_mm: [220, 0, 250] }] },
      { printers: [{ id: "p-1", nickname: "Printer", created_at: "2026-02-30T00:00:00Z" }] },
      { printers: [{ id: "p-1", nickname: "Printer", created_at: "2026-10-14" }] },
      {
        printers: [
          {
            id: "p-1",
            nickname: "Printer",
            updated_at: `2026-10-14T00:00:00.${"1".repeat(80)}Z`,
          },
        ],
      },
      { printer_maintenance_logs: [{ title: "Service", performedAt: "2026-02-30T00:00:00Z" }] },
      {
        printer_maintenance_reminders: [{ title: "Service", dueAt: "2026-10-10", intervalDays: 0 }],
      },
      {
        printer_maintenance_reminders: [
          { title: "Service", dueAt: "2026-10-10", status: "running" },
        ],
      },
    ];

    for (const collection of invalidDocuments) {
      expect(() =>
        parsePrintventoryExport(JSON.stringify({ models: [], ...collection })),
      ).toThrow();
    }
  });

  test("rejects an oversized selected file before calling its text reader", async () => {
    let textRead = false;
    const file = {
      size: MAX_PRINTVENTORY_JSON_BYTES + 1,
      text: async () => {
        textRead = true;
        return JSON.stringify({ models: [] });
      },
    };

    await expect(readPrintventoryExportFile(file)).rejects.toThrow(/25 MiB/i);
    expect(textRead).toBe(false);
  });

  test("bounds searchable target options and includes the current selection", () => {
    const inventory = Array.from({ length: 35 }, (_, index) =>
      libraryModel(`model-${index}`, {
        display_name: `Part ${String(index).padStart(2, "0")}`,
        original_name: `part-${index}.stl`,
        source_path: `/private/models/part-${index}.stl`,
      }),
    );
    inventory.push(libraryModel("managed", { source_kind: "managed" }));

    const search = createPrintventoryTargetSearch(inventory);
    const options = search("", "model-34");

    expect(options.length).toBeLessThanOrEqual(20);
    expect(options.some((model) => model.id === "model-34")).toBe(true);
    expect(options.every((model) => ["indexed", "managed"].includes(model.source_kind))).toBe(true);
    const filtered = search("part-3.stl");
    expect(filtered.length).toBeGreaterThan(0);
    expect(filtered.every((model) => model.original_name.includes("part-3"))).toBe(true);
  });

  test("selects later duplicate-name models by stable ID without exposing paths", () => {
    const inventory = Array.from({ length: 25 }, (_, index) =>
      libraryModel(`indexed-model-${index}`, {
        display_name: "Identical model",
        original_name: "identical.stl",
        source_path: `/private/models/${index}/identical.stl`,
      }),
    );
    const search = createPrintventoryTargetSearch(inventory);
    const laterModelId = "indexed-model-24";

    const options = search(laterModelId);

    expect(options).toHaveLength(1);
    expect(options[0].id).toBe(laterModelId);
    expect(options.length).toBeLessThanOrEqual(20);
    expect(printventoryTargetLabel(options[0])).toContain(laterModelId);
    expect(printventoryTargetLabel(options[0])).not.toContain(options[0].source_path);
  });

  test("uses filePath only as exact in-memory match hint and suggests a unique filename", () => {
    const document = {
      models: [
        printventoryFile({ fileName: "gear.stl" }),
        printventoryFile({ filePath: "/must/not/be/opened/other.stl", fileName: "unique.stl" }),
      ],
    };
    const inventory = [
      libraryModel("exact", { source_path: "/private/other-app/Models/gear.stl" }),
      libraryModel("unique", {
        original_name: "unique.stl",
        display_name: "Unique",
        source_path: null,
      }),
    ];

    const plan = buildPrintventoryImportPlan(document, inventory);

    expect(plan.map((row) => row.suggestedTargetId)).toEqual(["exact", "unique"]);
    expect(plan.map((row) => row.matchKind)).toEqual(["source-path", "unique-filename"]);
    expect(plan[1].suggestionMessage).toMatch(/filename/i);
  });

  test("leaves ambiguous and unmatched rows unassigned for explicit review", () => {
    const plan = buildPrintventoryImportPlan(
      {
        models: [
          printventoryFile({ filePath: "/other/ambiguous.stl", fileName: "same.stl" }),
          printventoryFile({ filePath: "/other/missing.stl", fileName: "missing.stl" }),
        ],
      },
      [
        libraryModel("same-a", { original_name: "same.stl" }),
        libraryModel("same-b", { original_name: "same.stl" }),
      ],
    );

    expect(plan.map((row) => row.suggestedTargetId)).toEqual([null, null]);
    expect(plan[0].suggestionMessage).toMatch(/ambiguous/i);
    expect(plan[1].suggestionMessage).toMatch(/no match/i);
  });

  test("allows reviewed metadata imports to target existing managed models", () => {
    const managed = libraryModel("managed", { source_kind: "managed" });
    const plan = buildPrintventoryImportPlan({ models: [printventoryFile()] }, [managed]);

    expect(plan[0].suggestedTargetId).toBe("managed");
    expect(plan[0].matchKind).toBe("source-path");
    expect(createPrintventoryTargetSearch([managed])("managed")).toEqual([managed]);
  });

  test("rejects duplicate target assignments before any metadata write", () => {
    const result = validatePrintventoryAssignments([
      { entryIndex: 0, targetModelId: "model-1" },
      { entryIndex: 1, targetModelId: "model-1" },
      { entryIndex: 2, targetModelId: null },
    ]);

    expect(result.valid).toBe(false);
    expect(result.duplicateTargetIds).toEqual(["model-1"]);
  });

  test("does not allow a second asynchronous import while the first is applying", () => {
    const assignments = [{ entryIndex: 0, targetModelId: "target" }];
    const reviewedTargets = new Map([[0, "target"]]);

    expect(canApplyPrintventoryImport(true, false, false, assignments)).toBe(false);
    expect(canApplyPrintventoryImport(true, false, false, assignments, reviewedTargets)).toBe(true);
    expect(
      canApplyPrintventoryImport(true, false, false, assignments, new Map([[0, "stale"]])),
    ).toBe(false);
    expect(canApplyPrintventoryImport(true, false, true, assignments, reviewedTargets)).toBe(false);
  });

  test("allows data-only printer imports when any reviewed printer collection is nonempty", () => {
    const emptyAssignments = [];
    const reviewedTargets = new Map();
    const emptyPrinterData = { printers: [], maintenanceLogs: [], reminders: [] };

    expect(
      canApplyPrintventoryImport(
        true,
        false,
        false,
        emptyAssignments,
        reviewedTargets,
        emptyPrinterData,
      ),
    ).toBe(false);
    for (const field of ["printers", "maintenanceLogs", "reminders"]) {
      expect(
        canApplyPrintventoryImport(true, false, false, emptyAssignments, reviewedTargets, {
          ...emptyPrinterData,
          [field]: [{}],
        }),
      ).toBe(true);
    }
    expect(
      canApplyPrintventoryImport(false, false, false, emptyAssignments, reviewedTargets, {
        ...emptyPrinterData,
        printers: [{}],
      }),
    ).toBe(false);
  });

  test("maps only safe local metadata, named tags, exact parent names, and unique local filaments", () => {
    const parent = libraryModel("parent", { display_name: "Assembly" });
    const target = libraryModel("target", { display_name: "Gear" });
    const mapping = buildPrintventoryFieldMapping(
      printventoryFile({
        parentModel: "Assembly",
        tags: ["mechanical", { name: "gear" }],
        filaments: [{ brand: "Local", material: "PLA", color: "Black", name: "PLA Black" }],
      }),
      "target",
      [target, parent],
      [{ id: "tag-1", name: "mechanical" }],
      [
        {
          id: "filament-1",
          brand: "Local",
          material: "PLA",
          color: "Black",
          name: "PLA Black",
          origin: "manual",
          resource_kind: "manual",
          archived: false,
        },
      ],
    );

    expect(mapping.patch).toMatchObject({
      designer: "Ada Maker",
      source_url: "https://example.test/models/42",
      notes: "Imported note",
      print_status: "printed",
      model_license: "CC BY 4.0",
      parent_model_id: "parent",
    });
    expect(mapping.tagNames).toEqual(["mechanical", "gear"]);
    expect(mapping.filamentIds).toEqual(["filament-1"]);
    expect(mapping.warnings).toEqual([]);
  });

  test("uses explicit null and empty text values to clear metadata while absent fields stay untouched", () => {
    const unchanged = buildPrintventoryFieldMapping({}, "target", [libraryModel("target")], [], []);
    const clearedNull = buildPrintventoryFieldMapping(
      { designer: null, source: null, notes: null, license: null },
      "target",
      [libraryModel("target")],
      [],
      [],
    );
    const clearedEmpty = buildPrintventoryFieldMapping(
      { designer: "", source: "", notes: "", license: "" },
      "target",
      [libraryModel("target")],
      [],
      [],
    );

    expect(unchanged.patch).toEqual({});
    expect(unchanged.mappedFields).toEqual([]);
    expect(clearedNull.patch).toEqual({
      designer: "",
      source_url: "",
      notes: "",
      model_license: "",
    });
    expect(clearedNull.mappedFields).toEqual(["Designer", "Source URL", "Notes", "License"]);
    expect(clearedNull.warnings).toEqual([]);
    expect(clearedEmpty.patch).toEqual(clearedNull.patch);
    expect(clearedEmpty.mappedFields).toEqual(clearedNull.mappedFields);
    expect(clearedEmpty.warnings).toEqual([]);
  });

  test("accepts safe source origin labels as well as HTTP(S) URLs", () => {
    const url = buildPrintventoryFieldMapping(
      { source: "https://example.test/models/42" },
      "target",
      [libraryModel("target")],
      [],
      [],
    );
    const uppercaseUrl = buildPrintventoryFieldMapping(
      { source: "HTTPS://example.test/models/42" },
      "target",
      [libraryModel("target")],
      [],
      [],
    );
    const origin = buildPrintventoryFieldMapping(
      { source: "Thingiverse" },
      "target",
      [libraryModel("target")],
      [],
      [],
    );

    expect(url.patch.source_url).toBe("https://example.test/models/42");
    expect(uppercaseUrl.patch.source_url).toBe("HTTPS://example.test/models/42");
    expect(origin.patch.source_url).toBe("Thingiverse");
    expect(url.warnings).toEqual([]);
    expect(uppercaseUrl.warnings).toEqual([]);
    expect(origin.warnings).toEqual([]);
  });

  test("applies explicit text clears and source values while omitted fields remain no-ops", async () => {
    const models = [
      libraryModel("clear-null"),
      libraryModel("clear-empty"),
      libraryModel("origin"),
      libraryModel("unchanged"),
    ];
    const updates = [];
    const result = await applyPrintventoryAssignments(
      [
        { designer: null, source: null, notes: null, license: null },
        { designer: "", source: "", notes: "", license: "" },
        { source: "Thingiverse" },
        {},
      ],
      models.map((model, entryIndex) => ({ entryIndex, targetModelId: model.id })),
      models,
      [],
      [],
      {
        createTag: async () => {
          throw new Error("unexpected tag creation");
        },
        updateMetadata: async (modelId, expectedVersion, patch) => {
          updates.push({ modelId, expectedVersion, patch });
          return {
            ...models.find((model) => model.id === modelId),
            metadata_version: expectedVersion + 1,
          };
        },
        setModelFilaments: async () => {
          throw new Error("unexpected filament assignment");
        },
        listPrintAttempts: async () => [],
        createPrintAttempt: async () => {
          throw new Error("unexpected print history import");
        },
      },
    );

    expect(updates).toEqual([
      {
        modelId: "clear-null",
        expectedVersion: 4,
        patch: { designer: "", source_url: "", notes: "", model_license: "" },
      },
      {
        modelId: "clear-empty",
        expectedVersion: 4,
        patch: { designer: "", source_url: "", notes: "", model_license: "" },
      },
      {
        modelId: "origin",
        expectedVersion: 4,
        patch: { source_url: "Thingiverse" },
      },
    ]);
    expect(result.updated).toBe(3);
    expect(result.rows[3].status).toBe("skipped");
  });

  test("preserves exact GoggleLab status, rating, favorite, and stable parent identity", () => {
    const mapping = buildPrintventoryFieldMapping(
      printventoryFile({
        printed: true,
        print_status: "failed",
        rating: 4,
        favorite: true,
        parentModel: "Parent display name changed since export",
        gogglelabParentModelId: "parent-id",
      }),
      "target",
      [
        libraryModel("target"),
        libraryModel("parent-id", { display_name: "Current parent display name" }),
      ],
      [],
      [],
    );

    expect(mapping.patch).toMatchObject({
      print_status: "failed",
      rating: 4,
      favorite: true,
      parent_model_id: "parent-id",
    });
    expect(mapping.warnings).toEqual([]);
  });

  test("falls back to a unique local parent name when a stable ID belongs to another Library", () => {
    const stableIdWins = buildPrintventoryFieldMapping(
      printventoryFile({
        parentModel: "Different local parent name",
        gogglelabParentModelId: "parent-id",
      }),
      "target",
      [
        libraryModel("target"),
        libraryModel("parent-id", { display_name: "Renamed parent" }),
        libraryModel("name-match", { display_name: "Different local parent name" }),
      ],
      [],
      [],
    );
    expect(stableIdWins.patch.parent_model_id).toBe("parent-id");
    expect(stableIdWins.warnings).toEqual([]);

    const selfReference = buildPrintventoryFieldMapping(
      printventoryFile({
        parentModel: "Other same-named candidate",
        gogglelabParentModelId: "target",
      }),
      "target",
      [
        libraryModel("target"),
        libraryModel("other-parent", { display_name: "Other same-named candidate" }),
      ],
      [],
      [],
    );
    expect(selfReference.patch.parent_model_id).toBeUndefined();
    expect(selfReference.warnings.join(" ")).toMatch(/itself/i);

    const unique = buildPrintventoryFieldMapping(
      printventoryFile({
        parentModel: "Parent from source Library",
        gogglelabParentModelId: "source-library-parent-id",
      }),
      "target",
      [
        libraryModel("target"),
        libraryModel("local-parent", { display_name: "Parent from source Library" }),
      ],
      [],
      [],
    );
    expect(unique.patch.parent_model_id).toBe("local-parent");
    expect(unique.warnings).toEqual([]);

    const ambiguous = buildPrintventoryFieldMapping(
      printventoryFile({
        parentModel: "Shared parent",
        gogglelabParentModelId: "foreign-parent-id",
      }),
      "target",
      [
        libraryModel("target"),
        libraryModel("parent-a", { display_name: "Shared parent" }),
        libraryModel("parent-b", { display_name: "Shared parent" }),
      ],
      [],
      [],
    );
    expect(ambiguous.patch.parent_model_id).toBeUndefined();
    expect(ambiguous.warnings.join(" ")).toMatch(/Shared parent.*ambiguous/i);

    const unmatched = buildPrintventoryFieldMapping(
      printventoryFile({
        parentModel: "Missing parent",
        gogglelabParentModelId: "foreign-parent-id",
      }),
      "target",
      [libraryModel("target")],
      [],
      [],
    );
    expect(unmatched.patch.parent_model_id).toBeUndefined();
    expect(unmatched.warnings.join(" ")).toMatch(/Missing parent.*does not uniquely match/i);
  });

  test("resolves legacy GoggleLab parent IDs in parentModel when the extension is absent", () => {
    const mapping = buildPrintventoryFieldMapping(
      printventoryFile({ parentModel: "parent-id" }),
      "target",
      [
        libraryModel("target"),
        libraryModel("parent-id", { display_name: "Current parent display name" }),
      ],
      [],
      [],
    );

    expect(mapping.patch.parent_model_id).toBe("parent-id");
    expect(mapping.warnings).toEqual([]);
  });

  test("maps tag categories from the GoggleLab JSON extension", () => {
    const mapping = buildPrintventoryFieldMapping(
      printventoryFile({
        tags: ["mechanical"],
        gogglelabTags: [{ name: "mechanical", category: "Assembly" }],
      }),
      "target",
      [libraryModel("target")],
      [],
      [],
    );

    expect(mapping.tagNames).toEqual(["mechanical"]);
    expect(mapping.tagAssignments).toEqual([{ name: "mechanical", category: "Assembly" }]);
  });

  test("matches exported vendor/color fields and exact local Spoolman identity without provider calls", () => {
    const mapping = buildPrintventoryFieldMapping(
      printventoryFile({
        filaments: [
          {
            name: "PLA White",
            vendor: "Local Vendor",
            material: "PLA",
            color_hex: "#ffffff",
            spoolman_id: "spool-42",
            source: "spoolman",
          },
        ],
      }),
      "target",
      [libraryModel("target")],
      [],
      [
        {
          id: "local-spoolman-filament",
          brand: "Local Vendor",
          material: "PLA",
          color: "#ffffff",
          name: "PLA White",
          external_id: "spool-42",
          origin: "spoolman",
          resource_kind: "filament",
          archived: false,
        },
      ],
    );

    expect(mapping.filamentIds).toEqual(["local-spoolman-filament"]);
    expect(mapping.warnings).toEqual([]);
  });

  test("maps exported printEvents into validated local print attempts", () => {
    const mapping = buildPrintventoryFieldMapping(
      {
        printEvents: [
          {
            printed_at: "2026-10-10T00:00:00Z",
            outcome: "printed",
            quantity: 2,
            notes: "First print",
            filaments: [
              {
                name: "PLA White",
                vendor: "Local Vendor",
                material: "PLA",
                color_hex: "#ffffff",
                spoolman_id: "spool-42",
                source: "spoolman",
              },
            ],
            parts: [],
          },
        ],
      },
      "target",
      [libraryModel("target")],
      [],
      [
        {
          id: "local-spoolman-filament",
          brand: "Local Vendor",
          material: "PLA",
          color: "#ffffff",
          name: "PLA White",
          external_id: "spool-42",
          origin: "spoolman",
          resource_kind: "filament",
          archived: false,
        },
      ],
    );

    expect(mapping.printAttempts).toEqual([
      {
        input: {
          printed_at: "2026-10-10T00:00:00Z",
          outcome: "printed",
          quantity: 2,
          notes: "First print",
          filament_ids: ["local-spoolman-filament"],
        },
      },
    ]);
    expect(mapping.mappedFields).toContain("Print history");
    expect(mapping.warnings).toEqual([]);
  });

  test("maps reviewed Printventory event part snapshots to per-copy local usage", () => {
    const importedParts = [
      {
        externalRef: "row:1",
        input: {
          name: "M3 bolt",
          category: "Hardware",
          quantity: 40,
          unit: "pcs",
          notes: "",
          lowStock: 8,
        },
      },
    ];
    const mapping = buildPrintventoryFieldMapping(
      {
        printEvents: [
          {
            printed_at: "2026-10-10T00:00:00Z",
            outcome: "printed",
            quantity: 3,
            notes: "",
            filaments: [],
            parts: [
              {
                name: "M3 bolt",
                category: "Hardware",
                unit: "pcs",
                quantity: 6,
              },
            ],
          },
        ],
      },
      "target",
      [libraryModel("target")],
      [],
      [],
      importedParts,
      new Map([["row:1", "local-bolt"]]),
    );

    expect(mapping.printAttempts?.[0].input.parts).toEqual([{ id: "local-bolt", quantity: 2 }]);
    expect(mapping.mappedFields).toContain("Parts");
    expect(mapping.warnings).toEqual([]);
  });

  test("preserves an explicit source ID as an orphan instead of name-matching a replacement", () => {
    const importedParts = [
      {
        externalRef: "replacement-part",
        sourceId: "replacement-part",
        input: {
          name: "M3 bolt",
          category: "Hardware",
          quantity: 34,
          unit: "pcs",
          notes: "",
          lowStock: 8,
        },
      },
    ];
    const entry = {
      printEvents: [
        {
          printed_at: "2026-10-10T00:00:00Z",
          outcome: "printed",
          quantity: 2,
          notes: "Historical event",
          filaments: [],
          parts: [
            {
              id: "deleted-source-part",
              name: "M3 bolt",
              category: "Hardware",
              unit: "pcs",
              quantity: 4,
              quantityPerCopy: 2,
            },
          ],
        },
      ],
    };
    const makeMapping = () =>
      buildPrintventoryFieldMapping(
        entry,
        "target",
        [libraryModel("target")],
        [],
        [],
        importedParts,
        new Map([["replacement-part", "local-replacement"]]),
      );
    const mapping = makeMapping();
    const orphan = mapping.printAttempts?.[0].input.parts?.[0];

    expect(orphan?.id).toMatch(/^printventory_orphan_/);
    expect(orphan?.id).not.toBe("local-replacement");
    expect(orphan?.quantity).toBe(2);
    expect(orphan?.snapshot).toEqual({ name: "M3 bolt", category: "Hardware", unit: "pcs" });
    expect(makeMapping().printAttempts?.[0].input.parts?.[0].id).toBe(orphan?.id);
    expect(mapping.warnings.join(" ")).toMatch(
      /historical usage is preserved but is not linked to stock/i,
    );
  });

  test("preserves historical usage when a reviewed top-level part was skipped", () => {
    const importedParts = [
      {
        externalRef: "skipped-part",
        sourceId: "skipped-part",
        input: {
          name: "M3 bolt",
          category: "Hardware",
          quantity: 34,
          unit: "pcs",
          notes: "",
          lowStock: 8,
        },
      },
    ];
    const mapping = buildPrintventoryFieldMapping(
      {
        printEvents: [
          {
            printed_at: "2026-10-10T00:00:00Z",
            outcome: "failed",
            quantity: 1,
            notes: "Historical event",
            filaments: [],
            parts: [
              {
                id: "skipped-part",
                name: "M3 bolt",
                category: "Hardware",
                unit: "pcs",
                quantity: 3,
              },
            ],
          },
        ],
      },
      "target",
      [libraryModel("target")],
      [],
      [],
      importedParts,
      new Map(),
    );
    const orphan = mapping.printAttempts?.[0].input.parts?.[0];
    expect(orphan?.id).toMatch(/^printventory_orphan_/);
    expect(orphan?.snapshot).toEqual({ name: "M3 bolt", category: "Hardware", unit: "pcs" });
    expect(mapping.warnings.join(" ")).toMatch(
      /historical usage is preserved but is not linked to stock/i,
    );
  });

  test("preserves unassigned snapshot-only usage and reuses reserved orphan IDs", () => {
    const reservedId = "printventory_orphan_0123456789abcdef";
    const build = (id) =>
      buildPrintventoryFieldMapping(
        {
          printEvents: [
            {
              printed_at: "2026-10-10T00:00:00Z",
              outcome: "printed",
              quantity: 1,
              notes: "Historical usage",
              filaments: [],
              parts: [
                {
                  ...(id ? { id } : {}),
                  name: "Unknown fitting",
                  category: null,
                  unit: "pcs",
                  quantity: 4,
                },
              ],
            },
          ],
        },
        "target",
        [libraryModel("target")],
        [],
        [],
      );
    const unassigned = build(null).printAttempts?.[0].input.parts?.[0];
    const reused = build(reservedId).printAttempts?.[0].input.parts?.[0];

    expect(unassigned?.id).toMatch(/^printventory_orphan_/);
    expect(unassigned?.id.length).toBeLessThanOrEqual(128);
    expect(unassigned?.snapshot).toEqual({ name: "Unknown fitting", category: null, unit: "pcs" });
    expect(reused?.id).toBe(reservedId);
    expect(reused?.snapshot).toEqual({ name: "Unknown fitting", category: null, unit: "pcs" });
  });

  test("prefers Printventory's explicit per-copy extension over rounded event totals", () => {
    const importedParts = [
      {
        externalRef: "upstream-part-42",
        sourceId: "upstream-part-42",
        input: {
          name: "M3 bolt",
          category: "Hardware",
          quantity: 34,
          unit: "pcs",
          notes: "",
          lowStock: 8,
        },
      },
    ];
    const mapping = buildPrintventoryFieldMapping(
      {
        printEvents: [
          {
            printed_at: "2026-10-10T00:00:00Z",
            outcome: "printed",
            quantity: 3,
            notes: "",
            filaments: [],
            parts: [
              {
                gogglelab_part_id: "upstream-part-42",
                id: "upstream-part-42",
                name: "M3 bolt",
                category: "Hardware",
                unit: "pcs",
                quantity: 5,
                quantityPerCopy: 2,
              },
            ],
          },
        ],
      },
      "target",
      [libraryModel("target")],
      [],
      [],
      importedParts,
      new Map([["upstream-part-42", "local-bolt"]]),
    );
    expect(mapping.printAttempts?.[0].input.parts).toEqual([{ id: "local-bolt", quantity: 2 }]);
    expect(mapping.warnings).toEqual([]);
  });

  test("imports cancelled outcomes and caps event part usage at the core limit", () => {
    const tooManyParts = buildPrintventoryFieldMapping(
      {
        printEvents: [
          {
            printed_at: "2026-10-10T00:00:00Z",
            outcome: "cancelled",
            quantity: 1,
            notes: "Cancelled job",
            filaments: [],
            parts: Array.from({ length: MAX_PRINTVENTORY_EVENT_PARTS + 1 }, (_, index) => ({
              id: index + 1,
              name: `Part ${index + 1}`,
              unit: "pcs",
              quantity: 1,
            })),
          },
        ],
      },
      "target",
      [libraryModel("target")],
      [],
      [],
    );
    expect(tooManyParts.printAttempts?.[0].input.outcome).toBe("cancelled");
    expect(tooManyParts.printAttempts?.[0].input.parts).toBeUndefined();
    expect(tooManyParts.warnings.join(" ")).toMatch(/256-part/i);
  });

  test("does not guess non-integral per-copy usage or unreviewed local parts", () => {
    const importedParts = [
      {
        externalRef: "row:1",
        input: {
          name: "M3 bolt",
          category: "Hardware",
          quantity: 40,
          unit: "pcs",
          notes: "",
          lowStock: 8,
        },
      },
    ];
    const mapping = buildPrintventoryFieldMapping(
      {
        printEvents: [
          {
            printed_at: "2026-10-10T00:00:00Z",
            outcome: "failed",
            quantity: 3,
            notes: "",
            filaments: [],
            parts: [
              {
                id: 42,
                name: "M3 bolt",
                category: "Hardware",
                unit: "pcs",
                quantity: 5,
              },
            ],
          },
        ],
      },
      "target",
      [libraryModel("target")],
      [],
      [],
      importedParts,
      new Map([["row:1", "local-bolt"]]),
    );

    expect(mapping.printAttempts?.[0].input.parts).toBeUndefined();
    expect(mapping.warnings.join(" ")).toMatch(/per-copy/i);
  });

  test("preserves print event printer snapshots and keeps printer references external", () => {
    const mapping = buildPrintventoryFieldMapping(
      {
        printEvents: [
          {
            printed_at: "2026-10-10T00:00:00Z",
            outcome: "printed",
            quantity: 1,
            notes: "Printed on imported machine",
            filaments: [],
            gogglelab_printer_id: "gogglelab-printer-1",
            printer_id: "foreign-printer-1",
            printer_nickname: "Workshop",
            printer_manufacturer: "Maker",
            printer_model: "M1",
            printer_type: "FDM",
          },
          {
            printed_at: "2026-10-11T00:00:00Z",
            outcome: "failed",
            quantity: 1,
            notes: "Legacy reference",
            filaments: [],
            printer_id: "foreign-printer-2",
            printer_nickname: "Legacy workshop",
          },
        ],
      },
      "target",
      [libraryModel("target")],
      [],
      [],
    );

    expect(mapping.printAttempts).toEqual([
      {
        input: {
          printed_at: "2026-10-10T00:00:00Z",
          outcome: "printed",
          quantity: 1,
          notes: "Printed on imported machine",
          filament_ids: [],
        },
        printerExternalRef: "gogglelab-printer-1",
        printerSnapshot: {
          printer_nickname: "Workshop",
          printer_manufacturer: "Maker",
          printer_model: "M1",
          printer_type: "FDM",
        },
      },
      {
        input: {
          printed_at: "2026-10-11T00:00:00Z",
          outcome: "failed",
          quantity: 1,
          notes: "Legacy reference",
          filament_ids: [],
        },
        printerExternalRef: "foreign-printer-2",
        printerSnapshot: { printer_nickname: "Legacy workshop" },
      },
    ]);
    expect(mapping.printAttempts[0].input).not.toHaveProperty("printer_id");
    expect(mapping.printAttempts[1].input).not.toHaveProperty("printer_id");
  });

  test("preserves valid event creation timestamps and leaves omitted ones to be regenerated", () => {
    const mapping = buildPrintventoryFieldMapping(
      {
        printEvents: [
          {
            printed_at: "2026-10-10T00:00:00Z",
            created_at: "2026-10-09T19:00:00-05:00",
            outcome: "printed",
            quantity: 1,
            notes: "Imported with original audit time",
            filaments: [],
          },
          {
            printed_at: "2026-10-11T00:00:00Z",
            outcome: "failed",
            quantity: 1,
            notes: "No original creation time",
            filaments: [],
          },
        ],
      },
      "target",
      [libraryModel("target")],
      [],
      [],
    );

    expect(mapping.printAttempts).toEqual([
      {
        input: {
          printed_at: "2026-10-10T00:00:00Z",
          outcome: "printed",
          quantity: 1,
          notes: "Imported with original audit time",
          filament_ids: [],
        },
        createdAt: "2026-10-10T00:00:00Z",
      },
      {
        input: {
          printed_at: "2026-10-11T00:00:00Z",
          outcome: "failed",
          quantity: 1,
          notes: "No original creation time",
          filament_ids: [],
        },
      },
    ]);
    expect(mapping.warnings).toEqual([]);
  });

  test("skips print events with malformed explicit creation timestamps", () => {
    const mapping = buildPrintventoryFieldMapping(
      {
        printEvents: [
          {
            printed_at: "2026-10-10T00:00:00Z",
            created_at: "2026-02-30T00:00:00Z",
            outcome: "printed",
            quantity: 1,
            notes: "Invalid creation time",
            filaments: [],
          },
          {
            printed_at: "2026-10-10T00:00:00Z",
            created_at: "2026-10-10",
            outcome: "printed",
            quantity: 1,
            notes: "Creation time must include a timezone",
            filaments: [],
          },
          {
            printed_at: "2026-10-11T00:00:00Z",
            outcome: "failed",
            quantity: 1,
            notes: "No creation time supplied",
            filaments: [],
          },
        ],
      },
      "target",
      [libraryModel("target")],
      [],
      [],
    );

    expect(mapping.printAttempts).toEqual([
      {
        input: {
          printed_at: "2026-10-11T00:00:00Z",
          outcome: "failed",
          quantity: 1,
          notes: "No creation time supplied",
          filament_ids: [],
        },
      },
    ]);
    expect(mapping.warnings.join(" ")).toMatch(/creation timestamp/i);
  });

  test("skips invalid printEvents and reports unsupported printer or part details", () => {
    const mapping = buildPrintventoryFieldMapping(
      {
        printEvents: [
          { printed_at: "not-a-date", outcome: "cancelled", quantity: 0, filaments: [] },
          {
            printed_at: "2026-10-10",
            outcome: "failed",
            quantity: 1,
            notes: "",
            filaments: [],
            printer_nickname: "Printer 1",
            parts: [{ id: 1, quantity: 1 }],
          },
        ],
      },
      "target",
      [libraryModel("target")],
      [],
      [],
    );

    expect(mapping.printAttempts).toHaveLength(1);
    expect(mapping.printAttempts?.[0].input).toMatchObject({
      printed_at: "2026-10-10T00:00:00.000Z",
      outcome: "failed",
      quantity: 1,
      notes: "",
      filament_ids: [],
    });
    expect(mapping.printAttempts?.[0].input.parts?.[0].id).toMatch(/^printventory_orphan_/);
    expect(mapping.printAttempts?.[0].input.parts?.[0].snapshot).toEqual({
      name: "Imported part 1",
      category: null,
      unit: "unit",
    });
    expect(mapping.warnings.join(" ")).toMatch(
      /historical usage is preserved but is not linked to stock/i,
    );
    expect(mapping.warnings.join(" ")).toMatch(/print event/i);
    expect(mapping.warnings.join(" ")).toMatch(/printer|part/i);
  });

  test("does not guess a local filament for an unknown exported source", () => {
    const mapping = buildPrintventoryFieldMapping(
      printventoryFile({
        filaments: [
          {
            brand: "Local",
            material: "PLA",
            color: "Black",
            name: "PLA Black",
            source: "unknown-provider",
          },
        ],
      }),
      "target",
      [libraryModel("target")],
      [],
      [
        {
          id: "manual",
          brand: "Local",
          material: "PLA",
          color: "Black",
          name: "PLA Black",
          origin: "manual",
          resource_kind: "manual",
          archived: false,
        },
      ],
    );

    expect(mapping.filamentIds).toBeUndefined();
    expect(mapping.warnings.join(" ")).toMatch(/local filament/i);
  });

  test("does not guess foreign filament IDs or ambiguous parent names", () => {
    const mapping = buildPrintventoryFieldMapping(
      printventoryFile({ parentModel: "Assembly", filaments: [47] }),
      "target",
      [
        libraryModel("target", { display_name: "Gear" }),
        libraryModel("parent-a", { display_name: "Assembly" }),
        libraryModel("parent-b", { display_name: "Assembly" }),
      ],
      [],
      [],
    );

    expect(mapping.patch).not.toHaveProperty("parent_model_id");
    expect(mapping.filamentIds).toBeUndefined();
    expect(mapping.warnings.join(" ")).toMatch(/ambiguous/i);
    expect(mapping.warnings.join(" ")).toMatch(/numeric/i);
  });

  test("skips nameless tags and reports foreign tag properties", () => {
    const mapping = buildPrintventoryFieldMapping(
      printventoryFile({ tags: [{ id: 11 }], filaments: [] }),
      "target",
      [libraryModel("target")],
      [],
      [],
    );

    expect(mapping.tagNames).toBeUndefined();
    expect(mapping.mappedFields).not.toContain("Tags");
    expect(mapping.warnings.join(" ")).toMatch(/explicit name/i);
  });

  test("matches local tag names without case-only duplicates and validates local tag names", () => {
    const mapping = buildPrintventoryFieldMapping(
      printventoryFile({ tags: ["Mechanical", "mechanical", "new tag"] }),
      "target",
      [libraryModel("target")],
      [{ id: "tag-1", name: "mechanical" }],
      [],
    );
    const invalid = buildPrintventoryFieldMapping(
      printventoryFile({ tags: ["../unsafe"] }),
      "target",
      [libraryModel("target")],
      [],
      [],
    );

    expect(mapping.tagNames).toEqual(["mechanical", "new tag"]);
    expect(
      missingPrintventoryTagNames(mapping.tagNames, [{ id: "tag-1", name: "MECHANICAL" }]),
    ).toEqual(["new tag"]);
    expect(invalid.tagNames).toBeUndefined();
    expect(invalid.warnings.join(" ")).toMatch(/valid local tag name/i);
  });

  test("rejects unsafe source values before saving a clickable source URL", () => {
    for (const source of [
      "javascript:alert(1)",
      "https://",
      "https://user:password@example.test/model",
      "https://@example.test/model",
      "https://example.test/model\nInjected",
    ]) {
      const mapping = buildPrintventoryFieldMapping(
        printventoryFile({ source }),
        "target",
        [libraryModel("target")],
        [],
        [],
      );

      expect(mapping.patch).not.toHaveProperty("source_url");
      expect(mapping.warnings.join(" ")).toMatch(/HTTP or HTTPS URL/i);
    }
  });

  test("does not match remote-origin or archived filament records", () => {
    const mapping = buildPrintventoryFieldMapping(
      printventoryFile({
        filaments: [{ brand: "Remote", material: "PLA", color: "Blue", name: "PLA Blue" }],
      }),
      "target",
      [libraryModel("target")],
      [],
      [
        {
          id: "remote",
          brand: "Remote",
          material: "PLA",
          color: "Blue",
          name: "PLA Blue",
          origin: "spoolman",
          resource_kind: "filament",
          archived: false,
        },
        {
          id: "archived",
          brand: "Remote",
          material: "PLA",
          color: "Blue",
          name: "PLA Blue",
          origin: "manual",
          resource_kind: "manual",
          archived: true,
        },
      ],
    );

    expect(mapping.filamentIds).toBeUndefined();
    expect(mapping.warnings.join(" ")).toMatch(/local filament/i);
  });

  test("keeps archived local filaments in historical print events but not current assignments", () => {
    const archivedFilament = {
      id: "archived-local-filament",
      brand: "Local",
      material: "PLA",
      color: "Blue",
      name: "Archived PLA",
      origin: "manual",
      resource_kind: "manual",
      archived: true,
    };
    const model = [libraryModel("target")];
    const archivedAssignment = buildPrintventoryFieldMapping(
      printventoryFile({
        filaments: [
          {
            brand: "Local",
            material: "PLA",
            color: "Blue",
            name: "Archived PLA",
            source: "manual",
          },
        ],
      }),
      "target",
      model,
      [],
      [archivedFilament],
    );
    const archivedHistory = buildPrintventoryFieldMapping(
      printventoryFile({
        printEvents: [
          {
            printed_at: "2026-10-10T00:00:00Z",
            outcome: "printed",
            quantity: 1,
            notes: "Used archived spool",
            filaments: [
              {
                brand: "Local",
                material: "PLA",
                color: "Blue",
                name: "Archived PLA",
                source: "manual",
              },
            ],
          },
        ],
      }),
      "target",
      model,
      [],
      [archivedFilament],
    );

    expect(archivedAssignment.filamentIds).toBeUndefined();
    expect(archivedHistory.printAttempts?.[0].input.filament_ids).toEqual([
      "archived-local-filament",
    ]);
    expect(archivedHistory.warnings).toEqual([]);
  });

  test("writes with expected metadata versions and advances before linking filaments", async () => {
    const model = libraryModel("target", { metadata_version: 4 });
    const metadataVersions = [];
    const filamentVersions = [];
    const result = await applyPrintventoryAssignments(
      [printventoryFile({ tags: ["new-tag"], filaments: [] })],
      [{ entryIndex: 0, targetModelId: "target" }],
      [model],
      [],
      [],
      {
        createTag: async (name, category) => ({ id: "tag-new", name, category }),
        updateMetadata: async (id, expectedVersion, patch) => {
          metadataVersions.push(expectedVersion);
          expect(patch.tags).toEqual(["tag-new"]);
          expect(patch).not.toHaveProperty("filePath");
          return { ...model, metadata_version: 5 };
        },
        setModelFilaments: async (id, expectedVersion, filamentIds) => {
          filamentVersions.push(expectedVersion);
          expect(filamentIds).toEqual([]);
          return { ...model, metadata_version: 6 };
        },
      },
    );

    expect(metadataVersions).toEqual([4]);
    expect(filamentVersions).toEqual([5]);
    expect(result.updated).toBe(1);
    expect(result.rows[0].warnings).toEqual([]);
  });

  test("creates categorized tags and retains conflicting existing global categories", async () => {
    const model = libraryModel("target", { metadata_version: 4 });
    const existingTag = { id: "tag-existing", name: "mechanical", category: "Old" };
    const createdTags = [];
    const result = await applyPrintventoryAssignments(
      [
        printventoryFile({
          tags: ["mechanical", "new-tag"],
          gogglelabTags: [
            { name: "mechanical", category: "Assembly" },
            { name: "new-tag", category: null },
          ],
        }),
      ],
      [{ entryIndex: 0, targetModelId: "target" }],
      [model],
      [existingTag],
      [],
      {
        createTag: async (name, category) => {
          createdTags.push({ name, category });
          return { id: "tag-new", name, category };
        },
        updateMetadata: async (_id, expectedVersion, patch) => {
          expect(expectedVersion).toBe(4);
          expect(patch.tags).toEqual(["tag-existing", "tag-new"]);
          return { ...model, metadata_version: 5 };
        },
        setModelFilaments: async (_id, expectedVersion, filamentIds) => {
          expect(expectedVersion).toBe(5);
          expect(filamentIds).toEqual([]);
          return { ...model, metadata_version: 6 };
        },
      },
    );

    expect(createdTags).toEqual([{ name: "new-tag", category: null }]);
    expect(existingTag.category).toBe("Old");
    expect(result.rows[0].status).toBe("partial");
    expect(result.rows[0].warnings.join(" ")).toMatch(/global category will be retained/i);
  });

  test("reports a partial result if metadata saves but the filament link fails", async () => {
    const model = libraryModel("target", { metadata_version: 4 });
    const result = await applyPrintventoryAssignments(
      [
        printventoryFile({
          designer: "Ada",
          tags: [],
          filaments: [{ brand: "Local", material: "PLA", color: "Black", name: "PLA Black" }],
        }),
      ],
      [{ entryIndex: 0, targetModelId: "target" }],
      [model],
      [],
      [
        {
          id: "filament-1",
          brand: "Local",
          material: "PLA",
          color: "Black",
          name: "PLA Black",
          origin: "manual",
          resource_kind: "manual",
          archived: false,
        },
      ],
      {
        createTag: async () => {
          throw new Error("unexpected tag creation");
        },
        updateMetadata: async () => ({ ...model, metadata_version: 5 }),
        setModelFilaments: async () => {
          throw new Error("version conflict");
        },
      },
    );

    expect(result.updated).toBe(1);
    expect(result.rows[0].status).toBe("partial");
    expect(result.rows[0].mappedFields).toContain("Designer");
    expect(result.rows[0].mappedFields).not.toContain("Filaments");
    expect(result.rows[0].warnings.join(" ")).toMatch(/Some metadata was saved/i);
  });

  test("uses the separate import writer for imported event creation times", async () => {
    const model = libraryModel("target");
    const createdInputs = [];
    const importedInputs = [];
    const result = await applyPrintventoryAssignments(
      [
        {
          printEvents: [
            {
              printed_at: "2026-10-10T12:00:00Z",
              created_at: "2026-10-09T19:00:00-05:00",
              outcome: "printed",
              quantity: 1,
              notes: "Imported event",
              filaments: [],
            },
          ],
        },
      ],
      [{ entryIndex: 0, targetModelId: "target" }],
      [model],
      [],
      [],
      {
        createTag: async () => {
          throw new Error("unexpected tag creation");
        },
        updateMetadata: async () => {
          throw new Error("unexpected metadata update");
        },
        setModelFilaments: async () => {
          throw new Error("unexpected filament assignment");
        },
        listPrintAttempts: async () => [],
        createPrintAttempt: async (_modelId, input) => {
          createdInputs.push(input);
          return { id: "created", model_id: "target", ...input, updated_at: "now" };
        },
        importPrintAttempt: async (_modelId, input, createdAt, printerId, printerName) => {
          importedInputs.push({ input, createdAt, printerId, printerName });
          return {
            id: "imported",
            model_id: "target",
            ...input,
            created_at: createdAt,
            updated_at: "now",
          };
        },
        importPrintAttempts: async (_modelId, attempts) => {
          importedInputs.push(...attempts);
          return {
            addedCount: attempts.length,
            skippedCount: 0,
            metadataVersion: model.metadata_version + 1,
          };
        },
      },
    );

    expect(createdInputs).toEqual([]);
    expect(importedInputs).toEqual([
      {
        input: {
          printed_at: "2026-10-10T12:00:00Z",
          outcome: "printed",
          quantity: 1,
          notes: "Imported event",
          filament_ids: [],
        },
        createdAt: "2026-10-10T00:00:00Z",
        printerId: null,
        printerName: null,
      },
    ]);
    expect(result.updated).toBe(1);
  });

  test("stores imported part snapshots through the historical event writer without stock deltas", async () => {
    const model = libraryModel("target");
    const importedParts = [
      {
        externalRef: "row:1",
        input: {
          name: "M3 bolt",
          category: "Hardware",
          quantity: 34,
          unit: "pcs",
          notes: "",
          lowStock: 8,
        },
      },
    ];
    const submitted = [];
    let normalCreateCalled = false;
    const result = await applyPrintventoryAssignments(
      [
        {
          printEvents: [
            {
              printed_at: "2026-10-10T00:00:00Z",
              outcome: "failed",
              quantity: 2,
              notes: "Imported historical usage",
              filaments: [],
              parts: [{ name: "M3 bolt", category: "Hardware", unit: "pcs", quantity: 4 }],
            },
          ],
        },
      ],
      [{ entryIndex: 0, targetModelId: "target" }],
      [model],
      [],
      [],
      {
        createTag: async () => {
          throw new Error("unexpected tag creation");
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
          throw new Error("normal creation would change stock");
        },
        importPrintAttempts: async (_modelId, attempts) => {
          submitted.push(...attempts);
          return { addedCount: attempts.length, skippedCount: 0, metadataVersion: 5 };
        },
      },
      new Map(),
      importedParts,
      new Map([["row:1", "local-bolt"]]),
    );

    expect(normalCreateCalled).toBe(false);
    expect(submitted[0].input.parts).toEqual([{ id: "local-bolt", quantity: 2 }]);
    expect(result.rows[0].mappedFields).toContain("Parts");
  });

  test("routes only reviewed local printer IDs and preserves snapshots for unknown and unlinked events", async () => {
    const model = libraryModel("target");
    const importedInputs = [];
    const createdInputs = [];
    const result = await applyPrintventoryAssignments(
      [
        {
          printEvents: [
            {
              printed_at: "2026-10-10T00:00:00Z",
              outcome: "printed",
              quantity: 1,
              notes: "Mapped printer",
              filaments: [],
              gogglelab_printer_id: "printer-known",
              printer_nickname: "Mapped snapshot",
            },
            {
              printed_at: "2026-10-11T00:00:00Z",
              outcome: "failed",
              quantity: 1,
              notes: "Unknown printer",
              filaments: [],
              printer_id: "printer-unknown",
              printer_nickname: "Unknown snapshot",
            },
            {
              printed_at: "2026-10-12T00:00:00Z",
              outcome: "printed",
              quantity: 1,
              notes: "Unlinked printer snapshot",
              filaments: [],
              printer_nickname: "Unlinked snapshot",
            },
          ],
        },
      ],
      [{ entryIndex: 0, targetModelId: "target" }],
      [model],
      [],
      [],
      {
        createTag: async () => {
          throw new Error("unexpected tag creation");
        },
        updateMetadata: async () => {
          throw new Error("unexpected metadata update");
        },
        setModelFilaments: async () => {
          throw new Error("unexpected filament assignment");
        },
        listPrintAttempts: async () => [],
        createPrintAttempt: async (_modelId, input) => {
          createdInputs.push(input);
          return { id: "created", model_id: "target", ...input, updated_at: "now" };
        },
        importPrintAttempt: async (_modelId, input, createdAt, printerId, printerName) => {
          importedInputs.push({ input, createdAt, printerId, printerName });
          return {
            id: "imported",
            model_id: "target",
            ...input,
            created_at: input.printed_at,
            updated_at: "now",
          };
        },
        importPrintAttempts: async (_modelId, attempts) => {
          importedInputs.push(...attempts);
          return {
            addedCount: attempts.length,
            skippedCount: 0,
            metadataVersion: model.metadata_version + 1,
          };
        },
      },
      new Map([["printer-known", "local-printer-1"]]),
    );

    expect(createdInputs).toEqual([]);
    expect(importedInputs).toEqual([
      {
        input: {
          printed_at: "2026-10-10T00:00:00Z",
          outcome: "printed",
          quantity: 1,
          notes: "Mapped printer",
          filament_ids: [],
        },
        createdAt: null,
        printerId: "local-printer-1",
        printerName: "Mapped snapshot",
      },
      {
        input: {
          printed_at: "2026-10-11T00:00:00Z",
          outcome: "failed",
          quantity: 1,
          notes: "Unknown printer",
          filament_ids: [],
        },
        createdAt: null,
        printerId: null,
        printerName: "Unknown snapshot",
      },
      {
        input: {
          printed_at: "2026-10-12T00:00:00Z",
          outcome: "printed",
          quantity: 1,
          notes: "Unlinked printer snapshot",
          filament_ids: [],
        },
        createdAt: null,
        printerId: null,
        printerName: "Unlinked snapshot",
      },
    ]);
    for (const imported of importedInputs) {
      expect(imported.input).not.toHaveProperty("printer_id");
    }
    expect(result.updated).toBe(1);
  });

  test("history import skips matching existing events and reimport stays idempotent", async () => {
    const model = libraryModel("target", { metadata_version: 4 });
    const priorAttempt = {
      id: "prior",
      model_id: "target",
      printed_at: "2026-10-10T00:00:00.000Z",
      quantity: 1,
      notes: "Existing event",
      filament_ids: [],
      outcome: "printed",
      created_at: "2026-10-10T00:00:00.000Z",
      updated_at: "2026-10-10T00:00:00.000Z",
    };
    const events = [
      {
        printed_at: "2026-10-10T01:00:00+01:00",
        created_at: "2026-10-09T00:00:00Z",
        outcome: "printed",
        quantity: 1,
        notes: "Existing event",
        filaments: [],
      },
      {
        printed_at: "2026-10-11T00:00:00Z",
        outcome: "failed",
        quantity: 2,
        notes: "New event",
        filaments: [],
      },
      {
        printed_at: "2026-10-11T00:00:00Z",
        outcome: "failed",
        quantity: 2,
        notes: "New event",
        filaments: [],
      },
    ];
    const created = [];
    const api = {
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
      listPrintAttempts: async () => [priorAttempt, ...created],
      createPrintAttempt: async (modelId, input) => {
        const attempt = {
          id: "created-" + created.length,
          model_id: modelId,
          ...input,
          created_at: input.printed_at,
          updated_at: input.printed_at,
        };
        created.push(attempt);
        return attempt;
      },
      importPrintAttempt: async (modelId, input, createdAt, printerId, printerName) => {
        const attempt = {
          id: "created-" + created.length,
          model_id: modelId,
          ...input,
          printer_id: printerId,
          printer_name: printerName,
          created_at: createdAt ?? input.printed_at,
          updated_at: input.printed_at,
        };
        created.push(attempt);
        return attempt;
      },
      importPrintAttempts: async (modelId, attempts) => {
        for (const item of attempts) {
          created.push({
            id: "created-" + created.length,
            model_id: modelId,
            ...item.input,
            printer_id: item.printerId,
            printer_name: item.printerName,
            created_at: item.createdAt ?? item.input.printed_at,
            updated_at: item.input.printed_at,
          });
        }
        return {
          addedCount: attempts.length,
          skippedCount: 0,
          metadataVersion: model.metadata_version + 1,
        };
      },
    };
    const entries = [{ fileName: "gear.stl", printEvents: events }];
    const assignments = [{ entryIndex: 0, targetModelId: "target" }];

    const first = await applyPrintventoryAssignments(entries, assignments, [model], [], [], api);
    const second = await applyPrintventoryAssignments(entries, assignments, [model], [], [], api);

    expect(created).toHaveLength(2);
    expect(created[0].notes).toBe("New event");
    expect(first.rows[0].mappedFields).toContain("Print history");
    expect(second.updated).toBe(0);
  });

  test("reports print events skipped by the atomic import when another import won the race", async () => {
    const model = libraryModel("target", { metadata_version: 10 });
    const submitted = [];
    const result = await applyPrintventoryAssignments(
      [
        {
          fileName: "gear.stl",
          printEvents: [
            {
              printed_at: "2026-10-10T00:00:00Z",
              outcome: "printed",
              quantity: 1,
              notes: "Concurrent event",
              filaments: [],
            },
          ],
        },
      ],
      [{ entryIndex: 0, targetModelId: "target" }],
      [model],
      [],
      [],
      {
        createTag: async () => {
          throw new Error("unexpected tag creation");
        },
        updateMetadata: async () => {
          throw new Error("unexpected metadata update");
        },
        setModelFilaments: async () => {
          throw new Error("unexpected filament assignment");
        },
        listPrintAttempts: async () => [],
        createPrintAttempt: async () => {
          throw new Error("unexpected print attempt creation");
        },
        importPrintAttempts: async (_modelId, attempts) => {
          submitted.push(...attempts);
          return { addedCount: 0, skippedCount: attempts.length, metadataVersion: 11 };
        },
      },
    );

    expect(submitted).toHaveLength(1);
    expect(result.updated).toBe(0);
    expect(result.rows[0].status).toBe("skipped");
    expect(result.rows[0].warnings.join(" ")).toMatch(/matching print event.*skipped/i);
  });

  test("printer attribution prevents matching unlinked history from dropping snapshots and reimport stays idempotent", async () => {
    const model = libraryModel("target", { metadata_version: 4 });
    const importedEvent = {
      printed_at: "2026-10-10T00:00:00Z",
      outcome: "printed",
      quantity: 1,
      notes: "Same visible event",
      filaments: [],
      gogglelab_printer_id: "foreign-printer-1",
      printer_nickname: "Mapped printer snapshot",
    };
    const priorAttempt = {
      id: "prior",
      model_id: "target",
      printed_at: "2026-10-10T00:00:00.000Z",
      quantity: 1,
      notes: "Same visible event",
      filament_ids: [],
      outcome: "printed",
      printer_id: null,
      printer_name: null,
      created_at: "2026-10-10T00:00:00.000Z",
      updated_at: "2026-10-10T00:00:00.000Z",
    };
    const created = [];
    const api = {
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
      listPrintAttempts: async () => [priorAttempt, ...created],
      importPrintAttempt: async (modelId, input, createdAt, printerId, printerName) => {
        const attempt = {
          id: "created-" + created.length,
          model_id: modelId,
          ...input,
          printer_id: printerId,
          printer_name: printerName,
          created_at: createdAt ?? input.printed_at,
          updated_at: input.printed_at,
        };
        created.push(attempt);
        return attempt;
      },
      importPrintAttempts: async (modelId, attempts) => {
        for (const item of attempts) {
          created.push({
            id: "created-" + created.length,
            model_id: modelId,
            ...item.input,
            printer_id: item.printerId,
            printer_name: item.printerName,
            created_at: item.createdAt ?? item.input.printed_at,
            updated_at: item.input.printed_at,
          });
        }
        return {
          addedCount: attempts.length,
          skippedCount: 0,
          metadataVersion: model.metadata_version + 1,
        };
      },
    };
    const entries = [{ fileName: "gear.stl", printEvents: [importedEvent] }];
    const assignments = [{ entryIndex: 0, targetModelId: "target" }];
    const printerIdByExternalRef = new Map([["foreign-printer-1", "local-printer-1"]]);

    const first = await applyPrintventoryAssignments(
      entries,
      assignments,
      [model],
      [],
      [],
      api,
      printerIdByExternalRef,
    );
    const second = await applyPrintventoryAssignments(
      entries,
      assignments,
      [model],
      [],
      [],
      api,
      printerIdByExternalRef,
    );

    expect(created).toHaveLength(1);
    expect(created[0].printer_id).toBe("local-printer-1");
    expect(created[0].printer_name).toBe("Mapped printer snapshot");
    expect(first.rows[0].mappedFields).toContain("Print history");
    expect(second.updated).toBe(0);
  });

  test("preserves imported current print status after applying historical events", async () => {
    const model = libraryModel("target", { metadata_version: 4, print_status: "unprinted" });
    let currentStatus = "unprinted";
    let version = model.metadata_version;
    const entry = {
      fileName: "gear.stl",
      print_status: "queued",
      printEvents: [
        {
          printed_at: "2026-10-10T00:00:00Z",
          outcome: "printed",
          quantity: 1,
          notes: "Earlier successful attempt",
          filaments: [],
        },
        {
          printed_at: "2026-10-11T00:00:00Z",
          outcome: "failed",
          quantity: 1,
          notes: "Later failed attempt",
          filaments: [],
        },
      ],
    };
    const result = await applyPrintventoryAssignments(
      [entry],
      [{ entryIndex: 0, targetModelId: "target" }],
      [model],
      [],
      [],
      {
        createTag: async () => {
          throw new Error("unexpected tag creation");
        },
        updateMetadata: async (_id, expectedVersion, patch) => {
          expect(expectedVersion).toBe(version);
          version += 1;
          currentStatus = patch.print_status ?? currentStatus;
          return { ...model, metadata_version: version, print_status: currentStatus };
        },
        setModelFilaments: async () => {
          throw new Error("unexpected filament assignment");
        },
        listPrintAttempts: async () => [],
        createPrintAttempt: async (modelId, input) => {
          version += 1;
          currentStatus = input.outcome;
          return {
            id: "created",
            model_id: modelId,
            ...input,
            created_at: input.printed_at,
            updated_at: input.printed_at,
          };
        },
        importPrintAttempt: async (modelId, input, createdAt, printerId, printerName) => {
          version += 1;
          currentStatus = input.outcome;
          return {
            id: "created",
            model_id: modelId,
            ...input,
            printer_id: printerId,
            printer_name: printerName,
            created_at: createdAt ?? input.printed_at,
            updated_at: input.printed_at,
          };
        },
        importPrintAttempts: async (_modelId, attempts) => {
          version += 1;
          for (const item of attempts) currentStatus = item.input.outcome;
          return {
            addedCount: attempts.length,
            skippedCount: 0,
            metadataVersion: version,
          };
        },
      },
    );

    expect(currentStatus).toBe("queued");
    expect(result.rows[0].mappedFields).toContain("Print status");
    expect(result.rows[0].mappedFields).toContain("Print history");
  });

  test("does not report print history as applied when loading existing history fails", async () => {
    const model = libraryModel("target");
    const result = await applyPrintventoryAssignments(
      [
        {
          fileName: "gear.stl",
          printEvents: [
            {
              printed_at: "2026-10-10T00:00:00Z",
              outcome: "printed",
              quantity: 1,
              notes: "Earlier attempt",
              filaments: [],
            },
          ],
        },
      ],
      [{ entryIndex: 0, targetModelId: "target" }],
      [model],
      [],
      [],
      {
        createTag: async () => {
          throw new Error("unexpected tag creation");
        },
        updateMetadata: async () => ({ ...model, metadata_version: 5 }),
        setModelFilaments: async () => {
          throw new Error("unexpected filament assignment");
        },
        listPrintAttempts: async () => {
          throw new Error("history unavailable");
        },
        createPrintAttempt: async () => {
          throw new Error("must not write without checking existing history");
        },
      },
    );

    expect(result.rows[0].status).toBe("failed");
    expect(result.rows[0].mappedFields).not.toContain("Print history");
  });

  test("creates one shared new local tag for multiple imported rows", async () => {
    const models = [libraryModel("first"), libraryModel("second")];
    let createCount = 0;
    let createdCategory = null;
    const updated = [];
    const result = await applyPrintventoryAssignments(
      [
        { fileName: "first.stl", gogglelabTags: [{ name: "shared", category: "Assembly" }] },
        { fileName: "second.stl", gogglelabTags: [{ name: "shared", category: "Hardware" }] },
      ],
      [
        { entryIndex: 0, targetModelId: "first" },
        { entryIndex: 1, targetModelId: "second" },
      ],
      models,
      [],
      [],
      {
        createTag: async (name, category) => {
          createCount += 1;
          createdCategory = category;
          return { id: "shared-tag", name, category, model_count: 0 };
        },
        updateMetadata: async (id, _version, patch) => {
          updated.push({ id, patch });
          return { ...models.find((model) => model.id === id), metadata_version: 5 };
        },
        setModelFilaments: async () => {
          throw new Error("unexpected filament assignment");
        },
      },
    );

    expect(createCount).toBe(1);
    expect(createdCategory).toBe("Assembly");
    expect(updated.map((item) => item.patch.tags)).toEqual([["shared-tag"], ["shared-tag"]]);
    expect(result.updated).toBe(2);
    expect(result.rows[1].warnings.join(" ")).toMatch(/existing global category will be retained/i);
  });

  test("refuses all writes when selected targets are duplicated", async () => {
    let writes = 0;
    await expect(
      applyPrintventoryAssignments(
        [printventoryFile(), printventoryFile()],
        [
          { entryIndex: 0, targetModelId: "target" },
          { entryIndex: 1, targetModelId: "target" },
        ],
        [libraryModel("target")],
        [],
        [],
        {
          createTag: async () => {
            writes += 1;
            throw new Error("must not write");
          },
          updateMetadata: async () => {
            writes += 1;
            throw new Error("must not write");
          },
          setModelFilaments: async () => {
            writes += 1;
            throw new Error("must not write");
          },
        },
      ),
    ).rejects.toThrow(/assigned more than once/i);
    expect(writes).toBe(0);
  });
});
