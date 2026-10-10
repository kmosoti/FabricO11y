import base64
import struct
import unittest
import zlib
import producer
import snapshot
import query_oracle


def framed(payload):
    head = b'FAB1' + struct.pack('<I', len(payload))
    header = head + struct.pack('<II', zlib.crc32(head), zlib.crc32(payload))
    marker = b'FAC1' + struct.pack('<Q', len(header) + len(payload))
    return header + payload + marker + struct.pack('<I', zlib.crc32(marker))


class IndependentSnapshots(unittest.TestCase):
    def fixture(self):
        batch = producer.batch('a' * 32, 1, producer.metrics(1, 10, 20))
        entry = producer.message(1, b'controlled') + producer.message(2, batch) + producer.integer(3, 30)
        return framed(producer.integer(1, 1) + producer.message(2, entry))

    def test_actual_group_bytes_select_declared_snapshot_without_answer_rows(self):
        groups = snapshot.decode(self.fixture())
        records = snapshot.select(groups, [{'snapshot': 'g1-1'}])
        self.assertEqual(len(records), 1)
        batch = query_oracle.decode_batch(base64.b64decode(records[0]['bytes']))
        self.assertEqual(batch['sequence'], 1)
        self.assertEqual(records[0]['received_ns'], 30)

    def test_corrupt_marker_and_truncated_tail_cannot_supply_expected_records(self):
        raw = self.fixture()
        for changed in (raw[:-1], raw[:-1] + bytes([raw[-1] ^ 1]), b'BAD!' + raw[4:]):
            with self.assertRaises(ValueError):
                snapshot.decode(changed)

    def test_substituted_snapshot_cannot_select_missing_or_unretained_group(self):
        groups = snapshot.decode(self.fixture())
        for token in ('g1-2', 'g2-2', 'scoped snapshot'):
            with self.assertRaises(ValueError):
                snapshot.select(groups, [{'snapshot': token}])
