import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { existsSync } from "node:fs";
import { isHelpRequest, parseBuildArguments } from "./build-app-args.mjs";
const root = process.cwd();
const { pro, release, forwarded, tauriArgs } = parseBuildArguments(process.argv.slice(2));
const core = pro ? resolve(root, "core") : root;
const helpRequest = isHelpRequest(forwarded);
const cli = resolve(root, "node_modules/@tauri-apps/cli/tauri.js");
const build = spawnSync(process.execPath, [cli, ...tauriArgs], {
  cwd: resolve(root, "src-tauri"),
  stdio: "inherit",
});
if (build.status !== 0) process.exit(build.status ?? 1);
if (process.platform === "darwin" && !helpRequest) {
  const targetIndex = forwarded.indexOf("--target");
  const target = targetIndex >= 0 ? forwarded[targetIndex + 1] : "";
  const product = release ? (pro ? "GoggleLab" : "GoggleLab Community") : "GoggleLab";
  const app = resolve(
    process.env.CARGO_TARGET_DIR ?? resolve(root, "target"),
    target,
    "release/bundle/macos",
    `${product}.app`,
  );
  if (!existsSync(app)) {
    console.error(`Expected macOS app bundle was not created: ${app}`);
    process.exit(1);
  }
  const patch = spawnSync("bash", [resolve(core, "scripts/patch-info-plist.sh"), app], {
    stdio: "inherit",
  });
  if (patch.status !== 0) process.exit(patch.status ?? 1);
  const expectedIdentifier =
    release && !pro ? "com.cubiclejuice.gogglelab.community" : "com.sonicparke.stl-handoff";
  const identifier = spawnSync(
    "/usr/libexec/PlistBuddy",
    ["-c", "Print :CFBundleIdentifier", resolve(app, "Contents/Info.plist")],
    { encoding: "utf8" },
  );
  if (identifier.status !== 0 || identifier.stdout.trim() !== expectedIdentifier) {
    console.error(`Unexpected macOS bundle identifier: ${identifier.stdout.trim()}`);
    process.exit(identifier.status && identifier.status !== 0 ? identifier.status : 1);
  }
  const sign = spawnSync("codesign", ["--force", "--deep", "--sign", "-", app], {
    stdio: "inherit",
  });
  if (sign.status !== 0) process.exit(sign.status ?? 1);
}
