import json, os, pathlib, subprocess, sys
if sys.platform != "win32":
    subprocess.run(["cargo", "test", "--workspace", "--locked"], check=True)
else:
    # Tauri's resource helper links manifests to app binaries, not lib test harnesses.
    # Embed the same Common Controls dependency in test artifacts before executing.
    build = subprocess.run(["cargo", "test", "--workspace", "--locked", "--no-run", "--message-format=json"], check=True, stdout=subprocess.PIPE, text=True, encoding="utf-8")
    artifacts = [json.loads(line) for line in build.stdout.splitlines() if line.startswith("{")]
    executables = [pathlib.Path(item["executable"]) for item in artifacts if item.get("reason") == "compiler-artifact" and item.get("profile", {}).get("test") and item.get("executable")]
    sdk = pathlib.Path(os.environ.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Windows Kits/10/bin"
    tools = sorted(sdk.glob("*/x64/mt.exe"))
    if not tools: raise RuntimeError("Windows SDK manifest tool is missing")
    target = pathlib.Path(os.environ.get("CARGO_TARGET_DIR", "target"))
    manifest = target / "windows-unit-tests.manifest"
    manifest.write_text('''<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
<dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*" /></dependentAssembly></dependency>
</assembly>
''', encoding="utf-8")
    if not executables: raise RuntimeError("Native test executables are missing")
    for executable in executables:
        subprocess.run([str(tools[-1]), "-nologo", "-manifest", str(manifest), "-outputresource:" + str(executable) + ";#1"], check=True)
    print(f"Prepared Windows manifests for {len(executables)} test artifacts")
    for executable in executables:
        subprocess.run([str(executable)], check=True)
    subprocess.run(["cargo", "test", "--workspace", "--locked", "--doc"], check=True)
