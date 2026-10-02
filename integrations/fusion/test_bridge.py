import importlib.util,json,os,sys,tempfile,time,types,unittest
from pathlib import Path
class Handler: pass
class Design:
    @staticmethod
    def cast(x): return x
adsk=types.ModuleType("adsk"); adsk.core=types.ModuleType("adsk.core"); adsk.fusion=types.ModuleType("adsk.fusion")
adsk.core.CustomEventHandler=Handler; adsk.core.Application=types.SimpleNamespace(get=lambda:None); adsk.fusion.Design=Design
sys.modules.update({"adsk":adsk,"adsk.core":adsk.core,"adsk.fusion":adsk.fusion})
spec=importlib.util.spec_from_file_location("bridge",Path(__file__).parent/"GoggleLabFusionBridge/GoggleLabFusionBridge.py")
b=importlib.util.module_from_spec(spec); spec.loader.exec_module(b)
class Doc:
    def __init__(self,design=None): self.isValid=True; self.isActive=True; self.closed=0; self.activated=0; self.products=types.SimpleNamespace(itemByProductType=lambda _:design)
    def close(self,_): self.closed+=1; return True
    def activate(self): self.activated+=1; return True
class Tests(unittest.TestCase):
    def setUp(self):
        self.t=tempfile.TemporaryDirectory(); b.ROOT=Path(self.t.name); b.JOBS=b.ROOT/"jobs"; b.JOBS.mkdir(); b._pending="busy"
    def tearDown(self): self.t.cleanup()
    def job(self):
        j=b.JOBS/("a"*32); j.mkdir(); (j/"request.json").write_text('{"version":1}'); (j/"source.f3d").write_bytes(b"x"); return j
    def test_failed_import_never_closes_user_document(self):
        j=self.job(); user=Doc(); m=types.SimpleNamespace(createFusionArchiveImportOptions=lambda _:object(),importToNewDocument=lambda _:None); b._app=types.SimpleNamespace(activeDocument=user,importManager=m); b._process(j.name); self.assertEqual(user.closed,0); self.assertEqual(user.activated,0); self.assertFalse(json.loads((j/"result.json").read_text())["ok"])
    def test_success_closes_import_and_restores_user(self):
        j=self.job(); user=Doc(); e=types.SimpleNamespace(createSTEPExportOptions=lambda p:p,execute=lambda p:(Path(p).write_bytes(b"step") or True)); imported=Doc(types.SimpleNamespace(exportManager=e)); m=types.SimpleNamespace(createFusionArchiveImportOptions=lambda _:object(),importToNewDocument=lambda _:imported); b._app=types.SimpleNamespace(activeDocument=user,importManager=m); b._process(j.name); self.assertEqual((imported.closed,user.activated),(1,1)); self.assertTrue(json.loads((j/"result.json").read_text())["ok"])
    def test_symlink_rejected(self):
        j=self.job(); (j/"source.f3d").unlink(); (j/"source.f3d").symlink_to(Path(self.t.name)/"out"); self.assertRaises(ValueError,b._validate,j.name)
    def test_job_symlink_cannot_write_result_outside_spool(self):
        outside=Path(self.t.name)/"outside"; outside.mkdir(); (b.JOBS/("b"*32)).symlink_to(outside, target_is_directory=True); b._result(b.JOBS/("b"*32),False,"bad"); self.assertFalse((outside/"result.json").exists())
    def test_cancel_prevents_import(self):
        j=self.job(); (j/"cancelled").touch(); calls=[]; b._app=types.SimpleNamespace(activeDocument=Doc(),importManager=types.SimpleNamespace(createFusionArchiveImportOptions=lambda _:calls.append(1))); b._process(j.name); self.assertEqual(calls,[])
    def test_false_export_reports_error(self):
        j=self.job(); e=types.SimpleNamespace(createSTEPExportOptions=lambda _:object(),execute=lambda _:False); imported=Doc(types.SimpleNamespace(exportManager=e)); b._app=types.SimpleNamespace(activeDocument=Doc(),importManager=types.SimpleNamespace(createFusionArchiveImportOptions=lambda _:object(),importToNewDocument=lambda _:imported)); b._process(j.name); r=json.loads((j/"result.json").read_text()); self.assertFalse(r["ok"]); self.assertIn("export",r["error"].lower())
    def test_stale_rejected(self):
        j=self.job(); old=time.time()-121; os.utime(j/"request.json",(old,old)); self.assertRaises(ValueError,b._validate,j.name)
    def test_incomplete_job_is_not_claimed(self):
        j=b.JOBS/("c"*32); j.mkdir(); b._app=types.SimpleNamespace(); b._claim(); self.assertFalse((j/"result.json").exists())
    def test_result_is_published_after_cleanup(self):
        j=self.job(); seen=[]; user=Doc(); user.activate=lambda:(seen.append((j/"result.json").exists()) or True); e=types.SimpleNamespace(createSTEPExportOptions=lambda p:p,execute=lambda p:(Path(p).write_bytes(b"step") or True)); imported=Doc(types.SimpleNamespace(exportManager=e)); imported.close=lambda _:(seen.append((j/"result.json").exists()) or True); b._app=types.SimpleNamespace(activeDocument=user,importManager=types.SimpleNamespace(createFusionArchiveImportOptions=lambda _:object(),importToNewDocument=lambda _:imported)); b._process(j.name); self.assertEqual(seen,[False,False]); self.assertTrue((j/"result.json").exists())
    def test_cancel_after_import_cleans_job_after_close_restore(self):
        j=self.job(); order=[]; user=Doc(); user.activate=lambda:(order.append("restore") or True); imported=Doc(); imported.close=lambda _:(order.append("close") or True)
        def execute(path): Path(path).write_bytes(b"step"); (j/"cancelled").touch(); return True
        imported.products=types.SimpleNamespace(itemByProductType=lambda _:types.SimpleNamespace(exportManager=types.SimpleNamespace(createSTEPExportOptions=lambda p:p,execute=execute)))
        b._app=types.SimpleNamespace(activeDocument=user,importManager=types.SimpleNamespace(createFusionArchiveImportOptions=lambda _:object(),importToNewDocument=lambda _:imported)); b._process(j.name); self.assertEqual(order,["close","restore"]); self.assertFalse(j.exists())
    def test_user_switch_does_not_steal_focus(self):
        j=self.job(); user=Doc(); imported=Doc(); imported.isActive=False; e=types.SimpleNamespace(createSTEPExportOptions=lambda p:p,execute=lambda p:(Path(p).write_bytes(b"step") or True)); imported.products=types.SimpleNamespace(itemByProductType=lambda _:types.SimpleNamespace(exportManager=e)); b._app=types.SimpleNamespace(activeDocument=user,importManager=types.SimpleNamespace(createFusionArchiveImportOptions=lambda _:object(),importToNewDocument=lambda _:imported)); b._process(j.name); self.assertEqual(user.activated,0)
    def test_close_failure_is_terminal_error(self):
        j=self.job(); imported=Doc(); imported.close=lambda _:False; e=types.SimpleNamespace(createSTEPExportOptions=lambda p:p,execute=lambda p:(Path(p).write_bytes(b"step") or True)); imported.products=types.SimpleNamespace(itemByProductType=lambda _:types.SimpleNamespace(exportManager=e)); b._app=types.SimpleNamespace(activeDocument=Doc(),importManager=types.SimpleNamespace(createFusionArchiveImportOptions=lambda _:object(),importToNewDocument=lambda _:imported)); original=b._result; observed=[]
        def result(*args,**kwargs): observed.append((j/"document-open").exists()); return original(*args,**kwargs)
        b._result=result
        try: b._process(j.name)
        finally: b._result=original
        response=json.loads((j/"result.json").read_text()); self.assertEqual(observed,[True]); self.assertTrue((j/"document-open").exists()); self.assertFalse(response["ok"]); self.assertIn("close",response["error"].lower())

    def test_falsy_queue_return_does_not_publish_failure_before_handler(self):
        job=self.job(); queued=[]; b._pending=None
        b._app=types.SimpleNamespace(fireCustomEvent=lambda event,rid:(queued.append(rid) and False))
        b._claim()
        self.assertEqual(queued,[job.name])
        self.assertEqual(b._pending,job.name)
        self.assertFalse((job/"result.json").exists())
    def test_queue_return_cannot_overwrite_completed_handler_result(self):
        job=self.job(); b._pending=None; user=Doc()
        export=types.SimpleNamespace(createSTEPExportOptions=lambda p:p,execute=lambda p:(Path(p).write_bytes(b"step") or True))
        imported=Doc(types.SimpleNamespace(exportManager=export))
        def fire(event,rid): b._process(rid); return False
        b._app=types.SimpleNamespace(activeDocument=user,fireCustomEvent=fire,importManager=types.SimpleNamespace(createFusionArchiveImportOptions=lambda _:object(),importToNewDocument=lambda _:imported))
        b._claim()
        self.assertTrue(json.loads((job/"result.json").read_text())["ok"])
        self.assertEqual(imported.closed,1)
    def test_queue_exception_reports_failure_and_releases_pending(self):
        job=self.job(); b._pending=None
        def fire(event,rid): raise RuntimeError("event unavailable")
        b._app=types.SimpleNamespace(fireCustomEvent=fire)
        b._claim()
        self.assertIsNone(b._pending)
        self.assertFalse(json.loads((job/"result.json").read_text())["ok"])

if __name__=="__main__": unittest.main()
