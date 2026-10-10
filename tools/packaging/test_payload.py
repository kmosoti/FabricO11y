import stat
import unittest
import sys
from unittest.mock import patch
import payload
from payload import cpio, identity


def entry(name, body=b'', mode=stat.S_IFREG | 0o644):
    name = name.encode() + b'\0'
    fields = [1, mode, 0, 0, 1, 0, len(body), 0, 0, 0, 0, len(name), 0]
    header = b'070701' + ''.join(f'{value:08x}' for value in fields).encode()
    result = header + name
    result += b'\0' * (-len(result) % 4)
    result += body
    return result + b'\0' * (-len(result) % 4)


class PayloadTests(unittest.TestCase):
    def test_extraction_stops_at_cap_before_materializing_oversized_output(self):
        with patch.object(payload, 'CAP', 1024):
            self.assertEqual(payload.bounded_command([sys.executable, '-c', 'print("small")']), b'small\n')
            with self.assertRaisesRegex(ValueError, 'bounded inventory cap'):
                payload.bounded_command([sys.executable, '-c', 'import sys; sys.stdout.buffer.write(b"x"*4096)'])

    def test_exact_content_inventory_with_complete_trailer(self):
        raw = entry('./usr/bin/fabric-node', b'ELF') + entry('TRAILER!!!')
        self.assertEqual(cpio(raw), {'usr/bin/fabric-node': identity(b'ELF')})
        for invalid in [raw[:-100], raw.replace(b'070701', b'070702', 1),
                        entry('../usr/bin/bad', b'bad') + entry('TRAILER!!!'),
                        entry('/usr/bin/bad', b'bad') + entry('TRAILER!!!'),
                        entry('usr/bin/link', b'elsewhere', stat.S_IFLNK | 0o777) + entry('TRAILER!!!'),
                        entry('usr/bin/x', b'a') + entry('usr/bin/x', b'b') + entry('TRAILER!!!')]:
            with self.subTest(invalid=invalid[:15]), self.assertRaises(ValueError):
                cpio(invalid)


if __name__ == '__main__':
    unittest.main()
