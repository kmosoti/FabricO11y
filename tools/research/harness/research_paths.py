"""Where the research harnesses find binaries, data and the corpus.

Every path is overridable by environment so a run on another machine needs
no edits: FABRIC_RESEARCH_ROOT (default: <repo>/target/research) holds
`bin/{stock,proto,budget}/fabric-server`, `bin/sealbench`, `bin/fobbench`,
`data/` (generated journals, states, results) and `corpus/`.
"""
import os
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
ROOT = Path(os.environ.get("FABRIC_RESEARCH_ROOT", REPO / "target" / "research")).resolve()
BIN = ROOT / "bin"
DATA = ROOT / "data"
CORPUS = ROOT / "corpus"
SEALBENCH = BIN / "sealbench"
FOBBENCH = BIN / "fobbench"
QUALIFICATION = REPO / "tools" / "qualification"
SMOKE = os.environ.get("FABRIC_RESEARCH_SMOKE") == "1"
MIB = 1048576
sys.path.insert(0, str(QUALIFICATION))
for d in (BIN, DATA, CORPUS):
    d.mkdir(parents=True, exist_ok=True)
