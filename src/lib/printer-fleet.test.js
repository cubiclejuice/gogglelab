import { expect, test } from "bun:test";
import {
  GENERIC_PRINTER_ID,
  MAX_DISCOVERY_ADDRESSES,
  MAX_DISCOVERY_SUGGESTIONS,
  MAX_MANAGED_PRINTERS,
  PrinterFleetState,
  connectionSnapshotCopy,
  connectionTestCopy,
  createMaintenanceLogDraft,
  createMaintenanceReminderDraft,
  createPrinterDraft,
  mapMaintenanceLog,
  mapMaintenanceLogDraftToInput,
  mapMaintenanceReminder,
  mapMaintenanceReminderDraftToInput,
  mapDiscoverySuggestion,
  mapLibraryPrinter,
  mapPrinterDraftToInput,
  mapReminderCompletion,
  mergeDiscoverySuggestions,
  resolveEndpointApproval,
  requiresElegooMainboardId,
  sanitizeConnectionTestStatus,
  validateDiscoveryRange,
  validateMaintenanceLogDraft,
  validateMaintenanceReminderDraft,
  validatePrinterDraft,
} from "./printer-fleet";

const printer = {
  id: "printer-1",
  metadataVersion: 2,
  name: "Workshop A1",
  make: "Maker",
  model: "A1 Mini",
  printerType: "fdm",
  firmware: null,
  webUrl: "http://192.168.50.10:8080",
  notes: "",
  bedMm: [180, 180, 180],
  connection: { kind: "network", address: "http://192.168.50.10:8080" },
  endpointApproved: true,
  connectionStatus: "unknown",
  jobStatus: "unknown",
  lastCheckedAt: null,
  connectionErrorCode: null,
  sourceKind: "manual",
  printCount: 0,
  lastPrintedAt: null,
  createdAt: "2026-10-01T00:00:00Z",
  updatedAt: "2026-10-01T00:00:00Z",
};

const slicerDetection = {
  id: "slicer-a1",
  name: "A1 Mini · Bambu Studio",
  model: "A1 Mini",
  bedMm: [180, 180, 180],
  source: "Bambu Studio",
};

const baseModel = {
  managedPrinters: [printer],
  slicerDetections: [slicerDetection],
  activePrinterId: printer.id,
  maintenanceLogs: [],
  maintenanceReminders: [],
};

function suggestion(overrides = {}) {
  return {
    key: "device-1",
    name: "Found printer",
    model: "Synthetic",
    bedMm: [220, 220, 250],
    connection: { kind: "network", address: "http://192.168.50.11:8080" },
    source: "moonraker",
    printerType: "FDM",
    ...overrides,
  };
}

function callbacks(overrides = {}) {
  return {
    savePrinter: async (draft) => ({
      ...printer,
      ...draft,
      id: draft.id ?? "printer-new",
      metadataVersion: draft.metadataVersion ?? 1,
    }),
    removePrinter: async () => {},
    setDefaultPrinter: async () => {},
    discoverPrinters: async () => [],
    cancelDiscovery: async () => {},
    addDiscoveredPrinters: async (items) =>
      items.map(({ draft, endpointApproved }, index) => ({
        ...printer,
        ...draft,
        endpointApproved,
        id: `discovered-${index}`,
        metadataVersion: 1,
      })),
    testConnection: async () => ({ result: "connected", printer }),
    saveMaintenanceLog: async (draft) => ({ ...draft, id: draft.id ?? "log-new" }),
    deleteMaintenanceLog: async () => {},
    saveMaintenanceReminder: async (draft) => ({
      ...draft,
      id: draft.id ?? "reminder-new",
      metadataVersion: draft.metadataVersion ?? 1,
      status: "open",
      completedAt: null,
      printerNickname: printer.name,
      lastCompletedAt: null,
      createdAt: "2026-10-01T00:00:00Z",
      updatedAt: "2026-10-01T00:00:00Z",
    }),
    completeMaintenanceReminder: async (id) => ({
      reminder: {
        id,
        metadataVersion: 2,
        printerId: printer.id,
        printerNickname: printer.name,
        task: "Clean nozzle",
        notes: "",
        dueAt: "2026-10-20",
        repeatEveryDays: null,
        status: "completed",
        completedAt: "2026-10-14",
        lastCompletedAt: "2026-10-14",
        createdAt: "2026-10-01T00:00:00Z",
        updatedAt: "2026-10-14T00:00:00Z",
      },
      maintenanceLog: {
        id: "completion-log",
        metadataVersion: 1,
        printerId: printer.id,
        task: "Clean nozzle",
        performedAt: "2026-10-14",
        notes: "",
      },
    }),
    deleteMaintenanceReminder: async () => {},
    ...overrides,
  };
}

test("catalog printer mapping keeps the persisted snapshot and redacts endpoint credentials", () => {
  const mapped = mapLibraryPrinter({
    id: "catalog-1",
    metadataVersion: 7,
    nickname: "Workshop",
    make: "Maker",
    model: "A1",
    printerType: "fdm",
    firmware: null,
    host: "http://user:secret@192.168.50.12:8080",
    webUrl: "http://user:secret@192.168.50.12:8080",
    endpointApproved: false,
    bedMm: [256, 256, 300],
    notes: "synthetic",
    sourceKind: "discovered",
    connectionStatus: "offline",
    jobStatus: "paused",
    lastCheckedAt: "2026-10-14T10:00:00Z",
    connectionErrorCode: "refused",
    printCount: 4,
    lastPrintedAt: null,
    createdAt: "2026-10-01T00:00:00Z",
    updatedAt: "2026-10-14T10:00:00Z",
  });

  expect(mapped).toMatchObject({
    id: "catalog-1",
    metadataVersion: 7,
    connectionStatus: "offline",
    jobStatus: "paused",
    lastCheckedAt: "2026-10-14T10:00:00Z",
    endpointApproved: false,
    connection: { kind: "network", address: "http://192.168.50.12:8080" },
  });
  expect(mapped.connection.address).not.toContain("secret");
});

test("printer input mapping keeps only endpoint locations and no URL credentials", () => {
  const draft = createPrinterDraft(printer);
  draft.connection = { kind: "network", address: "http://user:secret@192.168.50.20:8080" };
  const input = mapPrinterDraftToInput(draft);

  expect(input).toEqual({
    nickname: "Workshop A1",
    make: "Maker",
    model: "A1 Mini",
    printerType: "fdm",
    firmware: null,
    host: "http://192.168.50.20:8080",
    webUrl: "http://192.168.50.20:8080",
    bedMm: [180, 180, 180],
    notes: "",
  });
  expect(JSON.stringify(input)).not.toContain("secret");
});

test("discovery DTO mapping retains allowlisted metadata and direct network address", () => {
  expect(mapDiscoverySuggestion(suggestion())).toEqual(suggestion());
});

test("editing starts from a detached draft, so cancel does not mutate the saved printer", () => {
  const draft = createPrinterDraft(printer);
  draft.name = "Uncommitted name";
  draft.bedMm[0] = 250;
  draft.connection.address = "http://temporary.local";

  expect(printer.name).toBe("Workshop A1");
  expect(printer.bedMm).toEqual([180, 180, 180]);
  expect(printer.connection.address).toBe("http://192.168.50.10:8080");
});

test("credential-bearing saved addresses are redacted before editing", () => {
  const imported = {
    ...printer,
    connection: { kind: "network", address: "http://user:secret@printer.local:8080" },
  };
  const draft = createPrinterDraft(imported);
  const state = new PrinterFleetState({ ...baseModel, managedPrinters: [imported] }, callbacks());

  expect(draft.connection.address).toBe("http://printer.local:8080");
  expect(state.model.managedPrinters[0].connection.address).toBe("http://printer.local:8080");
  expect(draft.connection.address).not.toContain("secret");
});

test("printer drafts validate required names, bed dimensions, and credential-free endpoints", () => {
  const valid = createPrinterDraft(printer);
  expect(validatePrinterDraft(valid)).toBeNull();
  expect(validatePrinterDraft({ ...valid, name: " " })).toContain("name");
  expect(validatePrinterDraft({ ...valid, bedMm: [180, 0, 180] })).toContain("bed");
  expect(
    validatePrinterDraft({
      ...valid,
      connection: { kind: "network", address: "http://user:secret@printer.local" },
    }),
  ).toContain("password");
});

test("only an unchanged approved address keeps approval; changed addresses require a new confirmation", () => {
  expect(
    resolveEndpointApproval(
      true,
      printer.connection.address,
      printer.connection.address,
      true,
      false,
    ),
  ).toBe(true);
  expect(
    resolveEndpointApproval(
      true,
      "http://192.168.50.21:8080",
      printer.connection.address,
      true,
      false,
    ),
  ).toBe(false);
  expect(
    resolveEndpointApproval(
      true,
      "http://192.168.50.21:8080",
      printer.connection.address,
      true,
      true,
    ),
  ).toBe(true);
  expect(resolveEndpointApproval(false, "", null, true, true)).toBe(false);
});

test("printer snapshot labels old and missing status as stale or unchecked", () => {
  const checked = {
    ...printer,
    lastCheckedAt: "2026-10-14T10:00:00Z",
    connectionStatus: "online",
    jobStatus: "idle",
  };
  expect(connectionSnapshotCopy(checked, Date.parse("2026-10-14T10:10:00Z"))).toContain(
    "Status snapshot: online",
  );
  expect(connectionSnapshotCopy(checked, Date.parse("2026-10-14T10:30:00Z"))).toContain(
    "Stale status snapshot",
  );
  expect(connectionSnapshotCopy({ ...printer, lastCheckedAt: null })).toContain("not checked yet");
});

test("discovery accepts bounded private IPv4 literals, CIDRs, ranges, and comma terms", () => {
  expect(validateDiscoveryRange("192.168.50.12")).toBeNull();
  expect(validateDiscoveryRange("192.168.48.0/22")).toBeNull();
  expect(validateDiscoveryRange("10.0.0.1-10.0.0.10")).toBeNull();
  expect(validateDiscoveryRange("192.168.50.0/24, 10.0.0.12-10.0.0.20")).toBeNull();
  expect(MAX_DISCOVERY_ADDRESSES).toBe(1_022);
  expect(validateDiscoveryRange("192.168.48.0/21")).toContain("1,022");
  expect(validateDiscoveryRange("0.0.0.0/0")).toContain("private IPv4");
  expect(validateDiscoveryRange("8.8.8.0/24")).toContain("private IPv4");
  expect(validateDiscoveryRange("printer.local")).toContain("valid private IPv4");
  expect(validateDiscoveryRange("192.168.1.10-192.168.1.1")).toContain("valid private IPv4");
});

test("discovery results deduplicate stable keys and stay within the review limit", () => {
  const candidates = Array.from({ length: MAX_DISCOVERY_SUGGESTIONS + 2 }, (_, index) =>
    suggestion({
      key: `device-${index}`,
      name: `Printer ${index}`,
      connection: { kind: "network", address: `http://192.168.50.${index + 20}` },
    }),
  );
  const reviewed = mergeDiscoverySuggestions([
    ...candidates,
    { ...candidates[0], name: "Duplicate" },
  ]);

  expect(reviewed).toHaveLength(MAX_DISCOVERY_SUGGESTIONS);
  expect(new Set(reviewed.map((candidate) => candidate.key)).size).toBe(reviewed.length);
});

test("discovery hides endpoints already present in the managed fleet", () => {
  const duplicate = suggestion({
    key: "same-device",
    connection: { kind: "network", address: "http://192.168.50.10:8080/" },
  });
  expect(mergeDiscoverySuggestions([duplicate], [printer])).toEqual([]);
});

test("discovery review rejects candidate addresses outside the visible range", () => {
  const inRange = suggestion({
    connection: { kind: "network", address: "http://192.168.50.10:8080" },
  });
  const outOfRange = suggestion({
    key: "out-of-range",
    connection: { kind: "network", address: "http://192.168.51.10:8080" },
  });

  expect(mergeDiscoverySuggestions([inRange, outOfRange], [], "192.168.50.0/24")).toEqual([
    inRange,
  ]);
  expect(
    mergeDiscoverySuggestions(
      [suggestion({ connection: { kind: "network", address: "http://10.0.0.6" } })],
      [],
      "192.168.50.0/24, 10.0.0.1-10.0.0.10",
    ),
  ).toHaveLength(1);
});

test("constructing fleet state performs no discovery or connection requests", () => {
  let discoveries = 0;
  let connections = 0;
  new PrinterFleetState(
    baseModel,
    callbacks({
      discoverPrinters: async () => {
        discoveries += 1;
        return [];
      },
      testConnection: async () => {
        connections += 1;
        return { result: "connected", printer };
      },
    }),
  );

  expect(discoveries).toBe(0);
  expect(connections).toBe(0);
});

test("discovery runs only after an explicit valid request and leaves suggestions unsaved", async () => {
  let saved = false;
  let scannedRange = null;
  const state = new PrinterFleetState(
    baseModel,
    callbacks({
      discoverPrinters: async (range) => {
        scannedRange = range;
        return [suggestion()];
      },
      addDiscoveredPrinters: async () => {
        saved = true;
        return [];
      },
    }),
  );

  expect(await state.discover("10.0.0.0/16")).toBe(false);
  expect(scannedRange).toBeNull();
  expect(await state.discover("192.168.50.0/24")).toBe(true);
  expect(scannedRange).toBe("192.168.50.0/24");
  expect(state.model.managedPrinters).toHaveLength(1);
  expect(state.suggestions).toHaveLength(1);
  expect(saved).toBe(false);
});

test("discovered endpoints remain unapproved unless the review checkbox is explicitly checked", async () => {
  const added = [];
  const state = new PrinterFleetState(
    { ...baseModel, managedPrinters: [] },
    callbacks({
      discoverPrinters: async () => [suggestion()],
      addDiscoveredPrinters: async (items) => {
        added.push(...items);
        return items.map(({ draft, endpointApproved }, index) => ({
          ...printer,
          ...draft,
          endpointApproved,
          id: `discovered-${index}`,
          metadataVersion: 1,
        }));
      },
    }),
  );
  await state.discover("192.168.50.0/24");
  expect(added).toHaveLength(0);

  state.selectSuggestion("device-1", true);
  expect(await state.addSelectedSuggestions()).toBe(true);
  expect(added[0].endpointApproved).toBe(false);
  expect(added[0].draft.connection.address).toBe("http://192.168.50.11:8080");
  expect(added[0].draft.make).toBe("Klipper");
  expect(added[0].draft.firmware).toBe("Klipper");
  expect(state.model.managedPrinters.map((item) => item.id)).toEqual(["discovered-0"]);

  state.callbacks.discoverPrinters = async () => [
    suggestion({
      key: "device-2",
      connection: { kind: "network", address: "http://192.168.50.12:8080" },
    }),
  ];
  await state.discover("192.168.50.0/24");
  state.selectSuggestion("device-2", true);
  expect(await state.addSelectedSuggestions(true)).toBe(true);
  expect(added[1].endpointApproved).toBe(true);
});

test("allowlisted discovery sources map to their supported make and firmware labels", async () => {
  const savedInputs = [];
  const candidates = [
    suggestion({
      key: "moonraker-1",
      source: "moonraker",
      connection: { kind: "network", address: "http://192.168.50.21" },
    }),
    suggestion({
      key: "prusa-1",
      source: "prusalink",
      connection: { kind: "network", address: "http://192.168.50.22" },
    }),
    suggestion({
      key: "bambu-1",
      source: "bambu_mqtt_tls",
      connection: { kind: "network", address: "http://192.168.50.23" },
    }),
  ];
  const state = new PrinterFleetState(
    { ...baseModel, managedPrinters: [] },
    callbacks({
      discoverPrinters: async () => candidates,
      addDiscoveredPrinters: async (items) => {
        savedInputs.push(...items);
        return items.map(({ draft }, index) => ({ ...printer, ...draft, id: `found-${index}` }));
      },
    }),
  );
  await state.discover("192.168.50.0/24");
  for (const item of candidates) state.selectSuggestion(item.key, true);
  expect(await state.addSelectedSuggestions()).toBe(true);
  expect(savedInputs.map(({ draft }) => [draft.make, draft.firmware])).toEqual([
    ["Klipper", "Klipper"],
    ["Prusa", "PrusaLink"],
    ["Bambu Lab", "Bambu OS"],
  ]);
});

test("failed discovery leaves previous suggestions available and does not expose raw network errors", async () => {
  const state = new PrinterFleetState(
    baseModel,
    callbacks({ discoverPrinters: async () => [suggestion()] }),
  );
  await state.discover("192.168.50.0/24");
  state.selectSuggestion("device-1", true);
  state.callbacks.discoverPrinters = async () => {
    throw new Error("192.168.50.7:8080 token=hidden connection refused");
  };

  expect(await state.discover("192.168.50.0/24")).toBe(false);
  expect(state.suggestions).toHaveLength(1);
  expect(state.selectedSuggestions).toEqual(["device-1"]);
  expect(state.error).not.toContain("192.168.50.7");
  expect(state.error).not.toContain("token");
});

test("cancellation runs only after explicit discovery and preserves the previous review", async () => {
  let cancellations = 0;
  let resolveSearch;
  const state = new PrinterFleetState(
    baseModel,
    callbacks({
      discoverPrinters: async () => [suggestion()],
      cancelDiscovery: async () => {
        cancellations += 1;
      },
    }),
  );

  expect(cancellations).toBe(0);
  expect(await state.discover("192.168.50.0/24")).toBe(true);
  state.selectSuggestion("device-1", true);
  const previousSuggestions = state.suggestions;
  const previousSelection = state.selectedSuggestions;
  state.callbacks.discoverPrinters = () =>
    new Promise((resolve) => {
      resolveSearch = resolve;
    });

  const search = state.discover("192.168.50.0/24");
  expect(state.discovering).toBe(true);
  expect(await state.cancelDiscovery()).toBe(true);
  expect(cancellations).toBe(1);
  resolveSearch([suggestion({ key: "new-device" })]);

  expect(await search).toBe(false);
  expect(state.suggestions).toEqual(previousSuggestions);
  expect(state.selectedSuggestions).toEqual(previousSelection);
  expect(state.discovering).toBe(false);
  expect(state.status).toContain("cancelled");
});

test("connection test updates the persisted printer snapshot and renders only fixed result copy", async () => {
  const updated = {
    ...printer,
    connectionStatus: "offline",
    jobStatus: "paused",
    lastCheckedAt: "2026-10-14T10:00:00Z",
    connectionErrorCode: "secret endpoint response",
  };
  let forwardedMainboardId;
  let forwardedDiscoveryFlag;
  const state = new PrinterFleetState(
    baseModel,
    callbacks({
      testConnection: async (_id, mainboardId, discoverMainboardId) => {
        forwardedMainboardId = mainboardId;
        forwardedDiscoveryFlag = discoverMainboardId;
        return { result: "failed", printer: updated };
      },
    }),
  );
  expect(await state.testConnection(printer.id, "AB12CD34EF56")).toBe(false);
  expect(forwardedMainboardId).toBe("AB12CD34EF56");
  expect(forwardedDiscoveryFlag).toBe(false);
  expect(connectionTestCopy(state.connectionTests.get(printer.id))).toBe(
    "Connection test failed. Check the printer details and try again.",
  );
  expect(state.model.managedPrinters[0]).toMatchObject({
    connectionStatus: "offline",
    jobStatus: "paused",
    lastCheckedAt: "2026-10-14T10:00:00Z",
  });
  expect(state.status).not.toContain("secret endpoint response");
  expect(sanitizeConnectionTestStatus({ result: "failed", message: "token=hidden" })).toBe(
    "failed",
  );
  expect(sanitizeConnectionTestStatus({ result: "surprise", message: "connected" })).toBe("failed");
  expect(connectionTestCopy("authRequired")).toContain("did not collect or store credentials");
});

test("connection test forwards explicit MainboardID discovery without persisting the ID", async () => {
  let forwarded;
  const state = new PrinterFleetState(
    baseModel,
    callbacks({
      testConnection: async (...args) => {
        forwarded = args;
        return { result: "connected", printer };
      },
    }),
  );

  expect(await state.testConnection(printer.id, undefined, true)).toBe(true);
  expect(forwarded).toEqual([printer.id, undefined, true]);
  expect(state.model.managedPrinters[0]).not.toHaveProperty("mainboardId");
});

test("connection test reports an unknown SDCP event without inventing a job state", async () => {
  const state = new PrinterFleetState(
    baseModel,
    callbacks({
      testConnection: async () => ({
        result: "connected",
        printer,
        statusEventObserved: true,
      }),
    }),
  );

  expect(await state.testConnection(printer.id)).toBe(true);
  expect(state.status).toBe("SDCP event observed during this test; reported job state: unknown.");
  expect(state.model.managedPrinters[0].jobStatus).toBe("unknown");
  expect(state.model.managedPrinters[0]).not.toHaveProperty("mainboardId");
});

test("Community connection check shows only the matching printer's reported job snapshot", async () => {
  for (const jobStatus of ["idle", "printing", "paused", "complete"]) {
    const state = new PrinterFleetState(
      baseModel,
      callbacks({
        testConnection: async () => ({
          result: "connected",
          printer: { ...printer, jobStatus },
          statusEventObserved: true,
        }),
      }),
    );
    expect(await state.testConnection(printer.id)).toBe(true);
    expect(state.status).toBe(
      `SDCP event observed during this test; reported job state: ${jobStatus}.`,
    );
    expect(state.model.managedPrinters[0].jobStatus).toBe(jobStatus);
  }
  const wrongPrinter = new PrinterFleetState(
    baseModel,
    callbacks({
      testConnection: async () => ({
        result: "connected",
        printer: { ...printer, id: "different-printer", jobStatus: "printing" },
        statusEventObserved: true,
      }),
    }),
  );
  await wrongPrinter.testConnection(printer.id);
  expect(wrongPrinter.model.managedPrinters[0].jobStatus).toBe("unknown");
  expect(wrongPrinter.status).toBe(
    "SDCP event observed during this test; reported job state: unknown.",
  );
});

test("connection acknowledgement without a status event keeps the generic connected copy", async () => {
  const state = new PrinterFleetState(
    baseModel,
    callbacks({
      testConnection: async () => ({
        result: "connected",
        printer,
        statusEventObserved: false,
      }),
    }),
  );

  expect(await state.testConnection(printer.id)).toBe(true);
  expect(state.status).toBe(connectionTestCopy("connected"));
  expect(state.model.managedPrinters[0].jobStatus).toBe("unknown");
});

test("native discovery cancellation preserves the saved printer snapshot", async () => {
  const state = new PrinterFleetState(
    baseModel,
    callbacks({
      testConnection: async () => ({
        result: "cancelled",
        printer: { ...printer, connectionStatus: "online" },
      }),
    }),
  );

  expect(await state.testConnection(printer.id, undefined, true)).toBe(false);
  expect(state.model.managedPrinters[0]).toEqual(printer);
  expect(state.status).toBe("Connection test cancelled.");
});

test("only the original Elegoo Centauri Carbon asks for an ephemeral MainboardID", () => {
  expect(requiresElegooMainboardId({ make: "Elegoo", model: "Centauri Carbon" })).toBe(true);
  expect(requiresElegooMainboardId({ make: "Elegoo", model: "Centauri Carbon 2" })).toBe(false);
  expect(requiresElegooMainboardId({ make: "Bambu Lab", model: "Centauri Carbon" })).toBe(false);
});

test("failed printer save retains draft and does not overwrite the saved model", async () => {
  const state = new PrinterFleetState(
    baseModel,
    callbacks({
      savePrinter: async () => {
        throw new Error(
          "settings read-only; token=hidden endpoint http://user:secret@printer.local",
        );
      },
    }),
  );
  const draft = createPrinterDraft(printer);
  draft.name = "Retry this name";

  expect(await state.savePrinter(draft)).toBe(false);
  expect(state.model.managedPrinters[0].name).toBe("Workshop A1");
  expect(draft.name).toBe("Retry this name");
  expect(state.error).toContain("settings read-only");
  expect(state.error).not.toContain("hidden");
  expect(state.error).not.toContain("secret");
});

test("removing the active printer switches to Generic 256 only after successful removal", async () => {
  const state = new PrinterFleetState(baseModel, callbacks());
  state.callbacks.removePrinter = async () => {
    throw new Error("remove failed");
  };
  expect(await state.removePrinter(printer.id)).toBe(false);
  expect(state.model.activePrinterId).toBe(printer.id);
  expect(state.model.managedPrinters).toHaveLength(1);

  state.callbacks.removePrinter = async () => {};
  expect(await state.removePrinter(printer.id)).toBe(true);
  expect(state.model.activePrinterId).toBe(GENERIC_PRINTER_ID);
  expect(state.model.managedPrinters).toHaveLength(0);
});

test("active selection accepts managed, detected, and Generic 256 printers", async () => {
  const requested = [];
  const state = new PrinterFleetState(
    baseModel,
    callbacks({ setDefaultPrinter: async (id) => requested.push(id) }),
  );

  expect(await state.setDefaultPrinter("slicer-a1")).toBe(true);
  expect(await state.setDefaultPrinter(GENERIC_PRINTER_ID)).toBe(true);
  expect(await state.setDefaultPrinter("unknown-id")).toBe(false);
  expect(requested).toEqual(["slicer-a1", GENERIC_PRINTER_ID]);
  expect(state.model.activePrinterId).toBe(GENERIC_PRINTER_ID);
});

test("maintenance drafts validate and reminder completion records the generated maintenance log", async () => {
  const log = {
    id: null,
    metadataVersion: null,
    printerId: printer.id,
    task: "Clean nozzle",
    performedAt: "2026-10-14",
    notes: "Synthetic test",
  };
  const reminder = {
    id: null,
    printerId: printer.id,
    task: "Lubricate rails",
    dueAt: "2026-10-20",
    repeatEveryDays: 30,
    metadataVersion: null,
    notes: "Synthetic reminder",
  };
  expect(validateMaintenanceLogDraft(log)).toBeNull();
  expect(validateMaintenanceLogDraft({ ...log, performedAt: "not-a-date" })).toContain("date");
  expect(validateMaintenanceLogDraft({ ...log, id: "stale-log", metadataVersion: null })).toContain(
    "Reload",
  );
  expect(validateMaintenanceReminderDraft(reminder)).toBeNull();
  expect(validateMaintenanceReminderDraft({ ...reminder, repeatEveryDays: -1 })).toContain(
    "Repeat",
  );

  const savedReminder = {
    ...reminder,
    id: "reminder-1",
    metadataVersion: 1,
    status: "open",
    completedAt: null,
    printerNickname: printer.name,
    lastCompletedAt: null,
    createdAt: "2026-10-01T00:00:00Z",
    updatedAt: "2026-10-01T00:00:00Z",
  };
  const state = new PrinterFleetState(
    { ...baseModel, maintenanceReminders: [savedReminder] },
    callbacks(),
  );
  expect(createMaintenanceReminderDraft(printer.id, savedReminder).metadataVersion).toBe(1);
  expect(await state.completeMaintenanceReminder(savedReminder.id)).toBe(true);
  expect(state.model.maintenanceReminders[0].status).toBe("completed");
  expect(state.model.maintenanceLogs[0].task).toBe("Clean nozzle");
});

test("maintenance edits and deletes carry the last-read version", async () => {
  const log = {
    id: "log-versioned",
    metadataVersion: 7,
    printerId: printer.id,
    task: "Clean nozzle",
    performedAt: "2026-10-14",
    notes: "",
  };
  let savedRequest;
  let deletedRequest;
  const state = new PrinterFleetState(
    { ...baseModel, maintenanceLogs: [log] },
    callbacks({
      saveMaintenanceLog: async (draft) => {
        savedRequest = { id: draft.id, expectedVersion: draft.metadataVersion };
        return { ...draft, metadataVersion: 8 };
      },
      deleteMaintenanceLog: async (id, expectedVersion) => {
        deletedRequest = { id, expectedVersion };
      },
    }),
  );

  const draft = createMaintenanceLogDraft(printer.id, log);
  expect(await state.saveMaintenanceLog({ ...draft, task: "Replace nozzle" })).toBe(true);
  expect(savedRequest).toEqual({ id: log.id, expectedVersion: 7 });
  expect(await state.deleteMaintenanceLog(log.id, log.metadataVersion)).toBe(true);
  expect(deletedRequest).toEqual({ id: log.id, expectedVersion: 7 });
});

test("maintenance dates round-trip between UTC API timestamps and date controls", () => {
  const log = {
    id: "log-1",
    metadataVersion: 3,
    printerId: printer.id,
    title: "Clean nozzle",
    performedAt: "2026-10-14T00:00:00.000Z",
    notes: "",
  };
  const reminder = {
    id: "reminder-1",
    metadataVersion: 1,
    printerId: printer.id,
    printerNickname: printer.name,
    title: "Lubricate rails",
    notes: "",
    dueAt: "2026-10-20T00:00:00.000Z",
    intervalDays: 30,
    status: "open",
    lastCompletedAt: "2026-10-14T00:00:00.000Z",
    createdAt: "2026-10-01T00:00:00.000Z",
    updatedAt: "2026-10-14T00:00:00.000Z",
  };

  const mappedLog = mapMaintenanceLog(log);
  expect(mappedLog.performedAt).toBe("2026-10-14");
  expect(mappedLog.metadataVersion).toBe(3);
  expect(createMaintenanceLogDraft(printer.id, mappedLog).metadataVersion).toBe(3);
  expect(mapMaintenanceReminder(reminder)).toMatchObject({
    dueAt: "2026-10-20",
    completedAt: "2026-10-14",
    lastCompletedAt: "2026-10-14",
  });
  expect(
    mapMaintenanceLogDraftToInput({
      printerId: printer.id,
      task: "Clean nozzle",
      performedAt: "2026-10-14",
      notes: "",
    }).performedAt,
  ).toBe("2026-10-14T00:00:00.000Z");
  expect(
    mapMaintenanceReminderDraftToInput({
      printerId: printer.id,
      task: "Lubricate rails",
      dueAt: "2026-10-20",
      repeatEveryDays: 30,
      notes: "",
    }).dueAt,
  ).toBe("2026-10-20T00:00:00.000Z");
});

test("completion response mapping keeps the reminder and generated log DTOs", () => {
  const mapped = mapReminderCompletion({
    reminder: {
      id: "reminder-1",
      metadataVersion: 2,
      printerId: printer.id,
      printerNickname: printer.name,
      title: "Clean nozzle",
      notes: "",
      dueAt: "2026-10-14",
      intervalDays: null,
      status: "completed",
      lastCompletedAt: "2026-10-14",
      createdAt: "2026-10-01T00:00:00Z",
      updatedAt: "2026-10-14T00:00:00Z",
    },
    maintenanceLog: {
      id: "log-1",
      metadataVersion: 1,
      printerId: printer.id,
      printerNickname: printer.name,
      title: "Clean nozzle",
      performedAt: "2026-10-14",
      notes: "",
      createdAt: "2026-10-14T00:00:00Z",
    },
  });
  expect(mapped.reminder.status).toBe("completed");
  expect(mapped.maintenanceLog.task).toBe("Clean nozzle");
});

test("managed-printer creation is bounded", async () => {
  const many = Array.from({ length: MAX_MANAGED_PRINTERS }, (_, index) => ({
    ...printer,
    id: `printer-${index}`,
  }));
  const state = new PrinterFleetState({ ...baseModel, managedPrinters: many }, callbacks());
  expect(
    await state.savePrinter({ ...createPrinterDraft(), name: "One more", bedMm: [100, 100, 100] }),
  ).toBe(false);
  expect(state.model.managedPrinters).toHaveLength(MAX_MANAGED_PRINTERS);
});

test("managed-printer creation supports the catalog beyond the former 100-printer UI cap", async () => {
  const many = Array.from({ length: 101 }, (_, index) => ({
    ...printer,
    id: `legacy-printer-${index}`,
  }));
  const state = new PrinterFleetState({ ...baseModel, managedPrinters: many }, callbacks());

  expect(
    await state.savePrinter({
      ...createPrinterDraft(),
      name: "Printer 102",
      bedMm: [100, 100, 100],
    }),
  ).toBe(true);
  expect(state.model.managedPrinters).toHaveLength(102);
});
