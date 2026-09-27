"""Small E2R controls used for real source-mutation kills, not a full oracle."""
import importlib.util
from pathlib import Path
import sys
from oracle_support import bodies, group_bytes, persist_subset


def check(path, case):
    spec=importlib.util.spec_from_file_location('mutated_seal',path)
    impl=importlib.util.module_from_spec(spec)
    spec.loader.exec_module(impl)
    prior=group_bytes(0,bodies(1,11),512)
    current=group_bytes(1,bodies(2,11),512)
    clean=prior+current
    if case=='clean':
        assert impl.recover(clean,512)==('ok',(bodies(1,11),bodies(2,11)),len(clean)), 'clean group rejected'
    elif case=='corrupt':
        image=bytearray(clean);image[512+12]^=1;image=bytes(image)
        assert impl.recover(image,512)==('error',image), 'corruption silently discarded'
    elif case=='seal-only':
        image=prior+persist_subset(current,512,1<<(len(current)//512-1))
        assert impl.recover(image,512)==('error',image), 'seal-only mismatch silently discarded'
    elif case=='witness':
        result=impl.recover(clean,512)
        assert impl.supervise(False,result)=='resume_allowed', 'clean control blocked'
        assert impl.supervise(True,result)=='rebuild_from_trusted_source', 'known EIO ignored'
    else:
        raise ValueError('unknown case')


if __name__=='__main__':
    check(Path(sys.argv[1]),sys.argv[2])
