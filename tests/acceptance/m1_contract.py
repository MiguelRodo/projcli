"""Remote-only M1 acceptance; rerun on reviewed combined source before release."""
from __future__ import annotations
import argparse, base64, csv, hashlib, json, os, re, shutil, subprocess, tempfile
from pathlib import Path
from path_contract import sentinel_png, R_ADAPTER

def invoke(binary, root, env, *args):
    result=subprocess.run([str(binary),'--machine','--non-interactive','--project',str(root),*args],
                          cwd=root,env=env,text=True,capture_output=True)
    envelope=json.loads(result.stdout)
    assert envelope['protocol_version']==1 and envelope['ok']==(result.returncode==0)
    return result,envelope

def case(binary, workspace, artifacts, initialized, mode, failing):
    name=('initialized' if initialized else 'manual-no-init')+'-'+mode+('-failure' if failing else '')
    root=workspace/name;root.mkdir()
    env=os.environ.copy()
    for key in ['PROJ_PROJECT_ROOT','PROJ_RUN_CONTEXT','PROJR_PROFILE','QUARTO_PROFILE','PROJR_CLEAR_OUTPUT',
                'GITHUB_PAT','GH_TOKEN','GITHUB_TOKEN','GIT_ASKPASS','SSH_ASKPASS']:
        env.pop(key,None)
    env['PROJCLI_ACCEPT_BINARY']=str(binary)
    if initialized:
        result,_=invoke(binary,root,env,'init','directories')
        assert result.returncode==0,result.stderr
        assert not (root/'.git').exists() and not (root/'VERSION').exists()
        config=(root/'_proj.yml').read_text().replace('directories:\n','directories:\n  docs: {path: _docs}\n',1)
        assert 'scripts:' not in config
    else:
        config='directories:\n  raw-data: {path: _raw_data}\n  cache: {path: _tmp}\n  output: {path: _output}\n  docs: {path: _docs}\n'
    config+=('remotes:\n  local:\n    archive:\n      path: archive\n      structure: cas\n'
             '      inspect: file\n      content: [output, docs]\nsettings:\n  output-clear-stage: none\n'
             'build:\n  git: false\n  dest: [archive]\n'
             '  scripts: [01-analysis.py, 02-summary.R, 03-report.Rmd]\n  restrictions:\n    not_behind: false\n')
    (root/'_proj.yml').write_text(config)
    for name_dir in ['_raw_data','_tmp','_output','_docs','archive']:(root/name_dir).mkdir(exist_ok=True)
    (root/'_raw_data/data.csv').write_text('value\n42\n')
    (root/'_raw_data/helper.R').write_text('helper_value <- 42\n')
    image=sentinel_png();(root/'_raw_data/sentinel.png').write_bytes(image)
    for path,data in [('_output/previous.csv',b'old,release\n1,2\n'),('_docs/previous.html',b'previous report'),
                      ('archive/previous.bin',b'previous archive bytes'),('03-report.html',b'preexisting native report'),
                      ('VERSION',b'v1.2.3-4\n')]:
        (root/path).write_bytes(data)
    python=r'''import csv,json,os,subprocess
from pathlib import Path
def resource(label,*parts):
 value=json.loads(subprocess.check_output([os.environ['PROJCLI_ACCEPT_BINARY'],'--machine','path','get','--label',label,*parts],text=True))
 return Path(value['result']['path'])
value=int(next(csv.DictReader(resource('raw-data','data.csv').open()))['value'])
target=resource('output','results.csv');target.parent.mkdir(parents=True,exist_ok=True)
target.write_text('result\n'+str(value*2)+'\n')
with resource('cache','order.txt').open('a') as stream:stream.write('01-python\n')
'''
    (root/'01-analysis.py').write_text(python)
    r=R_ADAPTER+'\nstopifnot(read.csv(cli_path("output","results.csv"))$result[[1]] == 84)\ncat("02-R\\n",file=cli_path("cache","order.txt"),append=TRUE)\n'
    if failing:r+='stop("deliberate combined M1 failure")\n'
    (root/'02-summary.R').write_text(r)
    fence=chr(96)*3
    report='---\ntitle: M1 inspectable report\noutput: html_document\n---\n\n'+fence+'{r, echo=FALSE}\n'+R_ADAPTER
    report+='\ncat("03-report\\n",file=cli_path("cache","order.txt"),append=TRUE)\nknitr::kable(read.csv(cli_path("output","results.csv")))\n'
    report+=fence+'\n\n![exact sentinel](_raw_data/sentinel.png)\n'
    (root/'03-report.Rmd').write_text(report)
    original={p.relative_to(root):p.read_bytes() for p in root.rglob('*') if p.is_file() and '_tmp' not in p.relative_to(root).parts}
    result,envelope=invoke(binary,root,env,mode)
    assert (result.returncode!=0)==failing,result.stderr[-6000:]
    if failing:assert envelope.get('error') and 'deliberate combined M1 failure' in result.stderr
    order=(root/'_tmp/order.txt').read_text().splitlines()
    assert order==(['01-python','02-R'] if failing else ['01-python','02-R','03-report']),order
    key='v1.2.4' if mode=='build' else 'v1.2.3-4';cache=root/'_tmp/projr'/key
    assert list(csv.DictReader((cache/'output/results.csv').open()))==[{'result':'84'}]
    published=not failing and mode=='build'
    for path,data in original.items():
        if published and str(path) in ['VERSION','.gitignore','.Rbuildignore','_docs/previous.html']:continue
        assert (root/path).read_bytes()==data,(name,str(path))
    assert not (root/'.git').exists()
    assert (root/'_output/results.csv').exists()==published and (root/'_docs/03-report.html').exists()==published
    manifests=list((root/'archive/manifests').rglob('*.json'));assert bool(manifests)==published,name
    if not published:
        assert sorted(str(p.relative_to(root/'archive')) for p in (root/'archive').rglob('*'))==['previous.bin']
        assert (root/'VERSION').read_bytes()==b'v1.2.3-4\n'
    destination=artifacts/name;destination.mkdir(parents=True)
    shutil.copyfile(cache/'output/results.csv',destination/'results.csv')
    report_hash=None
    if not failing:
        rendered=cache/'docs/03-report.html';text=rendered.read_text()
        assert re.search(r'<td[^>]*>\s*84\s*</td>',text),'report must contain computed CSV value'
        assert base64.b64encode(image).decode() in text,'exact embedded image bytes'
        shutil.copyfile(rendered,destination/'report.html')
        report_hash=hashlib.sha256(rendered.read_bytes()).hexdigest()
        if published:
            assert (root/'_docs/03-report.html').read_bytes()==rendered.read_bytes()
            assert (root/'.projr/manifests.csv').is_file()
        root.rename(workspace/(name+'-source-hidden'))
        assert base64.b64encode(image).decode() in (destination/'report.html').read_text()
    return {'case':name,'initialized_via_cli':initialized,'exit_code':result.returncode,'script_order':order,
            'cache_key':key,'computed_csv_result':84,'report_sha256':report_hash,'archive_manifest_count':len(manifests),
            'previous_final_and_archive_bytes_preserved':True,'no_git_metadata_or_credentials_required':True}

def main():
    parser=argparse.ArgumentParser();parser.add_argument('binary',type=Path)
    parser.add_argument('--evidence',type=Path,required=True);parser.add_argument('--artifacts',type=Path,required=True)
    args=parser.parse_args();binary=args.binary.resolve(strict=True)
    evidence={'cases':[],'complete':False,'limitations':['rerun on reviewed combined source before release',
      'fixture CLI adapters; existing R package untouched','native Bookdown embedding not covered',
      'no R/IDE parity or nonembedded portability claim']}
    try:
        with tempfile.TemporaryDirectory(prefix='projcli-m1-combined-') as directory:
            for initialized in [False,True]:
                for mode in ['build-dev','build']:
                    for failing in [False,True]:
                        evidence['cases'].append(case(binary,Path(directory),args.artifacts,initialized,mode,failing))
        evidence['complete']=True
    except Exception as error:
        evidence['error']=str(error);raise
    finally:
        args.evidence.write_text(json.dumps(evidence,indent=2)+'\n');print(json.dumps(evidence))

if __name__=='__main__':main()
