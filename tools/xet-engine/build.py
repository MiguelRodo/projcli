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
a.output.parent.mkdir(parents=True,exist_ok=True)
# This helper belongs to the upstream workspace, not the proj workspace.
# Analyze its actual target as well; report upstream diagnostics without editing it.
import json
clippy=subprocess.run(["cargo","+1.95.0","clippy","--locked","--package","xet-data",
                      "--bin","proj-xet-engine","--message-format=json"],cwd=a.source,
                     capture_output=True,text=True)
findings=[]
for line in clippy.stdout.splitlines():
    try: entry=json.loads(line)
    except ValueError: continue
    if entry.get("reason")!="compiler-message": continue
    message=entry["message"]
    if message["level"] not in ["warning","error"]: continue
    if entry.get("target",{}).get("name")=="proj-xet-engine":
        findings.append(message)
a.output.with_suffix(".clippy.json").write_text(json.dumps(findings,indent=2)+chr(10))
a.output.with_suffix(".clippy.log").write_text(clippy.stdout+clippy.stderr)
if clippy.returncode or findings:
    print(json.dumps({"helper_clippy":findings,"returncode":clippy.returncode}))
    raise RuntimeError("Optional helper Clippy target did not pass; see retained diagnostics")
shutil.copyfile(compiled,a.output);a.output.chmod(0o755)

# Portable provenance accompanies the optional helper, never user datasets/credentials.
import json
metadata=json.loads(subprocess.check_output(["cargo","+1.95.0","metadata","--locked","--format-version","1"],cwd=a.source,text=True))
licenses=sorted([{"name":x["name"],"version":x["version"],"license":x.get("license"),
                  "license_file":x.get("license_file"),"source":x.get("source")}
                 for x in metadata["packages"]],key=lambda x:(x["name"],x["version"]))
# This conservative workspace inventory is not a claim about the final linked binary.
provenance={"schema":"projcli.xet-helper-build.v1","upstream":"https://github.com/huggingface/xet-core",
            "upstream_commit":COMMIT,"lock_sha256":LOCK,
            "helper_source_sha256":hashlib.sha256(pathlib.Path(__file__).with_name("main.rs").read_bytes()).hexdigest(),
            "binary_sha256":hashlib.sha256(a.output.read_bytes()).hexdigest(),
            "toolchain":subprocess.check_output(["rustc","+1.95.0","--version"],text=True).strip(),
            "dependency_inventory_scope":"resolved upstream workspace; conservative, not a linked-binary SBOM",
            "dependencies":licenses}
a.output.with_suffix(".provenance.json").write_text(json.dumps(provenance,indent=2)+"\n")
shutil.copyfile(a.source/"LICENSE",a.output.with_suffix(".upstream-LICENSE"))
