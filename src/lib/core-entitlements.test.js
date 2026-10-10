import { expect, test } from "bun:test";
import { withCoreEntitlements } from "./core-entitlements";

test("free features remain available when Pro licensing is absent or rejects them", () => {
  expect(withCoreEntitlements()).toEqual({
    library: true,
    colorways: false,
    slicerHandoff: true,
    revision: 0,
  });
  expect(
    withCoreEntitlements({ library: false, colorways: false, slicerHandoff: false, revision: 9 }),
  ).toEqual({ library: true, colorways: false, slicerHandoff: true, revision: 9 });
});

test("paid colorways remain controlled by the Pro entitlement", () => {
  expect(
    withCoreEntitlements({ library: false, colorways: true, slicerHandoff: false, revision: 4 }),
  ).toEqual({ library: true, colorways: true, slicerHandoff: true, revision: 4 });
});
