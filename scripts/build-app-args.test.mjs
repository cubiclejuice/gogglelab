import { expect, test } from "bun:test";
import { isHelpRequest, parseBuildArguments } from "./build-app-args.mjs";

test("identifies Tauri help invocations before platform post-processing", () => {
  expect(isHelpRequest(["--help"])).toBe(true);
  expect(isHelpRequest(["-h"])).toBe(true);
  expect(isHelpRequest(["--bundles", "nsis,msi", "--no-sign"])).toBe(false);
});

test("routes unsigned Windows test installers through the Community release config", () => {
  expect(parseBuildArguments(["--release", "--bundles", "nsis,msi", "--no-sign"])).toEqual({
    pro: false,
    release: true,
    forwarded: ["--bundles", "nsis,msi", "--no-sign"],
    tauriArgs: [
      "build",
      "--config",
      "tauri.release.conf.json",
      "--bundles",
      "nsis,msi",
      "--no-sign",
    ],
  });
});
