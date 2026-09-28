import struct

def varint(b, i):
    r = 0; s = 0
    while True:
        x = b[i]; i += 1; r |= (x & 127) << s; s += 7
        if x < 128: return r, i

def fields(b):
    i = 0; out = []
    while i < len(b):
        k, i = varint(b, i); f = k >> 3; w = k & 7
        if w == 0: v, i = varint(b, i)
        elif w == 2:
            n, i = varint(b, i); v = b[i:i+n]; i += n
        elif w == 1: v = b[i:i+8]; i += 8
        elif w == 5: v = b[i:i+4]; i += 4
        else: raise ValueError('wire')
        out.append((f, v))
    return out

def frames(path):
    data = open(path, 'rb').read(); at = 0; res = []
    while at + 16 <= len(data):
        size = struct.unpack('<I', data[at+4:at+8])[0]
        end = at + 16 + size
        if end + 16 > len(data): break
        payload = data[at+16:end]
        seq = 0; gaps = []; cursors = []
        for f, v in fields(payload):
            if f == 4: seq = v
            elif f == 8: gaps.append(v.decode('utf-8'))
            elif f == 7:
                c = dict(fields(v)); cursors.append((c.get(1, b'').decode(), c.get(4, 0)))
        res.append(dict(seq=seq, gaps=gaps, cursors=cursors))
        at = end + 16
    return res
