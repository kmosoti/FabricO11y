"""Fail-closed exact candidate and matching uninstalled-helper admission."""
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1] / 'packaging'))
from stage_candidate import digest
from payload import assert_equivalent, inventory
DATA = Path('/run/media/kmosoti/data/FabricO11y')


def validate_inputs(deb_receipt, rpm_receipt):
    for path in (deb_receipt, rpm_receipt):
        path.resolve().relative_to(DATA)
        if path.is_symlink():
            raise ValueError('build receipt cannot be a symlink')
    deb = json.loads(deb_receipt.read_text())
    rpm = json.loads(rpm_receipt.read_text())
    if (deb.get('state') != 'built' or not deb.get('container_cleanup_confirmed')
            or deb.get('package_family') != 'debian' or rpm.get('state') != 'built'
            or rpm.get('exit') != 0 or rpm.get('package_family') != 'fedora'
            or rpm.get('deb_receipt_sha256') != digest(deb_receipt)
            or rpm.get('deb_sha256') != deb.get('package_sha256')
            or rpm.get('source') != deb.get('source')):
        raise ValueError('successful exact linked Debian/RPM build receipts required')
    packages = {'debian': Path(deb['package']), 'fedora': Path(rpm['package'])}
    for family, receipt in [('debian', deb), ('fedora', rpm)]:
        path = packages[family]
        path.resolve().relative_to(DATA)
        if path.is_symlink() or digest(path) != receipt['package_sha256']:
            raise ValueError('exact package bytes changed')
    provenance = deb_receipt.parent / 'provenance'
    source = deb['source']
    if Path(source['source_bundle_file']).name != source['source_bundle_file']:
        raise ValueError('unsafe frozen source bundle filename')
    manifest = provenance / 'source-manifest.json'
    if (digest(manifest) != source['manifest_sha256']
            or digest(provenance / source['source_bundle_file']) != source['source_bundle_sha256']):
        raise ValueError('frozen package source manifest/bundle changed')
    payload = assert_equivalent(packages['debian'], packages['fedora'])
    if payload != rpm['payload_identity']:
        raise ValueError('actual exact payload inventory differs from reviewed RPM build receipt')
    if inventory(packages['fedora'])['usr/share/doc/fabrico11y/CANDIDATE-PROVENANCE.json']['sha256'] != rpm['candidate_provenance_sha256']:
        raise ValueError('RPM candidate provenance changed')
    helpers = deb_receipt.parent / 'qualification-helpers'
    helper_record = json.loads((helpers / 'manifest.json').read_text())
    if helper_record.get('source') != source:
        raise ValueError('recovery helpers must be compiled from the exact package source freeze')
    if set(helper_record['files']) != {'spool_dump', 'server_dump', 'spindle_sim'}:
        raise ValueError('exact three uninstalled qualification helpers required')
    for name, expected in helper_record['files'].items():
        if (helpers / name).is_symlink() or digest(helpers / name) != expected:
            raise ValueError('exact recovery helper bytes changed')
    return {**packages, 'helpers': helpers,
            'identity': {'source_commit': source['source_commit'], 'source_manifest_sha256': source['manifest_sha256'],
                         'source_bundle_sha256': source['source_bundle_sha256'],
                         'deb_sha256': deb['package_sha256'], 'rpm_sha256': rpm['package_sha256'],
                         'payload_identity': payload, 'helper_manifest': helper_record}}
