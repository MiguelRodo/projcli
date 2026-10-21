#!/usr/bin/env python3
"Unmerged diagnostic. Actual tools, fixture data only, no production patches."
import base64, functools, hashlib, html.parser, http.server, json, os, re, runpy, shutil, struct, subprocess, sys, tempfile, threading, urllib.error, urllib.parse, urllib.request, zlib
from pathlib import Path
import yaml

HERE = Path(__file__).resolve().parent
OUT = Path('diagnostic-results').resolve(); OUT.mkdir(exist_ok=True)
EVIDENCE = OUT / 'paths'; EVIDENCE.mkdir(exist_ok=True)
IMAGE_EVIDENCE = OUT / 'image-paths'; IMAGE_EVIDENCE.mkdir(exist_ok=True)
PROJECTS = OUT / 'projects with spaces'; PROJECTS.mkdir(exist_ok=True)
BUNDLES = OUT / 'standalone-bundles'; BUNDLES.mkdir(exist_ok=True)
PROJR_PIN = '90be50132d16aee33e39c0f7f35ef906c35f5397'
PROJ = Path('diagnostic-proj').resolve()
VERSION = 'v1.2.3.4'
BASE_ENV = {key: value for key, value in os.environ.items()
            if not any(part in key.upper() for part in ['TOKEN','SECRET','PASSWORD','CREDENTIAL','PRIVATE_KEY'])
            and not key.upper().endswith(('_KEY', '_PAT'))
            and not key.startswith(('GITHUB_', 'GH_', 'ACTIONS_', 'PROJ_', 'PROJR_', 'QUARTO_PROFILE'))}
quarto = Path(shutil.which('quarto')).resolve()
if not shutil.which('pandoc'):
    candidates = list((quarto.parent / 'tools').rglob('pandoc'))
    if candidates: BASE_ENV['RSTUDIO_PANDOC'] = str(candidates[0].parent)

def execute(command, cwd, env, label, timeout=180):
    result = subprocess.run([str(x) for x in command], cwd=cwd, env=env, capture_output=True, text=True, timeout=timeout)
    (OUT / (label + '.log')).write_text(result.stdout + '\nSTDERR\n' + result.stderr)
    return {'returncode': result.returncode, 'log': label + '.log'}

def png():
    def chunk(name, data): return struct.pack('>I',len(data)) + name + data + struct.pack('>I',zlib.crc32(name+data)&0xffffffff)
    rows = b''.join(b'\0' + bytes([12, 160, 90])*32 for _ in range(24))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR',struct.pack('>IIBBBBB',32,24,8,2,0,0,0)) + chunk(b'IDAT',zlib.compress(rows)) + chunk(b'IEND',b'')

def fixture(name, project=False, absolute=False, profile=False):
    root = PROJECTS / name; root.mkdir()
    (root/'analysis/deeper').mkdir(parents=True)
    for raw, value in [('_raw',17),('_raw/profile',23)]:
        folder=root/raw; (folder/'images').mkdir(parents=True,exist_ok=True)
        (folder/'data.csv').write_text('value\n'+str(value)+'\n')
        (folder/'helper.R').write_text('helper_marker <- function() "helper-sentinel"\n')
        (folder/'helper.py').write_text('def helper_marker(): return "helper-sentinel"\n')
        (folder/'images/sentinel.png').write_bytes(png())
    paths={'raw-data':'_raw','cache':'_cache','output':'_published/output','docs':'_published/site'}
    config={'directories': {key:{'path':str(root/path) if absolute else path} for key,path in paths.items()}}
    (root/'_projr.yml').write_text(yaml.safe_dump(config,sort_keys=False))
    if profile: (root/'_projr-analysis.yml').write_text('directories:\n  raw-data:\n    path: _raw/profile\n')
    (root/'VERSION').write_text(VERSION+'\n')
    if project: (root/'_quarto.yml').write_text('project:\n  type: default\n  output-dir: rendered\n')
    env=dict(BASE_ENV, DIAG_ROOT=str(root), DIAG_VERSION=VERSION, PROJ_PROJECT_ROOT=str(root),
             DIAG_PROFILES='analysis' if profile else '', PROJR_PROFILE='analysis' if profile else '',
             DIAG_SENTINEL='23' if profile else '17', DIAG_EVIDENCE=str(EVIDENCE), DIAG_CASE=name,
             DIAG_EXERCISE_R=str(HERE/'exercise.R'), DIAG_PROJ_BIN=str(PROJ), DIAG_AUTO_EXERCISE='no', DIAG_IMAGE_EVIDENCE=str(IMAGE_EVIDENCE))
    return root,env

class Images(html.parser.HTMLParser):
    def __init__(self): super().__init__(); self.images=[]; self.local_assets=[]
    def handle_starttag(self,tag,attrs):
        attrs=dict(attrs)
        if tag=='img':
            # Quarto 1.10 emits aria-label for fig.alt and no alt for Markdown captions.
            attrs['alt'] = attrs.get('alt') or attrs.get('aria-label') or 'native markdown sentinel'
            self.images.append(attrs)
        if tag in ('script','link'):
            value=attrs.get('src') or attrs.get('href')
            if value and not urllib.parse.urlsplit(value).scheme and not value.startswith('#'): self.local_assets.append(value)

def inspect_html(path, bundle=None, expected_images=3):
    parser=Images();parser.feed(path.read_text())
    images=[]
    for attrs in parser.images:
        src=attrs.get('src',''); split=urllib.parse.urlsplit(src)
        if src.startswith('data:'):
            payload=src.split(',',1)[1]
            data=base64.b64decode(payload) if ';base64,' in src else urllib.parse.unquote_to_bytes(payload)
            images.append({'alt':attrs.get('alt'),'embedded':True,'matches_sentinel':hashlib.sha256(data).hexdigest()==hashlib.sha256(png()).hexdigest()})
        elif not split.scheme:
            resource=urllib.parse.unquote(split.path)
            target=((bundle/resource.lstrip('/')) if bundle and resource.startswith('/') else (path.parent/resource)).resolve()
            images.append({'alt':attrs.get('alt'),'src':src,'target':str(target),'exists':target.is_file(), 'within_bundle':target.is_relative_to(bundle) if bundle else None})
        else: images.append({'alt':attrs.get('alt'),'src':src,'external':True})
    missing=[value for value in parser.local_assets if not (path.parent/urllib.parse.unquote(urllib.parse.urlsplit(value).path)).exists()]
    result = {'html':str(path),'sentinel_images':images,'missing_local_script_styles':missing}
    if bundle:
        class QuietHandler(http.server.SimpleHTTPRequestHandler):
            def log_message(self, *args): pass
        handler=functools.partial(QuietHandler,directory=str(bundle))
        server=http.server.ThreadingHTTPServer(('127.0.0.1',0),handler)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        base='http://127.0.0.1:'+str(server.server_port)+'/'+urllib.parse.quote(str(path.relative_to(bundle)))
        try:
            for image in images:
                if image.get('embedded'): continue
                try:
                    with urllib.request.urlopen(urllib.parse.urljoin(base,image['src']),timeout=5) as response:
                        data=response.read();image['http_status']=response.status
                        image['http_matches_sentinel']=hashlib.sha256(data).hexdigest()==hashlib.sha256(png()).hexdigest()
                except (urllib.error.URLError,ValueError) as error:
                    image['http_status']=getattr(error,'code',None);image['http_error']=type(error).__name__
            result['sentinel_bundle_complete']=len(images)>=expected_images and all(image.get('matches_sentinel') or (image.get('within_bundle') and image.get('http_status')==200 and image.get('http_matches_sentinel')) for image in images)
        finally:
            server.shutdown();server.server_close();thread.join(timeout=5)
    return result

results=[]
versions={}
for name,command in [('R',['Rscript','--version']),('Python',[sys.executable,'--version']),('Quarto',['quarto','--version']),('Pandoc',['quarto','pandoc','--version']),('Rust CLI',[PROJ,'--version'])]:
    result=subprocess.run([str(x) for x in command],env=BASE_ENV,capture_output=True,text=True)
    versions[name]=(result.stdout+result.stderr).splitlines()[:2]
package_versions=subprocess.run(['Rscript','--vanilla','-e','cat(jsonlite::toJSON(setNames(lapply(c("projr","rmarkdown","knitr","yaml","fs"), function(x) as.character(packageVersion(x))),c("projr","rmarkdown","knitr","yaml","fs")),auto_unbox=TRUE))'],env=BASE_ENV,capture_output=True,text=True,check=True)
versions['R_packages']=json.loads(package_versions.stdout)
versions['projr_source_commit']=PROJR_PIN
(OUT/'versions.json').write_text(json.dumps(versions,indent=2))



# Literal author markup is independent of proj role lookups and R code cwd.
if os.environ.get('DIAG_LITERAL_ONLY', '').lower() == 'true':
    VERSION='v1.2.3'
    scenarios=[]
    for renderer in ['rmarkdown','quarto-document','quarto-project']:
        scenarios.append((renderer,'root','relative','native'))
    for renderer in ['quarto-document','quarto-project']:
        for location,mode in [('root','native'),('nested','native'),('nested','root')]:
            scenarios.append((renderer,location,'leading-slash',mode))
    for renderer,location,url_kind,mode in scenarios:
      for embedded in [True,False]:
        name='literal-'+renderer+'-'+location+'-'+url_kind+'-'+mode+'-'+('embedded' if embedded else 'linked')
        root,env=fixture(name,project=renderer=='quarto-project')
        extension='Rmd' if renderer=='rmarkdown' else 'qmd'
        folder=root if location=='root' else root/'analysis'
        url='/_raw/images/sentinel.png' if url_kind=='leading-slash' else '_raw/images/sentinel.png'
        setting='output:\n  html_document:\n    self_contained: '+str(embedded).lower()+'\n' if extension=='Rmd' else 'format:\n  html:\n    embed-resources: '+str(embedded).lower()+'\n'
        document=folder/('report.'+extension)
        document.write_text('---\ntitle: Literal markup\n'+setting+'---\n\n![literal markdown sentinel]('+url+')\n')
        original=document.read_bytes()
        if extension=='Rmd':
            command=['Rscript','--vanilla','-e','args <- commandArgs(TRUE); rmarkdown::render(args[[1]], quiet=FALSE)',document]
        else:
            command=['quarto','render',document,'--to','html']
            if mode=='root': command+=['--execute-dir',root]
        entry=dict(case=name,runner=renderer,location=location,literal_url=url,execution_mode=mode,embedded=embedded,**execute(command,root,env,name,timeout=240))
        entry['literal_source_unchanged']=document.read_bytes()==original
        candidates=list(root.rglob('report.html'))
        if candidates:
            html=candidates[0];entry['native']=inspect_html(html,expected_images=1)
            bundle=BUNDLES/name/'html-plus-stem';bundle.mkdir(parents=True)
            target=bundle/'report.html';shutil.copy2(html,target)
            if (html.parent/'report_files').exists(): shutil.copytree(html.parent/'report_files',bundle/'report_files')
            whole_html=None
            if renderer=='quarto-project' and html.is_relative_to(root/'rendered'):
                whole=BUNDLES/name/'whole-project';shutil.copytree(root/'rendered',whole)
                whole_html=whole/html.relative_to(root/'rendered')
            root.rename(root.with_name(root.name+'-source-unavailable'))
            entry['source_and_cache_unavailable']=not root.exists()
            entry['relocated']=inspect_html(target,bundle,expected_images=1)
            if whole_html: entry['whole_project_relocated']=inspect_html(whole_html,whole,expected_images=1)
        results.append(entry)
    # Four actual released-CLI builds on disposable fixtures; no Git or publication.
    for extension in ['Rmd','qmd']:
      for location in ['root','nested']:
        name='literal-released-cli-'+extension+'-'+location+'-embedded'
        root,env=fixture(name)
        folder=root if location=='root' else root/'analysis'
        url=('_raw/' if location=='root' else '../_raw/')+'images/sentinel.png'
        setting='output:\n  html_document:\n    self_contained: true\n' if extension=='Rmd' else 'format:\n  html:\n    embed-resources: true\n'
        document=folder/('report.'+extension)
        document.write_text('---\ntitle: Literal CLI markup\n'+setting+'---\n\n![literal markdown sentinel]('+url+')\n')
        original=document.read_bytes()
        cfg=yaml.safe_load((root/'_projr.yml').read_text())
        cfg['config']={'git':{'use_proj_cred_helper':False}}
        cfg['build']={'scripts':[str(document.relative_to(root))],'git':False,'restrictions':{'not_behind':False}}
        (root/'_projr.yml').write_text(yaml.safe_dump(cfg,sort_keys=False))
        entry=dict(case=name,runner='released-projcli',location=location,literal_url=url,embedded=True,**execute([PROJ,'build','--patch','--non-interactive','--project',root],root,env,name,timeout=240))
        entry['literal_source_unchanged']=document.read_bytes()==original
        html=root/'_published/site/report.html'
        if html.exists():
            bundle=BUNDLES/name;shutil.copytree(html.parent,bundle)
            root.rename(root.with_name(root.name+'-source-unavailable'))
            entry['source_and_cache_unavailable']=not root.exists()
            entry['relocated']=inspect_html(bundle/'report.html',bundle,expected_images=1)
        results.append(entry)
    latex=next((shutil.which(name) for name in ['pdflatex','xelatex','lualatex'] if shutil.which(name)),None)
    pdf={'tested':False,'reason':'No preinstalled TeX engine; no large installation attempted','engine':latex}
    if latex:
        root,env=fixture('literal-raw-latex-pdf')
        document=root/'analysis/report.Rmd'
        document.write_text('---\ntitle: Literal LaTeX\n---\n\n\\includegraphics{../_raw/images/sentinel.png}\n')
        code='args <- commandArgs(TRUE); options(tinytex.install_packages=FALSE); rmarkdown::render(args[[1]],output_format=rmarkdown::pdf_document(latex_engine=args[[2]]),quiet=FALSE)'
        pdf=dict(tested=True,engine=latex,**execute(['Rscript','--vanilla','-e',code,document,Path(latex).name],root,env,'literal-raw-latex-pdf',timeout=180))
        artifact=root/'analysis/report.pdf';pdf['valid_pdf_header']=artifact.exists() and artifact.read_bytes().startswith(b'%PDF-')
        pdf['embedded_image_verified']=False
    missing_html=[entry['case'] for entry in results if entry['returncode']==0 and 'relocated' not in entry]
    changed_sources=[entry['case'] for entry in results if not entry['literal_source_unchanged']]
    summary={'diagnostic_only':True,'literal_only':True,'versions':versions,'runners':results,'raw_latex_pdf':pdf,'counts':{'cases':len(results),'render_process_failures':sum(entry['returncode']!=0 for entry in results),'source_changed':changed_sources,'successful_render_missing_html':missing_html},'scope':'Literal Markdown resource behavior; linked results are observations, not a commitment to repair portability. No GUI or R frontend change.'}
    (OUT/'literal-summary.json').write_text(json.dumps(summary,indent=2))
    print(json.dumps(summary['counts'],indent=2))
    assert len(results)==22 and not (missing_html or changed_sources), 'Literal probes incomplete or source changed'
    sys.exit(0)

# Narrow follow-up: six root-execution HTML cases, not the full diagnostic suite.
if os.environ.get('DIAG_ADAPTER_ONLY', '').lower() == 'true':
    VERSION = 'v1.2.3-4'  # Canonical R display spelling; no cache-escape claim from version aliases.
    for renderer in ['rmarkdown', 'quarto-document']:
      for variant in ['absolute-default', 'absolute-rel-path-false', 'document-relative-resource']:
        name=renderer+'-adapter-'+variant
        root,env=fixture(name)
        extension='Rmd' if renderer=='rmarkdown' else 'qmd'
        setting='output:\n  html_document:\n    self_contained: true\n' if extension=='Rmd' else 'format:\n  html:\n    embed-resources: true\n'
        code='source(Sys.getenv("DIAG_EXERCISE_R"))\n'
        code+='filesystem_relative <- diag_prototype("raw-data", "images", "sentinel.png")\n'
        code+='absolute_target <- normalizePath(filesystem_relative, mustWork=TRUE)\n'
        code+='stopifnot(file.exists(filesystem_relative), read.csv(diag_prototype("raw-data", "data.csv"))$value[[1]] == 17)\n'
        if variant=='document-relative-resource':
            code+='resource_path <- as.character(fs::path_rel(absolute_target, start=file.path(Sys.getenv("DIAG_ROOT"), "analysis")))\n'
            invocation='knitr::include_graphics(resource_path, rel_path=FALSE, error=FALSE)'
        else:
            code+='resource_path <- absolute_target\n'
            invocation='knitr::include_graphics(resource_path'+(', rel_path=FALSE' if variant=='absolute-rel-path-false' else '')+')'
        code+='jsonlite::write_json(list(cwd=getwd(), filesystem_relative=filesystem_relative, absolute_target=absolute_target, renderer_argument=resource_path, variant="'+variant+'", filesystem_read_ok=TRUE), file.path(Sys.getenv("DIAG_IMAGE_EVIDENCE"), "'+name+'.json"), auto_unbox=TRUE, pretty=TRUE)\n'
        code+=invocation+'\n'
        document=root/'analysis'/('report.'+extension)
        document.write_text('---\ntitle: Adapter diagnostic\n'+setting+'---\n\n```{r, echo=FALSE, fig.alt="adapter sentinel"}\n'+code+'```\n')
        if extension=='Rmd':
            command=['Rscript','--vanilla','-e','args <- commandArgs(TRUE); rmarkdown::render(args[[1]], knit_root_dir=args[[2]], quiet=FALSE)',document,root]
        else: command=['quarto','render',document,'--to','html','--execute-dir',root]
        entry=dict(case=name,variant=variant,runner=renderer,**execute(command,root,env,name,timeout=240))
        html=root/'analysis/report.html'
        if html.exists():
            entry['native']=inspect_html(html,expected_images=1)
            bundle=BUNDLES/name;bundle.mkdir(parents=True)
            target=bundle/'report.html';shutil.copy2(html,target)
            if (html.parent/'report_files').exists(): shutil.copytree(html.parent/'report_files',bundle/'report_files')
            root.rename(root.with_name(root.name+'-source-unavailable'))
            entry['source_and_cache_unavailable']=not root.exists()
            entry['relocated']=inspect_html(target,bundle,expected_images=1)
        results.append(entry)
    evidence={path.stem:json.loads(path.read_text()) for path in IMAGE_EVIDENCE.glob('*.json')}
    missing=[entry['case'] for entry in results if entry['case'] not in evidence]
    missing_html=[entry['case'] for entry in results if entry['returncode']==0 and 'relocated' not in entry]
    summary={'diagnostic_only':True,'adapter_only':True,'versions':versions,'runners':results,'lookup_evidence':evidence,'counts':{'cases':len(results),'render_process_failures':sum(entry['returncode']!=0 for entry in results),'portable_embedded_reports':sum(entry.get('relocated',{}).get('sentinel_bundle_complete',False) for entry in results),'missing_lookup_evidence':missing,'successful_render_missing_html':missing_html},'scope':'Root execution, explicit embedded HTML only; no PDF, GUI, linked adapter or frontend implementation claim.'}
    (OUT/'adapter-summary.json').write_text(json.dumps(summary,indent=2))
    print(json.dumps(summary['counts'],indent=2))
    assert len(results)==6 and not (missing or missing_html), 'Adapter probes incomplete'
    sys.exit(0)

# Native Rscript does not auto-chdir to a script file; source(chdir=FALSE) inherits caller cwd.
for name,cwd_mode,source_chdir,absolute,profile in [
    ('rscript-native','analysis',None,False,False),('rscript-root','root',None,False,False),
    ('source-caller-root','root',False,False,False),('source-caller-analysis','analysis',False,False,False),
    ('source-chdir-true','root',True,False,False),('rscript-absolute-config','analysis',None,True,False),
    ('rscript-profile','analysis',None,False,True)]:
    root,env=fixture(name,absolute=absolute,profile=profile)
    script=root/'analysis/script.R';script.write_text('source(Sys.getenv("DIAG_EXERCISE_R"))\ndiag_exercise()\n')
    cwd=root if cwd_mode=='root' else root/'analysis'
    if source_chdir is None: command=['Rscript','--vanilla',script]
    else: command=['Rscript','--vanilla','-e','args <- commandArgs(TRUE); source(args[[1]], chdir = args[[2]] == "yes")',script,'yes' if source_chdir else 'no']
    results.append(dict(case=name,runner='Rscript/source',**execute(command,cwd,env,name)))

# Re-query after cwd changes, including a distinct project: root pinning is tested, not assumed.
root,env=fixture('rscript-cwd-switch')
other,unused=fixture('other-project')
(other/'_raw/data.csv').write_text('value\n91\n')
env['DIAG_OTHER_ROOT']=str(other)
script=root/'analysis/switch.R';script.write_text('source(Sys.getenv("DIAG_EXERCISE_R")); diag_exercise("-before"); setwd("deeper"); diag_exercise("-deeper"); setwd(Sys.getenv("DIAG_OTHER_ROOT")); diag_exercise("-other-project")\n')
results.append(dict(case='rscript-cwd-switch',runner='Rscript',**execute(['Rscript','--vanilla',script],root/'analysis',env,'rscript-cwd-switch')))

for root_mode in [False,True]:
    name='python-'+('root' if root_mode else 'native')
    root,env=fixture(name)
    code=r'''import csv, json, os, pathlib, runpy, subprocess, yaml
root=pathlib.Path(os.environ['DIAG_ROOT']); cfg=yaml.safe_load((root/'_projr.yml').read_text()); records=[]
for resolver in ['released_rust_cli','test_only_prototype']:
 def get_path(label,*parts):
  if resolver=='released_rust_cli':
   value=json.loads(subprocess.check_output([os.environ['DIAG_PROJ_BIN'],'--machine','path','get','--label',label],text=True)); return str(pathlib.Path(value['result']['path']).joinpath(*parts))
  target=pathlib.Path(cfg['directories'][label]['path']); target=target if target.is_absolute() else root/target
  if label.startswith('output'): target=root/cfg['directories']['cache']['path']/'projr'/os.environ['DIAG_VERSION']/label
  return os.path.relpath(target.joinpath(*parts),pathlib.Path.cwd())
 record={'resolver':resolver,'cwd':str(pathlib.Path.cwd()),'captured_root':str(root)}
 try:
  data=get_path('raw-data','data.csv'); helper=get_path('raw-data','helper.py'); image=get_path('raw-data','images','sentinel.png'); output=get_path('output','python.txt')
  assert next(csv.DictReader(open(data)))['value']=='17'; assert runpy.run_path(helper)['helper_marker']()=='helper-sentinel'; assert pathlib.Path(image).is_file()
  pathlib.Path(output).parent.mkdir(parents=True,exist_ok=True); pathlib.Path(output).write_text('sentinel')
  record.update(ok=True,data_path=data,output_path=output,relative=not pathlib.Path(data).is_absolute(),output_absolute=str(pathlib.Path(output).resolve()))
 except Exception as error: record.update(ok=False,error=str(error))
 records.append(record)
pathlib.Path(os.environ['DIAG_EVIDENCE'],os.environ['DIAG_CASE']+'.json').write_text(json.dumps(records,indent=2))
'''
    script=root/'analysis/script.py';script.write_text(code)
    results.append(dict(case=name,runner='Python',**execute([sys.executable,script],root if root_mode else root/'analysis',env,name)))

for renderer in ['rmarkdown','quarto-document','quarto-project']:
 for root_mode in [False,True]:
  scenarios=[None,False,True]
  if renderer=='quarto-project': scenarios+=['inherited-false','profile-false']
  elif renderer=='rmarkdown': scenarios+=['inherited-false']
  for embedded in scenarios:
    name=renderer+'-'+('root' if root_mode else 'native')+'-'+(embedded if isinstance(embedded,str) else ('native-default' if embedded is None else ('embedded' if embedded else 'linked')))
    root,env=fixture(name,project=renderer=='quarto-project')
    extension='Rmd' if renderer=='rmarkdown' else 'qmd'
    if extension=='Rmd':
      setting='output: html_document\n' if embedded is None or isinstance(embedded,str) else 'output:\n  html_document:\n    self_contained: '+str(embedded).lower()+'\n'
    else:
      setting='format: html\n' if embedded is None or isinstance(embedded,str) else 'format:\n  html:\n    embed-resources: '+str(embedded).lower()+'\n'
    body='---\ntitle: Relative path diagnostic\n'+setting+'---\n\n![native markdown sentinel](../_raw/images/sentinel.png)\n\n'
    body+='```{r diagnostics, echo=FALSE}\nsource(Sys.getenv("DIAG_EXERCISE_R"))\ndiag_exercise()\n```\n\n'
    for resolver,caption in [('unchanged_projr_relative','unchanged projr sentinel'),('test_only_prototype','test only prototype sentinel')]:
      body+='```{r '+resolver+', echo=FALSE, fig.alt="'+caption+'"}\nimage <- diag_image_path("'+resolver+'")\nif (is.null(image)) cat("**DIAGNOSTIC IMAGE LOOKUP FAILED: '+resolver+'**") else knitr::include_graphics(image)\n```\n\n'
    document=root/'analysis'/('report.'+extension);document.write_text(body)
    if embedded=='inherited-false':
      if renderer=='rmarkdown': (root/'analysis/_output.yml').write_text('html_document:\n  self_contained: false\n')
      else:
        project_config=root/'_quarto.yml'
        project_config.write_text(project_config.read_text()+'format:\n  html:\n    embed-resources: false\n')
    if embedded=='profile-false': (root/'_quarto-linked.yml').write_text('format:\n  html:\n    embed-resources: false\n')
    if renderer=='rmarkdown':
      code='args <- commandArgs(TRUE); options(tinytex.install_packages=FALSE); rmarkdown::render(args[[1]], knit_root_dir=if(args[[2]] == "yes") args[[3]] else NULL, quiet=FALSE)'
      command=['Rscript','--vanilla','-e',code,document,'yes' if root_mode else 'no',root]
    else:
      command=['quarto','render',document,'--to','html']
      if root_mode: command.extend(['--execute-dir',root])
      if embedded=='profile-false': command.extend(['--profile','linked'])
    entry=dict(case=name,runner=renderer,root_toggle=root_mode,embedded=embedded,**execute(command,root,env,name,timeout=240))
    candidates=list(root.rglob('report.html'))
    if candidates:
      html=candidates[0];entry['native']=inspect_html(html)
      relocated=BUNDLES/name/'html-plus-stem-assets';relocated.mkdir(parents=True)
      target=relocated/'report.html';shutil.copy2(html,target)
      assets=html.parent/'report_files'
      if assets.exists(): shutil.copytree(assets,relocated/'report_files')
      complete_html=None
      if renderer=='quarto-project' and (root/'rendered').exists() and html.is_relative_to(root/'rendered'):
        complete=BUNDLES/name/'whole-project';shutil.copytree(root/'rendered',complete)
        complete_html=complete/html.relative_to(root/'rendered')
      root.rename(root.with_name(root.name+'-source-unavailable'))
      entry['original_source_and_cache_unavailable']=not root.exists()
      entry['html_plus_stem_assets_relocated']=inspect_html(target,relocated)
      if complete_html: entry['whole_project_output_relocated']=inspect_html(complete_html,complete)
    results.append(entry)

# PDF is optional; no TeX or engine installation is attempted by this diagnostic.
latex=next((shutil.which(name) for name in ['pdflatex','xelatex','lualatex'] if shutil.which(name)),None)
if latex:
    name='rmarkdown-pdf-existing-engine';root,env=fixture(name)
    document=root/'analysis/report.Rmd';document.write_text('---\ntitle: PDF diagnostic\n---\n\n![native markdown sentinel](../_raw/images/sentinel.png)\n\n```{r}\nsource(Sys.getenv("DIAG_EXERCISE_R")); diag_exercise()\n```\n')
    code='args <- commandArgs(TRUE); options(tinytex.install_packages=FALSE); rmarkdown::render(args[[1]], output_format=rmarkdown::pdf_document(latex_engine=args[[2]]), quiet=FALSE)'
    entry=dict(case=name,runner='rmarkdown-pdf',**execute(['Rscript','--vanilla','-e',code,document,Path(latex).name],root,env,name,timeout=180))
    pdf=root/'analysis/report.pdf';entry['pdf_header_valid']=pdf.is_file() and pdf.read_bytes().startswith(b'%PDF-');results.append(entry)
else: results.append({'case':'PDF','tested':False,'reason':'No existing TeX engine; no large installation attempted'})

paths={file.stem:json.loads(file.read_text()) for file in EVIDENCE.glob('*.json')}
image_paths={file.stem:json.loads(file.read_text()) for file in IMAGE_EVIDENCE.glob('*.json')}
summary={'image_lookup_evidence':image_paths,'diagnostic_only':True,'prototype_is_not_current_behavior':True,'versions':versions,'runners':results,'paths':paths,
         'r_gui_tested':False,'os_tested':'Linux only','proposed_embedding_fallback':'unspecified -> true; explicit false/true preserved; not applied by this diagnostic','pdf_visual_or_embedded_image_validation':False}
(OUT/'summary.json').write_text(json.dumps(summary,indent=2))
print(json.dumps({'cases':len(results),'path_evidence':len(paths),'runner_failures':[entry['case'] for entry in results if entry.get('returncode',0)!=0],
                  'image_lookup_failures':[case for case,value in image_paths.items() if not value.get('ok')],
                  'path_failures':{case:[item.get('resolver') for item in records if not item.get('ok')] for case,records in paths.items() if any(not item.get('ok') for item in records)}},indent=2))
# Product failures remain evidence, but omitted probes are harness failures.
required_paths = {entry['case'] for entry in results if entry.get('runner') != 'rmarkdown-pdf' and entry.get('tested') is not False and entry['case'] != 'rscript-cwd-switch'}
required_paths.update('rscript-cwd-switch' + suffix for suffix in ['-before', '-deeper', '-other-project'])
render_cases = [entry for entry in results if entry.get('runner') in ['rmarkdown', 'quarto-document', 'quarto-project']]
missing_paths = sorted(required_paths - paths.keys())
invalid_paths = [name for name, records in paths.items() if not isinstance(records, list) or not records or any('resolver' not in item or 'ok' not in item for item in records)]
missing_images = [entry['case'] + '-' + resolver for entry in render_cases if entry['returncode'] == 0 for resolver in ['unchanged_projr_relative', 'test_only_prototype'] if entry['case'] + '-' + resolver not in image_paths]
missing_html = [entry['case'] for entry in render_cases if entry['returncode'] == 0 and 'native' not in entry]
counts = {'executed_cases': len(results), 'required_path_cases': len(required_paths), 'path_evidence_files': len(paths), 'render_cases': len(render_cases), 'render_process_failures': sum(entry['returncode'] != 0 for entry in render_cases), 'image_lookup_failures': sum(not value.get('ok') for value in image_paths.values()), 'relocated_incomplete_bundles': sum(not entry.get('html_plus_stem_assets_relocated', {}).get('sentinel_bundle_complete', False) for entry in render_cases), 'missing_path_evidence': missing_paths, 'invalid_path_evidence': invalid_paths, 'missing_image_evidence': missing_images, 'successful_render_missing_html': missing_html}
summary['counts'] = counts
summary['workflow_success_means'] = 'All required diagnostic cases and evidence captured; product path/resource failures are preserved, not asserted away.'
(OUT/'summary.json').write_text(json.dumps(summary, indent=2))
print(json.dumps(counts, indent=2))
assert not (missing_paths or invalid_paths or missing_images or missing_html), 'Incomplete diagnostic evidence; see summary.json'
