"""Enter an owned cgroup before installing the one-server clock environment."""
import os
from pathlib import Path
import sys

sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'bench/labs/completion'))
import cgroups


def main(arguments):
    group,helper,descriptor,*command=arguments
    if any(name in os.environ for name in ('LD_PRELOAD','LD_AUDIT','FABRIC_CLOCK_FD')):
        raise ValueError('fixture-environment')
    if not command or not Path(command[0]).is_absolute() or not Path(helper).is_absolute():
        raise ValueError('fixture-server-executable')
    fd=int(descriptor)
    if fd<0 or fd>1048575 or not os.get_inheritable(fd):raise ValueError('fixture-server-control')
    cgroups.enter(group)
    os.environ.update(LD_PRELOAD=helper,FABRIC_CLOCK_FD=descriptor)
    os.execv(command[0],command)


if __name__=='__main__':main(sys.argv[1:])
