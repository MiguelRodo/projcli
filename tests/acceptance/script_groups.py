"""Actual grouped document and whole-website output preservation."""
import argparse, hashlib, json, os, re, subprocess, tempfile
from pathlib import Path
parser=argparse.ArgumentParser()
parser.add_argument("binary")
parser.add_argument("--evidence",required=True)
args=parser.parse_args()
binary=str(Path(args.binary).resolve())
env={k:v for k,v in os.environ.items() if not k.startswith(("GIT_","GH_","GITHUB_","PROJR_")) and k not in ("PROJ_PROJECT_ROOT","PROJ_RUN_CONTEXT","QUARTO_PROFILE")}
def run(root):
    result=subprocess.run([binary,"--non-interactive","--project",str(root),"build"],cwd=root,env=env,capture_output=True,text=True)
    assert result.returncode==0,result.stderr[-5000:]+result.stdout[-1000:]
def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def source_bytes(root):
    return {str(path.relative_to(root)):path.read_bytes() for path in root.rglob("*") if path.is_file() and (path.suffix in (".qmd",".svg") or path.name=="_quarto.yml") and not any(part in ("_tmp","docs") for part in path.relative_to(root).parts)}
def local_links(root):
    checked=0
    for html in root.rglob("*.html"):
        for value in re.findall(r'(?:href|src)="([^"]+)"',html.read_text()):
            value=value.split("#",1)[0].split("?",1)[0]
            if not value or value.startswith(("#","/","data:","http:","https:","mailto:","javascript:")):continue
            assert (html.parent/value).exists(),f"Broken copied reference: {html.relative_to(root)} -> {value}"
            checked+=1
    return checked
evidence={}
with tempfile.TemporaryDirectory(prefix="proj-script-groups-") as temporary:
    root=Path(temporary)
    (root/"_proj.yml").write_text("directories:\n  scripts-l1: {path: source/l1}\n  scripts-l2: {path: source/l2}\n  cache: {path: _tmp}\n  docs: {path: docs, ignore: []}\nbuild:\n  git: false\n  scripts: [scripts-l1, scripts-l2]\n")
    (root/"VERSION").write_text("v1.2.3-4\n")
    (root/"DESCRIPTION").write_text("Title: Grouped teaching project\nVersion: 1.2.3.4\n")
    (root/".gitignore").write_text("manual-rule\n")
    for number in (1,2):
        group=root/f"source/l{number}/unit";group.mkdir(parents=True)
        other=3-number
        fence=chr(96)*3
        (group/"report.qmd").write_text(f'---\ntitle: Group {number}\nformat:\n  html:\n    embed-resources: false\n---\n\n[Other group](../../l{other}/unit/report.html)\n\n'+
            fence+'{r}\nplot(c(1,2,3), c('+str(number)+',2,3))\n'+fence+'\n')
        (group/"manual.html").write_text("Hand-authored page")
    before=source_bytes(root)
    run(root)
    assert source_bytes(root)==before
    pngs=[]
    for number in (1,2):
        output=root/f"docs/l{number}/unit"
        assert (output/"report.html").is_file()
        figures=list((output/"report_files").rglob("*.png"));assert figures
        pngs.extend(digest(path) for path in figures)
        assert not (root/f"source/l{number}/unit/report.html").exists()
    assert len(set(pngs))==2,"Groups must retain their own distinct figures"
    git=(root/".gitignore").read_text()
    assert "manual-rule" in git and "/source/l1/unit/report.html" in git
    assert "manual.html" not in git and "docs" not in git
    rbuild=(root/".Rbuildignore").read_text()
    assert "^source/l1$" in rbuild and "^source/l2$" in rbuild and "^R$" not in rbuild
    membership=sorted(str(path.relative_to(root/"docs")) for path in (root/"docs").rglob("*") if path.is_file())
    run(root)
    assert source_bytes(root)==before
    assert (root/".gitignore").read_text()==git and (root/".Rbuildignore").read_text()==rbuild
    assert sorted(str(path.relative_to(root/"docs")) for path in (root/"docs").rglob("*") if path.is_file())==membership
    assert sorted(digest(path) for path in (root/"docs").rglob("*.png"))==sorted(pngs)
    evidence["independent_groups"]={"repeat_build_stable":True,"htmls":2,"png_sha256":pngs,"local_references_checked":local_links(root/"docs"),"sources_preserved":True}
for explicit in (False,True):
    with tempfile.TemporaryDirectory(prefix="proj-quarto-website-") as temporary:
        root=Path(temporary);(root/"chapters").mkdir();(root/"assets").mkdir()
        config="directories:\n  cache: {path: _tmp}\n  docs: {path: docs, ignore: []}\nbuild:\n  git: false\n"
        if explicit:config+='  scripts: ["**/*.qmd", "!draft.qmd"]\n'
        (root/"_proj.yml").write_text(config)
        (root/"VERSION").write_text("v1.2.3-4\n")
        (root/"_quarto.yml").write_text("project:\n  type: website\n  output-dir: native-site\n  render: [index.qmd, chapters/about.qmd, '!draft.qmd']\n  resources: [assets/shared.svg]\nwebsite:\n  title: Teaching site\n  navbar:\n    left: [index.qmd, chapters/about.qmd]\nformat:\n  html:\n    embed-resources: false\n")
        (root/"index.qmd").write_text("---\ntitle: Home\n---\n\n[About](chapters/about.qmd)\n\n![](assets/shared.svg)\n")
        (root/"chapters/about.qmd").write_text("---\ntitle: About\n---\n\n[Home](../index.qmd)\n")
        (root/"draft.qmd").write_text("---\nengine: no-such-engine\n---\nExcluded draft\n")
        (root/"assets/shared.svg").write_text('<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>')
        (root/"native-site").mkdir();(root/"native-site/previous.html").write_text("Prior native tree")
        before=source_bytes(root)
        run(root)
        assert source_bytes(root)==before
        assert (root/"native-site/previous.html").read_text()=="Prior native tree"
        assert sorted(str(path.relative_to(root/"native-site")) for path in (root/"native-site").rglob("*") if path.is_file())==["previous.html"]
        for relative in ("index.html","chapters/about.html","assets/shared.svg"):
            assert (root/"docs"/relative).is_file(),relative
        assert not (root/"docs/draft.html").exists()
        assert (root/"docs/site_libs").is_dir()
        membership=sorted(str(path.relative_to(root/"docs")) for path in (root/"docs").rglob("*") if path.is_file())
        run(root)
        assert source_bytes(root)==before
        assert sorted(str(path.relative_to(root/"docs")) for path in (root/"docs").rglob("*") if path.is_file())==membership
        assert not (root/"docs/docs").exists() and not (root/"docs/native-site").exists()
        evidence[f"website_{'explicit' if explicit else 'native'}"]={"repeat_build_stable":True,"htmls":len(list((root/"docs").rglob("*.html"))),"local_references_checked":local_links(root/"docs"),"source_and_native_output_preserved":True,"excluded_draft_absent":True}
Path(args.evidence).write_text(json.dumps(evidence,indent=2)+"\n")
print("PASS grouped nested HTML/figures, source-safe ignore rules and complete native/explicit Quarto websites")
