import os,sys
import cgroups
cgroups.enter(sys.argv[1])
os.execvp(sys.argv[2],sys.argv[2:])
