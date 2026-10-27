"""Linux child resource measurement using only the Python standard library."""
import json,resource,subprocess,sys,time
from pathlib import Path
if __name__=="__main__":
    output=Path(sys.argv[1]);start=time.monotonic()
    result=subprocess.run(sys.argv[2:])
    usage=resource.getrusage(resource.RUSAGE_CHILDREN)
    output.write_text(json.dumps({"elapsed_s":time.monotonic()-start,
      "user_s":usage.ru_utime,"system_s":usage.ru_stime,
      "max_rss_kib":usage.ru_maxrss,"exit":result.returncode,
      "scope":"Linux waited child/descendant rusage; max RSS is not total simultaneous process-tree memory"},indent=2)+"\n")
    raise SystemExit(result.returncode)
