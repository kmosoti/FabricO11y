"""Contained admission controls; no historical writes or builder execution."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    'spill_measure', Path(__file__).with_name('spill_measure.py'))
spill = importlib.util.module_from_spec(spec)
spec.loader.exec_module(spill)


class PreflightAccepted(Exception):
    pass


def control(mismatch, explicit=False):
    binary = Path(os.environ['CARGO_TARGET_DIR']) / 'release/examples/completion_builder'
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']) / 'spill-accounting'
    payload = b'provenance-control-binary'
    actual = hashlib.sha256(payload).hexdigest()
    original_text, original_bytes = Path.read_text, Path.read_bytes
    data = spill.ROOT / 'docs/experiments/benchmarks/data/lab-completion-run-01/memory'
    argv = ['spill-control', '--id', 'provenance-control']
    if explicit:
        data = Path(os.environ['FABRIC_SCRATCH_ROOT']) / 'prospective-campaign'
        binary = data / 'frozen-builder'
        argv += ['--campaign-root', str(data), '--binary', str(binary), '--out', str(data / 'supplement')]
    records = {data / ('builder-' + cell) / 'provenance.json': actual for cell in spill.CELLS}
    if mismatch:
        records[data / 'builder-bigrows-64/provenance.json'] = '0' * 64
    captured, directories = [], []

    def read_text(path, *args, **kwargs):
        if path in records:
            return json.dumps({'hashes': {'binary': records[path]}})
        return original_text(path, *args, **kwargs)

    def read_bytes(path):
        return payload if path == binary else original_bytes(path)

    def mkdir(path, *args, **kwargs):
        directories.append(path)
        if path == scratch:
            raise PreflightAccepted()

    with patch.object(Path, 'read_text', read_text), patch.object(Path, 'read_bytes', read_bytes), \
         patch.object(Path, 'mkdir', mkdir), \
         patch.object(spill, 'dump', side_effect=lambda path, value: captured.append((path, value))), \
         patch.object(spill.subprocess, 'run', side_effect=AssertionError('compiler/workload reached')), \
         patch.object(sys, 'argv', argv):
        try:
            spill.main()  # Real require_limits reads real cgroup files.
        except PreflightAccepted:
            assert not mismatch, 'mismatched binary admitted'
        except RuntimeError as error:
            assert mismatch and str(error) == 'selected binary does not match every original builder provenance'
        else:
            raise AssertionError('expected admission boundary was not reached')
    assert len(captured) == 1 and captured[0][0].name == 'provenance.json'
    rows = captured[0][1]['comparisons']
    assert captured[0][1]['campaign_root'] == str(data)
    assert captured[0][1]['binary'] == str(binary)
    assert len(rows) == len(spill.CELLS)
    assert sum(row['matches'] for row in rows) == len(spill.CELLS) - int(mismatch)
    assert len(directories) == (1 if mismatch else 2)
    assert (scratch in directories) == (not mismatch)
    print(json.dumps({'control': 'mismatch' if mismatch else 'matched',
                      'explicit_campaign': explicit,
                      'comparisons': rows, 'scratch_created': False, 'builder_executed': False}))


def child_hash_controls():
    binary = Path('/never-read-builder')
    original = b'frozen-builder'
    expected = hashlib.sha256(original).hexdigest()
    payload = [original]
    calls = []

    def changed_child(*args, **kwargs):
        calls.append(args)
        payload[0] = b'changed-builder'
        return object()

    with patch.object(Path, 'read_bytes', side_effect=lambda: payload[0]), \
         patch.object(spill.subprocess, 'run', side_effect=changed_child):
        try:
            spill.frozen_child(binary, expected, ['synthetic-child'])
        except RuntimeError as error:
            assert str(error) == 'frozen builder binary changed during child'
        else:
            raise AssertionError('post-child binary change admitted')
    assert len(calls) == 1
    calls.clear()
    with patch.object(Path, 'read_bytes', return_value=b'changed-builder'), \
         patch.object(spill.subprocess, 'run', side_effect=lambda *args, **kwargs: calls.append(args)):
        try:
            spill.frozen_child(binary, expected, ['synthetic-child'])
        except RuntimeError as error:
            assert str(error) == 'frozen builder binary changed before child'
        else:
            raise AssertionError('pre-child binary change admitted')
    assert not calls
    with patch.dict(os.environ, {'FABRIC_FAULT_KILL': '1', 'FABRIC_FAULT_MATCH': '.run-', 'FABRIC_FAULT_OP': 'write'}):
        env = spill.measurement_env(Path('/observer.so'), Path('/owned-work'))
        assert not any(key.startswith('FABRIC_FAULT_') for key in env)
        assert env['LD_PRELOAD'] == '/observer.so'
        assert env['FABRIC_MEASURE_ROOT'] == '/owned-work'
    print(json.dumps({'control': 'child_hashes_and_fault_environment', 'pre_child_rejected': True,
                      'post_child_rejected': True, 'fault_environment_removed': True, 'builder_executed': False}))


if __name__ == '__main__':
    spill.require_limits()
    control(False)
    control(True)
    control(False, explicit=True)
    control(True, explicit=True)
    child_hash_controls()
