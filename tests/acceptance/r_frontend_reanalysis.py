"""Explicit named R migration gate; downloads pinned public sources only in CI."""
from __future__ import annotations
import argparse,csv,hashlib,json,os,re,subprocess,tarfile,tempfile,urllib.request
from pathlib import Path,PurePosixPath

PROJECT="0d1851599764ec6df07dbba7ce92a36edb3695a2"
LEGACY="3ac29595d0e39552495d863661b99ea015d59d3d"
FILES={
 "01_weeds_process-raw-data-PIVI.R":("65e6b39153fc0a07ab74da98cafd9034a20eb5b0","fb0638fd297973280bee9fe0469036092588ea029fbdcc24f9f0271c56e8ebf2"),
 "04_weeds_biomass-vs-density.R":("e37407a9da38dcd3e973677016192bce87b577db","0eca9374c474decbf124ba8dab915943a417c52c891db4e7a70034a3b0023c43"),
 "data-raw-PIVI.csv":("6b746e511982ffd1c4991afa12e86be3586f051c",None)}
OUTPUTS={"data-tidy-PIVI.csv":265,"data-bio-den-paired.csv":236}
MARKER="M4_WEISBERGER_ROWS:265:236"
FENCE=chr(96)*3
CHUNK=r"""
legacy <- identical(Sys.getenv("PROJCLI_M4_BACKEND"),"legacy")
settings <- projcli::projr_yml_get(check=TRUE,
 backend=if(legacy)projr::projr_yml_get else "cli",
 binary=if(legacy)NULL else Sys.getenv("PROJCLI_M4_CLI"))
expected <- projcli::projr_par_get("expected",
 backend=if(legacy)projr::projr_par_get else "cli",
 binary=if(legacy)NULL else Sys.getenv("PROJCLI_M4_CLI"))
stopifnot(is.list(settings),settings$directories[["raw-data"]]$path=="_raw_data",
 expected$pivi==265L,expected$paired==236L)
lookup <- function(label,...) projcli::projr_path_get(label,...,create=FALSE,
 format="absolute",safe=TRUE,backend=if(legacy)projr::projr_path_get else "cli",
 binary=if(legacy)NULL else Sys.getenv("PROJCLI_M4_CLI"))
target <- lookup("output","analysis")
dir.create(target,recursive=TRUE,showWarnings=FALSE)
root <- Sys.getenv("PROJCLI_M4_CASE_ROOT")
scripts <- c("01_weeds_process-raw-data-PIVI.R","04_weeds_biomass-vs-density.R")
stopifnot(all(file.copy(file.path(root,"science",scripts),target,overwrite=TRUE)))
stopifnot(file.copy(lookup("raw-data","data-raw-PIVI.csv"),target,overwrite=TRUE))
file.create(file.path(target,".here"))
for(script in scripts) {
 status <- processx::run(file.path(R.home("bin"),"Rscript"),c("--vanilla",script),wd=target,
   stdout=file.path(target,paste0(script,".stdout")),stderr=file.path(target,paste0(script,".stderr")),
   error_on_status=FALSE)
 if(status$status!=0L)stop("Pinned science script failed: ",script)
}
counts <- vapply(c("data-tidy-PIVI.csv","data-bio-den-paired.csv"),
 function(name)nrow(read.csv(file.path(target,name),check.names=FALSE)),integer(1))
stopifnot(identical(unname(counts),as.integer(c(expected$pivi,expected$paired))))
"""
REPORT=("\n".join(["---","title: Two existing weed-study processing scripts","output: html_document","---","",
 FENCE+"{r,include=FALSE}",CHUNK,FENCE,"","This comparison covers scripts 01 and 04 only.","",
 FENCE+"{r,echo=FALSE,results='asis'}","cat(paste0('M4_WEISBERGER_ROWS:',counts[[1]],':',counts[[2]]))",FENCE,""]))

def digest(data):return hashlib.sha256(data).hexdigest()
def inventory(path):
 return {p.relative_to(path).as_posix():digest(p.read_bytes()) for p in sorted(path.rglob("*")) if p.is_file()}
def csv_data(path):
 with path.open(newline="",encoding="utf-8-sig") as stream:rows=list(csv.reader(stream))
 assert len(rows)>1 and len({len(row) for row in rows})==1,"CSV must be rectangular"
 return rows
def summarize(rows):
 encoded=json.dumps(rows,ensure_ascii=False,separators=(",",":")).encode()
 return {"rows":len(rows)-1,"columns":len(rows[0]),"ordered_cells_sha256":digest(encoded)}
def download(url,limit):
 with urllib.request.urlopen(url,timeout=60) as response:data=response.read(limit+1)
 assert len(data)<=limit,"Source exceeds explicit size limit"
 return data
def extract_package(data,directory):
 import io
 total=0;roots=set()
 with tarfile.open(fileobj=io.BytesIO(data),mode="r:gz") as archive:
  for item in archive.getmembers():
   relative=PurePosixPath(item.name)
   assert not relative.is_absolute() and ".." not in relative.parts
   roots.add(relative.parts[0])
   target=directory.joinpath(*relative.parts)
   assert target.resolve().is_relative_to(directory.resolve())
   if item.isdir():target.mkdir(parents=True,exist_ok=True)
   elif item.isfile():
    assert item.size<=16*1024**2;total+=item.size;assert total<=128*1024**2
    target.parent.mkdir(parents=True,exist_ok=True)
    stream=archive.extractfile(item);assert stream is not None
    target.write_bytes(stream.read());target.chmod(item.mode & 0o777)
   else:raise AssertionError("Source archive contains unsupported links or special entries")
 assert len(roots)==1
 result=directory/next(iter(roots));assert result.name=="projr-"+LEGACY
 assert (result/"DESCRIPTION").is_file()
 return result
def project(root,source,legacy,store):
 root.mkdir();(root/"science").mkdir();(root/"_raw_data").mkdir();(root/"library").mkdir()
 for name,data in source.items():
  folder=root/"_raw_data" if name.endswith(".csv") else root/"science"
  (folder/name).write_bytes(data)
 (root/"bounded.Rmd").write_text(REPORT)
 (root/"M4.Rproj").write_text("Version: 1.0\n")
 # Clean projr reads a plain version line; the CLI accepts this format too.
 (root/"VERSION").write_bytes(b"v1.2.3-7\n")
 directories={label:{"path":path} for label,path in
  [("raw-data","_raw_data"),("output","_output"),("docs","_docs"),("cache","_tmp")]}
 if legacy:
  cfg={"directories":directories,"build":{"scripts":["bounded.Rmd"],"renv":False,
   "git":{"commit":False,"push":False},"restrictions":{"not_behind":False}}}
  name="_projr.yml"
 else:
  directories["cache"]["safe"]=True
  cfg={"directories":directories,"remotes":{"local":{"backup":{"path":str(store),
    "structure":"archive","send":{"inspect":"file"}}}},
    "build":{"scripts":["bounded.Rmd"],"git":False,"dest":{"backup":{"content":["raw-data","output","docs"]}}}}
  name="_proj.yml"
 cfg["parameters"]={"expected":{"pivi":265,"paired":236}}
 (root/name).write_text(json.dumps(cfg))
 return cfg
def find_preview(root,name):
 matches=list((root/"_tmp").rglob(name));assert len(matches)==1,(name,len(matches))
 return matches[0]
def check_rows(rows):
 for name,count in OUTPUTS.items():assert len(rows[name])-1==count,(name,len(rows[name])-1)
def report_marker(root,preview,directory=None):
 matches=[directory/"bounded.html"] if directory else list((root/("_tmp" if preview else "_docs")).rglob("bounded.html"))
 assert len(matches)==1
 data=matches[0].read_bytes();assert MARKER.encode() in data
 return {"bytes":len(data),"sha256":digest(data),"expected_rows_visible":True}
def main():
 parser=argparse.ArgumentParser();parser.add_argument("--binary",type=Path,required=True)
 parser.add_argument("--evidence",type=Path,required=True);args=parser.parse_args()
 assert os.environ.get("PROJCLI_R_REANALYSIS_REQUIRED")=="1","Explicit gate selection required"
 binary=args.binary.resolve(strict=True);driver=Path(__file__).with_suffix(".R").resolve(strict=True)
 args.evidence.mkdir(parents=True,exist_ok=False)
 evidence={"complete":False,"scope":"Weisberger unchanged scripts01+04 and owned report harness",
  "project_commit":PROJECT,"legacy_commit":LEGACY,"runtime_image":"sha256:a58a8a4dd6f0be83879d08b3a43ab92e256425bbfb81372b257d77a7ff9077f9",
  "runtime_R":"4.6.1","historical_R_4_1_1_reproduction":False,
  "retained":"metadata only; no original scripts, datasets, generated CSVs, reports, libraries or stores"}
 try:
  with tempfile.TemporaryDirectory(prefix="projcli-m4-weisberger-") as temp:
   base=Path(temp);home=base/"home";home.mkdir()
   env=os.environ.copy()
   for key in list(env):
    if key.upper().startswith(("GIT_","GH_","GITHUB_","ACTIONS_","SSH_","AWS_","AZURE_","PROJR_","PROJCLI_","RENV_")) or key in ["PROJ_PROJECT_ROOT","PROJ_RUN_CONTEXT","QUARTO_PROFILE"]:
     env.pop(key,None)
   environment={"RENV_PATHS_ROOT":str(base/"renv-state"),"RENV_PATHS_CACHE":str(base/"renv-cache"),
    "RENV_CONFIG_CACHE_ENABLED":"TRUE","RENV_CONFIG_SANDBOX_ENABLED":"FALSE",
    "RENV_CONFIG_SYSREQS_CHECK":"FALSE","RENV_CONFIG_AUTO_SNAPSHOT":"FALSE","TZ":"UTC"}
   env.update(environment);env["PROJCLI_R_REANALYSIS_REQUIRED"]="1";env.update(HOME=str(home),USERPROFILE=str(home),XDG_CONFIG_HOME=str(home))
   seed=base/"seed";legacy_library=base/"legacy-library";bootstrap=base/"bootstrap"
   source={};source_evidence={}
   for name,(blob,sha) in FILES.items():
    data=download(f"https://raw.githubusercontent.com/vanichols/Weisberger-et-al-2019/{PROJECT}/{name}",2*1024**2)
    assert hashlib.sha1(b"blob "+str(len(data)).encode()+b"\0"+data).hexdigest()==blob
    if sha:assert digest(data)==sha
    source[name]=data;source_evidence[name]={"git_blob":blob,"sha256":digest(data),"bytes":len(data)}
   evidence["source_files"]=source_evidence
   package=download(f"https://codeload.github.com/SATVILab/projr/tar.gz/{LEGACY}",32*1024**2)
   legacy_source=extract_package(package,base/"legacy-source")
   evidence["legacy_archive_sha256"]=digest(package)
   def run(phase,root=None,backend="cli"):
    print("M4 phase:",phase,"backend:",backend,flush=True)
    name=phase+"-"+(root.name if root else "setup");spec=base/(name+".json");result=base/(name+"-result.json")
    data={"phase":phase,"project":str(root) if root else None,"backend":backend,"binary":str(binary),
      "environment":environment,"bootstrap":str(bootstrap),"seed":str(seed),"legacy_library":str(legacy_library),
      "legacy_source":str(legacy_source),"result":str(result),"install_stdout":str(base/"legacy-install.out"),
      "install_stderr":str(base/"legacy-install.err")}
    spec.write_text(json.dumps(data))
    with (base/(name+".out")).open("wb") as stdout,(base/(name+".err")).open("wb") as stderr:
     status=subprocess.run(["Rscript","--vanilla",str(driver),str(spec)],cwd=root or base,env=env,stdin=subprocess.DEVNULL,
                           stdout=stdout,stderr=stderr,timeout=1800)
    reply=json.loads(result.read_text()) if result.exists() else {"ok":False,"phase":phase,"condition":"no_result"}
    if status.returncode!=0 or not reply.get("ok"):
     evidence["failed_phase"]=reply
     raise AssertionError({k:reply.get(k) for k in ["phase","action","condition","package","diagnostic"]})
    return reply
   prepared=run("prepare");evidence["matched_environment"]=prepared
   lock=(seed/"renv.lock").read_bytes();evidence["lockfile_sha256"]=digest(lock)
   # This controlled snapshot has public package records. Never publish uncertain
   # paths/credential-bearing URLs merely because renv normally avoids them.
   def public_values(value):
    if isinstance(value,dict):
     for key,child in value.items():
      if re.search(r"password|secret|credential|access.?token",key,re.I):return False
      if not public_values(child):return False
    elif isinstance(value,list):return all(public_values(x) for x in value)
    elif isinstance(value,str):
     if value.startswith(("/","file:")) or re.match(r"^[A-Za-z]:[/\\]",value):return False
     if re.search(r"https?://[^/\s]*@|[?&](?:token|signature|password|credential)=",value,re.I):return False
    return True
   lock_json=json.loads(lock)
   if public_values(lock_json):
    (args.evidence/"matched-renv.lock.json").write_bytes(lock);evidence["full_public_lock_retained"]=True
   else:evidence["full_public_lock_retained"]=False
   store=base/"store";results={};observations={}
   for backend in ["legacy","cli"]:
    root=base/backend;project(root,source,backend=="legacy",store)
    (root/"renv.lock").write_bytes(lock)
    receipt=run("restore-packages",root,backend)
    assert receipt["receipt"]["lockfile_md5"]==hashlib.md5(lock).hexdigest()
    original_version=(root/"VERSION").read_bytes()
    run("preview",root,backend)
    preview={name:csv_data(find_preview(root,name)) for name in OUTPUTS};check_rows(preview)
    preview_report=report_marker(root,True)
    if backend=="cli":assert (root/"VERSION").read_bytes()==original_version and not (root/"_output").exists()
    run("build",root,backend)
    produced={name:csv_data(root/"_output"/"analysis"/name) for name in OUTPUTS};check_rows(produced)
    assert preview==produced
    for name,data in source.items():
     path=root/("_raw_data" if name.endswith(".csv") else "science")/name
     assert path.read_bytes()==data
     assert (root/"_output"/"analysis"/name).read_bytes()==data
    results[backend]=produced
    observations[backend]={"dependency_receipt":receipt["receipt"],"preview":{n:summarize(v) for n,v in preview.items()},
      "production":{n:summarize(v) for n,v in produced.items()},
      "preview_report":preview_report,"production_report":report_marker(root,False),
      "unchanged_pinned_source_and_input":True}
   assert results["legacy"]==results["cli"],"Fresh complete ordered CSV cells differ across engines"
   root=base/"cli"
   expected={label:inventory(root/folder) for label,folder in
             [("raw-data","_raw_data"),("output","_output"),("docs","_docs")]}
   for label,files in expected.items():assert inventory(store/label/"v1.2.4")==files
   before_version=(root/"VERSION").read_bytes();before_store=inventory(store)
   assert re.findall(rb"\d+",before_version)==[b"1",b"2",b"4",b"1"]
   evidence["deliberate_failure"]=run("failed-build",root)
   assert (root/"VERSION").read_bytes()==before_version and inventory(store)==before_store
   for label,folder in [("raw-data","_raw_data"),("output","_output"),("docs","_docs")]:
    assert inventory(root/folder)==expected[label]
   fresh=base/"fresh";cfg=project(fresh,source,False,store)
   # Start recovery without any original data/output/cache or old project indexes.
   (fresh/"_raw_data"/"data-raw-PIVI.csv").unlink();(fresh/"_raw_data").rmdir()
   cfg["build"]["dest"]={};(fresh/"_proj.yml").write_text(json.dumps(cfg))
   (fresh/"VERSION").write_bytes(before_version);(fresh/"renv.lock").write_bytes(lock)
   receipt=run("restore-packages",fresh)
   restored=run("restore-data",fresh)
   assert restored["labels"]==["raw-data","output","docs"]
   for label,folder in [("raw-data","_raw_data"),("output","_output"),("docs","_docs")]:
    assert inventory(fresh/folder)==expected[label]
   assert (fresh/"VERSION").read_bytes()==before_version and inventory(store)==before_store
   assert not (fresh/"_tmp").exists(),"Fresh recovery must not reuse an old analysis cache"
   run("preview",fresh)
   def safe_role(label):
    result=subprocess.run([str(binary),"--machine","--non-interactive","--project",str(fresh),
      "path","get","--label",label,"--format","absolute"],cwd=fresh,env=env,capture_output=True,text=True)
    reply=json.loads(result.stdout);assert result.returncode==0 and reply["ok"]
    return Path(reply["result"]["path"])
   rebuilt={name:csv_data(safe_role("output")/"analysis"/name) for name in OUTPUTS};check_rows(rebuilt)
   assert rebuilt==results["cli"]==results["legacy"]
   assert (fresh/"VERSION").read_bytes()==before_version and inventory(store)==before_store
   for label,folder in [("raw-data","_raw_data"),("output","_output"),("docs","_docs")]:
    assert inventory(fresh/folder)==expected[label]
   evidence.update(complete=True,cases=observations,
    recovered={"dependency_receipt":receipt["receipt"],"rows":{n:summarize(v) for n,v in rebuilt.items()},
      "report":report_marker(fresh,True,safe_role("docs")),"all_selected_restored_bytes_match":True,"source_available_with_uploads_disabled":True,
      "data_restore_through_R_frontend":True,"configuration_and_parameter_helpers_executed":True},
    previous_final_and_archive_bytes_preserved_on_failure=True,
    full_ordered_csv_comparison=True,no_live_provider_upload=True)
 finally:
  (args.evidence/"acceptance.json").write_text(json.dumps(evidence,indent=2)+"\n")
if __name__=="__main__":main()
