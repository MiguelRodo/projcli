"""Only dedicated synthetic prereleases; no existing store may be overwritten."""
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import subprocess
import tempfile
import urllib.error
import urllib.request

REPO = "MiguelRodo/projcli"
TAGS = {kind: "projcli-" + kind + "-store-acceptance-20261027" for kind in ["cas", "xet"]}


def api(path, payload=None):
    request = urllib.request.Request(
        "https://api.github.com/repos/" + REPO + path,
        data=None if payload is None else json.dumps(payload).encode(),
        headers={"Authorization": "Bearer " + os.environ["GH_TOKEN"],
                 "Accept": "application/vnd.github+json", "Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.load(response)


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(root, *args):
    env = {k: v for k, v in os.environ.items() if not k.upper().startswith("GIT_")}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT="0")
    subprocess.run(["git", *args], cwd=root, env=env, check=True, capture_output=True)


def inventory(root):
    return {str(p.relative_to(root)): sha(p) for p in root.rglob("*") if p.is_file()}


def main():
    assert os.environ["GITHUB_REPOSITORY"] == REPO
    artifact = Path(os.environ["LIVE_ARTIFACT"]).resolve() / "helper"
    source = (artifact / "source-commit.txt").read_text().strip()
    assert source == os.environ["GITHUB_SHA"]
    cli, engine = artifact / "proj", artifact / "proj-xet-engine"
    metadata = json.loads((artifact / "proj-xet-engine.provenance.json").read_text())
    assert metadata["upstream_commit"] == "629562cd9fc37432317eb57811faf13a6935c601"
    assert sha(engine) == metadata["binary_sha256"]
    assert sha(cli) == (artifact / "proj.sha256").read_text().split()[0]
    cli.chmod(0o700)
    engine.chmod(0o700)
    # Check all destinations before the first write; existing fixtures need inspection.
    for tag in TAGS.values():
        for route in ["/releases/tags/", "/git/ref/tags/"]:
            try:
                api(route + tag)
            except urllib.error.HTTPError as error:
                if error.code != 404:
                    raise
            else:
                raise RuntimeError("Dedicated fixture already exists; do not overwrite")
    result = {"source": source, "cli_sha256": sha(cli), "helper_sha256": sha(engine), "stores": []}
    evidence = Path("live-store-evidence")
    evidence.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="proj-live-synthetic-") as temporary:
        work = Path(temporary)
        for structure, tag in TAGS.items():
            release = api("/releases", {"tag_name": tag, "target_commitish": source,
                "name": "Synthetic " + structure + " store transport acceptance",
                "body": "Public synthetic bytes only. Retained transport evidence, not a product release.",
                "draft": False, "prerelease": True})
            config = {"directories": {"raw-data": {"path": "input"},
                "cache": {"path": "cache", "safe": True}, "output": {"path": "output"}},
                "remotes": {"github": {tag: {"structure": structure, "content": ["raw-data"]}}},
                "build": {"scripts": [], "git": False, "dest": {tag: {}}}}
            if structure == "cas":
                config["remotes"]["github"][tag]["manifest-style"] = "tree"
            def project(path, configuration):
                path.mkdir()
                git(path, "init", "-b", "main")
                git(path, "remote", "add", "origin", "https://github.com/" + REPO + ".git")
                for directory in ["input", "output", "cache"]:
                    (path / directory).mkdir()
                (path / "_proj.yml").write_text(json.dumps(configuration))
                (path / "VERSION").write_text("v1.2.3\n")
            root = work / structure
            project(root, config)
            env = dict(os.environ, HOME=str(root / "home"),
                PATH=str(engine.parent) + os.pathsep + os.environ["PATH"],
                GITHUB_API_URL="https://api.github.com")
            def invoke(path, *args):
                response = subprocess.run([str(cli), "--machine", "--non-interactive",
                    "--project", str(path), *args], cwd=path, env=env,
                    capture_output=True, timeout=600)
                if response.returncode:
                    text = response.stdout.decode(errors="replace")
                    text = text.replace(os.environ["GH_TOKEN"], "[redacted]")
                    raise RuntimeError("Synthetic CLI " + args[0] + " failed: " + text[:2000])
                assert json.loads(response.stdout)["ok"] is True
            payload = random.Random(751).randbytes(2 * 1024 * 1024)
            (root / "input/data.bin").write_bytes(payload)
            (root / "input/shared.csv").write_text("name,value\none,1\ntwo,2\n")
            versions, ids, old_assets = [], set(), set()
            for version in [1, 2]:
                if version == 2:
                    (root / "input/data.bin").write_bytes(payload[:100000] + b"changed" + payload[100007:])
                    (root / "input/additional.txt").write_text("second version\n")
                    if structure == "cas":
                        config["remotes"]["github"][tag].update(
                            {"manifest-style": "flat", "manifest-format": "csv"})
                        (root / "_proj.yml").write_text(json.dumps(config))
                invoke(root, "build", "--patch")
                bindings = list((root / ".proj/remotes").glob("**/current.json"))
                assert len(bindings) == 1
                reference = json.loads(bindings[0].read_text())["reference"]
                new_ids = set(reference["snapshots"]) - ids
                assert len(new_ids) == 1
                snapshot = new_ids.pop()
                ids.add(snapshot)
                assets = api("/releases/" + str(release["id"]) + "/assets")
                names = {item["name"] for item in assets}
                assert old_assets <= names
                old_assets = names
                versions.append({"snapshot": snapshot, "files": inventory(root / "input"),
                                 "asset": reference["asset"], "zip_blake3": reference["hash"]})
            # After both writes, replay BOTH immutable project locators from fresh stores.
            for index, version in enumerate(versions):
                fresh = work / (structure + "-fresh-" + str(index))
                restore_config = json.loads(json.dumps(config))
                restore_config["build"]["dest"] = {}
                project(fresh, restore_config)
                (fresh / "VERSION").write_text("v9.8.7\n")
                shutil.copytree(root / ".proj", fresh / ".proj")
                args = ["--title", tag, "--snapshot", version["snapshot"],
                        "--max-bytes", "268435456", "--max-expanded-bytes", "8589934592"]
                invoke(fresh, "remote", "restore", *args, "--label", "raw-data", "--clear")
                assert inventory(fresh / "input") == version["files"]
                assert (fresh / "VERSION").read_text() == "v9.8.7\n"
                invoke(fresh, "remote", "verify", *args)
            result["stores"].append({"structure": structure, "release": release["html_url"],
                                     "versions": versions, "assets": sorted(old_assets)})
            (evidence / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print("PASS real GitHub", structure, release["html_url"], flush=True)


if __name__ == "__main__":
    main()
