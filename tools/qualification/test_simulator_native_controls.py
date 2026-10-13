"""Hand-written finite wire/authority controls; no native process or clock runs."""
import copy
from pathlib import Path
import unittest

from query_oracle import ProtoError
from release_fixture import integer, message
from simulator_native_controls import CANONICAL, binding, batch_identity, read_json


def wire(sequence=1, body=b'R' * 512, points=32):
    # Independent finite protobuf literal construction, not Rust offer code.
    record = message(5, message(1, body))
    logs = message(1, message(2, message(2, record) + message(2, record)))
    metric = message(1, b'diagnostic') + message(5, message(1, b''))
    metrics = message(1, message(2, b''.join(message(2, metric) for _ in range(points))))
    return (integer(1, 1) + message(2, bytes(range(16))) + integer(3, 1) + integer(4, sequence)
            + (message(5, metrics) if sequence == 1 else b'') + message(6, logs))


class NativeRegistrationTests(unittest.TestCase):
    def test_exact_canonical_expected_tuple(self):
        literal = Path(__file__).resolve().parents[2] / 'docs/experiments/benchmarks/simulator-native-witness-canonical.json'
        self.assertEqual(binding(read_json(literal), 'canonical-red'), CANONICAL)

    def test_every_canonical_pin_is_independent(self):
        for key in CANONICAL:
            mutant = copy.deepcopy(CANONICAL)
            mutant[key] = '0' * len(mutant[key])
            with self.assertRaisesRegex(ValueError, 'sealed canonical binding'):
                binding(mutant, 'canonical-red')

    def test_separate_binary_keeps_original_helper_authority(self):
        expected = CANONICAL | {'binary_sha256': 'a' * 64, 'source_commit': 'b' * 40,
                                'source_tree': 'c' * 40, 'build_receipt_sha256': 'd' * 64}
        self.assertEqual(binding(expected, 'positive'), expected)
        with self.assertRaisesRegex(ValueError, 'separate helper required'):
            binding(CANONICAL, 'positive')
        with self.assertRaisesRegex(ValueError, 'separate helper required'):
            binding(expected | {'canonical_helper_manifest_sha256': 'e' * 64}, 'positive')

    def test_malformed_missing_or_extra_authority_refused(self):
        for mutant in (CANONICAL | {'source_commit': ''}, CANONICAL | {'binary_sha256': True},
                       CANONICAL | {'extra': 'x'}, {k: v for k, v in CANONICAL.items() if k != 'source_tree'}):
            with self.assertRaises(ValueError):
                binding(mutant, 'canonical-red')

    def test_two_seconds_use_original_finite_shapes(self):
        for sequence in (1, 2):
            self.assertEqual(batch_identity(wire(sequence)), (bytes(range(16)).hex(), 1, sequence))

    def test_duplicate_truncated_unknown_and_identity_wire_refused(self):
        for mutant in (wire() + integer(4, 1), wire()[:-1], wire() + message(9, b'trace'),
                       wire().replace(message(2, bytes(range(16))), message(2, b'short'), 1),
                       wire().replace(integer(4, 1), integer(4, 3), 1)):
            with self.assertRaises((ValueError, ProtoError)):
                batch_identity(mutant)

    def test_payload_shape_failure_cannot_be_missing_begin_red(self):
        for body in (b'R' * 511, b'R' * 513):
            with self.assertRaisesRegex(ValueError, 'two original 512-byte logs'):
                batch_identity(wire(body=body))

    def test_first_second_requires_all_original_metric_points(self):
        for points in (0, 31, 33):
            with self.assertRaisesRegex(ValueError, 'original short metric shape'):
                batch_identity(wire(points=points))


if __name__ == '__main__':
    unittest.main()
