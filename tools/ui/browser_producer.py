"""Independent finite OTLP trace fixture, encoded from published protobuf fields.

Schema source: OpenTelemetry proto v1.9.0 trace/v1/trace.proto and
collector/trace/v1/trace_service.proto. Expected values are producer constants;
this module imports no Fabric codec, query implementation or oracle.
"""
import struct

TRACE_ID = '1234567890abcdef1234567890abcdef'
PARENT = '1111111111111111'
CHILD = '2222222222222222'
ORPHAN = '3333333333333333'
MISSING = '4444444444444444'


def varint(value):
    result = bytearray()
    while value > 127:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return bytes(result)


def message(field, body):
    return varint((field << 3) | 2) + varint(len(body)) + body


def span(identifier, parent, name, start, end):
    result = message(1, bytes.fromhex(TRACE_ID)) + message(2, bytes.fromhex(identifier))
    if parent:
        result += message(4, bytes.fromhex(parent))
    return (result + message(5, name.encode()) + b'\x30\x01'
            + b'\x39' + struct.pack('<Q', start) + b'\x41' + struct.pack('<Q', end))


def traces(origin_ns):
    # Adjacent nanoseconds above JS's exact integer range discriminate rounding.
    rows = [
        {'span_id': PARENT, 'parent_span_id': '', 'name': 'browser-parent', 'start_ns': origin_ns, 'end_ns': origin_ns + 101},
        {'span_id': CHILD, 'parent_span_id': PARENT, 'name': 'browser-child', 'start_ns': origin_ns + 1, 'end_ns': origin_ns + 17},
        {'span_id': ORPHAN, 'parent_span_id': MISSING, 'name': 'browser-orphan', 'start_ns': origin_ns + 33, 'end_ns': origin_ns + 67},
    ]
    scope = b''.join(message(2, span(r['span_id'], r['parent_span_id'], r['name'], r['start_ns'], r['end_ns'])) for r in rows)
    return message(1, message(2, scope)), rows
