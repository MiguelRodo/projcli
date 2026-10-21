"""Actual CLI acceptance in the pinned remote image; never run on the user's PC.

The small R fixture adapter invokes the Rust CLI. It is not an existing projr
helper or a proposed production R frontend. Individual HTML leaves embedding
unspecified to exercise defaults; project/explicit-false cases below verify policy.
Path target/representation flags are omitted to exercise safe/auto defaults.
"""
from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tempfile
import zlib


def sentinel_png() -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', binascii.crc32(kind + data))
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 2, 2, 8, 2, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(b'\0\xff\0\0\0\xff\0\0\0\0\xff\xff\xff\0')) + chunk(b'IEND', b''))


R_ADAPTER = r'''cli_path <- function(label, ...) {
  args <- c("--non-interactive", "path", "get", "--label", label,
            ...)
  result <- system2(Sys.getenv("PROJCLI_ACCEPT_BINARY"), vapply(args, shQuote, ""), stdout=TRUE)
  if (!is.null(attr(result, "status"))) stop("CLI path query failed")
  stopifnot(length(result) == 1L)
  result
}
input <- cli_path("raw-data", "data.csv")
stopifnot(read.csv(input)$value[[1]] == 42)
source(cli_path("raw-data", "helper.R"))
stopifnot(helper_value == 42)
'''


def run_case(binary: Path, container: Path, name: str, mode: str, failure: str | None, nested: bool) -> dict:
    root = container / name / 'project with spaces é'
    caller = container / name / 'caller'
    docs = root / ('analysis' if nested else '.')
    for directory in [root / '_raw/images', root / 'cache', root / 'released', root / 'published', root / '.git', docs, caller]:
        directory.mkdir(parents=True, exist_ok=True)
    image = sentinel_png()
    (root / '_raw/images/sentinel.png').write_bytes(image)
    (root / '_raw/data.csv').write_text('value\n42\n')
    (root / '_raw/helper.R').write_text('helper_value <- 42\n')
    (root / 'VERSION').write_bytes(b'v1.2.3-4\n')
    (root / '.git/sentinel').write_bytes(b'no git operation allowed')
    (root / 'released/previous.txt').write_bytes(b'previous released output')
    (root / 'published/previous.html').write_bytes(b'previous rendered output')
    python_script = '''import json,os,subprocess
from pathlib import Path
def cli_path(label,*parts):
 args=[os.environ['PROJCLI_ACCEPT_BINARY'],'--machine','path','get','--label',label,*parts]
 result=json.loads(subprocess.check_output(args,text=True))
 return Path(result['result']['path'])
assert cli_path('raw-data','data.csv').read_text()=='value\\n42\\n'
os.chdir(Path(os.environ['PROJ_PROJECT_ROOT'])/'_raw')
assert cli_path('raw-data','data.csv').read_text()=='value\\n42\\n'
target=cli_path('output','python.txt');target.parent.mkdir(parents=True,exist_ok=True);target.write_text('python input/helper paths passed')
'''
    (docs / 'run.py').write_text(python_script + ('raise SystemExit(17)\n' if failure == 'python' else ''))
    (docs / 'run.R').write_text(R_ADAPTER + r'''
setwd(tempdir())
stopifnot(read.csv(cli_path("raw-data", "data.csv"))$value[[1]] == 42)
out <- cli_path("output", "r.txt")
dir.create(dirname(out), recursive=TRUE, showWarnings=FALSE)
writeLines("R input/helper paths passed", out)
''' + ('stop("deliberate R acceptance failure")\n' if failure == 'r' else ''))
    literal = '../_raw/images/sentinel.png' if nested else '_raw/images/sentinel.png'
    chunk = R_ADAPTER + r'''
image <- cli_path("raw-data", "images", "sentinel.png")
stopifnot(grepl("^/", image)) # managed renderer auto is absolute on this Linux job.
knitr::include_graphics(image)
'''
    (docs / "qreport's é.qmd").write_text('---\ntitle: CLI Quarto acceptance\nformat: html\nexecute:\n  echo: false\n---\n\n```{r}\n' + chunk + '\n```\n\n![literal sentinel](' + literal + ')\n')
    (docs / "rreport's é.Rmd").write_text('---\ntitle: CLI R Markdown acceptance\noutput: html_document\n---\n\n```{r, echo=FALSE}\n' + chunk + '\n```\n\n![literal sentinel](' + literal + ')\n')
    prefix = 'analysis/' if nested else ''
    scripts = [prefix + 'run.py', prefix + 'run.R', prefix + "qreport's é.qmd", prefix + "rreport's é.Rmd"]
    if failure == 'r':
        scripts[0:2] = [prefix + 'run.py', prefix + 'run.R']
    config = ('directories:\n  raw-data: {path: _raw}\n  cache: {path: cache}\n'
              '  output: {path: released}\n  docs: {path: published}\n'
              'settings:\n  output-clear-stage: none\nbuild:\n  git: false\n  scripts: ' + json.dumps(scripts, ensure_ascii=False) + '\n')
    (root / '_proj.yml').write_text(config)
    for stem in ["qreport's é", "rreport's é"]:
        (docs / (stem + '.html')).write_bytes(b'preexisting native html')
        assets = docs / (stem + '_files')
        assets.mkdir()
        (assets / 'old.bin').write_bytes(b'preexisting native asset')
    original = {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in root.rglob('*') if path.is_file()}
    env = os.environ.copy()
    for key in ['PROJ_PROJECT_ROOT', 'PROJ_RUN_CONTEXT', 'PROJR_PROFILE', 'QUARTO_PROFILE', 'PROJR_CLEAR_OUTPUT', 'GITHUB_PAT', 'GH_TOKEN', 'GITHUB_TOKEN']:
        env.pop(key, None)
    env['PROJCLI_ACCEPT_BINARY'] = str(binary)
    result = subprocess.run([str(binary), '--machine', '--non-interactive', mode, '--project', str(root)], cwd=caller, env=env, capture_output=True, text=True)
    envelope = json.loads(result.stdout)
    assert bool(result.returncode == 0) == (failure is None), result.stderr[-6000:]
    assert envelope['ok'] == (failure is None)
    expected_key = 'v1.2.4' if mode == 'build' else 'v1.2.3-4'
    cache = root / 'cache/projr' / expected_key
    assert (cache / 'output/python.txt').is_file()
    if failure != 'python':
        assert (cache / 'output/r.txt').is_file()
    assert (root / '.git/sentinel').read_bytes() == b'no git operation allowed'
    for relative, digest in original.items():
        if mode == 'build' and failure is None and relative in ['VERSION', 'published/previous.html']:
            continue
        assert hashlib.sha256((root / relative).read_bytes()).hexdigest() == digest, relative
    if mode == 'build-dev' or failure:
        assert not (root / 'released/python.txt').exists()
        assert (root / 'VERSION').read_bytes() == b'v1.2.3-4\n'
    else:
        assert (root / 'released/python.txt').read_text() == (cache / 'output/python.txt').read_text()
        assert (root / 'released/r.txt').is_file()
        assert b'1.2.4' in (root / 'VERSION').read_bytes()
    checks = []
    if failure is None:
        bundle = container / name / 'relocated'
        shutil.copytree(cache / 'docs', bundle)
        if mode == 'build':
            for filename in ["qreport's é.html", "rreport's é.html"]:
                assert (root / 'published' / filename).read_bytes() == (cache / 'docs' / filename).read_bytes()
        root.rename(root.with_name('source-unavailable'))
        for filename in ["qreport's é.html", "rreport's é.html"]:
            content = (bundle / filename).read_text()
            payloads = re.findall(r'data:image/png;base64,([^\s"\'<>]+)', content)
            count = sum(base64.b64decode(payload) == image for payload in payloads)
            assert count >= 2, f'{filename}: code/literal sentinel embedding count {count}'
            checks.append({'file': filename, 'exact_sentinel_payloads': count, 'source_hidden': True})
    return {'case': name, 'mode': mode, 'failure': failure, 'exit_code': result.returncode,
            'protocol_version': envelope['protocol_version'], 'cache_key': expected_key,
            'original_source_hashes_preserved': True, 'html_checks': checks}


def notebook_preservation(binary: Path, container: Path) -> dict:
    root = container / 'notebook-preservation'
    (root / 'analysis/report.nb_files').mkdir(parents=True)
    (root / 'analysis/report_files').mkdir()
    (root / 'analysis/report.nb_files/sentinel').write_bytes(b'preexisting notebook assets')
    (root / 'analysis/report_files/sentinel').write_bytes(b'preexisting ordinary assets')
    (root / 'analysis/report.nb.html').write_bytes(b'preexisting notebook html')
    (root / 'analysis/report.html').write_bytes(b'preexisting intermediate html')
    (root / 'VERSION').write_bytes(b'v1.2.3-4\n')
    (root / '_proj.yml').write_text('directories:\n  cache: {path: cache}\n  docs: {path: published}\nbuild:\n  scripts: [analysis/report.Rmd]\n  git: false\n')
    (root / 'analysis/report.Rmd').write_text('---\noutput: html_notebook\n---\n\n```{r}\n1 + 1\n```\n')
    result = subprocess.run([str(binary), 'build-dev', '--non-interactive', '--project', str(root)], cwd=root, capture_output=True, text=True)
    assert result.returncode == 0, result.stdout + result.stderr
    assert (root / 'analysis/report.nb_files/sentinel').read_bytes() == b'preexisting notebook assets'
    assert (root / 'analysis/report_files/sentinel').read_bytes() == b'preexisting ordinary assets'
    assert (root / 'analysis/report.nb.html').read_bytes() == b'preexisting notebook html'
    assert (root / 'analysis/report.html').read_bytes() == b'preexisting intermediate html'
    cached = root / 'cache/projr/v1.2.3-4/docs/report.nb.html'
    assert cached.is_file() and cached.stat().st_size > 0
    return {'case': 'real-html-notebook-preservation', 'exit_code': result.returncode,
            'native_notebook_intermediate_and_ordinary_asset_sentinels_preserved': True, 'fresh_cached_notebook': True}


def embedding_policy_cases(binary: Path, container: Path) -> list[dict]:
    cases = []
    for name in ['individual-qmd-false', 'individual-rmd-false', 'project-default',
                 'project-format-false', 'project-document-false',
                 'project-directory-false', 'project-profile-false', 'project-metadata-file-false',
                 'project-docx', 'project-format-alias-false', 'project-document-alias-false',
                 'project-directory-alias-false', 'project-profile-alias-false',
                 'project-metadata-file-alias-false', 'project-mixed-alias', 'project-inherited-docx-html', 'project-late-alias-false', 'project-late-singular-alias-false',
                 'project-late-local-change', 'project-late-local-create']:
        root = container / name
        (root / 'analysis').mkdir(parents=True)
        image = sentinel_png()
        (root / 'analysis/sentinel.png').write_bytes(image)
        (root / 'VERSION').write_bytes(b'v1.2.3-4\n')
        is_rmd = name == 'individual-rmd-false'
        extension = 'Rmd' if is_rmd else 'qmd'
        document = root / f'analysis/report.{extension}'
        metadata = 'output:\n  html_document:\n    self_contained: false\n' if is_rmd else 'format: html\n'
        if name in ['individual-qmd-false', 'project-document-false']:
            metadata = 'format:\n  html:\n    embed-resources: false\n'
        if name in ['project-document-alias-false', 'project-mixed-alias']:
            metadata = 'format:\n  html:\n    self-contained: false\n'
        if name == 'project-inherited-docx-html':
            metadata = ''
        if name == 'project-docx':
            metadata = 'format: docx\n'
        document.write_text('---\ntitle: Embedding policy\n' + metadata + '---\n\n![sentinel](sentinel.png)\n')
        (root / '_proj.yml').write_text('directories:\n  cache: {path: cache}\n  docs: {path: published}\nbuild:\n  git: false\n  scripts: [analysis/report.' + extension + ']\n')
        if name == 'project-mixed-alias':
            (root / 'analysis/other.qmd').write_text('---\nformat: html\n---\n\n![sentinel](sentinel.png)\n')
            configured = (root / '_proj.yml').read_text().replace('scripts: [analysis/report.qmd]', 'scripts: [analysis/report.qmd, analysis/other.qmd]')
            (root / '_proj.yml').write_text(configured)
        env = os.environ.copy()
        for key in ['PROJ_PROJECT_ROOT', 'PROJ_RUN_CONTEXT', 'PROJR_PROFILE', 'QUARTO_PROFILE', 'PROJR_CLEAR_OUTPUT']:
            env.pop(key, None)
        if name.startswith('project-'):
            project = 'project:\n  type: default\nformat: html\n'
            if name == 'project-format-alias-false':
                project = 'project:\n  type: default\nformat:\n  html:\n    self-contained: false\n'
            if name == 'project-format-false':
                project = 'project:\n  type: default\nformat:\n  html:\n    embed-resources: false\n'
            if name == 'project-inherited-docx-html':
                project = 'project:\n  type: default\nformat:\n  docx:\n    toc: true\n  html:\n    toc: false\n'
            if name == 'project-docx':
                project = 'project:\n  type: default\nformat: docx\n'
            if name == 'project-metadata-file-false':
                project += 'metadata-files: [inherited.yml]\n'
                (root / 'inherited.yml').write_text('embed-resources: false\n')
            if name == 'project-metadata-file-alias-false':
                project += 'metadata-files: [inherited.yml]\n'
                (root / 'inherited.yml').write_text('self-contained: false\n')
            (root / '_quarto.yml').write_text(project)
            if name == 'project-directory-alias-false':
                (root / 'analysis/_metadata.yml').write_text('self-contained: false\n')
            if name == 'project-profile-alias-false':
                (root / '_quarto-linked.yml').write_text('self-contained: false\n')
                env['QUARTO_PROFILE'] = 'linked'
            if name == 'project-directory-false':
                (root / 'analysis/_metadata.yml').write_text('embed-resources: false\n')
            if name == 'project-profile-false':
                (root / '_quarto-linked.yml').write_text('embed-resources: false\n')
                env['QUARTO_PROFILE'] = 'linked'
        late_metadata = name.startswith('project-late-')
        if late_metadata:
            dependency = 'analysis/_metadata.yml'
            if name == 'project-late-singular-alias-false':
                dependency = 'inherited.yml'
                with (root / '_quarto.yml').open('a') as stream:
                    stream.write('metadata-file: inherited.yml\n')
            if name in ['project-late-local-change', 'project-late-local-create']:
                dependency = '_quarto.yml.local'
            if name != 'project-late-local-create':
                (root / dependency).write_text('toc: false\n')
            (root / 'change.py').write_text("from pathlib import Path\nPath(" + repr(dependency) + ").write_text('self-contained: false\\n')\n")
            configured = (root / '_proj.yml').read_text().replace('scripts: [analysis/report.qmd]', 'scripts: [change.py, analysis/report.qmd]')
            (root / '_proj.yml').write_text(configured)
        if name == 'project-inherited-docx-html':
            (root / 'inspect.py').write_text("import subprocess\nfrom pathlib import Path\nPath('cache').mkdir(exist_ok=True)\nPath('cache/prepared-formats.json').write_text(subprocess.check_output(['quarto','inspect','analysis/report.qmd'],text=True))\n")
            configured = (root / '_proj.yml').read_text().replace('scripts: [analysis/report.qmd]', 'scripts: [inspect.py, analysis/report.qmd]')
            (root / '_proj.yml').write_text(configured)
            native = container / (name + '-native')
            shutil.copytree(root, native)
            reference = subprocess.run(['quarto','inspect','analysis/report.qmd'],cwd=native,env=env,capture_output=True,text=True)
            assert reference.returncode == 0, reference.stderr
            native_formats = json.loads(reference.stdout)['formats']
            assert list(native_formats) == ['docx','html']
            baseline = subprocess.run(['quarto','render'],cwd=native,env=env,capture_output=True,text=True)
            assert baseline.returncode == 0, baseline.stderr
            native_outputs = sorted(p.suffix for p in native.rglob('report.*') if p.suffix in ['.docx','.html'])
            assert '.docx' in native_outputs
        originals = {p.relative_to(root): p.read_bytes() for p in root.rglob('*') if p.is_file()}
        result = subprocess.run([str(binary), '--machine', '--non-interactive', 'build-dev', '--project', str(root)],
                                cwd=root, env=env, capture_output=True, text=True)
        assert (result.returncode == 0) == (not late_metadata), name + ': ' + result.stdout + result.stderr[-6000:]
        if late_metadata:
            assert not json.loads(result.stdout)['ok']
            assert 'inherited metadata changed after embedding preparation' in result.stderr
        for path, data in originals.items():
            assert (root / path).read_bytes() == data, str(path)
        if late_metadata:
            if name == 'project-late-local-create':
                assert not (root / dependency).exists()
            assert not list((root / 'cache/projr/v1.2.3-4/docs').rglob('report.html'))
            cases.append({'case':name,'exit_code':result.returncode,'changed_inherited_false_rejected_before_render':True,
                          'source_and_internal_metadata_bytes_preserved':True})
            continue
        if name == 'project-inherited-docx-html':
            prepared_formats = json.loads((root / 'cache/prepared-formats.json').read_text())['formats']
            assert list(prepared_formats) == list(native_formats), 'native format choices/default order changed'
            assert all(prepared_formats[key]['pandoc'].get('toc') == native_formats[key]['pandoc'].get('toc') for key in native_formats)
            produced = root / 'cache/projr/v1.2.3-4/docs'
            assert sorted(p.suffix for p in produced.rglob('report.*') if p.suffix in ['.docx','.html']) == native_outputs
            html = list(produced.rglob('report.html'))
            if html:
                assert base64.b64encode(image).decode() in html[0].read_text()
            cases.append({'case':name,'native_formats':list(native_formats),'native_outputs':native_outputs,
                          'format_choices_order_and_options_preserved':True,'source_bytes_preserved':True})
            continue
        if name == 'project-docx':
            assert list((root / 'cache/projr/v1.2.3-4/docs').rglob('report.docx'))
            cases.append({'case': name, 'native_nonhtml_success': True, 'source_bytes_preserved': True})
            continue
        if name == 'project-mixed-alias':
            other = list((root / 'cache/projr/v1.2.3-4/docs').rglob('other.html'))
            assert len(other) == 1 and base64.b64encode(image).decode() in other[0].read_text()
        files = list((root / 'cache/projr/v1.2.3-4/docs').rglob('report.html'))
        assert len(files) == 1, (name, files)
        content = files[0].read_text()
        payloads = re.findall(r'data:image/png;base64,([^\s"\'<>]+)', content)
        count = sum(base64.b64decode(payload) == image for payload in payloads)
        embedded = name == 'project-default'
        assert (count >= 1) == embedded, (name, count)
        assert embedded or 'sentinel.png' in content, name
        if embedded:
            relocated = container / (name + '-relocated.html')
            shutil.copyfile(files[0], relocated)
            root.rename(container / (name + '-source-hidden'))
            assert base64.b64encode(image).decode() in relocated.read_text()
        cases.append({'case': name, 'expected_embedded': embedded, 'exact_sentinel_payloads': count,
                      'source_bytes_preserved': True})
    return cases


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument('binary', type=Path)
    parser.add_argument('--evidence', type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    evidence: dict = {'cases': [], 'versions': {}, 'limitations': ['existing R package untouched; fixture adapter calls CLI', 'RStudio GUI and other OS renderers not exercised', 'individual and Quarto project embedding defaults/false tested; native Bookdown site policy unchanged', 'nonembedded portability and M2 provenance not claimed']}
    try:
        for executable, flags in [('Rscript', ['--version']), ('quarto', ['--version']), ('python3', ['--version'])]:
            probe = subprocess.run([executable, *flags], capture_output=True, text=True, check=True)
            evidence['versions'][executable] = (probe.stdout + probe.stderr).strip()
        versions = subprocess.run(['Rscript', '--vanilla', '-e', 'cat(as.character(utils::packageVersion("rmarkdown")),as.character(utils::packageVersion("knitr")),sep="\\n")'], capture_output=True, text=True, check=True)
        evidence['versions']['rmarkdown_knitr'] = versions.stdout.strip()
        with tempfile.TemporaryDirectory(prefix='projcli-actual-acceptance-') as directory:
            for name, mode, failure, nested in [('preview-root', 'build-dev', None, False), ('preview-nested', 'build-dev', None, True), ('release-nested', 'build', None, True), ('failing-r', 'build-dev', 'r', True), ('failing-python', 'build-dev', 'python', True)]:
                evidence['cases'].append(run_case(binary, Path(directory), name, mode, failure, nested))
            evidence['cases'].append(notebook_preservation(binary, Path(directory)))
            evidence['cases'].extend(embedding_policy_cases(binary, Path(directory)))
        evidence['complete'] = True
    except Exception as error:
        evidence['complete'] = False
        evidence['error'] = str(error)
        raise
    finally:
        args.evidence.write_text(json.dumps(evidence, indent=2, ensure_ascii=False) + '\n')
        print(json.dumps(evidence, ensure_ascii=False))


if __name__ == '__main__':
    main()
