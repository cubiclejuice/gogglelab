import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { existsSync } from "node:fs";
const root = process.cwd();
const args = process.argv.slice(2);
const pro = args.includes("--pro");
const release = args.includes("--release");
const core = pro ? resolve(root, "core") : root;
const forwarded = args.filter((arg) => arg !== "--pro" && arg !== "--release");
const cli = resolve(root, "node_modules/@tauri-apps/cli/tauri.js");
const build = spawnSync(
  process.execPath,
  [cli, "build", ...(release ? ["--config", "tauri.release.conf.json"] : []), ...forwarded],
  { cwd: resolve(root, "src-tauri"), stdio: "inherit" },
);
if (build.status !== 0) process.exit(build.status ?? 1);
if (process.platform === "darwin") {
  const targetIndex = forwarded.indexOf("--target");
  const target = targetIndex >= 0 ? forwarded[targetIndex + 1] : "";
  const product = release ? (pro ? "GoggleLab Pro" : "GoggleLab Community") : "GoggleLab";
  const app = resolve(
    process.env.CARGO_TARGET_DIR ?? resolve(root, "target"),
    target,
    "release/bundle/macos",
    `${product}.app`,
  );
  if (existsSync(app)) {
    const patch = spawnSync("bash", [resolve(core, "scripts/patch-info-plist.sh"), app], {
      stdio: "inherit",
    });
    if (patch.status !== 0) process.exit(patch.status ?? 1);
    const sign = spawnSync("codesign", ["--force", "--deep", "--sign", "-", app], {
      stdio: "inherit",
    });
    if (sign.status !== 0) process.exit(sign.status ?? 1);
  }
}
