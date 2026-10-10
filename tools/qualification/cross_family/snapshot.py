"""Independent published FAB1/FAC1 and server Group protobuf decoding.

Stopped guest journal bytes supply group membership for the declared query
snapshot. Batch contents are also checked against the separately built replay
helper; query answers never supply expected row contents.
"""
import base64
import re
import struct
import zlib
import query_oracle


def decode(raw):
    if len(raw) > 128 * 1024**2:
        raise ValueError('journal exceeds bounded decoder allowance')
    at, groups = 0, []
    while at < len(raw):
        if len(raw) - at < 16:
            raise ValueError('stopped journal has an incomplete header')
        header = raw[at:at+16]
        size, header_crc, payload_crc = struct.unpack('<III', header[4:])
        if header[:4] != b'FAB1' or zlib.crc32(header[:8]) != header_crc or not 0 < size <= 4 * 1024**2:
            raise ValueError('invalid journal header or checksum')
        end = at + 16 + size
        if end + 16 > len(raw):
            raise ValueError('stopped journal has incomplete payload/commit marker')
        payload, marker = raw[at+16:end], raw[end:end+16]
        if (zlib.crc32(payload) != payload_crc or marker[:4] != b'FAC1'
                or struct.unpack('<Q', marker[4:12])[0] != end
                or zlib.crc32(marker[:12]) != struct.unpack('<I', marker[12:])[0]):
            raise ValueError('invalid journal payload or durable marker checksum')
        fields = query_oracle.parse_fields(payload)
        if set(fields) - {1, 2} or len(fields.get(1, [])) != 1 or fields[1][0][0] != 0:
            raise ValueError('invalid server Group fields')
        records = []
        for wire, entry in fields.get(2, []):
            values = query_oracle.parse_fields(entry)
            if wire != 2 or set(values) != {1, 2, 3} or any(len(v) != 1 for v in values.values()):
                raise ValueError('invalid server Entry fields')
            if [values[i][0][0] for i in (1, 2, 3)] != [2, 2, 0]:
                raise ValueError('invalid server Entry wire types')
            records.append({'label': values[1][0][1].decode('utf-8'),
                            'bytes': base64.b64encode(values[2][0][1]).decode(),
                            'received_ns': values[3][0][1]})
        groups.append({'group': fields[1][0][1], 'records': records})
        at = end + 16
    return groups


def select(groups, pages):
    if not pages or len({p['snapshot'] for p in pages}) != 1:
        raise ValueError('one stable query snapshot required across all pages')
    match = re.fullmatch(r'g(\d+)-(\d+)', pages[0]['snapshot'])
    if not match:
        raise ValueError('legacy query snapshot lacks group boundaries')
    lower, upper = map(int, match.groups())
    all_ids = [g['group'] for g in groups]
    if all_ids != list(range(1, len(groups) + 1)):
        raise ValueError('independent retained journal prefix missing or noncontiguous')
    if lower != 1 or not 1 <= upper <= len(groups):
        raise ValueError('query snapshot differs from independent retained group coverage')
    return [r for g in groups if lower <= g['group'] <= upper for r in g['records']]
