import { expect, test } from "bun:test";
import { isLibraryRequestCurrent } from "./library-load-guard";

const request = { generation: 12 };

test("a Library response is current only while its request and viewer generations match", () => {
  expect(
    isLibraryRequestCurrent(request, request, {
      generation: 12,
    }),
  ).toBe(true);
  expect(
    isLibraryRequestCurrent(request, null, {
      generation: 12,
    }),
  ).toBe(false);
  expect(isLibraryRequestCurrent(request, { generation: 13 }, { generation: 13 })).toBe(false);
  expect(
    isLibraryRequestCurrent(request, request, {
      generation: 13,
    }),
  ).toBe(false);
});
