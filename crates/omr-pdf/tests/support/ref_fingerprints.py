"""Reference fingerprints for the omr-pdf real-data differential test (tests/real_iau.rs).

Independent decode path: pypdf reads each page's image stream, the CCITT bytes are wrapped in
a one-strip TIFF and decoded by Pillow/libtiff. Per page it records the SHA-256 of the packed
1-bit bitmap (1 = black, MSB first, rows padded to whole bytes), the black-pixel count and the
SHA-256 of Pillow's reduce(3) 8-bit image. No pixels, names or IDs are written.

Usage (outputs must go to a scratch directory, never into the repository):
    python3 -I ref_fingerprints.py <pdf_dir> <out.json>
"""
import glob
import hashlib
import io
import json
import struct
import sys

import numpy as np
import pypdf
from PIL import Image


def ccitt(data, w, h, k, blackis1):
    comp = 4 if k < 0 else 3
    tags = [(256, 4, 1, w), (257, 4, 1, h), (258, 3, 1, 1), (259, 3, 1, comp),
            (262, 3, 1, 1 if blackis1 else 0), (273, 4, 1, 0), (278, 4, 1, h), (279, 4, 1, len(data))]
    n = len(tags)
    off = 8 + 2 + n * 12 + 4
    o = b'II*\x00' + struct.pack('<I', 8) + struct.pack('<H', n)
    for t, ty, c, v in tags:
        if t == 273:
            v = off
        o += struct.pack('<HHI', t, ty, c) + (struct.pack('<HH', v, 0) if ty == 3 else struct.pack('<I', v))
    return Image.open(io.BytesIO(o + struct.pack('<I', 0) + data))


def main(src, out):
    res = []
    for f in sorted(glob.glob(src + '/*.pdf')):
        r = pypdf.PdfReader(f)
        for i, p in enumerate(r.pages):
            xo = p['/Resources']['/XObject']
            o = [xo[k].get_object() for k in xo][0]
            dp = o.get('/DecodeParms', {})
            dp = dp[0] if isinstance(dp, list) else dp
            im = ccitt(o._data, o['/Width'], o['/Height'], int(dp.get('/K', 0)), bool(dp.get('/BlackIs1', False)))
            gray = im.convert('L')
            black = np.asarray(gray) == 0
            small = gray.reduce(3)
            res.append({'file': f.split('/')[-1], 'page': i, 'w': im.size[0], 'h': im.size[1],
                        'black': int(black.sum()),
                        'sha256_1bit': hashlib.sha256(np.packbits(black, axis=1).tobytes()).hexdigest(),
                        'w200': small.size[0], 'h200': small.size[1],
                        'sha256_200': hashlib.sha256(np.asarray(small).tobytes()).hexdigest()})
    json.dump(res, open(out, 'w'), indent=0)
    print(len(res))


if __name__ == '__main__':
    main(sys.argv[1], sys.argv[2])
