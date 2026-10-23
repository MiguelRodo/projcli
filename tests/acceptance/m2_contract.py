"""Remote integrated M2 LOCAL traceability gate; no new failed-record retention policy."""
from __future__ import annotations
import argparse,csv,hashlib,json,os,shutil,subprocess,tempfile
from pathlib import Path

ANALYSIS="""import csv,json,os,subprocess
from pathlib import Path
binary=os.environ['PROJCLI_M2_BINARY']
def resource(label,*parts):
    reply=json.loads(subprocess.check_output([binary,'--machine','path','get','--label',label,*parts],text=True))
    return Path(reply['result']['path'])
value=int(next(csv.DictReader(resource('raw','input.csv').open()))['value'])
factor=json.loads(Path('_proj.yml').read_text())['parameters']['factor']
target=resource('output','result.csv')
target.parent.mkdir(parents=True,exist_ok=True)
target.write_bytes(('value\\n'+str(value*factor)+'\\n').encode())
if os.environ.get('PROJCLI_M2_FAIL')=='1':
    target.write_bytes(b'failed-only-output\\n')
    Path(os.environ['PROJCLI_M2_FAILURE_WITNESS']).write_bytes(b'analysis reached exit17 after output overwrite')
    raise SystemExit(17)
"""
def invoke(binary,root,env,*args):
    result=subprocess.run([str(binary),'--machine','--non-interactive','--project',str(root),*args],
                          cwd=root,env=env,capture_output=True,text=True)
    envelope=json.loads(result.stdout)
    assert envelope['protocol_version']==1 and envelope['ok']==(result.returncode==0)
    return result,envelope
def snapshot(path):
    return {p.relative_to(path).as_posix():p.read_bytes() for p in path.rglob('*') if p.is_file()}
def main():
    parser=argparse.ArgumentParser();parser.add_argument('binary',type=Path)
    parser.add_argument('--evidence',required=True,type=Path);parser.add_argument('--artifacts',required=True,type=Path)
    args=parser.parse_args();binary=args.binary.resolve(strict=True);args.artifacts.mkdir(parents=True,exist_ok=True)
    evidence={'complete':False,'cases':[],'scope':'integrated local M2 fixture; optional Git/provider failures covered by existing suites',
              'pre_distribution_failure_retention':'unchanged and undecided','not_claimed':['full environment reproduction','frozen transitive imports','remote availability']}
    try:
        with tempfile.TemporaryDirectory(prefix='projcli-m2-') as temp:
            base=Path(temp);root=base/'project';root.mkdir();guard=base/'git-guard';guard.mkdir()
            shim=guard/'git';shim.write_text('#!/bin/sh\nprintf called >> "$PROJCLI_GIT_CALL_WITNESS"\nexit 97\n');shim.chmod(0o755)
            witness=guard/'called';(guard/'config').write_text('')
            env=os.environ.copy()
            for key in list(env):
                if key.startswith('GIT_') or key in ['GH_TOKEN','GITHUB_TOKEN','GITHUB_PAT','SSH_AUTH_SOCK',
                    'PROJ_PROJECT_ROOT','PROJ_RUN_CONTEXT','PROJR_PROFILE','QUARTO_PROFILE','PROJR_CLEAR_OUTPUT']:
                    env.pop(key,None)
            env.update(PROJCLI_M2_BINARY=str(binary),PROJCLI_GIT_CALL_WITNESS=str(witness),
                       GIT_CONFIG_NOSYSTEM='1',GIT_CONFIG_GLOBAL=str(guard/'config'),GIT_CONFIG_SYSTEM=str(guard/'config'),
                       GIT_TERMINAL_PROMPT='0',PROJCLI_M2_FAIL='0',PATH=str(guard)+os.pathsep+env.get('PATH',''))
            cfg={'directories':{'raw':{'path':'input'},'cache':{'path':'cache'},'output':{'path':'released'},
                  'docs':{'path':'docs'},'raw-excluded':{'path':'private','hash':False}},
                 'parameters':{'factor':2,'token':'M2-PRIVATE-TOKEN','endpoint':'https://u:p@private.invalid/?token=M2-QUERY'},
                 'build':{'scripts':['analysis.py'],'git':False}}
            for folder in ['input','released','docs','private']:(root/folder).mkdir()
            (root/'VERSION').write_bytes(b'v1.2.3-19\r\n')
            (root/'DESCRIPTION').write_bytes(b'Package: m2fixture\nVersion: 1.2.3.19\n')
            (root/'analysis.py').write_text(ANALYSIS)
            (root/'requirements.txt').write_bytes(b'# fixture declaration only\n')
            (root/'private/excluded.csv').write_bytes(b'excluded data')
            prior=None
            for name,input_value,factor,release in [('first',21,2,'v1.2.4'),('second',22,3,'v1.2.5')]:
                cfg['parameters']['factor']=factor
                (root/'_proj.yml').write_text(json.dumps(cfg))
                input_bytes=f'value\n{input_value}\n'.encode();(root/'input/input.csv').write_bytes(input_bytes)
                result,_=invoke(binary,root,env,'build','--patch','--description','integrated M2 fixture')
                assert result.returncode==0,result.stderr
                expected=f'value\n{input_value*factor}\n'.encode()
                final=root/'released/result.csv';assert final.read_bytes()==expected
                assert (root/f'cache/projr/{release}/output/result.csv').read_bytes()==expected
                records=[(path,json.loads(path.read_text())) for path in (root/'.proj/log').glob('*.json')]
                path,record=next((p,r) for p,r in records if r['release_version']==release)
                assert record['status']=='completed' and record['git']['commit']=='not_requested'
                configuration=record['startup_provenance']['configuration']['effective']
                assert configuration['status']=='observed_redacted' and configuration['redacted_values']==2
                assert len(configuration['blake3'])==64
                if prior:
                    assert record['attempt_id']!=prior['attempt_id']
                    assert configuration['blake3']!=prior['startup_provenance']['configuration']['effective']['blake3']
                    assert record['inventory']['inputs']['canonical_csv_blake3']!=prior['inventory']['inputs']['canonical_csv_blake3']
                prior=record
                result,reply=invoke(binary,root,env,'history','--attempt',record['attempt_id'])
                assert result.returncode==0
                rows=reply['result']['history']['entries'][0]['final_files']['rows']
                expected_md5=hashlib.md5(expected).hexdigest()
                assert {(r['label'],r['fn'],r['hash']) for r in rows}=={('output','result.csv',expected_md5),('docs','','')}
                manifest=list(csv.DictReader((root/'manifest.csv').open()))
                raw=next(r for r in manifest if r['version']==release and r['label']=='raw')
                assert raw['fn']=='input.csv' and raw['hash']==hashlib.md5(input_bytes).hexdigest()
                assert all(r['label']!='raw-excluded' for r in manifest)
                csv_ref=record['final_outputs']['rows'];retained=root/'.proj/log'/csv_ref['file']
                assert retained.stat().st_size==csv_ref['bytes']
                assert list(csv.DictReader(retained.open()))==rows
                output=args.artifacts/name;output.mkdir()
                for source,target in [(path,'record.json'),(retained,'final.csv'),(final,'result.csv'),
                    (root/'manifest.csv','manifest.csv'),(root/'input/input.csv','input.csv')]:
                    shutil.copyfile(source,output/target)
                evidence['cases'].append({'case':name,'attempt_id':record['attempt_id'],'release_version':release,
                    'expected_result':input_value*factor,'expected_md5':expected_md5,'configuration_blake3':configuration['blake3'],
                    'input_md5':raw['hash'],'final_rows':len(rows)})
            before={name:(root/name).read_bytes() for name in ['VERSION','DESCRIPTION','manifest.csv','released/result.csv']}
            logs=snapshot(root/'.proj/log')
            failure_env=env.copy();failure_env['PROJCLI_M2_FAIL']='1'
            failure_witness=base/'failed-analysis-ran';failure_env['PROJCLI_M2_FAILURE_WITNESS']=str(failure_witness)
            result,failure=invoke(binary,root,failure_env,'build','--patch')
            assert result.returncode!=0 and failure['error']['code']=='command_failed'
            assert failure_witness.read_bytes()==b'analysis reached exit17 after output overwrite'
            failed_cache=root/'cache/projr/v1.2.6/output/result.csv'
            assert failed_cache.read_bytes()==b'failed-only-output\n'
            shutil.copyfile(failed_cache,args.artifacts/'failed-cache-result.csv')
            shutil.copyfile(failure_witness,args.artifacts/'failure-witness.txt')
            assert {name:(root/name).read_bytes() for name in before}==before
            assert snapshot(root/'.proj/log')==logs
            assert not (root/'.proj/build.lock').exists()
            assert not (root/'.git').exists() and not witness.exists()
            result,history=invoke(binary,root,env,'history');assert result.returncode==0
            entries=history['result']['history']['entries']
            assert len(entries)==2 and all(entry['status']=='completed' for entry in entries)
            assert entries[0]['release_version']=='v1.2.5'
            assert before['VERSION']==b'Version: v1.2.5.1'
            result,_=invoke(binary,root,env,'build','--patch')
            assert result.returncode==0,result.stderr
            assert (root/'released/result.csv').read_bytes()==b'value\n66\n'
            assert failed_cache.read_bytes()==b'value\n66\n'
            assert (root/'VERSION').read_bytes()==b'Version: v1.2.6.1'
            for name,data in logs.items():assert (root/'.proj/log'/name).read_bytes()==data
            retry_path,retry=next((path,json.loads(path.read_text())) for path in (root/'.proj/log').glob('*.json')
                if json.loads(path.read_text())['release_version']=='v1.2.6')
            assert retry['status']=='completed' and retry['attempt_id'] not in {case['attempt_id'] for case in evidence['cases']}
            assert retry['startup_provenance']['configuration']==prior['startup_provenance']['configuration']
            output=args.artifacts/'retry';output.mkdir()
            for source,target in [(retry_path,'record.json'),(root/'.proj/log'/retry['final_outputs']['rows']['file'],'final.csv'),
                (root/'released/result.csv','result.csv'),(root/'manifest.csv','manifest.csv'),(root/'input/input.csv','input.csv')]:
                shutil.copyfile(source,output/target)
            evidence['cases'].append({'case':'retry','attempt_id':retry['attempt_id'],'release_version':'v1.2.6',
                'expected_result':66,'expected_md5':hashlib.md5(b'value\n66\n').hexdigest(),
                'configuration_blake3':retry['startup_provenance']['configuration']['effective']['blake3']})
            result,history=invoke(binary,root,env,'history');assert result.returncode==0
            assert len(history['result']['history']['entries'])==3
            assert history['result']['history']['entries'][0]['release_version']=='v1.2.6'
            result=subprocess.run([str(binary),'--project',str(root),'history'],cwd=root,env=env,capture_output=True,text=True)
            assert result.returncode==0 and all(case['attempt_id'] in result.stdout for case in evidence['cases'])
            (args.artifacts/'history.md').write_text(result.stdout)
            (args.artifacts/'failure.json').write_text(json.dumps(failure))
            (args.artifacts/'analysis.py').write_text(ANALYSIS)
            shutil.copyfile(root/'requirements.txt',args.artifacts/'requirements.txt')
            for data in logs.values():
                for secret in [b'M2-PRIVATE-TOKEN',b'M2-QUERY',b'private.invalid',str(root).encode()]:
                    assert secret not in data
            evidence.update(complete=True,failed_build_preserves_exact_last_good_state=True,no_false_successful_release=True,
                no_git_calls_or_credentials_required=True,history_from_records=True,exact_prior_development_version_preserved=True,retry_succeeds_without_rewriting_prior_records=True)
    except Exception as error:
        evidence['error']=str(error);raise
    finally:
        args.evidence.write_text(json.dumps(evidence,indent=2)+'\n');print(json.dumps(evidence))
if __name__=='__main__':main()
