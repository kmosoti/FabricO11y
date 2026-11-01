import json
from pathlib import Path
root = Path(__file__).resolve().parents[2]
print(json.dumps({"systemMessage": (root / "AGENTS.md").read_text()[:4000]}))
