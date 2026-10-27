"""Changed-source checks; subprocess arguments never pass through a shell."""
import argparse
import collections
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import tempfile


def command(args, cwd, **kwargs):
    return subprocess.run(args, cwd=cwd, check=False, capture_output=True, **kwargs)


def git_environment():
    environment = {key: value for key, value in os.environ.items()
                   if not key.upper().startswith("GIT_") and key != "SSH_AUTH_SOCK"}
    environment.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                       GIT_TERMINAL_PROMPT="0", GIT_NO_REPLACE_OBJECTS="1")
    return environment


def git(root, *args):
    result = command(["git", *args], root, env=git_environment())
    if result.returncode:
        raise RuntimeError("Git quality inspection failed")
    return result.stdout


def parse_changes(raw):
    fields = raw.split(b"\0")
    result = []
    index = 0
    while index < len(fields) and fields[index]:
        status = fields[index].decode("ascii")
        index += 1
        if not re.fullmatch(r"[ACDMRT][0-9]*", status):
            raise ValueError("Unsupported Git change status")
        old = None
        if status[0] in "RC":
            old = os.fsdecode(fields[index])
            index += 1
        new = os.fsdecode(fields[index])
        index += 1
        result.append((status[0], old, new))
    return result


def source_kind(name):
    path = PurePosixPath(name)
    if path.suffix.lower() == ".rs":
        return "rust"
    if path.suffix.lower() == ".r" or path.name == ".Rprofile":
        return "r"
    return None


def selection(root, base, head):
    for sha in [base, head]:
        if not re.fullmatch(r"[0-9a-f]{40}", sha):
            raise ValueError("Quality refs must be exact 40-character commit IDs")
    base = git(root, "merge-base", base, head).decode().strip()
    changes = parse_changes(git(root, "diff", "--no-ext-diff", "--no-textconv", "--name-status", "--find-renames", "-z",
                               "--diff-filter=ACDMRT", base, head, "--"))
    result = {"base": base, "head": head, "rust": [], "r": [], "clippy": False}
    for status, old, name in changes:
        names = [value for value in [old, name] if value is not None]
        if any(source_kind(value) == "rust" or PurePosixPath(value).name in
               ["Cargo.toml", "Cargo.lock"] for value in names):
            result["clippy"] = True
        if status == "D" or source_kind(name) is None:
            continue
        relative = PurePosixPath(name)
        if relative.is_absolute() or ".." in relative.parts or relative.parts[0] == "_reference":
            raise ValueError("Changed source escapes permitted maintained source")
        target = root.joinpath(*relative.parts)
        if target.is_symlink() or not target.is_file() or not target.resolve().is_relative_to(root):
            raise ValueError("Changed source must be an existing in-project regular file")
        result[source_kind(name)].append(name)
    for kind in ["rust", "r"]:
        result[kind] = sorted(set(result[kind]))
    return result


def refs(root, explicit_base):
    head = git(root, "rev-parse", "HEAD").decode().strip()
    event_path = os.environ.get("GITHUB_EVENT_PATH")
    event = json.loads(Path(event_path).read_text()) if event_path else {}
    if explicit_base:
        base = explicit_base
    elif "pull_request" in event:
        base = event["pull_request"]["base"]["sha"]
        if head != event["pull_request"]["head"]["sha"]:
            raise ValueError("Quality checkout must be the declared pull-request head")
    else:
        base = git(root, "rev-parse", "origin/main").decode().strip()
    return selection(root, base, head)


def formatted(root, selected, air):
    result = {}
    for kind, names in [("rust", selected["rust"]), ("r", selected["r"])]:
        for name in names:
            path = root / name
            args = ["rustfmt", "--edition", "2024", "--config-path",
                    str(root / "rustfmt.toml"), "--emit", "stdout"]
            if kind == "r":
                args = [air, "format", "--stdin-file-path", str(path)]
            process = command(args, root, input=path.read_bytes())
            if process.returncode:
                raise RuntimeError("Formatter failed for " + json.dumps(name) + ": " +
                                   process.stderr.decode(errors="replace"))
            result[name] = process.stdout
    return result


def format_check(root, selected, air, mode, evidence):
    outputs = formatted(root, selected, air)
    changed = [name for name, data in outputs.items() if data != (root / name).read_bytes()]
    with tempfile.TemporaryDirectory() as temporary:
        work = Path(temporary)
        for directory in ["old", "new"]:
            (work / directory).mkdir()
        for name in changed:
            for directory, data in [("old", (root / name).read_bytes()), ("new", outputs[name])]:
                target = work / directory / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)
        process = command(["git", "diff", "--no-ext-diff", "--no-textconv", "--no-index", "--binary", "--", "old", "new"], work, env=git_environment())
        if process.returncode not in [0, 1]:
            raise RuntimeError("Cannot prepare formatting patch")
        (evidence / "formatting.patch").write_bytes(process.stdout)
    if mode == "format":
        for name in changed:
            (root / name).write_bytes(outputs[name])
    return changed


def lint_r(root, selected, evidence):
    if not selected["r"]:
        return {"skipped": "No changed R source files", "returncode": 0}
    process = command(["Rscript", "--vanilla", str(root / "tools/lint_changed.R"),
                       *[str(root / name) for name in selected["r"]]], root)
    (evidence / "lintr.log").write_bytes(process.stdout + process.stderr)
    return {"returncode": process.returncode, "files": selected["r"]}


def diagnostics(root):
    process = command(["cargo", "clippy", "--workspace", "--all-targets", "--locked",
                       "--message-format=json"], root)
    output = []
    for line in process.stdout.splitlines():
        try:
            entry = json.loads(line)
        except ValueError:
            continue
        if entry.get("reason") != "compiler-message":
            continue
        item = entry["message"]
        if item["level"] not in ["warning", "error"]:
            continue
        spans = [span for span in item["spans"] if span["is_primary"]]
        file = spans[0]["file_name"] if spans else "(crate)"
        file_path = Path(file)
        if file_path.is_absolute() and file_path.is_relative_to(root):
            file = str(file_path.relative_to(root))
        source = "".join(part["text"] for span in spans for part in span["text"])
        source = re.sub(r"\s+", "", source).replace(",)", ")")
        output.append({"level": item["level"], "file": file.replace("\\", "/"),
                       "code": (item.get("code") or {}).get("code"),
                       "message": item["message"], "source": source})
    return process.returncode, output, process.stderr


def lint_rust(root, selected, evidence):
    if not selected["clippy"]:
        return {"skipped": "No changed Rust/Cargo inputs", "returncode": 0}
    with tempfile.TemporaryDirectory(prefix="proj-quality-base-") as temporary:
        baseline = Path(temporary) / "base"
        add = command(["git", "worktree", "add", "--detach", str(baseline), selected["base"]], root, env=git_environment())
        if add.returncode:
            raise RuntimeError("Cannot prepare the read-only baseline checkout")
        try:
            base_code, base, base_log = diagnostics(baseline)
        finally:
            command(["git", "worktree", "remove", "--force", str(baseline)], root, env=git_environment())
    head_code, head, head_log = diagnostics(root)
    (evidence / "clippy-base.json").write_text(json.dumps(base, indent=2) + "\n")
    (evidence / "clippy-head.json").write_text(json.dumps(head, indent=2) + "\n")
    (evidence / "clippy-build.log").write_bytes(base_log + head_log)
    fingerprint = lambda item: json.dumps(item, sort_keys=True)
    counts = collections.Counter(fingerprint(item) for item in base)
    blocking = []
    for item in head:
        key = fingerprint(item)
        if item["level"] == "error" or item["file"] in selected["rust"] or counts[key] == 0:
            blocking.append(item)
        counts[key] -= 1
    # Existing warnings in untouched files remain visible, never blanket-disabled.
    (evidence / "clippy-blocking.json").write_text(json.dumps(blocking, indent=2) + "\n")
    return {"returncode": int(bool(base_code or head_code or blocking)),
            "baseline_returncode": base_code, "head_returncode": head_code,
            "blocking": len(blocking), "head_diagnostics": len(head)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["detect", "check", "format"])
    parser.add_argument("--base", default=os.environ.get("QUALITY_BASE") or None)
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--air", default="air")
    args = parser.parse_args()
    root = Path(git(Path.cwd(), "rev-parse", "--show-toplevel").decode().strip()).resolve()
    args.evidence.mkdir(parents=True, exist_ok=True)
    selected = refs(root, args.base)
    (args.evidence / "changed-files.json").write_text(json.dumps(selected, indent=2) + "\n")
    print(json.dumps(selected))
    if args.mode == "detect":
        return
    changed = format_check(root, selected, args.air, args.mode, args.evidence)
    r_result = lint_r(root, selected, args.evidence)
    rust_result = lint_rust(root, selected, args.evidence)
    result = {"mode": args.mode, "formatted_files": changed, "r": r_result, "rust": rust_result}
    (args.evidence / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))
    raise SystemExit(int(bool((changed and args.mode == "check") or
                              r_result["returncode"] or rust_result["returncode"])))


if __name__ == "__main__":
    main()
