import argparse, pathlib, subprocess, tempfile, time
p=argparse.ArgumentParser();p.add_argument("binary");a=p.parse_args()
with tempfile.TemporaryDirectory(prefix="gogglelab-smoke-") as d:
 model=pathlib.Path(d)/"sample.stl"
 model.write_text("solid sample\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 10 0 0\nvertex 0 10 0\nendloop\nendfacet\nendsolid sample\n")
 log=pathlib.Path("release-smoke.log")
 with log.open("w") as stream:
  process=subprocess.Popen([str(pathlib.Path(a.binary).resolve()),str(model)],stdout=stream,stderr=stream)
  try:
   time.sleep(10)
   if process.poll() is not None:raise RuntimeError(f"Desktop app exited during startup: {process.returncode}; see {log}")
   print("Desktop process launched with an STL and remained running for 10 seconds.")
  finally:
   if process.poll() is None:process.terminate()
   try:process.wait(timeout=10)
   except subprocess.TimeoutExpired:process.kill();process.wait()
