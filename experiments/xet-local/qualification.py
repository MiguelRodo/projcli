"""Remote-only qualification of actual pinned Xet; owned synthetic fixtures only."""
import argparse,gzip,hashlib,json,os,random,re,shutil,signal,subprocess,tempfile,time,zipfile
from pathlib import Path,PurePosixPath
import experiment as base

def env_at(root):
    home=root/"home";tmp=root/"tmp";home.mkdir(parents=True,exist_ok=True);tmp.mkdir(exist_ok=True)
    return {"PATH":os.environ["PATH"],"HOME":str(home),"TMPDIR":str(tmp),
            "LANG":"C.UTF-8","HF_HUB_DISABLE_TELEMETRY":"1","DO_NOT_TRACK":"1"}
def strip_cache(store,paths,owned):
    for rel in paths:
        p=(store/rel).resolve()
        assert p.is_relative_to(store.resolve()) and not p.is_relative_to((store/"xet/xorbs").resolve())
        base.safe_remove(p,owned)
def download(binary,store,pointer,destination,evidence,tag,owned,expected):
    receipt=evidence/(tag+".json")
    result=subprocess.run([str(binary),"download",str(store),str(pointer),str(destination),str(receipt)],
        env=env_at(owned/"read-env"),capture_output=True,timeout=60)
    exact=result.returncode==0 and destination.is_file() and base.sha(destination)==expected["sha256"] and destination.stat().st_size==expected["size"]
    observed={"status":result.returncode,"exact_bytes":exact,"partial_output_exists":destination.exists(),
       "stderr_tail":result.stderr.decode(errors="replace")[-4096:]}
    if result.returncode==0 and not exact:raise AssertionError("FALSE VALID CONTENT: "+tag)
    base.write_json(evidence/(tag+"-observation.json"),observed)
    return observed
def expected_bytes(data):return {"sha256":hashlib.sha256(data).hexdigest(),"size":len(data)}
def normal_upload(binary,store,source,pointer,evidence,tag,owned):
    return base.invoke(binary,"upload",store,source,pointer,evidence/(tag+".json"),owned)
def fault_case(root,evidence,binary,stage,seed,pointer_a,pointer_b,expected_a,expected_b,b):
    own=root/stage;own.mkdir();store=own/"store";shutil.copytree(seed,store)
    refs=own/"references";refs.mkdir();a=refs/"A.json";shutil.copyfile(pointer_a,a)
    observed_b=refs/"oracle-B.json";shutil.copyfile(pointer_b,observed_b)
    committed_b=refs/"B.json";source=own/"B.bin";source.write_bytes(b)
    barrier=own/"barrier.json";before=base.sha(a)
    log=(evidence/(stage+"-writer.log")).open("wb")
    started=time.monotonic()
    proc=subprocess.Popen([str(binary),stage,str(store),str(source),str(committed_b),str(barrier)],
        stdin=subprocess.DEVNULL,stdout=log,stderr=log,env=env_at(own/"writer-env"))
    try:
        deadline=started+45
        while not barrier.exists() and proc.poll() is None and time.monotonic()<deadline:time.sleep(.005)
        assert barrier.exists(),"writer did not reach controlled barrier"
        point=json.loads(barrier.read_text())
        proc.kill() # Only this owned remote fixture process, never a host/session/process search.
        status=proc.wait(timeout=10)
        assert status==-signal.SIGKILL
    finally:
        if proc.poll() is None:proc.kill();proc.wait(timeout=10)
        log.close()
    source.unlink()
    assert base.sha(a)==before
    should_commit=stage=="fault-committed"
    assert committed_b.exists()==should_commit
    prior=download(binary,store,a,own/"recovered-A",evidence,stage+"-prior-A",own,expected_a)
    assert prior["exact_bytes"],"previous committed version lost"
    candidate=download(binary,store,observed_b,own/"candidate-B",evidence,stage+"-candidate-B",own,expected_b)
    if stage=="fault-partial":assert candidate["status"]!=0,"partial ingest unexpectedly published complete new content"
    if stage in ["fault-finalized","fault-committed"]:assert candidate["exact_bytes"],"finalize returned but complete new content was unavailable"
    if should_commit:assert json.loads(committed_b.read_text())==json.loads(observed_b.read_text())
    # Retry in the actual crashed store, retaining writer session state. Never delete locks or repair indexes ourselves.
    source.write_bytes(b)
    retry=normal_upload(binary,store,source,committed_b,evidence,stage+"-retry",own)
    source.unlink()
    assert json.loads(committed_b.read_text())==json.loads(observed_b.read_text())
    for name,pointer,expected in [("A",a,expected_a),("B",committed_b,expected_b)]:
        observed=download(binary,store,pointer,own/("after-retry-"+name),evidence,stage+"-retry-"+name,own,expected)
        assert observed["exact_bytes"],"safe retry lost committed content"
    assert base.sha(a)==before
    result={"stage":stage,"passed":True,"barrier":point,"writer_exit":status,
      "whole_case_elapsed_s":time.monotonic()-started,"prior_version_exact":True,
      "reference_present_after_kill":should_commit,"candidate_after_kill":candidate,
      "retry_both_versions_exact":True,"manual_lock_or_index_repair":False,
      "store_after_retry":base.measure(store)}
    base.write_json(evidence/(stage+"-summary.json"),result)
    base.safe_remove(own,root)
    return result
def damage_case(root,evidence,binary,seed,pointer_a,expected_a,mode,payload_rel):
    own=root/("damage-"+mode);own.mkdir();store=own/"store";shutil.copytree(seed,store)
    healthy=download(binary,store,pointer_a,own/"healthy-private-stage-A",evidence,"damage-"+mode+"-before",own,expected_a)
    assert healthy["exact_bytes"],"negative fixture must restore correctly before damage"
    target=store/payload_rel
    before=base.sha(target);size=target.stat().st_size
    if mode=="missing":target.unlink()
    else:
        # Upstream packs are read-only. Replace only this owned disposable copy
        # with a newly written corrupt inode; never change seed permissions/content.
        with tempfile.NamedTemporaryFile(dir=target.parent,prefix=".corrupt-copy-",delete=False) as f:
            temporary=Path(f.name)
            with target.open("rb") as source:shutil.copyfileobj(source,f)
            f.seek(size//2);old=f.read(1);assert old;f.seek(size//2);f.write(bytes([old[0]^0x80]));f.flush();os.fsync(f.fileno())
        os.replace(temporary,target)
        assert base.sha(target)!=before and base.sha(seed/payload_rel)==before
    observation=download(binary,store,pointer_a,own/"private-stage-A",evidence,"damage-"+mode,own,expected_a)
    assert observation["status"]!=0,"missing/corrupt content was not rejected"
    result={"mode":mode,"passed":True,"target":payload_rel,"original_sha256":before,
      "healthy_control_exact":True,"download":observation,"scope":"upstream error and private staging only; no user destination promotion attempted"}
    base.write_json(evidence/("damage-"+mode+"-summary.json"),result);base.safe_remove(own,root)
    return result
def export_import(root,evidence,binary,label,a,b):
    own=root/("portable-"+label);own.mkdir();store=own/"store";store.mkdir();refs=own/"references";refs.mkdir()
    working=own/"working";working.mkdir()
    expected={};last=None
    for name,data in [("A",a),("B",b)]:
        source=working/(name+".bin");source.write_bytes(data);expected[name]=expected_bytes(data)
        last=normal_upload(binary,store,source,refs/(name+".json"),evidence,"portable-"+label+"-"+name,own)
    full=base.measure(store)
    strip_cache(store,last["client_cache_relative"],own);durable=base.measure(store)
    # All upload processes have returned and writer/database handles are closed. No concurrent hot-copy claim.
    archive=own/"closed-store.zip";members={}
    for prefix,directory in [("store",store),("references",refs)]:
        for file in sorted(directory.rglob("*")):
            if file.is_file():
                assert not file.is_symlink()
                name=prefix+"/"+file.relative_to(directory).as_posix()
                members[name]={"bytes":file.stat().st_size,"sha256":base.sha(file)}
    manifest={"schema":"xet-qualification-export.v1","upstream":base.PIN,
      "writer_stopped":True,"cache_excluded":last["client_cache_relative"],"members":members}
    with zipfile.ZipFile(archive,"w",compression=zipfile.ZIP_DEFLATED,compresslevel=6) as z:
        for name in sorted(members):
            source=(store if name.startswith("store/") else refs)/name.split("/",1)[1];z.write(source,name)
        z.writestr("export.json",json.dumps(manifest,sort_keys=True))
    zip_stats={"bytes":archive.stat().st_size,"allocated":archive.stat().st_blocks*512,"sha256":base.sha(archive)}
    base.safe_remove(working,own);base.safe_remove(store,own);base.safe_remove(refs,own)
    imported=own/"fresh-import";imported.mkdir()
    with zipfile.ZipFile(archive) as z:
        names=z.namelist();assert len(names)==len(set(names)) and len(names)<1000
        assert sum(i.file_size for i in z.infolist())<128*1024*1024
        recovered=json.loads(z.read("export.json"));assert recovered==manifest
        assert set(names)==set(members)|{"export.json"}
        for name in names:
            path=PurePosixPath(name);assert not path.is_absolute() and ".." not in path.parts
            info=z.getinfo(name);assert (info.external_attr>>16)&0o170000!=0o120000
            target=imported/path;target.parent.mkdir(parents=True,exist_ok=True)
            with z.open(info) as source,target.open("wb") as output:shutil.copyfileobj(source,output)
            if name!="export.json":assert base.sha(target)==members[name]["sha256"] and target.stat().st_size==members[name]["bytes"]
    imported_size=base.measure(imported/"store")
    for name in ["A","B"]:
        observed=download(binary,imported/"store",imported/"references"/(name+".json"),own/("import-restored-"+name),
            evidence,"portable-"+label+"-restore-"+name,own,expected[name])
        assert observed["exact_bytes"]
        strip_cache(imported/"store",last["client_cache_relative"],own)
    result={"case":label,"passed":True,"closed_writer_export":True,"hot_copy_tested":False,
      "originals_and_client_caches_absent":True,"both_versions_exact":True,"full_store":full,
      "closed_durable_store":durable,"zip":zip_stats,"expanded_member_bytes":sum(x["bytes"] for x in members.values()),
      "imported_store_before_reads":imported_size,"manifest":manifest,
      "note":"ZIP preserves bytes, not sparse allocation; no merge into existing store, ACL/permission fidelity or crash-atomic publication claim"}
    base.write_json(evidence/("portable-"+label+"-summary.json"),result);base.safe_remove(own,root)
    return result
def main():
    parser=argparse.ArgumentParser();parser.add_argument("--binary",type=Path,required=True)
    parser.add_argument("--root",type=Path,required=True);parser.add_argument("--evidence",type=Path,required=True)
    args=parser.parse_args();binary=args.binary.resolve();root=args.root.resolve();root.mkdir()
    evidence=args.evidence.resolve();evidence.mkdir(exist_ok=True)
    rng=random.Random(20261027);n=16*1024*1024;a=rng.randbytes(n)
    b=bytearray(a);b[n//2:n//2+32768]=rng.randbytes(32768);b[n//4:n//4]=rng.randbytes(65536);b=bytes(b)
    setup=root/"setup";setup.mkdir();source_a=setup/"A.bin";source_b=setup/"B.bin";source_a.write_bytes(a);source_b.write_bytes(b)
    seed=root/"seed-store";seed.mkdir();oracle=root/"oracle-store";oracle.mkdir()
    pa=setup/"A.json";pb=setup/"oracle-B.json"
    normal_upload(binary,seed,source_a,pa,evidence,"setup-A",setup)
    normal_upload(binary,oracle,source_b,pb,evidence,"setup-oracle-B",setup)
    source_a.unlink();source_b.unlink();base.safe_remove(oracle,root)
    payloads=sorted(p for p in (seed/"xet/xorbs/xorbs").iterdir() if re.fullmatch(r"default\.[0-9a-f]{64}",p.name));assert len(payloads)==1,"expected one canonical A payload pack"
    payload_rel=payloads[0].relative_to(seed).as_posix()
    result={"upstream_commit":base.PIN,"experiment_commit":os.environ.get("GITHUB_SHA"),
      "faults":[],"negative_content":[],"portable":[],"failures":[],
      "guarantee_boundary":"SIGKILL at controlled public-API barriers, not system power loss or arbitrary intra-transaction instruction; reference fsync/rename is supplied by harness",
      "no_live_services_or_auth":True}
    def collect(section,label,fn):
        try:result[section].append(fn())
        except Exception as e:result["failures"].append({"case":label,"error":repr(e)})
        base.write_json(evidence/"qualification-summary.json",result)
    for stage in ["fault-partial","fault-finished","fault-finalized","fault-committed"]:
        collect("faults",stage,lambda stage=stage:fault_case(root,evidence,binary,stage,seed,pa,pb,expected_bytes(a),expected_bytes(b),b))
    for mode in ["missing","corrupt"]:
        collect("negative_content",mode,lambda mode=mode:damage_case(root,evidence,binary,seed,pa,expected_bytes(a),mode,payload_rel))
    # Remove the seed/oracle file references before the independent portable-store fixtures.
    base.safe_remove(seed,root);base.safe_remove(setup,root)
    collect("portable","high-entropy",lambda:export_import(root,evidence,binary,"high-entropy",a,b))
    unit=b"subject,value,status\n"+b"1001,0.125,observed\n"*1024
    text=(unit*((n+len(unit)-1)//len(unit)))[:n];changed=bytearray(text);changed[n//2:n//2+32768]=rng.randbytes(32768);changed[n//4:n//4]=rng.randbytes(65536)
    collect("portable","gzip-compressible",lambda:export_import(root,evidence,binary,"gzip-compressible",gzip.compress(text,mtime=0),gzip.compress(bytes(changed),mtime=0)))
    result["passed"]=not result["failures"] and len(result["faults"])==4 and len(result["negative_content"])==2 and len(result["portable"])==2
    base.write_json(evidence/"qualification-summary.json",result)
    print(json.dumps({"passed":result["passed"],"faults":len(result["faults"]),"negative":len(result["negative_content"]),
       "portable":len(result["portable"]),"failures":result["failures"]},indent=2))
    if not result["passed"]:raise SystemExit(1)
if __name__=="__main__":main()
