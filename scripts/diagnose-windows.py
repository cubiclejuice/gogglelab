import ctypes, glob, json, pathlib, struct
kernel=ctypes.WinDLL("kernel32",use_last_error=True)
kernel.LoadLibraryW.argtypes=[ctypes.c_wchar_p];kernel.LoadLibraryW.restype=ctypes.c_void_p
kernel.GetProcAddress.argtypes=[ctypes.c_void_p,ctypes.c_char_p];kernel.GetProcAddress.restype=ctypes.c_void_p
kernel.GetModuleFileNameW.argtypes=[ctypes.c_void_p,ctypes.c_wchar_p,ctypes.c_uint];kernel.GetModuleFileNameW.restype=ctypes.c_uint
for name in glob.glob("target/debug/deps/gogglelab*_lib-*.exe"):
 b=pathlib.Path(name).read_bytes();pe=struct.unpack_from("<I",b,0x3c)[0];opt=pe+24
 sections=struct.unpack_from("<H",b,pe+6)[0];size=struct.unpack_from("<H",b,pe+20)[0];magic=struct.unpack_from("<H",b,opt)[0]
 table=[]
 for i in range(sections):
  p=opt+size+40*i;vs,va,rs,raw=struct.unpack_from("<IIII",b,p+8);table.append((va,max(vs,rs),raw))
 def off(rva):
  for va,n,raw in table:
   if va<=rva<va+n:return raw+rva-va
  return rva
 def string(rva):
  p=off(rva);return b[p:b.index(0,p)].decode("ascii")
 directory=opt+(112 if magic==0x20b else 96)+8
 rva=struct.unpack_from("<I",b,directory)[0];p=off(rva);step=8 if magic==0x20b else 4;fmt="<Q" if step==8 else "<I"
 missing=[]
 while True:
  lookup,_,_,dll_rva,first=struct.unpack_from("<IIIII",b,p);p+=20
  if not dll_rva:break
  dll=string(dll_rva);handle=kernel.LoadLibraryW(dll);q=off(lookup or first)
  buffer=ctypes.create_unicode_buffer(32768)
  if handle:kernel.GetModuleFileNameW(handle,buffer,len(buffer))
  print("DLL",dll,"loaded",buffer.value or f"ERROR {ctypes.get_last_error()}")
  while True:
   thunk=struct.unpack_from(fmt,b,q)[0];q+=step
   if not thunk:break
   if thunk & (1<<(step*8-1)):continue
   symbol=string(thunk+2)
   if not handle or not kernel.GetProcAddress(handle,symbol.encode("ascii")):
    missing.append({"dll":dll,"symbol":symbol,"path":buffer.value})
 print("MISSING IMPORTS",json.dumps({"executable":name,"imports":missing},indent=2))
