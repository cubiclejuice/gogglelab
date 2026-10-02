import { expect, test } from "bun:test";
import { isLibraryRequestCurrent } from "./library-load-guard";

const request = { generation: 12, entitlementRevision: 4 };

test("a Library response is current only while its viewer and license revisions still match", () => {
  expect(
    isLibraryRequestCurrent(request, request, {
      generation: 12,
      entitlementRevision: 4,
      entitled: true,
    }),
  ).toBe(true);
  expect(
    isLibraryRequestCurrent(request, null, {
      generation: 12,
      entitlementRevision: 4,
      entitled: true,
    }),
  ).toBe(false);
  expect(
    isLibraryRequestCurrent(
      request,
      { generation: 13, entitlementRevision: 4 },
      { generation: 13, entitlementRevision: 4, entitled: true },
    ),
  ).toBe(false);
  expect(
    isLibraryRequestCurrent(request, request, {
      generation: 13,
      entitlementRevision: 4,
      entitled: true,
    }),
  ).toBe(false);
  expect(
    isLibraryRequestCurrent(request, request, {
      generation: 12,
      entitlementRevision: 6,
      entitled: true,
    }),
  ).toBe(false);
  expect(
    isLibraryRequestCurrent(request, request, {
      generation: 12,
      entitlementRevision: 4,
      entitled: false,
    }),
  ).toBe(false);
});
