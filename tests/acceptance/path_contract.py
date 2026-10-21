"""Actual CLI acceptance in the pinned remote image; never run on the user's PC.

The small R fixture adapter invokes the Rust CLI. It is not an existing projr
helper or a proposed production R frontend. HTML embedding is explicit here;
fallback/inheritance acceptance is added with that separately reviewed slice.
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
            "--target", "safe", "--format", "auto", ...)
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
 args=[os.environ['PROJCLI_ACCEPT_BINARY'],'--machine','path','get','--label',label,'--target','safe','--format','auto',*parts]
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
    (docs / "qreport's é.qmd").write_text('---\ntitle: CLI Quarto acceptance\nformat:\n  html:\n    embed-resources: true\nexecute:\n  echo: false\n---\n\n```{r}\n' + chunk + '\n```\n\n![literal sentinel](' + literal + ')\n')
    (docs / "rreport's é.Rmd").write_text('---\ntitle: CLI R Markdown acceptance\noutput:\n  html_document:\n    self_contained: true\n---\n\n```{r, echo=FALSE}\n' + chunk + '\n```\n\n![literal sentinel](' + literal + ')\n')
    prefix = 'analysis/' if nested else ''
    scripts = [prefix + 'run.py', prefix + 'run.R', prefix + "qreport's é.qmd", prefix + "rreport's é.Rmd"]
    if failure == 'r':
        scripts[0:2] = [prefix + 'run.py', prefix + 'run.R']
    config = ('directories:\n  raw-data: {path: _raw}\n  cache: {path: cache}\n'
              '  output: {path: released}\n  docs: {path: published}\n'
              'settings:\n  output-clear-stage: none\nbuild:\n  git: false\n  scripts: ' + json.dumps(scripts, ensure_ascii=False) + '\n')
    (root / '_proj.yml').write_text(config)
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


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument('binary', type=Path)
    parser.add_argument('--evidence', type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    evidence: dict = {'cases': [], 'versions': {}, 'limitations': ['existing R package untouched; fixture adapter calls CLI', 'RStudio GUI and other OS renderers not exercised', 'HTML embedding explicitly configured; unspecified fallback separate', 'nonembedded portability and M2 provenance not claimed']}
    try:
        for executable, flags in [('Rscript', ['--version']), ('quarto', ['--version']), ('python3', ['--version'])]:
            probe = subprocess.run([executable, *flags], capture_output=True, text=True, check=True)
            evidence['versions'][executable] = (probe.stdout + probe.stderr).strip()
        with tempfile.TemporaryDirectory(prefix='projcli-actual-acceptance-') as directory:
            for name, mode, failure, nested in [('preview-root', 'build-dev', None, False), ('preview-nested', 'build-dev', None, True), ('release-nested', 'build', None, True), ('failing-r', 'build-dev', 'r', True), ('failing-python', 'build-dev', 'python', True)]:
                evidence['cases'].append(run_case(binary, Path(directory), name, mode, failure, nested))
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
