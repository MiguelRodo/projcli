"""Remote-only synthetic actual-Xet local pipeline experiment. No user files."""
import argparse, gc, gzip, hashlib, json, os, platform, random, shutil, subprocess, sys, time
from pathlib import Path

PIN="629562cd9fc37432317eb57811faf13a6935c601"
def sha(p):
    h=hashlib.sha256()
    with p.open("rb") as f:
        for b in iter(lambda:f.read(1024*1024),b""):h.update(b)
    return h.hexdigest()
def write_json(p,value):p.write_text(json.dumps(value,indent=2)+"\n")
def measure(p):
    entries=[];directory_blocks=p.stat().st_blocks*512
    for item in sorted(p.rglob("*")):
        s=item.lstat()
        if item.is_symlink():raise RuntimeError("unexpected link in synthetic store")
        if item.is_dir():directory_blocks+=s.st_blocks*512
        else:entries.append({"path":item.relative_to(p).as_posix(),"bytes":s.st_size,"allocated_bytes":s.st_blocks*512})
    return {"file_bytes":sum(x["bytes"] for x in entries),
            "allocated_file_bytes":sum(x["allocated_bytes"] for x in entries),
            "allocated_directory_bytes":directory_blocks,
            "files":len(entries),"entries":entries}
def safe_remove(p,root):
    assert p!=root and p.is_relative_to(root)
    if p.exists():shutil.rmtree(p)
def invoke(binary,op,store,source,out,receipt,root):
    home=root/"process-home";safe_remove(home,root);home.mkdir()
    tmp=root/"process-tmp";safe_remove(tmp,root);tmp.mkdir()
    env={"PATH":os.environ["PATH"],"HOME":str(home),"TMPDIR":str(tmp),
         "LANG":"C.UTF-8","HF_HUB_DISABLE_TELEMETRY":"1","DO_NOT_TRACK":"1"}
    usage=receipt.with_suffix(".resources.json")
    start=time.monotonic()
    subprocess.run([sys.executable,str(Path(__file__).with_name("timed.py")),
        str(usage),str(binary),op,str(store),str(source),str(out),str(receipt)],
        env=env,check=True,timeout=180)
    result=json.loads(receipt.read_text())
    assert result["local"] and result["auth"] is False
    result["process_resources"]=json.loads(usage.read_text())
    result["parent_elapsed_s"]=time.monotonic()-start
    return result
def baseline_add(base,source):
    digest=sha(source);target=base/"objects"/digest[:2]/digest[2:]
    target.parent.mkdir(parents=True,exist_ok=True)
    if not target.exists():shutil.copyfile(source,target)
    else:assert sha(target)==digest
    return digest
def gzip_add(base,source):
    digest=sha(source);target=base/"objects"/digest[:2]/digest[2:]
    target.parent.mkdir(parents=True,exist_ok=True)
    if not target.exists():
        with source.open("rb") as fin,target.open("wb") as raw,gzip.GzipFile(filename="",fileobj=raw,mode="wb",mtime=0,compresslevel=6) as out:
            shutil.copyfileobj(fin,out)
def case(root,label,a,b,binary,evidence):
    case_root=root/label;case_root.mkdir()
    source=case_root/"working";source.mkdir()
    store=case_root/"store";store.mkdir()
    pointers=case_root/"pointers";pointers.mkdir()
    whole=case_root/"whole-file-baseline";whole.mkdir()
    zipped=case_root/"gzip-whole-file-baseline";zipped.mkdir()
    (source/"v1.bin").write_bytes(a);(source/"v2.bin").write_bytes(b)
    expected={v:{"size":(source/(v+".bin")).stat().st_size,"sha256":sha(source/(v+".bin"))} for v in ["v1","v2"]}
    operations=[]
    for name,v in [("v1","v1"),("v1-repeat","v1"),("v2","v2"),("v2-repeat","v2")]:
        receipt=evidence/(label+"-"+name+".json")
        result=invoke(binary,"upload",store,source/(v+".bin"),pointers/(name+".json"),receipt,case_root)
        assert result["result"]["file"]["sha256"]==expected[v]["sha256"]
        result["phase"]=name;result["store_after_process_exit"]=measure(store)
        digest=baseline_add(whole,source/(v+".bin"));gzip_add(zipped,source/(v+".bin"))
        for baseline in [whole,zipped]:
            refs=baseline/"pointers";refs.mkdir(exist_ok=True)
            write_json(refs/(name+".json"),{"hash":digest,"file_size":expected[v]["size"],"sha256":digest})
        operations.append(result)
        write_json(receipt,result)
    assert operations[0]["result"]["file"]==operations[1]["result"]["file"]
    assert operations[2]["result"]["file"]==operations[3]["result"]["file"]
    store_size=measure(store);whole_size=measure(whole);compressed_size=measure(zipped);pointer_size=measure(pointers)
    # Exact quiescent full-store export: processes have exited and DB handles closed.
    exported=case_root/"relocated-store";shutil.copytree(store,exported)
    # Remove client caches only; keep the upstream local service's durable redb/xorb/shard tree.
    removed=[]
    for rel in operations[-1]["client_cache_relative"]:
        path=(exported/rel).resolve()
        assert path.is_relative_to(exported) and not path.is_relative_to(exported/"xet/xorbs")
        safe_remove(path,case_root);removed.append(rel)
    durable_only=measure(exported)
    assert any(x["path"].endswith(".redb") for x in durable_only["entries"])
    safe_remove(source,case_root);safe_remove(store,case_root)
    safe_remove(whole,case_root);safe_remove(zipped,case_root)
    # Each version reconstructs in a new OS process with a fresh home/tmp and no chunk cache.
    restored=case_root/"restored";restored.mkdir()
    recovery=[]
    for v in ["v1","v2"]:
        receipt=evidence/(label+"-recover-"+v+".json")
        result=invoke(binary,"download",exported,pointers/(v+".json"),restored/(v+".bin"),receipt,case_root)
        assert not source.exists() and not store.exists()
        assert sha(restored/(v+".bin"))==expected[v]["sha256"]
        assert (restored/(v+".bin")).stat().st_size==expected[v]["size"]
        result["sha256_verified"]=True;recovery.append(result);write_json(receipt,result)
        # Remove newly populated client caches before the next fresh process.
        for rel in result["client_cache_relative"]:safe_remove((exported/rel).resolve(),case_root)
    result={"case":label,"expected":expected,"operations":operations,"recoveries":recovery,
      "store_including_client_metadata":store_size,"relocated_without_client_cache":durable_only,
      "whole_file_dedup_baseline":whole_size,"gzip_whole_file_dedup_baseline":compressed_size,
      "pointer_metadata":pointer_size,
      "metadata_inclusive_xet_file_bytes":store_size["file_bytes"]+pointer_size["file_bytes"],
      "metadata_inclusive_xet_allocated_bytes":sum(store_size[k]+pointer_size[k] for k in ["allocated_file_bytes","allocated_directory_bytes"]),"removed_client_cache_paths":removed,
      "source_and_original_store_deleted":True,"two_fresh_process_recoveries_sha256_exact":True,
      "store_file_byte_ratio_to_whole_baseline":(store_size["file_bytes"]+pointer_size["file_bytes"])/whole_size["file_bytes"],
      "store_allocated_ratio_to_whole_baseline":sum(store_size[k]+pointer_size[k] for k in ["allocated_file_bytes","allocated_directory_bytes"])/sum(whole_size[k] for k in ["allocated_file_bytes","allocated_directory_bytes"]),
      "note":"File and directory allocated blocks measured; filesystem inode/superblock overhead not attributed. Export copy may change sparse allocation. Default upstream compression is enabled; compare gzip baseline, not dedup metrics alone."}
    write_json(evidence/(label+"-summary.json"),result)
    # Keep metadata evidence only; synthetic payload/store are not uploaded as artifacts.
    safe_remove(case_root,root)
    return result
def main():
    parser=argparse.ArgumentParser();parser.add_argument("--binary",type=Path,required=True)
    parser.add_argument("--root",type=Path,required=True);parser.add_argument("--evidence",type=Path,required=True)
    args=parser.parse_args();root=args.root.resolve();root.mkdir();evidence=args.evidence.resolve();evidence.mkdir(exist_ok=True)
    rng=random.Random(20261027);n=16*1024*1024
    random_a=rng.randbytes(n)
    def changed(a):
        b=bytearray(a);b[n//2:n//2+32768]=rng.randbytes(32768)
        b[n//4:n//4]=rng.randbytes(65536);return bytes(b)
    random_b=changed(random_a)
    record=b"subject,value,status\n"+b"1001,0.125,observed\n"*1024
    structured_a=(record*((n+len(record)-1)//len(record)))[:n];structured_b=changed(structured_a)
    cases=[("high-entropy",random_a,random_b),("compressible",structured_a,structured_b),
           ("gzip-compressible",gzip.compress(structured_a,mtime=0),gzip.compress(structured_b,mtime=0))]
    summaries=[]
    for label,a,b in cases:
        summaries.append(case(root,label,a,b,args.binary.resolve(),evidence));gc.collect()
    compact={"upstream_commit":PIN,"python":platform.python_version(),"payloads":"synthetic deterministic seed20261027;16MiB originals;64KiB insertion+32KiB replacement",
      "baseline":"one stored copy per distinct whole-file SHA256 plus matching four pointer records; separate gzip level6 baseline with identical version references",
      "cases":[{k:v for k,v in s.items() if k not in ["operations","recoveries"]} for s in summaries],
      "passed":True,"no_hf_auth_or_server":True,"unchanged_repeat_in_new_process":True,
      "limits":"Synthetic local backend experiment, not crash durability, transfer efficiency, arbitrary RDS/PDF behavior or production integration proof"}
    write_json(evidence/"summary.json",compact)
    print(json.dumps({"passed":True,"cases":[{"case":s["case"],"whole":s["whole_file_dedup_baseline"]["file_bytes"],
      "gzip_whole":s["gzip_whole_file_dedup_baseline"]["file_bytes"],"xet_all":s["store_including_client_metadata"]["file_bytes"],
      "xet_durable":s["relocated_without_client_cache"]["file_bytes"],
      "xet_allocated":s["store_including_client_metadata"]["allocated_file_bytes"]} for s in summaries]},indent=2))
if __name__=="__main__":main()
