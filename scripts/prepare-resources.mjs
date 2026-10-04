import { mkdirSync, copyFileSync, existsSync } from "node:fs";
import { resolve } from "node:path";
import { createRequire } from "node:module";
import { spawnSync } from "node:child_process";
const root = process.cwd();
const pro = process.argv.includes("--pro");
const core = pro ? resolve(root, "core") : root;
const require = createRequire(resolve(core, "package.json"));
const wasm = require.resolve("occt-wasm/dist/occt-wasm.wasm");
for (const resourceRoot of pro ? [root, core] : [root]) {
  mkdirSync(resolve(resourceRoot, "resources/cad"), { recursive: true });
  copyFileSync(wasm, resolve(resourceRoot, "resources/cad/occt-wasm.wasm"));
}
const licenses = spawnSync(
  process.platform === "win32" ? "python" : "python3",
  [resolve(core, "scripts/prepare-licenses.py"), "--project", root],
  { stdio: "inherit" },
);
if (licenses.status !== 0) process.exit(licenses.status ?? 1);
if (!existsSync(resolve(root, "resources/licenses"))) throw new Error("Release licenses missing");
