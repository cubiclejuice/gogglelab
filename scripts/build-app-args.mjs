export function isHelpRequest(args) {
  return args.some((arg) => arg === "--help" || arg === "-h");
}

export function parseBuildArguments(args) {
  const pro = args.includes("--pro");
  const release = args.includes("--release");
  const forwarded = args.filter((arg) => arg !== "--pro" && arg !== "--release");

  return {
    pro,
    release,
    forwarded,
    tauriArgs: ["build", ...(release ? ["--config", "tauri.release.conf.json"] : []), ...forwarded],
  };
}
