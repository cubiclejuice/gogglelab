import { expect, test } from "bun:test";

test("every literal core desktop IPC invocation is present in the Tauri command registry", async () => {
  const [client, native] = await Promise.all([
    Bun.file(new URL("./ipc.ts", import.meta.url)).text(),
    Bun.file(new URL("../../src-tauri/src/lib.rs", import.meta.url)).text(),
  ]);
  const registry = native.split(".invoke_handler(tauri::generate_handler![")[1]?.split("])")[0];
  expect(registry).toBeDefined();
  const registered = new Set([...registry.matchAll(/(\w+)\s*,/g)].map((match) => match[1]));
  const invoked = new Set(
    [...client.matchAll(/\binvoke(?:<[^;]+?>)?\(\s*["']([a-z_]+)["']/g)].map((match) => match[1]),
  );
  expect(invoked.size).toBeGreaterThan(100);
  expect([...invoked].filter((command) => !registered.has(command)).sort()).toEqual([]);
});
