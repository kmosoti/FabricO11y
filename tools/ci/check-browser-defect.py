"""Require a registered browser defect failure, refusing unrelated errors."""
import argparse
import json
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from resource_group import require_limits


def detected(status, receipt, kind='api-cache'):
    if kind == 'nonresident-enrollment':
        witnesses = receipt.get('resident_credential_witnesses', [])
        return (status == 1 and receipt.get('exit') == 1
                and receipt.get('injected_nonresident_enrollment') is True
                and receipt.get('error') == 'owner enrollment stores actual resident credentials for the fixture RP'
                and 'first owner consumed protected one-time bootstrap' in receipt.get('checks', [])
                and 'server enrollment requires resident credentials and user verification' in receipt.get('checks', [])
                and len(witnesses) == 1 and witnesses[0].get('count') == 1
                and witnesses[0].get('credentials') == [{'resident':False, 'rp':'localhost'}]
                and 'owned TLS/server/browser fixtures removed' in receipt.get('cleanup', ''))
    return (status == 1 and receipt.get('exit') == 1
            and receipt.get('injected_api_cache_defect') is True
            and receipt.get('error') == 'worker caches only public shell paths'
            and 'actual browser matches reviewed version' in receipt.get('checks', [])
            and 'two real virtual-authenticator passkeys enrolled' in receipt.get('checks', [])
            and 'owned TLS/server/browser fixtures removed' in receipt.get('cleanup', ''))


if __name__ == '__main__':
    require_limits()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('status', type=int)
    parser.add_argument('receipt', type=Path)
    parser.add_argument('--kind', choices=('api-cache', 'nonresident-enrollment'), default='api-cache')
    args = parser.parse_args()
    if not detected(args.status, json.loads(args.receipt.read_text()), args.kind):
        raise SystemExit(args.kind + ' negative control did not reach its registered failure')
    print('registered ' + args.kind + ' defect detected')
