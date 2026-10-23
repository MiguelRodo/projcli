"""Remote-only real renderer/Git recovery acceptance; no M2 record format implied."""
from __future__ import annotations
import argparse, csv, hashlib, json, os, shutil, subprocess, tempfile
from pathlib import Path


def files(root):
    return {str(p.relative_to(root)): p.read_bytes() for p in root.rglob('*') if p.is_file()}


FAILED_STEMS = ('01-failed-html', '02-failed-pdf', '03-break')
FAILED_SOURCES = {stem + '.qmd' for stem in FAILED_STEMS}


def failed_artifact(path, allow_root_source=False):
    """Detect failed report files or companion trees; only exact root inputs may pass."""
    return not (allow_root_source and path in FAILED_SOURCES) and any(
        part.startswith(FAILED_STEMS) for part in Path(path).parts)


def assert_retained_scratch(path, expected):
    """An empty retained directory must still exist; compare all retained bytes."""
    assert path.is_dir(), 'retained failed scratch directory disappeared'
    assert files(path) == expected, 'retry changed retained failed staging'


def run_case(binary, workspace, artifacts, project):
    name = 'quarto-project' if project else 'individual-documents'
    root = workspace / name / 'project with spaces'
    root.mkdir(parents=True)
    env = os.environ.copy()
    for key in ['PROJ_PROJECT_ROOT', 'PROJ_RUN_CONTEXT', 'PROJR_PROFILE', 'QUARTO_PROFILE',
                'PROJR_CLEAR_OUTPUT', 'GITHUB_PAT', 'GH_TOKEN', 'GITHUB_TOKEN',
                'GIT_ASKPASS', 'SSH_ASKPASS', 'SSH_AUTH_SOCK', 'GIT_SSH', 'GIT_SSH_COMMAND']:
        env.pop(key, None)
    empty_config = workspace / (name + '-empty-git-config')
    empty_config.write_text('')
    env.update(GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_SYSTEM=str(empty_config),
               GIT_CONFIG_GLOBAL=str(empty_config), GIT_TERMINAL_PROMPT='0')

    def git(*args):
        result = subprocess.run(['git', *args], cwd=root, env=env, capture_output=True)
        assert result.returncode == 0, result.stderr.decode(errors='replace')
        return result.stdout

    def build():
        result = subprocess.run([str(binary), '--machine', '--non-interactive',
                                 '--project', str(root), 'build', '--description', name],
                                cwd=root, env=env, capture_output=True, text=True)
        envelope = json.loads(result.stdout)
        assert envelope['protocol_version'] == 1 and envelope['ok'] == (result.returncode == 0)
        return result, envelope

    config = ('directories:\n  cache: {path: cache}\n  docs: {path: published, ignore: []}\n'
              'build:\n  git: {commit: true, push: false}\n'
              '  scripts: [01-failed-html.qmd, 02-failed-pdf.qmd, 03-break.qmd]\n'
              '  restrictions:\n    not_behind: false\n')
    (root / '_proj.yml').write_text(config)
    (root / 'VERSION').write_bytes(b'v1.2.3-4\n')
    # Ignore disposable cache/native state, never report names or extensions.
    (root / '.gitignore').write_text('cache/\n.quarto/\n')
    if project:
        (root / '_quarto.yml').write_text('project:\n  type: default\n')
    for filename, format_name, text in [
            ('01-failed-html.qmd', 'html', 'FAILED-RUN-HTML-ONLY'),
            ('02-failed-pdf.qmd', 'typst', 'FAILED-RUN-PDF-ONLY'),
            ('04-kept.qmd', 'html', 'SUCCESSFUL-SMALLER-REPORT')]:
        (root / filename).write_text(f'---\ntitle: {text}\nformat: {format_name}\n---\n\n{text}\n')
    # Prove both earlier artifacts exist before the last renderer fails.
    stage = '"."'  # Quarto moves project outputs only after all renders succeed.
    fence = chr(96) * 3
    (root / '03-break.qmd').write_text(
        f'---\nformat: html\n---\n\n{fence}{{r}}\nstage <- {stage}\n'
        'html <- file.path(stage, "01-failed-html.html")\n'
        'pdf <- file.path(stage, "02-failed-pdf.pdf")\n'
        'stopifnot(file.exists(html), file.exists(pdf))\n'
        'stopifnot(grepl("FAILED-RUN-HTML-ONLY", paste(readLines(html, warn=FALSE), collapse="")))\n'
        'stopifnot(identical(readBin(pdf, "raw", n=5), charToRaw("%PDF-")))\n'
        'dir.create("cache", showWarnings=FALSE)\n'
        'writeLines("real HTML and PDF existed before failure", "cache/rendered-before-failure")\n'
        f'stop("DELIBERATE-MIXED-DOCUMENT-FAILURE")\n{fence}\n')
    (root / 'published').mkdir()
    for filename, data in [('published/previous.html', b'previous released HTML'),
                           ('published/previous.pdf', b'%PDF-previous released PDF'),
                           ('previous.html', b'earlier valid native HTML'),
                           ('previous.pdf', b'%PDF-earlier valid native PDF')]:
        (root / filename).write_bytes(data)
    (root / 'cache').mkdir()
    (root / 'cache/previous-scratch').write_bytes(b'earlier inspectable cache bytes')
    preserved = files(root)
    git('init', '-b', 'fixture-analysis')
    git('config', 'user.name', 'Fixture')
    git('config', 'user.email', 'fixture@example.invalid')
    git('config', 'commit.gpgsign', 'false')
    git('add', '-A')
    git('commit', '-m', 'previous valid project')
    initial = git('rev-parse', 'HEAD').decode().strip()

    failed, envelope = build()
    assert failed.returncode != 0 and envelope['error']['code'] == 'command_failed'
    assert 'DELIBERATE-MIXED-DOCUMENT-FAILURE' in failed.stderr, failed.stderr[-8000:]
    assert (root / 'cache/rendered-before-failure').read_text().strip() == 'real HTML and PDF existed before failure'
    assert git('rev-parse', 'HEAD').decode().strip() == initial
    status = git('status', '--porcelain')
    assert status == b'', 'failed renderer left commit-eligible residue: ' + status.decode()
    for path, data in preserved.items():
        assert (root / path).read_bytes() == data, path
    for filename in ['01-failed-html.html', '02-failed-pdf.pdf', '03-break.html']:
        assert not (root / filename).exists(), filename

    scratch = list((root / 'cache/projr/v1.2.4').glob('.proj-render-*'))
    assert len(scratch) == 1 and scratch[0].is_dir()
    assert 'Renderer scratch retained for inspection' in failed.stderr
    retained = files(scratch[0])
    if project:
        assert b'FAILED-RUN-HTML-ONLY' in retained['01-failed-html.html']
        assert retained['02-failed-pdf.pdf'].startswith(b'%PDF-')
    destination = artifacts / name
    destination.mkdir(parents=True)
    shutil.copytree(scratch[0], destination / 'failed-scratch')

    # Stage only the intentional edit, never sweep residue into this commit.
    (root / '_proj.yml').write_text(config.replace(
        '[01-failed-html.qmd, 02-failed-pdf.qmd, 03-break.qmd]', '[04-kept.qmd]'))
    git('add', '--', '_proj.yml')
    git('commit', '-m', 'select smaller successful build')
    before_success = git('rev-parse', 'HEAD').decode().strip()
    successful, _ = build()
    assert successful.returncode == 0, successful.stderr[-8000:]
    committed = git('rev-parse', 'HEAD').decode().strip()
    assert committed != before_success
    assert git('rev-list', '--count', f'{before_success}..HEAD').strip() == b'1'
    paths = git('ls-tree', '-r', '-z', '--name-only', 'HEAD').decode().split('\0')[:-1]
    for path in paths:
        assert not failed_artifact(path, allow_root_source=True), path
        assert not path.startswith('cache/'), path
    assert 'published/04-kept.html' in paths
    assert not (root / '04-kept.html').exists(), 'new native scratch entered working tree'
    assert_retained_scratch(scratch[0], retained)
    for path in ['previous.html', 'previous.pdf', 'cache/previous-scratch']:
        assert (root / path).read_bytes() == preserved[path], path
    for path in ['previous.html', 'previous.pdf']:
        assert git('show', f'HEAD:{path}') == preserved[path], path
    final = files(root / 'published')
    cached = files(root / 'cache/projr/v1.2.4/docs')
    assert final == cached
    assert '04-kept.html' in final and b'SUCCESSFUL-SMALLER-REPORT' in final['04-kept.html']
    assert not any(failed_artifact(path) for path in final)
    assert not any(path in final for path in ['previous.html', 'previous.pdf'])
    tracked = {path.removeprefix('published/'): git('show', f'HEAD:{path}')
               for path in paths if path.startswith('published/')}
    assert tracked == final, 'committed report bytes differ from final/cache'
    assert git('show', 'HEAD:VERSION') == b'Version: v1.2.4'
    assert (root / 'VERSION').read_bytes() == b'Version: v1.2.4.1'
    status = git('status', '--porcelain').decode().splitlines()
    receipts = list((root / '.proj/log').glob('*.git.json'))
    assert len(receipts) == 1, receipts
    receipt_path = receipts[0].relative_to(root).as_posix()
    receipt = json.loads(receipts[0].read_text())
    assert receipt['status'] == 'completed'
    assert receipt['git']['commit'] == 'completed'
    assert receipt['git']['head'] == committed
    assert receipt['version_after'] == 'v1.2.4.1'
    snapshot_path = f".proj/log/{receipt['attempt_id']}.json"
    assert git('show', f'HEAD:{snapshot_path}') == (root / snapshot_path).read_bytes()
    final_csv = root / '.proj/log' / receipt['final_outputs']['rows']['file']
    assert final_csv.is_file()
    assert len(final_csv.read_bytes()) == receipt['final_outputs']['rows']['bytes']
    final_rows = list(csv.DictReader(final_csv.open()))
    assert final_rows and all(row['version'] == 'v1.2.4' for row in final_rows)
    assert sorted(status) == sorted([' M VERSION', f'?? {receipt_path}', f'?? {final_csv.relative_to(root).as_posix()}']), status
    shutil.copytree(root / 'published', destination / 'final')
    for path, data in tracked.items():
        target = destination / 'committed' / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    (destination / 'committed-paths.json').write_text(json.dumps(paths, indent=2) + '\n')
    return {'case': name, 'failed_exit': failed.returncode, 'success_exit': successful.returncode,
            'initial_commit': initial, 'selection_commit': before_success, 'release_commit': committed,
            'earlier_html_and_pdf_rendered_before_deliberate_failure': True,
            'failed_build_preserved_previous_release_and_git_state': True,
            'retained_scratch_files': {p: hashlib.sha256(data).hexdigest() for p, data in retained.items()},
            'retained_scratch_unchanged_after_retry': True,
            'previous_native_documents_preserved': True,
            'previous_final_documents_replaced_only_on_success': True,
            'committed_final_and_cache_sha256': {p: hashlib.sha256(data).hexdigest() for p, data in final.items()},
            'failed_artifacts_absent_from_release_commit': True,
            'committed_release_version': '1.2.4', 'working_next_development_version': '1.2.4.1'}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('binary', type=Path)
    parser.add_argument('--evidence', type=Path, required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    evidence = {'complete': False, 'cases': [], 'limitations': [
        'bounded managed recovery; no arbitrary script sandbox or transaction claim',
        'Quarto individual/project fixtures; no Bookdown or existing R-package parity claim',
        'no M2 record identity, schema, sealing or remote-storage policy selected']}
    try:
        with tempfile.TemporaryDirectory(prefix='projcli-mixed-render-retry-') as directory:
            for project in [False, True]:
                evidence['cases'].append(run_case(binary, Path(directory), args.artifacts, project))
        evidence['complete'] = True
    except Exception as error:
        evidence['error'] = str(error)
        raise
    finally:
        args.evidence.write_text(json.dumps(evidence, indent=2) + '\n')
        print(json.dumps(evidence))


if __name__ == '__main__':
    main()
