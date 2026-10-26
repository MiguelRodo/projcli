"""Own-PTY fixtures for real terminal prompts and EOF cancellation; no network actions."""
import argparse,errno,os,pty,select,time,tempfile
from pathlib import Path

parser=argparse.ArgumentParser()
parser.add_argument("--binary",required=True)
binary=str(Path(parser.parse_args().binary).resolve())
def run(root,cancel):
    env={k:v for k,v in os.environ.items() if not k.startswith(("GIT_","GH_","GITHUB_","PROJR_"))
         and k not in ("PROJ_PROJECT_ROOT","PROJ_RUN_CONTEXT","QUARTO_PROFILE","PROJ_NON_INTERACTIVE","PROJCLI_NON_INTERACTIVE")}
    pid,fd=pty.fork()
    if pid==0:
        os.execve(binary,[binary,"--project",str(root),"init","--git=false","--github=false","--code-license","none"],env)
    prompts=[(b"Project title",b"Prompt project\n"),(b"Please finish:",b"compare models.\n"),
             (b"Project contact name",b"Test Contact\n"),(b"Project contact email",b"contact@example.invalid\n")]
    buffer=b"";index=0;status=None;deadline=time.monotonic()+20
    try:
        while time.monotonic()<deadline:
            if select.select([fd],[],[],0.1)[0]:
                try:chunk=os.read(fd,8192)
                except OSError as error:
                    if error.errno==errno.EIO:break
                    raise
                if not chunk:break
                buffer+=chunk
                if index<len(prompts) and prompts[index][0] in buffer:
                    os.write(fd,b"\x04" if cancel else prompts[index][1]);index+=1;buffer=b""
            done,observed=os.waitpid(pid,os.WNOHANG)
            if done:status=observed;break
        assert time.monotonic()<deadline,"Interactive init did not finish within the fixture deadline"
    finally:
        os.close(fd)
        if status is None:_,status=os.waitpid(pid,0)
    code=os.waitstatus_to_exitcode(status)
    if cancel:
        assert code!=0
        assert not (root/"VERSION").exists() and not (root/"README.md").exists()
    else:
        assert code==0 and index==4
        text=(root/"README.md").read_text()
        assert "compare models." in text and "Test Contact" in text
        assert text.index("compare models.")<text.index("## Contact")
        assert not (root/".git").exists() and not (root/"CITATION.cff").exists()
with tempfile.TemporaryDirectory(prefix="proj-init-terminal-") as tmp:
    for cancel in (False,True):
        root=Path(tmp)/("cancel" if cancel else "complete");root.mkdir();run(root,cancel)
print("PASS actual interactive metadata questions, purpose placement and EOF cancellation")
