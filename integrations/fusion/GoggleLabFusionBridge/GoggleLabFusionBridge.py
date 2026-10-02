import json, os, re, shutil, tempfile, threading, time
from pathlib import Path
import adsk.core, adsk.fusion

ROOT=Path.home()/"Library/Application Support/GoggleLab/FusionBridge"
JOBS=ROOT/"jobs"; EVENT_ID="org.gogglelab.fusion-bridge.convert"
ID=re.compile(r"^[0-9a-f]{32}$"); MAX_SIZE=50*1024*1024
_app=_event=_handler=_worker=None; _stop=threading.Event(); _lock=threading.Lock(); _pending=None

def _ensure_spool():
    home=Path.home(); current=home
    for part in ROOT.relative_to(home).parts:
        current=current/part
        if current.is_symlink(): raise RuntimeError("Unsafe spool path")
        if current.exists():
            if not current.is_dir(): raise RuntimeError("Unsafe spool path")
        else: current.mkdir(mode=0o700)
    if JOBS.is_symlink(): raise RuntimeError("Unsafe jobs path")
    JOBS.mkdir(mode=0o700,exist_ok=True)
    os.chmod(ROOT,0o700); os.chmod(JOBS,0o700)

def _atomic(path, data):
    fd,name=tempfile.mkstemp(prefix=path.name+".",suffix=".tmp",dir=path.parent)
    tmp=Path(name)
    with os.fdopen(fd,"w",encoding="utf-8") as f:
        json.dump(data,f,separators=(",",":")); f.flush(); os.fsync(f.fileno())
    try: os.replace(tmp,path)
    finally:
        try: tmp.unlink(missing_ok=True)
        except OSError: pass

def _result(job,ok,error=None,warning=None):
    try:
        if job.is_symlink() or job.resolve(strict=True).parent!=JOBS.resolve(strict=True): return
    except OSError: return
    data={"ok":bool(ok)}
    if error: data["error"]=str(error)[:240]
    if warning: data["warning"]=str(warning)[:240]
    _atomic(job/"result.json",data)

def _child(path,parent):
    try: return path.resolve(strict=False).parent==parent.resolve(strict=True)
    except OSError: return False

def _validate(request_id):
    if not ID.fullmatch(request_id): raise ValueError("Invalid request identifier")
    job=JOBS/request_id; req=job/"request.json"; src=job/"source.f3d"; out=job/"conversion.step"
    if job.is_symlink() or not job.is_dir() or not all(_child(p,job) for p in (req,src,out)): raise ValueError("Unsafe job path")
    if any(p.is_symlink() for p in (req,src,out)): raise ValueError("Symbolic links are not allowed")
    if not req.is_file() or not src.is_file(): raise ValueError("Job input is incomplete")
    if time.time()-req.stat().st_mtime>120: raise ValueError("Job request expired")
    with req.open(encoding="utf-8") as f: metadata=json.load(f)
    if metadata!={"version":1}: raise ValueError("Unsupported request format")
    if src.stat().st_size>MAX_SIZE: raise ValueError("Fusion archive exceeds 50 MiB")
    return job,src,out

def _cancelled(job):
    p=job/"cancelled"; return p.exists() or p.is_symlink()

def _mark_document_open(job):
    marker=job/"document-open"
    try:
        fd=os.open(marker,os.O_CREAT|os.O_EXCL|os.O_WRONLY|getattr(os,"O_NOFOLLOW",0),0o600)
        os.close(fd)
    except FileExistsError: pass

def _cleanup_cancelled(job):
    try:
        if not ID.fullmatch(job.name) or job.is_symlink() or job.resolve(strict=True).parent!=JOBS.resolve(strict=True): return False
        for base,dirs,files in os.walk(job,followlinks=False):
            if any((Path(base)/name).is_symlink() for name in dirs+files): return False
        shutil.rmtree(job); return True
    except OSError: return False

def _janitor(limit=20):
    now=time.time(); removed=0
    try: entries=list(JOBS.iterdir())
    except OSError: return
    for job in entries:
        if removed>=limit or not ID.fullmatch(job.name) or job.is_symlink() or not job.is_dir() or (job/"document-open").exists(): continue
        try:
            old=now-job.stat().st_mtime>600
            removable=not (job/"processing").exists() or (job/"result.json").is_file()
            if old and removable and _cleanup_cancelled(job): removed+=1
        except OSError: continue

def _process(request_id):
    global _pending
    job=None; imported=previous=None; ok=False; error=None; released=True
    try:
        if not isinstance(request_id,str): raise ValueError("Invalid request identifier")
        job=JOBS/request_id
        job,src,out=_validate(request_id)
        if _cancelled(job): raise RuntimeError("Conversion cancelled")
        previous=_app.activeDocument
        options=_app.importManager.createFusionArchiveImportOptions(str(src))
        if not options: raise RuntimeError("Fusion could not prepare the archive import")
        imported=_app.importManager.importToNewDocument(options)
        if not imported: raise RuntimeError("Fusion could not import the archive")
        if _cancelled(job): raise RuntimeError("Conversion cancelled")
        design=adsk.fusion.Design.cast(imported.products.itemByProductType("DesignProductType"))
        if not design: raise RuntimeError("Imported document is not a Fusion design")
        manager=design.exportManager; options=manager.createSTEPExportOptions(str(out))
        if not options or not manager.execute(options): raise RuntimeError("Fusion STEP export failed")
        if _cancelled(job):
            try: out.unlink(missing_ok=True)
            except OSError: pass
            raise RuntimeError("Conversion cancelled")
        if out.is_symlink() or not out.is_file() or out.stat().st_size==0 or not _child(out,job):
            raise RuntimeError("Fusion did not produce a valid STEP file")
        ok=True
    except Exception as exc:
        error=str(exc) or "Fusion conversion failed"
    finally:
        cleanup_error=None; restore_previous=False
        if imported is not None:
            try: restore_previous=bool(imported.isActive)
            except Exception: restore_previous=False
            try:
                released=bool(imported.close(False))
                if not released: cleanup_error="Could not close the imported document"
            except Exception: released=False; cleanup_error="Could not close the imported document"
        if previous is not None and restore_previous:
            try:
                if previous.isValid and not previous.activate(): cleanup_error="Could not restore the previous document"
            except Exception: cleanup_error="Could not restore the previous document"
        if cleanup_error:
            ok=False; error=cleanup_error
            if not released and job is not None: _mark_document_open(job)
        if job is not None:
            try:
                _result(job,ok,error)
            except Exception: pass
            if released and _cancelled(job): _cleanup_cancelled(job)
        with _lock: _pending=None

class _Handler(adsk.core.CustomEventHandler):
    def __init__(self): super().__init__()
    def notify(self,args): _process(args.additionalInfo)

def _claim():
    global _pending
    try: entries=sorted(JOBS.iterdir(),key=lambda p:p.stat().st_mtime)
    except OSError: return
    for job in entries:
        rid=job.name
        if not ID.fullmatch(rid) or not (job/"request.json").is_file() or (job/"result.json").exists() or (job/"processing").exists() or (job/"document-open").exists(): continue
        try:
            _validate(rid)
            if _cancelled(job): _result(job,False,"Conversion cancelled"); continue
            fd=os.open(job/"processing",os.O_CREAT|os.O_EXCL|os.O_WRONLY,0o600); os.close(fd)
        except FileExistsError: continue
        except Exception as exc:
            try: _result(job,False,str(exc) or "Invalid conversion job")
            except Exception: pass
            continue
        with _lock:
            if _pending is not None:
                try: (job/"processing").unlink()
                except OSError: pass
                return
            _pending=rid
        try:
            # Autodesk's Python sample treats this as a notification. A falsy
            # return can occur even though the main-thread handler is running.
            # Only that handler may publish completion after document cleanup.
            _app.fireCustomEvent(EVENT_ID,rid)
        except Exception as exc:
            with _lock: _pending=None
            _result(job,False,"Fusion could not queue the conversion: "+str(exc)[:160])
        return

def _loop():
    _ensure_spool()
    for job in JOBS.iterdir():
        if ID.fullmatch(job.name) and job.is_dir() and not job.is_symlink() and not (job/"result.json").exists() and not (job/"document-open").exists():
            try: (job/"processing").unlink(missing_ok=True)
            except OSError: pass
    _janitor()
    ticks=0
    while not _stop.is_set():
        try:
            _atomic(ROOT/"ready.json",{"version":1,"updated_at":int(time.time())})
            with _lock: idle=_pending is None
            if idle: _claim()
            ticks+=1
            if ticks>=120: _janitor(); ticks=0
        except Exception: pass
        _stop.wait(.5)

def run(context):
    global _app,_event,_handler,_worker,_pending
    with _lock: _pending=None
    _app=adsk.core.Application.get(); _event=_app.registerCustomEvent(EVENT_ID)
    if not _event: return
    _handler=_Handler(); _event.add(_handler); _stop.clear()
    _worker=threading.Thread(target=_loop,name="GoggleLabFusionBridge",daemon=True); _worker.start()

def stop(context):
    global _event,_handler,_worker,_pending
    _stop.set()
    if _worker: _worker.join(timeout=2)
    if _event and _handler:
        try: _event.remove(_handler)
        except Exception: pass
    if _app:
        try: _app.unregisterCustomEvent(EVENT_ID)
        except Exception: pass
    try: (ROOT/"ready.json").unlink(missing_ok=True)
    except OSError: pass
    _event=_handler=_worker=None
    with _lock: _pending=None
