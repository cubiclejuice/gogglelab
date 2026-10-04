import argparse, hashlib, json, pathlib, shutil, subprocess
p=argparse.ArgumentParser();p.add_argument("--edition", required=True);p.add_argument("--target", required=True);p.add_argument("--platform", required=True);p.add_argument("--arch", required=True);a=p.parse_args()
root=pathlib.Path.cwd();version=json.loads((root/"package.json").read_text())["version"]
out=root/"release-assets";out.mkdir(exist_ok=True)
base=f"gogglelab-{a.edition}-{version}-{a.platform}-{a.arch}"
bundle=root/"target"/a.target/"release/bundle"
if a.platform=="macos":
 apps=list((bundle/"macos").glob("*.app"));assert len(apps)==1
 subprocess.run(["codesign","--verify","--deep","--strict",str(apps[0])],check=True)
 subprocess.run(["ditto","-c","-k","--sequesterRsrc","--keepParent",str(apps[0]),str(out/(base+".zip"))],check=True)
else:
 formats=[("nsis","*.exe","-setup.exe"),("msi","*.msi",".msi")] if a.platform=="windows" else [("deb","*.deb",".deb"),("appimage","*.AppImage",".AppImage")]
 for folder,pattern,suffix in formats:
  files=list((bundle/folder).glob(pattern));assert len(files)==1,(folder,files)
  shutil.copy2(files[0],out/(base+suffix))
manifest={f.name:hashlib.sha256(f.read_bytes()).hexdigest() for f in out.iterdir() if f.is_file()}
(out/(base+"-manifest.json")).write_text(json.dumps({"edition":a.edition,"version":version,"target":a.target,"assets":manifest},indent=2)+"\n")
