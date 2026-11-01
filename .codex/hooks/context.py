import json
from pathlib import Path
import subprocess
root = Path(__file__).resolve().parents[2]
message = (root / 'AGENTS.md').read_text()[:4000]
receipt = root / '.local/last-verification'
if receipt.is_file():
    fields = dict(line.split('=', 1) for line in receipt.read_text().splitlines() if '=' in line)
    result = subprocess.run(['/usr/bin/jj', '--ignore-working-copy', 'log', '--no-graph', '-r', '@', '-T', 'commit_id'], cwd=root, capture_output=True, text=True, timeout=2)
    subject = fields.get('candidate', '')
    if result.returncode == 0 and result.stdout.strip() == subject:
        message += '\nA successful receipt names this snapshot. Unsnapshotted edits and external inputs are not checked by this hook; run candidate verify before publication.'
    else:
        message += '\nLast verification receipt is stale or current identity is unavailable. Run candidate capture and verify.'
else:
    message += '\nNo local verification receipt. Run candidate capture and verify before publication.'
print(json.dumps({'systemMessage': message}))
