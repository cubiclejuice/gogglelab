import { expect, test } from "bun:test";
import { ThumbnailCleanupDialog } from "./thumbnail-cleanup-dialog";
class Node {
  constructor(tag = "node") {
    this.tag = tag;
  }
  style = {};
  children = [];
  listeners = new Map();
  textContent = "";
  value = "";
  disabled = false;
  open = false;
  isConnected = true;
  append(...children) {
    this.children.push(...children);
  }
  replaceChildren(...children) {
    for (const child of this.children) child.disconnect();
    this.children = children;
  }
  disconnect() {
    this.isConnected = false;
    for (const child of this.children) child.disconnect();
  }
  setAttribute() {}
  addEventListener(type, callback) {
    this.listeners.set(type, callback);
  }
  focus() {
    document.activeElement = this;
  }
  showModal() {
    this.open = true;
  }
  close() {
    this.open = false;
    this.fire("close");
  }
  fire(type) {
    let prevented = false;
    if (type === "click" && this.disabled) return false;
    this.listeners.get(type)?.({
      preventDefault() {
        prevented = true;
      },
    });
    return prevented;
  }
  all() {
    return [this, ...this.children.flatMap((child) => child.all())];
  }
  querySelectorAll(selector) {
    return this.all().filter((node) => node.tag === selector);
  }
  querySelector(selector) {
    return this.all().find((node) => node.tag === selector);
  }
}
const settle = async () => {
  for (let i = 0; i < 10; i++) await Promise.resolve();
};
async function dom(run) {
  const before = { document: globalThis.document, HTMLElement: globalThis.HTMLElement };
  globalThis.HTMLElement = Node;
  const invoker = new Node("button");
  globalThis.document = { activeElement: invoker, createElement: (tag) => new Node(tag) };
  try {
    await run(new Node(), invoker);
  } finally {
    globalThis.document = before.document;
    globalThis.HTMLElement = before.HTMLElement;
  }
}

const find = (root, text) => root.all().find((n) => n.textContent === text);
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((a, b) => {
    resolve = a;
    reject = b;
  });
  return { promise, resolve, reject };
};
const batch = (extra = {}) => ({
  mode: "orphans",
  reviewedFiles: 4,
  eligibleFiles: 1,
  eligibleBytes: "9007199254740993",
  preservedFiles: 3,
  protectedSourceCopies: 2,
  defaultRecordsToClear: 0,
  partial: false,
  hasMore: false,
  remainingFiles: 0,
  continuation: null,
  ...extra,
});
const plan = (token = "plan", extra = {}) => ({
  token,
  expiresAt: "2099-01-01T00:00:00Z",
  thumbnailReview: batch(extra),
});
function setup(container, overrides = {}) {
  const calls = { review: [], execute: [], discard: [], notice: [], changed: 0 };
  const adapter = {
    review: async (request) => {
      calls.review.push(request);
      return plan();
    },
    execute: async (token) => {
      calls.execute.push(token);
      return {
        files: "1",
        bytes: "9007199254740993",
        thumbnailProgress: { review: batch(), defaultRecordsCleared: 0, stopped: false },
      };
    },
    discard: async (token) => {
      calls.discard.push(token);
    },
    ...overrides,
  };
  const dialog = new ThumbnailCleanupDialog(container, adapter, {
    onChanged: async () => {
      calls.changed++;
    },
    onNotice: (text) => calls.notice.push(text),
  });
  return { dialog, calls };
}
test("cleanup review never executes before explicit confirmation and Cancel only discards", async () =>
  dom(async (root) => {
    const { dialog, calls } = setup(root);
    dialog.show("orphans");
    await settle();
    expect(calls.execute).toEqual([]);
    expect(root.all().some((n) => n.textContent.includes("9007199254740993"))).toBe(true);
    find(root, "Cancel").fire("click");
    await settle();
    expect(calls.discard).toEqual(["plan"]);
    expect(calls.execute).toEqual([]);
  }));
test("late review after Escape is discarded without exposing an execute action", async () =>
  dom(async (root) => {
    const pending = deferred();
    const { dialog, calls } = setup(root, { review: () => pending.promise });
    dialog.show("orphans");
    root
      .all()
      .find((n) => n.tag === "dialog")
      .fire("cancel");
    pending.resolve(plan("late"));
    await settle();
    expect(calls.discard).toEqual(["late"]);
    expect(calls.execute).toEqual([]);
  }));
test("each continuation page is freshly reviewed and explicitly confirmed", async () =>
  dom(async (root) => {
    const { dialog, calls } = setup(root, {
      execute: async (token) => {
        calls.execute.push(token);
        return {
          files: "1",
          bytes: "7",
          thumbnailProgress: {
            review: batch({ hasMore: true, remainingFiles: null, continuation: "next" }),
            defaultRecordsCleared: 0,
            stopped: false,
          },
        };
      },
    });
    dialog.show("orphans");
    await settle();
    find(root, "Remove 1 preview").fire("click");
    await settle();
    expect(calls.execute).toEqual(["plan"]);
    expect(calls.review).toHaveLength(1);
    find(root, "Review remaining files").fire("click");
    await settle();
    expect(calls.review[1]).toEqual({ mode: "orphans", continuation: "next" });
    expect(calls.execute).toHaveLength(1);
  }));
test("failed discard reports expiry fallback without claiming release", async () =>
  dom(async (root) => {
    const { dialog, calls } = setup(root, {
      discard: async () => {
        throw Error("unavailable");
      },
    });
    dialog.show("orphans");
    await settle();
    dialog.dismiss();
    await settle();
    expect(calls.notice.join(" ")).toContain("could not be confirmed");
    expect(calls.notice.join(" ")).toContain("expires");
    expect(calls.execute).toEqual([]);
  }));
test("replacement review discards prior owner and ignores stale results", async () =>
  dom(async (root) => {
    const old = deferred();
    let index = 0;
    const { dialog, calls } = setup(root, {
      review: async () =>
        ++index === 1
          ? old.promise
          : plan("new", { mode: "resetDefaults", defaultRecordsToClear: 8 }),
    });
    dialog.show("orphans");
    dialog.show("resetDefaults");
    await settle();
    old.resolve(plan("old"));
    await settle();
    expect(calls.discard).toEqual(["old"]);
    expect(find(root, "Reset 8 default preview entries")).toBeDefined();
    expect(calls.execute).toEqual([]);
  }));

test("missing review metadata fails closed and discards instead of executing old host response", async () =>
  dom(async (root) => {
    const { dialog, calls } = setup(root, {
      review: async () => ({ token: "legacy", expiresAt: "2099-01-01" }),
    });
    dialog.show("orphans");
    await settle();
    expect(calls.discard).toEqual(["legacy"]);
    expect(calls.execute).toEqual([]);
    expect(find(root, "Remove 1 preview")).toBeUndefined();
  }));
test("default reset confirms only database entries then offers a separate orphan review", async () =>
  dom(async (root) => {
    const { dialog, calls } = setup(root, {
      review: async () => plan("reset", { mode: "resetDefaults", defaultRecordsToClear: 8 }),
      execute: async (token) => {
        calls.execute.push(token);
        return {
          files: 0,
          bytes: 0,
          thumbnailProgress: {
            review: batch({ mode: "resetDefaults" }),
            defaultRecordsCleared: 8,
            stopped: false,
          },
        };
      },
    });
    dialog.show("resetDefaults");
    await settle();
    expect(calls.execute).toEqual([]);
    find(root, "Reset 8 default preview entries").fire("click");
    await settle();
    expect(calls.execute).toEqual(["reset"]);
    expect(root.all().some((n) => n.textContent.includes("All files were kept"))).toBe(true);
    expect(calls.review).toHaveLength(0);
    expect(find(root, "Review unused previews")).toBeDefined();
  }));
test("one pending execution blocks replacement and duplicate writes; late result only emits a notice", async () =>
  dom(async (root) => {
    const work = deferred();
    const { dialog, calls } = setup(root, {
      execute: (token) => {
        calls.execute.push(token);
        return work.promise;
      },
    });
    dialog.show("orphans");
    await settle();
    const execute = find(root, "Remove 1 preview");
    execute.fire("click");
    execute.fire("click");
    dialog.dismiss();
    dialog.show("resetDefaults");
    expect(calls.execute).toEqual(["plan"]);
    expect(calls.review).toHaveLength(1);
    work.resolve({
      files: 1,
      bytes: 7,
      thumbnailProgress: { review: batch(), defaultRecordsCleared: 0, stopped: false },
    });
    await settle();
    expect(calls.changed).toBe(1);
    expect(calls.notice.join(" ")).toContain("Removed 1");
    expect(calls.discard).toEqual([]);
  }));
test("execute failure requires a fresh review and never reuses a consumed token", async () =>
  dom(async (root) => {
    const { dialog, calls } = setup(root, {
      execute: async (token) => {
        calls.execute.push(token);
        throw Error("/private/user/library/private-source.stl");
      },
    });
    dialog.show("orphans");
    await settle();
    const execute = find(root, "Remove 1 preview");
    execute.fire("click");
    await settle();
    execute.fire("click");
    expect(calls.execute).toEqual(["plan"]);
    const message = root
      .all()
      .map((n) => n.textContent)
      .join(" ");
    expect(message).not.toContain("/private/user/library");
    expect(message).toContain("Review the current Library before trying again");
    expect(root.all().some((n) => n.textContent.includes("Some work may have completed"))).toBe(
      true,
    );
  }));

test("pre-dispatch execution rejection discards its retained review and cannot execute twice", async () =>
  dom(async (root) => {
    const { dialog, calls } = setup(root, {
      execute: async (token) => {
        calls.execute.push(token);
        throw Error("native scan Busy");
      },
    });
    dialog.show("orphans");
    await settle();
    find(root, "Remove 1 preview").fire("click");
    await settle();
    dialog.dismiss();
    await settle();
    expect(calls.execute).toEqual(["plan"]);
    expect(calls.discard).toEqual(["plan"]);
  }));
test("confirmed cleanup counts survive a failed catalog refresh", async () =>
  dom(async (root) => {
    const calls = [];
    const dialog = new ThumbnailCleanupDialog(
      root,
      {
        review: async () => plan(),
        execute: async () => ({
          files: "2",
          bytes: "18446744073709551615",
          thumbnailProgress: { review: batch(), defaultRecordsCleared: 0, stopped: false },
        }),
        discard: async (token) => calls.push(token),
      },
      {
        onChanged: async () => {
          throw Error("view failed");
        },
      },
    );
    dialog.show("orphans");
    await settle();
    find(root, "Remove 1 preview").fire("click");
    await settle();
    const text = root
      .all()
      .map((n) => n.textContent)
      .join(" ");
    expect(text).toContain("Removed 2 rebuilt previews (18446744073709551615 bytes)");
    expect(text).toContain("catalog view could not be refreshed");
    expect(text).not.toContain("cleanup could not be fully confirmed");
    expect(calls).toEqual([]);
  }));

test("closing restores a visible fallback when refresh removed the invoking control", async () =>
  dom(async (root, invoker) => {
    const fallback = new Node("input");
    const dialog = new ThumbnailCleanupDialog(
      root,
      {
        review: async () => plan(),
        execute: async () => ({ files: 0, bytes: 0 }),
        discard: async () => {},
      },
      { focusFallback: () => fallback.focus() },
    );
    dialog.show("orphans");
    await settle();
    invoker.disconnect();
    dialog.dismiss();
    expect(document.activeElement).toBe(fallback);
  }));
