import { expect, test } from "bun:test";
import {
  KEEP_HISTORICAL_PRINTER,
  calculatePartUsage,
  describePartStockError,
  initialPrinterSelection,
  partStockValidationError,
  printerIdChangeForAttempt,
  printAttemptSummary,
  validatePrintAttempt,
} from "./library-history";

const printers = [
  { id: "printer-a", name: "Workshop A1" },
  { id: "printer-b", name: "Workshop P1S" },
];

test("new print attempts select the active managed printer only when it is available", () => {
  expect(initialPrinterSelection(printers, "printer-b", null)).toBe("printer-b");
  expect(initialPrinterSelection(printers, "slicer-preset", null)).toBe("");
});

test("editing preserves an existing printer attribution unless the user changes it", () => {
  const attempt = { printer_id: "printer-a", printer_name: "Old name" };
  expect(initialPrinterSelection(printers, "printer-b", attempt)).toBe("printer-a");
  expect(printerIdChangeForAttempt("printer-a", attempt)).toBeUndefined();
  expect(printerIdChangeForAttempt("printer-b", attempt)).toBe("printer-b");
  expect(printerIdChangeForAttempt("", attempt)).toBeNull();
});

test("removed printer snapshots stay attached until the user explicitly clears them", () => {
  const attempt = { printer_id: null, printer_name: "Retired Workshop" };
  expect(initialPrinterSelection([], null, attempt)).toBe(KEEP_HISTORICAL_PRINTER);
  expect(printerIdChangeForAttempt(KEEP_HISTORICAL_PRINTER, attempt)).toBeUndefined();
  expect(printerIdChangeForAttempt("", attempt)).toBeNull();
  expect(printerIdChangeForAttempt("printer-b", attempt)).toBe("printer-b");
});

test("print history summary retains the historical printer name", () => {
  expect(
    printAttemptSummary({
      id: "attempt",
      model_id: "model",
      printed_at: "2026-10-10T00:00:00.000Z",
      quantity: 2,
      notes: "",
      filament_ids: [],
      outcome: "printed",
      printer_id: null,
      printer_name: "Retired Workshop",
      created_at: "",
      updated_at: "",
    }),
  ).toBe("2026-10-10 · 2 · printed · Retired Workshop");
});

test("print history summary retains total part use snapshots", () => {
  expect(
    printAttemptSummary({
      id: "attempt",
      model_id: "model",
      printed_at: "2026-10-10T00:00:00.000Z",
      quantity: 2,
      notes: "",
      filament_ids: [],
      outcome: "cancelled",
      parts: [
        {
          id: null,
          name: "M3 bolt",
          category: "Hardware",
          unit: "pcs",
          quantityPerCopy: 2,
          quantity: 4,
        },
      ],
      created_at: "",
      updated_at: "",
    }),
  ).toBe("2026-10-10 · 2 · cancelled · M3 bolt (Hardware) 4 pcs");
});

test("legacy part events warn that deletion will not change stock only when the marker is unknown", () => {
  const attempt = {
    id: "attempt",
    model_id: "model",
    printed_at: "2026-10-10T00:00:00.000Z",
    quantity: 1,
    notes: "",
    filament_ids: [],
    outcome: "printed",
    parts: [
      {
        id: null,
        name: "M3 bolt",
        category: "Hardware",
        unit: "pcs",
        quantityPerCopy: 1,
        quantity: 1,
      },
    ],
    created_at: "",
    updated_at: "",
  };

  expect(printAttemptSummary({ ...attempt, stock_adjusted: null })).toContain(
    "Stock impact unknown; deleting this event will leave stock unchanged",
  );
  expect(printAttemptSummary({ ...attempt, stock_adjusted: true })).not.toContain(
    "Stock impact unknown",
  );
  expect(printAttemptSummary({ ...attempt, stock_adjusted: false })).not.toContain(
    "Stock impact unknown",
  );
  expect(printAttemptSummary({ ...attempt, stock_adjusted: null, parts: [] })).not.toContain(
    "Stock impact unknown",
  );
});

test("part usage reports per-copy and total quantities against available stock", () => {
  expect(
    calculatePartUsage([{ id: "bolt", quantity: 3 }], 4, [
      {
        id: "bolt",
        name: "M3 bolt",
        category: "Hardware",
        quantity: 10,
        unit: "pcs",
        notes: "",
        low_stock: 2,
        metadata_version: 1,
      },
    ]),
  ).toEqual([
    {
      id: "bolt",
      name: "M3 bolt",
      category: "Hardware",
      unit: "pcs",
      quantityPerCopy: 3,
      quantity: 12,
      available: 10,
      sufficient: false,
    },
  ]);
});

test("print attempt validation accepts cancellation and bounds per-copy parts", () => {
  const valid = {
    printed_at: "2026-10-10",
    quantity: 2,
    notes: "",
    filament_ids: [],
    outcome: "cancelled",
    parts: [{ id: "bolt", quantity: 2 }],
  };
  expect(validatePrintAttempt(valid)).toBeNull();
  expect(validatePrintAttempt({ ...valid, parts: [{ id: "bolt", quantity: 0 }] })).toContain(
    "part",
  );
  expect(
    validatePrintAttempt({
      ...valid,
      parts: [
        { id: "bolt", quantity: 1 },
        { id: "bolt", quantity: 2 },
      ],
    }),
  ).toContain("once");
});

test("part stock editor validation enforces the core inventory limits", () => {
  expect(
    partStockValidationError({
      name: "M3 bolt",
      category: "Hardware",
      quantity: 10,
      unit: "pcs",
      notes: "",
      lowStock: 2,
    }),
  ).toBeNull();
  expect(
    partStockValidationError({
      name: "",
      category: null,
      quantity: 10,
      unit: "pcs",
      notes: "",
      lowStock: 2,
    }),
  ).toContain("name");
});

test("stock conflicts and insufficient inventory have actionable feedback", () => {
  expect(describePartStockError({ message: "insufficient stock for bolt" })).toMatch(
    /Not enough stock/i,
  );
  expect(describePartStockError({ message: "part stock version conflict" })).toMatch(
    /changed since it was loaded/i,
  );
});
