"""Integrated M3: real local build, remote-only recovery and read-only corruption failures."""
from __future__ import annotations
import argparse,csv,hashlib,json,os,shutil,subprocess,tempfile
from pathlib import Path

ANALYSIS="""import json,os,subprocess
from pathlib import Path
def resource(label,name):
    value=json.loads(subprocess.check_output([os.environ['PROJCLI_M3_BINARY'],'--machine','path','get','--label',label,name],text=True))
    return Path(value['result']['path'])
source=resource('raw','input.csv')
target=resource('output','result.csv')
target.parent.mkdir(parents=True,exist_ok=True)
target.write_bytes(b'value\\n42\\n')
assert source.read_bytes()==b'value\\n21\\n'
"""
def invoke(binary,root,env,*args,ok=True):
    result=subprocess.run([str(binary),'--machine','--non-interactive','--project',str(root),*args],
                          cwd=root,env=env,capture_output=True,text=True)
    reply=json.loads(result.stdout)
    assert reply['protocol_version']==1 and reply['ok']==(result.returncode==0)
    assert (result.returncode==0)==ok,result.stderr
    if not ok:assert reply['error']['code'] in ['query_failed','command_failed','invalid_arguments']
    return reply
def files(path):
    return {p.relative_to(path).as_posix():p.read_bytes() for p in path.rglob('*') if p.is_file()}
def ids(path):
    return sorted(p.parent.name+p.stem for p in path.glob('*/*.json'))
def main():
    parser=argparse.ArgumentParser();parser.add_argument('binary',type=Path)
    parser.add_argument('--evidence',required=True,type=Path);parser.add_argument('--artifacts',required=True,type=Path)
    args=parser.parse_args();binary=args.binary.resolve(strict=True);args.artifacts.mkdir(parents=True,exist_ok=True)
    evidence={'complete':False,'cases':[],'scope':'integrated local M3; explicit named snapshot selection',
      'not_claimed':['automatic discovery','performance gains','live consistent source dataset','permission restoration','new live GitHub transport test']}
    try:
        with tempfile.TemporaryDirectory(prefix='projcli-m3-') as temp:
            base=Path(temp);home=base/'home';home.mkdir();guard=base/'git-guard';guard.mkdir()
            witness=guard/'called';shim=guard/'git'
            shim.write_text('#!/bin/sh\nprintf called >> "$PROJCLI_GIT_CALL_WITNESS"\nexit 97\n');shim.chmod(0o755)
            env=os.environ.copy()
            for key in list(env):
                if key.upper().startswith(('GIT_','GH_','GITHUB_','PROJR_','PROJCLI_')) or key in ['SSH_AUTH_SOCK','QUARTO_PROFILE','PROJ_PROJECT_ROOT','PROJ_RUN_CONTEXT']:
                    env.pop(key,None)
            env.update(HOME=str(home),USERPROFILE=str(home),XDG_CONFIG_HOME=str(home),GIT_CONFIG_NOSYSTEM='1',
                       GIT_CONFIG_GLOBAL=str(home/'gitconfig'),GIT_CONFIG_SYSTEM=str(home/'gitconfig'),GIT_TERMINAL_PROMPT='0',
                       PROJCLI_M3_BINARY=str(binary),PROJCLI_GIT_CALL_WITNESS=str(witness),PATH=str(guard)+os.pathsep+env.get('PATH',''))
            for case,style,mode in [('tree','tree',None),('flat-csv','flat','csv'),('flat-file-dictionary','flat','json-dedupe-file')]:
                owner=base/case;root=owner/'project';root.mkdir(parents=True);store=owner/'store';backup=owner/'ordinary'
                for folder in ['input/nested/empty','released','private']:(root/folder).mkdir(parents=True)
                (root/'input/input.csv').write_bytes(b'value\n21\n')
                (root/'input/nested/cafe,notes.txt').write_bytes('Unicode: café\n'.encode())
                (root/'private/excluded').write_bytes(b'never distributed')
                (root/'VERSION').write_bytes(b'v1.2.3-19\r\n');(root/'analysis.py').write_text(ANALYSIS)
                remote={'path':'../store','structure':'cas','manifest':'remote','manifest-style':style}
                if mode:remote['manifest-format']=mode
                cfg={'directories':{'raw':{'path':'input'},'output':{'path':'released'},'cache':{'path':'cache','safe':True},
                    'raw-excluded':{'path':'private','hash':False}},
                    'remotes':{'local':{'snapshots':remote,'ordinary':{'path':'../ordinary','structure':'archive',
                      'manifest-style':'tree','manifest':'both','send':{'inspect':'file'}}}},
                    'build':{'scripts':['analysis.py'],'git':False,'dest':{'snapshots':{'content':['raw','output']},'ordinary':{'content':['output']}}}}
                (root/'_proj.yml').write_text(json.dumps(cfg))
                expected_raw=files(root/'input');expected_output={'result.csv':b'value\n42\n'}
                namespace='tree-snapshots/v1' if style=='tree' else 'flat-snapshots/v1'
                invoke(binary,root,env,'build','--patch')
                first_ids=ids(store/namespace);assert len(first_ids)==1;selected=first_ids[0]
                first_metadata=files(store/namespace);first_objects=files(store/'objects')
                assert files(root/'released')==expected_output
                assert files(backup/'output/v1.2.4')==expected_output
                invoke(binary,root,env,'build','--patch')
                assert files(store/'objects')==first_objects
                assert all(files(store/namespace)[name]==data for name,data in first_metadata.items())
                assert len(ids(store/namespace))==2
                assert (root/'VERSION').read_text().strip()=='Version: v1.2.5.1'
                rows=list(csv.DictReader((root/'manifest.csv').open()))
                assert all(row['label']!='raw-excluded' for row in rows)
                for label,path,data in [('raw','input.csv',expected_raw['input.csv']),('output','result.csv',expected_output['result.csv'])]:
                    row=next(row for row in rows if row['label']==label and row['fn']==path and row['version']=='v1.2.5')
                    assert row['hash']==hashlib.md5(data).hexdigest()
                records=[json.loads(path.read_text()) for path in (root/'.proj/log').glob('*.json')]
                assert len(records)==2 and all(record['status']=='completed' for record in records)
                artifact=args.artifacts/case;artifact.mkdir()
                shutil.copytree(store,artifact/'store')
                shutil.copyfile(root/'manifest.csv',artifact/'manifest.csv')
                for path in (root/'.proj/log').glob('*.json'):shutil.copyfile(path,artifact/path.name)
                # Recreate only this disposable project. Remote full metadata is sufficient;
                # neither the old ordinary CSV, lookup ledger nor small project references remain.
                shutil.rmtree(root);root.mkdir()
                for folder in ['input','released','cache']:(root/folder).mkdir()
                cfg['build']={'scripts':[],'git':False,'dest':{}}
                (root/'_proj.yml').write_text(json.dumps(cfg));version=b'Version: v9.0.0.7\n';(root/'VERSION').write_bytes(version)
                before_store=files(store)
                observation=invoke(binary,root,env,'remote','verify','--title','snapshots','--snapshot',selected)
                assert observation['result']['snapshot']['snapshot_id']==selected
                for label,path,expected in [('raw','input',expected_raw),('output','released',expected_output)]:
                    invoke(binary,root,env,'remote','restore','--title','snapshots','--snapshot',selected,'--label',label,'--clear')
                    assert files(root/path)==expected
                    shutil.copytree(root/path,artifact/('recovered-'+label))
                assert (root/'input/nested/empty').is_dir()
                assert not (root/'.proj').exists() and not (root/'.projr').exists() and not (root/'manifest.csv').exists()
                assert (root/'VERSION').read_bytes()==version and files(store)==before_store
                probes=[];destination=files(root)
                destination_dirs={p.relative_to(root).as_posix() for p in root.rglob('*') if p.is_dir()}
                obj=next(path for path in (store/'objects').glob('*/*') if path.is_file() and path.stat().st_size)
                original=obj.read_bytes();snapshot_file=next((store/namespace).glob(selected[:2]+'/'+selected[2:]+'.json'))
                for damage in ['corrupt-object','missing-object','malformed-snapshot']:
                    metadata=snapshot_file.read_bytes()
                    if damage=='corrupt-object':obj.write_bytes(b'corrupt fixture object')
                    elif damage=='missing-object':obj.unlink()
                    else:snapshot_file.write_bytes(b'{}')
                    damaged_store=files(store)
                    failed=invoke(binary,root,env,'remote','verify','--title','snapshots','--snapshot',selected,ok=False)
                    invoke(binary,root,env,'remote','restore','--title','snapshots','--snapshot',selected,'--label','raw','--clear',ok=False)
                    assert files(root)==destination and files(store)==damaged_store
                    assert {p.relative_to(root).as_posix() for p in root.rglob('*') if p.is_dir()}==destination_dirs
                    (artifact/(damage+'.json')).write_text(json.dumps(failed,indent=2))
                    probes.append({'case':damage,'error':failed['error']['code'],'destination_preserved':True,'store_not_repaired':True})
                    obj.write_bytes(original);snapshot_file.write_bytes(metadata)
                invoke(binary,root,env,'remote','verify','--title','snapshots','--snapshot',selected)
                assert files(store)==before_store and not witness.exists()
                (artifact/'verification.json').write_text(json.dumps(observation,indent=2))
                evidence['cases'].append({'case':case,'snapshot':selected,'directories':2,
                    'raw_files':len(expected_raw),'output_files':len(expected_output),'unique_objects':len(first_objects),
                    'remote_only_recovery':True,'ordinary_tree_backup':True,'earlier_bytes_preserved':True,
                    'git_invoked':False,'probes':probes})
        evidence['complete']=True
    finally:
        args.evidence.write_text(json.dumps(evidence,indent=2)+'\n')
if __name__=='__main__':main()
