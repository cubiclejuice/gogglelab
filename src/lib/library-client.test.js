import { expect, test } from "bun:test";
import {
  admitBrowserServerUrl,
  formatWireInteger,
  HttpLibraryClient,
  LibraryHttpError,
  managementActionAvailability,
} from "./library-client";

test("catalog maintenance visibility is independent of physical source-write access", () => {
  const features = {
    trashSource: false,
    purgeCatalog: true,
    cleanupOrphans: true,
  };
  const backupOnly = [
    { id: "backups", catalogRootId: null, label: "Backups", access: "backupOnly" },
  ];
  expect(managementActionAvailability(features, backupOnly)).toEqual({
    physicalSource: false,
    purgeCatalog: true,
    cleanupOrphans: true,
  });
  expect(managementActionAvailability(features, [])).toEqual({
    physicalSource: false,
    purgeCatalog: false,
    cleanupOrphans: true,
  });
});

test("browser server URL admits HTTPS and exact loopback HTTP origins", () => {
  expect(admitBrowserServerUrl("https://library.example").origin).toBe("https://library.example");
  expect(admitBrowserServerUrl("http://localhost:3745").origin).toBe("http://localhost:3745");
  expect(admitBrowserServerUrl("http://127.0.0.1:3745").hostname).toBe("127.0.0.1");
  expect(admitBrowserServerUrl("http://[::1]:3745").hostname).toBe("[::1]");
});

test("browser server URL refuses credentials for nonlocal plaintext and URL secrets", () => {
  for (const url of [
    "http://192.168.1.20:3745",
    "http://library.example",
    "http://localhost.example:3745",
    "ftp://localhost/library",
    "https://user:secret@library.example",
    "https://library.example?access_token=secret",
    "https://library.example/#token=secret",
  ]) {
    expect(() => admitBrowserServerUrl(url)).toThrow(LibraryHttpError);
  }
});

test("unsafe browser origins reject before accepting or sending an access key", async () => {
  const original = globalThis.fetch;
  let called = false;
  globalThis.fetch = async () => {
    called = true;
    return new Response();
  };
  try {
    const client = new HttpLibraryClient("http://10.0.0.2:3745");
    expect(client.endpointSecurityError?.code).toBe("tlsRequired");
    expect(() => client.setToken("never-send-this")).toThrow(LibraryHttpError);
    await expect(client.listModels()).rejects.toBeInstanceOf(LibraryHttpError);
    expect(called).toBe(false);
  } finally {
    globalThis.fetch = original;
  }
});

test("browser client requires a memory token and never embeds it in model URLs", async () => {
  const client = new HttpLibraryClient("https://library.test");
  await expect(client.listModels()).rejects.toBeInstanceOf(LibraryHttpError);
  client.setToken(" secret-token ");
  const original = globalThis.fetch;
  let seen;
  try {
    globalThis.fetch = async (url, init) => {
      seen = {
        url: String(url),
        authorization: new Headers(init.headers).get("Authorization"),
        redirect: init.redirect,
        credentials: init.credentials,
        referrerPolicy: init.referrerPolicy,
      };
      return new Response(JSON.stringify({ items: [], nextCursor: null, snapshotRevision: "1" }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    };
    await client.listModels({ query: "gear" });
    expect(seen.url).not.toContain("secret-token");
    expect(seen.authorization).toBe("Bearer secret-token");
    expect(seen.redirect).toBe("error");
    expect(seen.credentials).toBe("omit");
    expect(seen.referrerPolicy).toBe("no-referrer");
  } finally {
    globalThis.fetch = original;
  }
});

test("gallery mutations encode identifiers and send optimistic versions", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  const requests = [];
  try {
    globalThis.fetch = async (url, init) => {
      requests.push({ url: String(url), init });
      return Response.json({ metadataVersion: "8" });
    };
    expect(await client.setActiveThumbnail("model/a", "7", "thumb b")).toBe("8");
    expect(await client.deleteThumbnail("model/a", "8", "thumb b")).toBe("8");
    expect(requests.map((request) => new URL(request.url).pathname)).toEqual([
      "/api/v1/models/model%2Fa/thumbnails/thumb%20b/active",
      "/api/v1/models/model%2Fa/thumbnails/thumb%20b",
    ]);
    expect(requests.map((request) => request.init.method)).toEqual(["PATCH", "DELETE"]);
    expect(JSON.parse(requests[0].init.body)).toEqual({ expectedVersion: "7" });
  } finally {
    globalThis.fetch = original;
  }
});

test("download leases bind the current content hash and stream with authentication", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  const requests = [];
  try {
    globalThis.fetch = async (url, init) => {
      requests.push({ url: String(url), init });
      if (requests.length === 1) {
        return Response.json({
          id: "lease/id",
          fileName: "part.stl",
          mime: "model/stl",
          bytes: "3",
          sha256: "abc",
          libraryEpoch: "4",
          expiresAt: "2026-10-11T00:00:00Z",
        });
      }
      return new Response(new Uint8Array([1, 2, 3]));
    };
    const lease = await client.issueModelDownload("model/id", "abc");
    const blob = await client.readContent(lease.id, lease.mime);
    expect(JSON.parse(requests[0].init.body)).toEqual({ expectedHash: "abc" });
    expect(new URL(requests[1].url).pathname).toBe("/api/v1/content/lease%2Fid");
    expect(new Headers(requests[1].init.headers).get("Authorization")).toBe("Bearer token");
    expect(blob.size).toBe(3);
    expect(blob.type).toBe("model/stl");
  } finally {
    globalThis.fetch = original;
  }
});

test("metadata mutations use camelCase patches and one atomic bulk request", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  const requests = [];
  try {
    globalThis.fetch = async (url, init) => {
      requests.push({ url: String(url), init });
      return Response.json([]);
    };
    await client.bulkUpdateMetadata(
      [
        { modelId: "a", expectedVersion: "3" },
        { modelId: "b", expectedVersion: "7" },
      ],
      { designer: "Ada", printStatus: "queued", tagIds: ["tag-1"], tagMode: "merge" },
    );
    expect(new URL(requests[0].url).pathname).toBe("/api/v1/models/metadata/bulk");
    expect(JSON.parse(requests[0].init.body)).toEqual({
      targets: [
        { modelId: "a", expectedVersion: "3" },
        { modelId: "b", expectedVersion: "7" },
      ],
      patch: {
        designer: "Ada",
        printStatus: "queued",
        tagIds: ["tag-1"],
        tagMode: "merge",
      },
    });
  } finally {
    globalThis.fetch = original;
  }
});

test("part stock REST operations use reviewed versioned writes", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  const requests = [];
  const part = {
    id: "part/bolt",
    name: "M3 bolt",
    category: "Hardware",
    quantity: "40",
    unit: "pcs",
    notes: "",
    lowStock: "8",
    metadataVersion: "3",
  };
  try {
    globalThis.fetch = async (url, init) => {
      requests.push({ url: String(url), init });
      if (requests.length === 1) return Response.json([part]);
      if (requests.length === 2 || requests.length === 3) return Response.json(part);
      return new Response(null, { status: 204 });
    };
    expect(await client.listPartStock()).toEqual([part]);
    const input = {
      name: "M3 bolt",
      category: "Hardware",
      quantity: 40,
      unit: "pcs",
      notes: "",
      lowStock: 8,
    };
    await client.createPartStock(input);
    await client.updatePartStock("part/bolt", "3", input);
    await client.deletePartStock("part/bolt", "4");

    expect(requests.map((request) => new URL(request.url).pathname)).toEqual([
      "/api/v1/parts",
      "/api/v1/parts",
      "/api/v1/parts/part%2Fbolt",
      "/api/v1/parts/part%2Fbolt",
    ]);
    expect(requests.map((request) => request.init.method ?? "GET")).toEqual([
      "GET",
      "POST",
      "PUT",
      "DELETE",
    ]);
    expect(JSON.parse(requests[2].init.body)).toEqual({ expectedVersion: "3", input });
    expect(JSON.parse(requests[3].init.body)).toEqual({ expectedVersion: "4" });
  } finally {
    globalThis.fetch = original;
  }
});

test("capture import sends complete bound upload tokens", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  let body;
  try {
    globalThis.fetch = async (_url, init) => {
      body = JSON.parse(init.body);
      return Response.json({ itemId: "item", status: "imported", downloads: [] });
    };
    const envelope = {
      version: 1,
      itemId: "item",
      createdAt: "2026-10-11T00:00:00Z",
      page: { url: "https://printables.com/model/1", title: "Part" },
      downloads: [],
    };
    const upload = {
      id: "token",
      itemId: "item",
      captureId: "capture",
      expiresAt: "2026-10-11T00:05:00Z",
      libraryEpoch: "2",
    };
    await client.importCapture(envelope, [upload]);
    expect(body).toEqual({ envelope, uploads: [upload] });
  } finally {
    globalThis.fetch = original;
  }
});

test("filament assignment preserves per-model IDs in one atomic request", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  let request;
  try {
    globalThis.fetch = async (url, init) => {
      request = { url: String(url), body: JSON.parse(init.body) };
      return Response.json([]);
    };
    const assignments = [
      { target: { modelId: "a", expectedVersion: "2" }, filamentIds: ["manual:1"] },
      { target: { modelId: "b", expectedVersion: "9" }, filamentIds: ["spool:7", "legacy"] },
    ];
    await client.assignFilaments(assignments);
    expect(new URL(request.url).pathname).toBe("/api/v1/models/filaments/bulk");
    expect(request.body).toEqual({ assignments });
  } finally {
    globalThis.fetch = original;
  }
});

test("partial source-hold receipts survive authenticated HTTP errors", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  const sourceHolds = [
    {
      modelId: "model-1",
      rootHandleId: "models",
      relativePath: "part.stl",
      holdName: ".gogglelab-held-a1.data",
      journalName: ".gogglelab-held-a1.journal",
      bytes: "21",
      sha256: "a".repeat(64),
    },
  ];
  try {
    globalThis.fetch = async () =>
      Response.json(
        {
          error: {
            code: "conflict",
            message: "A later batch target failed.",
            requestId: "request-1",
            retryable: false,
            sourceHolds,
          },
        },
        { status: 409 },
      );
    const error = await client.listModels().catch((value) => value);
    expect(error).toBeInstanceOf(LibraryHttpError);
    expect(error.sourceHolds).toEqual(sourceHolds);
  } finally {
    globalThis.fetch = original;
  }
});

test("browser wire integers preserve values above Number.MAX_SAFE_INTEGER", async () => {
  const huge = "18446744073709551615";
  expect(formatWireInteger(huge)).toBe(BigInt(huge).toLocaleString());
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  let body;
  try {
    globalThis.fetch = async (_url, init) => {
      body = JSON.parse(init.body);
      return Response.json([{ modelId: "a", metadataVersion: huge }]);
    };
    const result = await client.bulkUpdateMetadata([{ modelId: "a", expectedVersion: huge }], {
      printStatus: "printed",
      rating: 5,
      favorite: true,
    });
    expect(body.patch.rating).toBe(5);
    expect(body.patch.favorite).toBe(true);
    expect(body.targets[0].expectedVersion).toBe(huge);
    expect(result[0].metadataVersion).toBe(huge);
  } finally {
    globalThis.fetch = original;
  }
});

test("capture upload keeps declared byte counts and epochs as decimal strings", async () => {
  const huge = "9007199254740993";
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  let intent;
  try {
    globalThis.fetch = async (_url, init) => {
      intent = JSON.parse(await init.body.get("intent").text());
      return Response.json({
        id: "upload",
        itemId: "item",
        captureId: "capture",
        expiresAt: "2026-10-11T00:05:00Z",
        libraryEpoch: huge,
      });
    };
    const token = await client.stageCaptureUpload(
      {
        itemId: "item",
        captureId: "capture",
        fileName: "part.stl",
        bytes: huge,
        sha256: "a".repeat(64),
      },
      new Blob(["data"]),
    );
    expect(intent.bytes).toBe(huge);
    expect(token.libraryEpoch).toBe(huge);
  } finally {
    globalThis.fetch = original;
  }
});

test("catalog query sends global filters and sort before cursor paging", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  let url;
  try {
    globalThis.fetch = async (input) => {
      url = new URL(String(input));
      return Response.json({ items: [], nextCursor: null, snapshotRevision: "9" });
    };
    await client.listModels({
      limit: 100,
      cursor: "opaque",
      query: "gear",
      folderId: "folder",
      sort: "size",
      direction: "desc",
      tagId: "tag",
      category: "Machines",
      designer: "Ada",
      modelLicense: "CC0",
      printStatus: "queued",
      everPrinted: true,
      bundleId: "bundle",
      rootId: "root",
      sourceDirectory: "projects/gears",
      filamentId: "filament",
      sourceMissing: false,
      duplicatesOnly: true,
      sourceKind: "indexed",
      group: "archive",
      thumbnailMode: "custom",
      minimumRating: 3,
      favorite: false,
    });
    expect(Object.fromEntries(url.searchParams)).toEqual({
      limit: "100",
      cursor: "opaque",
      query: "gear",
      folderId: "folder",
      sort: "size",
      direction: "desc",
      tagId: "tag",
      category: "Machines",
      designer: "Ada",
      modelLicense: "CC0",
      printStatus: "queued",
      everPrinted: "true",
      bundleId: "bundle",
      rootId: "root",
      sourceDirectory: "projects/gears",
      filamentId: "filament",
      sourceMissing: "false",
      duplicatesOnly: "true",
      sourceKind: "indexed",
      group: "archive",
      thumbnailMode: "custom",
      minimumRating: "3",
      favorite: "false",
    });
  } finally {
    globalThis.fetch = original;
  }
});

test("scan and capture inbox methods use bounded explicit routes", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  const requests = [];
  try {
    globalThis.fetch = async (url, init) => {
      requests.push({ url: String(url), init });
      if (String(url).includes("/captures"))
        return Response.json({ items: [], nextCursor: null, snapshotRevision: "1" });
      return Response.json({
        running: true,
        cancelling: false,
        jobKind: "scan",
        currentRootId: "root",
        rootsTotal: 1,
        rootsCompleted: 0,
        filesVisited: 2,
        filesIndexed: 1,
        errors: [],
      });
    };
    await client.startScan("root");
    await client.startScanDirectory("root", "parts/nested");
    await client.cancelScan();
    await client.listCaptures({ limit: 25, cursor: "opaque" });
    await client.startThumbnailJob(["a", "b"], true);
    expect(JSON.parse(requests[0].init.body)).toEqual({ rootId: "root" });
    expect(JSON.parse(requests[1].init.body)).toEqual({
      rootId: "root",
      directory: "parts/nested",
    });
    expect(requests[2].init.method).toBe("DELETE");
    expect(new URL(requests[3].url).searchParams.toString()).toBe("limit=25&cursor=opaque");
    expect(JSON.parse(requests[4].init.body)).toEqual({ modelIds: ["a", "b"], force: true });
  } finally {
    globalThis.fetch = original;
  }
});

test("bundle preview geometry uses token-bound binary routes and explicit close", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  const requests = [];
  try {
    globalThis.fetch = async (url, init) => {
      requests.push({ url: String(url), init });
      if (String(url).endsWith("/geometry")) return new Response(new Uint8Array([1, 2, 3]));
      if (init.method === "DELETE") return new Response(null, { status: 204 });
      return Response.json({ token: "token/id", bundleId: "bundle/id", members: [] });
    };
    await client.prepareBundle("bundle/id");
    expect([...new Uint8Array(await client.readBundleGeometry("token/id"))]).toEqual([1, 2, 3]);
    await client.closeBundle("token/id");
    expect(requests.map((request) => new URL(request.url).pathname)).toEqual([
      "/api/v1/bundles/bundle%2Fid/preview",
      "/api/v1/bundle-previews/token%2Fid/geometry",
      "/api/v1/bundle-previews/token%2Fid",
    ]);
    expect(requests[2].init.method).toBe("DELETE");
  } finally {
    globalThis.fetch = original;
  }
});

test("single-model geometry and full bundle archive leases bind hashes, plates, and tokens", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  const requests = [];
  try {
    globalThis.fetch = async (url, init) => {
      requests.push({ url: String(url), init });
      if (String(url).includes("geometry-lease"))
        return Response.json({
          content: { id: "geometry", kind: "geometry", bytes: "64", libraryEpoch: "9" },
          packetFormat: "stl2",
          packetVersion: 2,
          plates: [],
          parts: [],
          materials: [],
          embeddedFilaments: [],
        });
      return Response.json({
        id: "bundle",
        kind: "bundleArchive",
        bytes: "4294967296",
        libraryEpoch: "9",
      });
    };
    const geometry = await client.issueModelGeometry("model/id", "a".repeat(64), 7);
    const archive = await client.issueBundleDownload("token/id");
    expect(JSON.parse(requests[0].init.body)).toEqual({ expectedHash: "a".repeat(64), plateId: 7 });
    expect(geometry.packetVersion).toBe(2);
    expect(archive.bytes).toBe("4294967296");
    expect(new URL(requests[1].url).pathname).toBe(
      "/api/v1/bundle-previews/token%2Fid/download-lease",
    );
  } finally {
    globalThis.fetch = original;
  }
});

test("settings and print batch preserve decimal fields and one idempotent atomic body", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  const requests = [];
  try {
    globalThis.fetch = async (url, init) => {
      requests.push({ url: String(url), init });
      return Response.json([]);
    };
    await client.updateRootSettings("root/id", {
      enabled: true,
      scanIntervalMinutes: 60,
      maxFileBytes: "104857600",
      scanZip: true,
    });
    const items = [
      {
        modelId: "model",
        input: {
          printedAt: "2026-10-11T12:00:00.000Z",
          quantity: 1,
          notes: "",
          filamentIds: [],
          outcome: "printed",
        },
      },
    ];
    await client.createHistoryBatch("stable-key", items);
    expect(new URL(requests[0].url).pathname).toBe("/api/v1/settings/roots/root%2Fid");
    expect(JSON.parse(requests[0].init.body).maxFileBytes).toBe("104857600");
    expect(JSON.parse(requests[1].init.body)).toEqual({ idempotencyKey: "stable-key", items });
  } finally {
    globalThis.fetch = original;
  }
});

test("management requires a separate server plan and explicit token execution", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  const requests = [];
  try {
    globalThis.fetch = async (url, init) => {
      requests.push({ url: String(url), init });
      return requests.length === 1
        ? Response.json({ token: "one-use", actionKind: "recoverableRemove", impact: [] })
        : Response.json({ kind: "removed", snapshotId: "snapshot", removedIds: ["model"] });
    };
    const action = {
      kind: "recoverableRemove",
      targets: [{ modelId: "model", expectedVersion: "18446744073709551615" }],
    };
    const plan = await client.planManagementAction(action);
    await client.executeManagementAction(plan.token);
    expect(JSON.parse(requests[0].init.body)).toEqual(action);
    expect(JSON.parse(requests[1].init.body)).toEqual({ token: "one-use" });
  } finally {
    globalThis.fetch = original;
  }
});

test("catalog purge and orphan cleanup carry explicit confirmation to the server", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  const requests = [];
  try {
    globalThis.fetch = async (url, init) => {
      requests.push({ url: String(url), init });
      return Response.json({ token: "one-use", actionKind: "planned", impact: [] });
    };
    await client.planManagementAction({
      kind: "purgeCatalog",
      backupDestinationHandleId: "backups",
      backupFileName: "before-purge.zip",
      confirm: true,
    });
    await client.planManagementAction({ kind: "cleanupOrphans", confirm: true });
    expect(requests.map((request) => JSON.parse(request.init.body))).toEqual([
      {
        kind: "purgeCatalog",
        backupDestinationHandleId: "backups",
        backupFileName: "before-purge.zip",
        confirm: true,
      },
      { kind: "cleanupOrphans", confirm: true },
    ]);
  } finally {
    globalThis.fetch = original;
  }
});

test("integration credential is request-only and never enters the provider URL", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("server-token");
  const original = globalThis.fetch;
  let request;
  try {
    globalThis.fetch = async (url, init) => {
      request = { url: String(url), body: JSON.parse(init.body) };
      return Response.json({ ...request.body.config, hasCredential: true });
    };
    const config = {
      id: "provider/id",
      label: "Provider",
      kind: "openaiCompatible",
      baseUrl: "https://provider.test",
      endpointScope: "remote",
      model: "model",
      credentialStorage: "session",
      hasCredential: false,
      maxTags: 5,
      detail: "balanced",
      categoryMode: "existingOnly",
      concurrency: 1,
      timeoutSeconds: 30,
      structuredOutput: "jsonSchema",
      categorySharing: "local",
    };
    await client.saveAiProvider(config, { action: "setSession", secret: "secret-key" });
    expect(request.url).not.toContain("secret-key");
    expect(request.body.credentialUpdate).toEqual({ action: "setSession", secret: "secret-key" });
    expect(request.body.config.categorySharing).toBe("local");
  } finally {
    globalThis.fetch = original;
  }
});

test("browser printer fleet requires authentication and preserves decimal wire versions", async () => {
  const client = new HttpLibraryClient("https://library.test");
  await expect(client.listPrinters()).rejects.toBeInstanceOf(LibraryHttpError);
  client.setToken("fleet-token");
  const original = globalThis.fetch;
  const requests = [];
  const version = "18446744073709551614";
  const printer = { id: "printer/a", metadataVersion: version, printCount: version };
  globalThis.fetch = async (url, init) => {
    requests.push({
      url: String(url),
      headers: new Headers(init.headers),
      body: init.body,
      method: init.method ?? "GET",
    });
    return Response.json(printer);
  };
  try {
    const input = {
      nickname: "Workshop",
      make: null,
      model: null,
      printerType: "FDM",
      firmware: null,
      host: null,
      webUrl: null,
      bedMm: null,
      notes: "",
    };
    expect((await client.updatePrinter("printer/a", version, input)).metadataVersion).toBe(version);
    await client.deletePrinter("printer/a", version);
    expect(requests[0].url).toBe("https://library.test/api/v1/printers/printer%2Fa");
    expect(requests[0].headers.get("Authorization")).toBe("Bearer fleet-token");
    expect(JSON.parse(requests[0].body)).toEqual({ expectedVersion: version, input });
    expect(JSON.parse(requests[1].body)).toEqual({ expectedVersion: version });
    expect(requests[0].url).not.toContain("fleet-token");
  } finally {
    globalThis.fetch = original;
  }
});

test("printer fleet conflicts expose current decimal version without retrying a write", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  let calls = 0;
  globalThis.fetch = async () => {
    calls += 1;
    return Response.json(
      {
        error: {
          code: "versionConflict",
          message: "Printer changed",
          requestId: "fleet-conflict",
          retryable: false,
          currentVersion: "9007199254740999",
        },
      },
      { status: 409 },
    );
  };
  try {
    await expect(client.deletePrinter("printer", "9007199254740998")).rejects.toMatchObject({
      code: "versionConflict",
      currentVersion: "9007199254740999",
    });
    expect(calls).toBe(1);
  } finally {
    globalThis.fetch = original;
  }
});

test("browser fleet maintenance and reminder writes bind exact IDs and decimal versions", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("fleet-token");
  const original = globalThis.fetch;
  const requests = [];
  globalThis.fetch = async (url, init) => {
    requests.push({
      url: String(url),
      method: init.method ?? "GET",
      body: init.body ? JSON.parse(init.body) : null,
    });
    return Response.json({ metadataVersion: "9007199254740999" });
  };
  try {
    const log = { title: "Lubricate", performedAt: "2026-10-19T12:00:00Z", notes: "" };
    const reminder = { title: "Clean", dueAt: "2026-10-20T12:00:00Z", intervalDays: 7, notes: "" };
    await client.listPrinterMaintenance();
    await client.listPrinterMaintenance("a/b");
    await client.createPrinterMaintenance("a/b", log);
    await client.updatePrinterMaintenance("log/id", "9007199254740998", log);
    await client.deletePrinterMaintenance("log/id", "9007199254740998");
    await client.listPrinterReminders("a/b");
    await client.createPrinterReminder("a/b", reminder);
    await client.updatePrinterReminder("rem/id", "9007199254740998", reminder);
    await client.pausePrinterReminder("rem/id", "9007199254740998", true);
    await client.completePrinterReminder("rem/id", "9007199254740998", {
      completedAt: "2026-10-21T12:00:00Z",
      notes: "Done",
    });
    await client.deletePrinterReminder("rem/id", "9007199254740998");
    expect(requests[0].url).toEndWith("/api/v1/printer-maintenance-logs");
    expect(requests[1].url).toEndWith("/api/v1/printers/a%2Fb/maintenance-logs");
    expect(requests[2].body).toEqual(log);
    expect(requests[3].body).toEqual({ expectedVersion: "9007199254740998", input: log });
    expect(requests[4].body).toEqual({ expectedVersion: "9007199254740998" });
    expect(requests[5].url).toEndWith(
      "/api/v1/printer-reminders?includeCompleted=true&printerId=a%2Fb",
    );
    expect(requests[6].body).toEqual({ printerId: "a/b", input: reminder });
    expect(requests[7].body).toEqual({ expectedVersion: "9007199254740998", input: reminder });
    expect(requests[8].body).toEqual({ expectedVersion: "9007199254740998", paused: true });
    expect(requests[9].body).toEqual({
      expectedVersion: "9007199254740998",
      completedAt: "2026-10-21T12:00:00Z",
      notes: "Done",
    });
    expect(requests[10].method).toBe("DELETE");
  } finally {
    globalThis.fetch = original;
  }
});

test("history printer assignment supports explicit clear and omitted preservation", async () => {
  const client = new HttpLibraryClient("https://library.test");
  client.setToken("token");
  const original = globalThis.fetch;
  const bodies = [];
  globalThis.fetch = async (_url, init) => {
    bodies.push(JSON.parse(init.body));
    return Response.json({ printerId: "printer", printerNicknameSnapshot: "Workshop" });
  };
  const input = {
    printedAt: "2026-10-19T12:00:00Z",
    quantity: 1,
    outcome: "printed",
    notes: "",
    filamentIds: [],
  };
  try {
    expect(
      (await client.createHistory("model", { ...input, printerId: "printer" }))
        .printerNicknameSnapshot,
    ).toBe("Workshop");
    await client.updateHistory("event", input);
    await client.updateHistory("event", { ...input, printerId: null });
    expect(bodies[0].printerId).toBe("printer");
    expect(Object.hasOwn(bodies[1], "printerId")).toBe(false);
    expect(bodies[2].printerId).toBe(null);
  } finally {
    globalThis.fetch = original;
  }
});

test("printer network client keeps explicit review confirmation and version precision", async () => {
  const client = new HttpLibraryClient("https://library.test");
  await expect(client.getPrinterNetworkPolicy()).rejects.toBeInstanceOf(LibraryHttpError);
  client.setToken("token");
  const original = globalThis.fetch;
  const requests = [];
  globalThis.fetch = async (url, init) => {
    requests.push({
      url: String(url),
      method: init.method ?? "GET",
      body: init.body ? JSON.parse(init.body) : null,
    });
    return Response.json({});
  };
  try {
    await client.getPrinterNetworkPolicy();
    await client.planPrinterDiscovery("10.7.8.1-10.7.8.2", true);
    await client.startPrinterDiscovery("plan-secret", true);
    await client.getPrinterDiscovery("job/id");
    await client.cancelPrinterDiscovery("job/id");
    await client.setPrinterEndpointApproval("printer/id", "18446744073709551614", true);
    await client.testPrinterConnection("printer/id", "18446744073709551614", true, true);
    expect(requests.map((item) => item.method)).toEqual([
      "GET",
      "POST",
      "POST",
      "GET",
      "DELETE",
      "PUT",
      "POST",
    ]);
    expect(requests[0].url).toEndWith("/api/v1/printer-network/policy");
    expect(requests[1].body).toEqual({ range: "10.7.8.1-10.7.8.2", includeBambuDiscovery: true });
    expect(requests[2].body).toEqual({ planToken: "plan-secret", confirm: true });
    expect(requests[3].url).toEndWith("/api/v1/printer-network/discovery-jobs/job%2Fid");
    expect(requests[5].body).toEqual({ expectedVersion: "18446744073709551614", approved: true });
    expect(requests[6].body).toEqual({
      expectedVersion: "18446744073709551614",
      discoverMainboardId: true,
      confirm: true,
    });
    expect(requests.every((item) => !item.url.includes("plan-secret"))).toBe(true);
  } finally {
    globalThis.fetch = original;
  }
});
