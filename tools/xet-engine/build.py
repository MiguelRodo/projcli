"""Build optional engine against the audited official workspace; never run by proj."""
import argparse,hashlib,pathlib,re,shutil,subprocess
COMMIT="629562cd9fc37432317eb57811faf13a6935c601"
LOCK="ecb897ce0eaa6e3d2ec4f55ab903104d3ebe4345b7a0c8752d2b16d381c7fd7c"
p=argparse.ArgumentParser();p.add_argument("--source",type=pathlib.Path,required=True)
p.add_argument("--output",type=pathlib.Path,required=True);a=p.parse_args()
assert not a.source.exists(),"use a fresh isolated upstream directory"
subprocess.run(["git","init",str(a.source)],check=True)
subprocess.run(["git","-C",str(a.source),"fetch","--depth","1","https://github.com/huggingface/xet-core.git",COMMIT],check=True)
subprocess.run(["git","-C",str(a.source),"checkout","--detach","FETCH_HEAD"],check=True)
assert subprocess.check_output(["git","-C",str(a.source),"rev-parse","HEAD"],text=True).strip()==COMMIT
lock=(a.source/"Cargo.lock").read_bytes();assert hashlib.sha256(lock).hexdigest()==LOCK
assert set(re.findall(r'^source = "([^"]+)"',lock.decode(),re.M))=={"registry+https://github.com/rust-lang/crates.io-index"}
for file in a.source.rglob("Cargo.toml"):
    assert not re.search(r'^[^#\n]*\bgit\s*=',file.read_text(),re.M),str(file)
target=a.source/"xet_data/src/bin/proj-xet-engine.rs";target.parent.mkdir(exist_ok=True)
shutil.copyfile(pathlib.Path(__file__).with_name("main.rs"),target)
subprocess.run(["cargo","+1.95.0","build","--locked","--package","xet-data","--bin","proj-xet-engine"],cwd=a.source,check=True)
import os
compiled=pathlib.Path(os.environ.get("CARGO_TARGET_DIR",str(a.source/"target")))/"debug"/("proj-xet-engine.exe" if os.name=="nt" else "proj-xet-engine")
a.output.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(compiled,a.output);a.output.chmod(0o755)
