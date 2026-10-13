import { expect, test } from "bun:test";
import {
  admitBrowserServerUrl,
  formatWireInteger,
  HttpLibraryClient,
  LibraryHttpError,
} from "./library-client";

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
