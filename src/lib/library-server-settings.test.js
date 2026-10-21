import { expect, test } from "bun:test";
import {
  libraryServerExternalIntegrationChoice,
  libraryServerStatusCopy,
  shouldRevealAccessKey,
} from "./library-server-settings";

test("desktop server settings show external integration access off by default", () => {
  expect(
    libraryServerStatusCopy({
      running: false,
      baseUrl: null,
      mcpUrl: null,
      allowExternalIntegrations: false,
    }),
  ).toEqual({
    state: "Stopped",
    address: "Not accepting connections",
    action: "Start server",
    externalIntegrations: "Off",
  });
  expect(
    libraryServerStatusCopy({
      running: true,
      baseUrl: "http://127.0.0.1:3745",
      mcpUrl: "http://127.0.0.1:3745/mcp",
      allowExternalIntegrations: true,
    }),
  ).toEqual({
    state: "Running on this computer",
    address: "http://127.0.0.1:3745",
    action: "Stop server",
    externalIntegrations: "On for this session",
  });
});

test("a delayed access-key response cannot reappear after stop, refresh, or dialog close", () => {
  expect(shouldRevealAccessKey(3, 3, true, true)).toBe(true);
  expect(shouldRevealAccessKey(3, 4, true, true)).toBe(false);
  expect(shouldRevealAccessKey(3, 3, false, true)).toBe(false);
  expect(shouldRevealAccessKey(3, 3, true, false)).toBe(false);
});

test("refresh preserves an explicit next-start choice while stopped", () => {
  const stopped = {
    running: false,
    baseUrl: null,
    mcpUrl: null,
    allowExternalIntegrations: false,
  };
  expect(libraryServerExternalIntegrationChoice(stopped, true, true)).toBe(true);
  expect(libraryServerExternalIntegrationChoice(stopped, true, false)).toBe(false);
});

test("refresh follows the live server policy and discards a pending next-start choice", () => {
  const running = {
    running: true,
    baseUrl: "http://127.0.0.1:3745",
    mcpUrl: "http://127.0.0.1:3745/mcp",
    allowExternalIntegrations: false,
  };
  expect(libraryServerExternalIntegrationChoice(running, true, true)).toBe(false);
});

class SettingsNode {
  children = [];
  listeners = new Map();
  textContent = "";
  value = "";
  checked = false;
  disabled = false;
  hidden = false;
  isConnected = true;
  append(...items) {
    this.children.push(...items);
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
    return [this, ...this.children.flatMap((node) => node.all())];
  }
}
const settle = async () => {
  for (let i = 0; i < 10; i++) await Promise.resolve();
};
const field = (section, caption) =>
  section.element
    .all()
    .find((node) => node.children.some((child) => child.textContent === caption))
    ?.children.find((node) => node.type === "checkbox");
const control = (section, caption) =>
  section.element.all().find((node) => node.textContent === caption);
async function settingsDom(run) {
  const before = globalThis.document;
  globalThis.document = {
    createElement: () => new SettingsNode(),
    createTextNode: (value) => {
      const node = new SettingsNode();
      node.textContent = value;
      return node;
    },
  };
  try {
    await run();
  } finally {
    globalThis.document = before;
  }
}

test("AI access updates preserve applied printer policy and never apply pending LAN draft", async () => {
  await settingsDom(async () => {
    const { LibraryServerSettingsSection } = await import("./library-server-settings");
    let status = {
      running: true,
      baseUrl: "http://127.0.0.1:3745",
      mcpUrl: null,
      allowExternalIntegrations: false,
      printerNetwork: null,
    };
    const starts = [];
    const section = new LibraryServerSettingsSection({
      status: async () => status,
      start: async (external, policy) => {
        starts.push([external, policy]);
        return (status = {
          ...status,
          allowExternalIntegrations: external,
          printerNetwork: policy,
        });
      },
      stop: async () => ({ ...status, running: false, printerNetwork: null }),
      revealAccessKey: async () => ({ accessKey: "fixture" }),
    });
    await settle();
    const allowPrinter = field(section, "Allow printer LAN requests from Browser and MCP");
    allowPrinter.checked = true;
    allowPrinter.fire("change");
    const scopes = section.element.all().find((node) => node.placeholder === "10.250.0.1/32");
    scopes.value = "10.250.0.1/32";
    scopes.fire("input");
    const carbon = field(section, "Allow Centauri Carbon discovery broadcast");
    carbon.checked = true;
    carbon.fire("change");
    expect(starts).toEqual([]);
    const external = field(section, "Allow external integration requests from Browser and MCP");
    external.checked = true;
    external.fire("change");
    await settle();
    expect(starts).toEqual([[true, null]]);
    expect(allowPrinter.checked).toBe(true);
    expect(carbon.checked).toBe(true);
    await section.refresh();
    expect(scopes.value).toBe("10.250.0.1/32");
    expect(starts).toHaveLength(1);
    control(section, "Apply printer LAN access").fire("click");
    await settle();
    expect(starts[1]).toEqual([
      true,
      {
        allowedScopes: ["10.250.0.1/32"],
        allowMulticastDiscovery: false,
        allowCarbonBroadcast: true,
      },
    ]);
  });
});

test("stop clears printer permissions and next start defaults to no LAN access", async () => {
  await settingsDom(async () => {
    const { LibraryServerSettingsSection } = await import("./library-server-settings");
    let status = {
      running: true,
      baseUrl: "http://127.0.0.1:3745",
      mcpUrl: null,
      allowExternalIntegrations: false,
      printerNetwork: {
        allowedScopes: ["10.250.0.1/32"],
        allowMulticastDiscovery: true,
        allowCarbonBroadcast: true,
      },
    };
    const starts = [];
    const section = new LibraryServerSettingsSection({
      status: async () => status,
      start: async (external, policy) => {
        starts.push([external, policy]);
        return (status = { ...status, running: true, printerNetwork: policy });
      },
      stop: async () => (status = { ...status, running: false, printerNetwork: null }),
      revealAccessKey: async () => ({ accessKey: "fixture" }),
    });
    await settle();
    control(section, "Stop server").fire("click");
    await settle();
    expect(field(section, "Allow printer LAN requests from Browser and MCP").checked).toBe(false);
    expect(field(section, "Allow Bambu SSDP multicast discovery").checked).toBe(false);
    expect(field(section, "Allow Centauri Carbon discovery broadcast").checked).toBe(false);
    control(section, "Start server").fire("click");
    await settle();
    expect(starts).toEqual([[false, null]]);
  });
});

test("stopped printer draft changes perform no IPC until an explicit valid start", async () => {
  await settingsDom(async () => {
    const { LibraryServerSettingsSection } = await import("./library-server-settings");
    let status = {
      running: false,
      baseUrl: null,
      mcpUrl: null,
      allowExternalIntegrations: false,
      printerNetwork: null,
    };
    const starts = [];
    const section = new LibraryServerSettingsSection({
      status: async () => status,
      start: async (external, policy) => {
        starts.push([external, policy]);
        return (status = { ...status, running: true, printerNetwork: policy });
      },
      stop: async () => status,
      revealAccessKey: async () => ({ accessKey: "fixture" }),
    });
    await settle();
    const enabled = field(section, "Allow printer LAN requests from Browser and MCP");
    enabled.checked = true;
    enabled.fire("change");
    expect(starts).toEqual([]);
    control(section, "Start server").fire("click");
    await settle();
    expect(starts).toEqual([]);
    const scopes = section.element.all().find((node) => node.placeholder === "10.250.0.1/32");
    scopes.value = " 10.250.0.1/32\n10.250.0.2/32 ";
    scopes.fire("input");
    await section.refresh();
    expect(enabled.checked).toBe(true);
    expect(scopes.value).toBe(" 10.250.0.1/32\n10.250.0.2/32 ");
    control(section, "Start server").fire("click");
    await settle();
    expect(starts).toEqual([
      [
        false,
        {
          allowedScopes: ["10.250.0.1/32", "10.250.0.2/32"],
          allowMulticastDiscovery: false,
          allowCarbonBroadcast: false,
        },
      ],
    ]);
  });
});

test("pending printer revocation cannot be overwritten by refresh or settings reopen", async () => {
  await settingsDom(async () => {
    const { LibraryServerSettingsSection } = await import("./library-server-settings");
    const oldPolicy = {
      allowedScopes: ["10.250.0.1/32"],
      allowMulticastDiscovery: true,
      allowCarbonBroadcast: true,
    };
    const oldStatus = {
      running: true,
      baseUrl: "http://127.0.0.1:3745",
      mcpUrl: null,
      allowExternalIntegrations: false,
      printerNetwork: oldPolicy,
    };
    const revokedStatus = { ...oldStatus, printerNetwork: null };
    let resolveRevoke;
    let resolveStaleRead;
    const revocation = new Promise((resolve) => {
      resolveRevoke = resolve;
    });
    const staleRead = new Promise((resolve) => {
      resolveStaleRead = resolve;
    });
    const starts = [];
    let reads = 0;
    const section = new LibraryServerSettingsSection({
      status: async () => (++reads === 1 ? oldStatus : staleRead),
      start: async (external, policy) => {
        starts.push([external, policy]);
        return starts.length === 1
          ? revocation
          : { ...revokedStatus, allowExternalIntegrations: external, printerNetwork: policy };
      },
      stop: async () => ({ ...revokedStatus, running: false }),
      revealAccessKey: async () => ({ accessKey: "fixture" }),
    });
    await settle();
    const enabled = field(section, "Allow printer LAN requests from Browser and MCP");
    enabled.checked = false;
    enabled.fire("change");
    control(section, "Apply printer LAN access").fire("click");
    await settle();
    expect(starts).toEqual([[false, null]]);
    const refresh = section.refresh();
    resolveStaleRead(oldStatus);
    await refresh;
    section.clearSensitive(); // Closing/reopening settings must invalidate only reads and key reveals.
    resolveRevoke(revokedStatus);
    await settle();
    const external = field(section, "Allow external integration requests from Browser and MCP");
    external.checked = true;
    external.fire("change");
    await settle();
    expect(starts).toEqual([
      [false, null],
      [true, null],
    ]);
    expect(enabled.checked).toBe(false);
  });
});
