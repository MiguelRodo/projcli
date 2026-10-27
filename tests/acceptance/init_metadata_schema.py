"""Validate actual CLI-generated generic CFF with the official pinned schema."""
import argparse,json,os,subprocess,tempfile
from pathlib import Path
import jsonschema,yaml

parser=argparse.ArgumentParser()
parser.add_argument("--binary",required=True)
args=parser.parse_args()
binary=str(Path(args.binary).resolve())
schema=json.loads(Path("tests/fixtures/cff-schema-1.2.0.json").read_text())
with tempfile.TemporaryDirectory(prefix="proj-init-cff-") as temporary:
    root=Path(temporary)
    env={k:v for k,v in os.environ.items()
         if not k.startswith(("GIT_","GH_","GITHUB_","PROJR_"))
         and k not in ("PROJ_PROJECT_ROOT","PROJ_RUN_CONTEXT","QUARTO_PROFILE")}
    run=subprocess.run([binary,"--non-interactive","--project",str(root),"init",
        "--citation","--description-file","--title","A quoted: title",
        "--purpose","A complete project description.",
        "--contact-name","Contact is not an author","--contact-email","contact@example.invalid",
        "--author-family","Example","--author-given","Ada","--author-name","Research Team"],
        env=env,capture_output=True,text=True)
    assert run.returncode==0,run.stderr[-2000:]
    value=yaml.safe_load((root/"CITATION.cff").read_text())
    jsonschema.Draft7Validator(schema).validate(value)
    assert value["authors"]==[{"family-names":"Example","given-names":"Ada"},{"name":"Research Team"}]
    assert not {"doi","license","references"}&value.keys()
    assert "Contact is not an author" not in (root/"CITATION.cff").read_text()
    assert not any((root/name).exists() for name in ("R","NAMESPACE","renv","renv.lock",".git"))
print("PASS generic CFF validates against official schema 1.2.0; explicit authors, no invented identifiers")
