import { expect, test } from "bun:test";
import {
  PrinterDiscoveryReview,
  printerNetworkAvailable,
  carbonBroadcastNeeded,
} from "./browser-printer-network";
const plan = {
  planToken: "one-use",
  normalizedRange: "10.7.8.1-10.7.8.2",
  addressCount: 2,
  includeBambuDiscovery: false,
  multicastDisclosure: null,
  expiresAt: "2026-10-19T13:00:00Z",
};
const policy = {
  enabled: true,
  discoveryAvailable: true,
  statusChecksAvailable: true,
  multicastDiscoveryAllowed: false,
  carbonBroadcastAllowed: false,
  maxDiscoveryAddresses: 1022,
  maxSuggestions: 32,
  maximumDurationMs: 30000,
};
test("missing or disabled network policy never enables printer actions", () => {
  expect(printerNetworkAvailable(null)).toBe(false);
  expect(printerNetworkAvailable({ ...policy, enabled: false })).toBe(false);
  expect(printerNetworkAvailable(policy)).toBe(true);
});
test("scope or protocol edit discards review token and rejects stale plan", () => {
  const state = new PrinterDiscoveryReview();
  const revision = state.edit();
  expect(state.acceptPlan(revision, plan)).toBe(true);
  state.edit();
  expect(state.plan).toBe(null);
  expect(state.acceptPlan(revision, plan)).toBe(false);
});
test("review tokens are consumed once and expired reviews cannot start", () => {
  const state = new PrinterDiscoveryReview();
  state.acceptPlan(state.revision, plan);
  expect(state.consumePlan(Date.parse("2026-10-19T12:00:00Z"))).toEqual(plan);
  expect(() => state.consumePlan(Date.parse("2026-10-19T12:00:00Z"))).toThrow();
  state.acceptPlan(state.revision, plan);
  expect(() => state.consumePlan(Date.parse("2026-10-19T14:00:00Z"))).toThrow("expired");
  expect(state.plan).toBe(null);
});
test("job replies must match active owner handle and current dialog generation", () => {
  const state = new PrinterDiscoveryReview();
  const rev = state.revision;
  const job = {
    jobId: "opaque-a",
    status: "running",
    suggestions: [],
    normalizedRange: plan.normalizedRange,
    errorCode: null,
  };
  expect(state.acceptJob(rev, job)).toBe(true);
  expect(state.acceptJob(rev, { ...job, jobId: "opaque-b" })).toBe(false);
  state.edit();
  expect(state.acceptJob(rev, { ...job, status: "completed" })).toBe(false);
  expect(state.job).toBe(null);
});
test("Carbon broadcast is offered only for original Carbon model", () => {
  expect(carbonBroadcastNeeded({ make: "Elegoo", model: "Centauri Carbon" })).toBe(true);
  expect(carbonBroadcastNeeded({ make: "Elegoo", model: "Centauri Carbon 2" })).toBe(false);
  expect(carbonBroadcastNeeded({ make: null, model: null })).toBe(false);
});

// Small DOM harness for event/state contracts; real layout/focus QA remains browser-owned.
class FakeNode {
  children = [];
  listeners = new Map();
  textContent = "";
  value = "";
  checked = false;
  disabled = false;
  hidden = false;
  append(...items) {
    this.children.push(...items);
  }
  replaceChildren(...items) {
    this.children = [...items];
  }
  setAttribute() {}
  addEventListener(type, handler) {
    this.listeners.set(type, handler);
  }
  fire(type) {
    if (type === "click" && this.disabled) return;
    this.listeners.get(type)?.({});
  }
  all() {
    return [this, ...this.children.flatMap((child) => child.all())];
  }
}
async function fakeDom(run) {
  const previousDocument = globalThis.document;
  const previousWindow = globalThis.window;
  const timers = [];
  globalThis.document = {
    createElement: () => new FakeNode(),
    createTextNode: (value) => {
      const node = new FakeNode();
      node.textContent = value;
      return node;
    },
  };
  globalThis.window = {
    setTimeout: (fn) => {
      timers.push(fn);
      return timers.length;
    },
    clearTimeout: () => {},
    confirm: () => true,
  };
  try {
    await run(timers);
  } finally {
    globalThis.document = previousDocument;
    globalThis.window = previousWindow;
  }
}
const flush = async () => {
  for (let i = 0; i < 8; i++) await Promise.resolve();
};
const find = (network, label) => network.element.all().find((node) => node.textContent === label);

test("network initialization and disabled policy perform metadata reads only", async () => {
  await fakeDom(async () => {
    const { BrowserPrinterNetwork } = await import("./browser-printer-network");
    const calls = [];
    const client = {
      getPrinterNetworkPolicy: async () => {
        calls.push("policy");
        return { ...policy, enabled: false };
      },
      planPrinterDiscovery: async () => {
        throw new Error("must not plan");
      },
      testPrinterConnection: async () => {
        throw new Error("must not test");
      },
    };
    const unavailable = new BrowserPrinterNetwork(
      client,
      async () => {},
      () => {},
    );
    await unavailable.loadPolicy(false);
    expect(calls).toEqual([]);
    const network = new BrowserPrinterNetwork(
      client,
      async () => {},
      () => {},
    );
    await network.loadPolicy(true);
    expect(calls).toEqual(["policy"]);
    expect(find(network, "Review discovery scope").disabled).toBe(true);
    expect(network.printerActions({ host: "10.7.8.1" }).children).toEqual([]);
    network.dismiss();
  });
});

test("only explicit reviewed start creates a job and closed dialog drops late results", async () => {
  await fakeDom(async () => {
    const { BrowserPrinterNetwork } = await import("./browser-printer-network");
    const calls = [];
    let finish;
    const client = {
      getPrinterNetworkPolicy: async () => policy,
      planPrinterDiscovery: async (range, bambu) => {
        calls.push(["plan", range, bambu]);
        return { ...plan, expiresAt: new Date(Date.now() + 60000).toISOString() };
      },
      startPrinterDiscovery: async (token, confirm) => {
        calls.push(["start", token, confirm]);
        return await new Promise((resolve) => {
          finish = resolve;
        });
      },
      cancelPrinterDiscovery: async (id) => {
        calls.push(["cancel", id]);
        return {
          jobId: id,
          status: "cancelled",
          suggestions: [],
          normalizedRange: plan.normalizedRange,
          errorCode: null,
        };
      },
    };
    const network = new BrowserPrinterNetwork(
      client,
      async () => {},
      () => {},
    );
    await network.loadPolicy(true);
    const range = network.element.all().find((node) => node.placeholder);
    range.value = "10.7.8.1-10.7.8.2";
    range.fire("input");
    expect(calls).toEqual([]);
    find(network, "Review discovery scope").fire("click");
    await flush();
    expect(calls).toEqual([["plan", "10.7.8.1-10.7.8.2", false]]);
    expect(find(network, "Start reviewed discovery").hidden).toBe(false);
    find(network, "Start reviewed discovery").fire("click");
    await flush();
    expect(calls.at(-1)).toEqual(["start", "one-use", true]);
    network.dismiss();
    finish({
      jobId: "late-job",
      status: "running",
      suggestions: [],
      normalizedRange: plan.normalizedRange,
      errorCode: null,
    });
    await flush();
    expect(calls.at(-1)).toEqual(["cancel", "late-job"]);
  });
});

test("Carbon status needs explicit broadcast choice and keeps saved version exact", async () => {
  await fakeDom(async () => {
    const { BrowserPrinterNetwork } = await import("./browser-printer-network");
    const calls = [];
    const printer = {
      id: "printer",
      metadataVersion: "18446744073709551614",
      make: "Elegoo",
      model: "Centauri Carbon",
      nickname: "Workshop",
      host: "10.7.8.1",
      webUrl: null,
      endpointApproved: true,
      jobStatus: "idle",
    };
    const client = {
      getPrinterNetworkPolicy: async () => ({ ...policy, carbonBroadcastAllowed: true }),
      testPrinterConnection: async (...args) => {
        calls.push(args);
        return { result: "connected", printer, statusEventObserved: true };
      },
    };
    const network = new BrowserPrinterNetwork(
      client,
      async () => {},
      () => {},
    );
    await network.loadPolicy(true);
    const group = network.printerActions(printer);
    const check = group.all().find((node) => node.textContent === "Check saved printer status");
    expect(check.disabled).toBe(true);
    check.fire("click");
    await flush();
    expect(calls).toEqual([]);
    const consent = group.all().find((node) => node.type === "checkbox");
    consent.checked = true;
    consent.fire("change");
    expect(check.disabled).toBe(false);
    check.fire("click");
    await flush();
    expect(calls).toEqual([["printer", "18446744073709551614", true, true]]);
    network.dismiss();
  });
});
