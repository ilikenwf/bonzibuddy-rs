#!/usr/bin/env python3
"""Extract sprites, animations and audio from a Microsoft Agent .acs file.

Implements the unofficial ACS v2 spec (Lebeau Software, spec v1.6).
Outputs:
  out/sprites/<ANIM>/frame_NNN[_mouthK].png
  out/audio/<name>_NNN.wav
  out/animations.json
  out/character.json
"""
import json, struct, sys, os
from PIL import Image

class R:
    def __init__(self, data, pos=0):
        self.d = data; self.p = pos
    def u8(self):
        v = self.d[self.p]; self.p += 1; return v
    def i8(self):
        v = self.d[self.p]; self.p += 1; return v - 256 if v > 127 else v
    def bool(self): return self.u8() != 0
    def u16(self):
        v = struct.unpack_from('<H', self.d, self.p)[0]; self.p += 2; return v
    def i16(self):
        v = struct.unpack_from('<h', self.d, self.p)[0]; self.p += 2; return v
    def u32(self):
        v = struct.unpack_from('<I', self.d, self.p)[0]; self.p += 4; return v
    def i32(self):
        v = struct.unpack_from('<i', self.d, self.p)[0]; self.p += 4; return v
    def guid(self):
        g = self.d[self.p:self.p+16]; self.p += 16; return g.hex()
    def locator(self):
        return (self.u32(), self.u32())
    def string(self):
        n = self.u32()
        s = self.d[self.p:self.p+2*n].decode('utf-16-le', 'replace')
        self.p += 2*n
        if n: self.p += 2  # null terminator
        return s.rstrip('\x00')
    def list(self, count_type, item_fn):
        n = self.u16() if count_type == 'u16' else self.u32() if count_type == 'u32' else self.u8()
        return [item_fn() for _ in range(n)]

class BitReader:
    def __init__(self, data):
        self.d = data; self.byte = 0; self.bit = 0
    def read(self, n):
        v = 0
        for i in range(n):
            if self.byte >= len(self.d):
                raise EOFError('bitstream end')
            b = (self.d[self.byte] >> self.bit) & 1
            v |= b << i
            self.bit += 1
            if self.bit == 8:
                self.bit = 0; self.byte += 1
        return v

def msagent_decompress(data, out_size):
    """MS Agent bit-level LZ77. data = compressed payload (may start with 0x00 marker)."""
    if data and data[0] == 0x00:
        data = data[1:]
    br = BitReader(data)
    out = bytearray()
    while len(out) < out_size:
        try:
            t = br.read(1)
        except EOFError:
            break
        if t == 0:
            out.append(br.read(8))
        else:
            n = 0
            while n < 3 and br.read(1) == 1:
                n += 1
            # when n < 3 the terminating 0 bit was already consumed by the loop
            bitcount = (6, 9, 12, 20)[n]
            add = (1, 65, 577, 4673)[n]
            raw = br.read(bitcount)
            if bitcount == 20 and raw == 0xFFFFF:
                break  # end of stream
            offset = raw + add
            count = 3 if bitcount == 20 else 2
            m = 0
            while m < 11 and br.read(1) == 1:
                m += 1
            if m == 11 and br.read(1) == 1:
                raise ValueError('corrupt count run')
            count += (1 << m) - 1
            if m:
                count += br.read(m)
            src = len(out) - offset
            for i in range(count):
                out.append(out[src + i])
    return bytes(out[:out_size]) if len(out) >= out_size else bytes(out) + b'\x00' * (out_size - len(out))

def parse_rgn(data):
    """RGNDATA -> list of (x, y, w, h) rects."""
    if len(data) < 32:
        return []
    _, _, ncount, _ = struct.unpack_from('<4I', data, 0)
    rects = []
    for i in range(ncount):
        x1, y1, x2, y2 = struct.unpack_from('<4i', data, 32 + 16 * i)
        rects.append((x1, y1, x2 - x1, y2 - y1))
    return rects

def decode_image(data, pos, palette, trans_index):
    r = R(data, pos)
    r.u8()  # unknown
    w = r.u16(); h = r.u16()
    compressed = r.bool()
    img_size = r.u32()
    img_data = data[r.p:r.p + img_size]; r.p += img_size
    # region data (COMPRESSED)
    csize = r.u32(); usize = r.u32()
    reg_raw = data[r.p:r.p + (csize if csize else usize)]; r.p += (csize if csize else usize)
    if csize:
        reg_raw = msagent_decompress(reg_raw, usize)
    rects = parse_rgn(reg_raw)

    stride = (w + 3) & ~3
    if compressed:
        img_data = msagent_decompress(img_data, stride * h)
    # 8bpp indexed, bottom-up DIB rows
    px = bytearray(w * h * 4)
    for y in range(h):
        srow = (h - 1 - y) * stride
        drow = y * w * 4
        for x in range(w):
            idx = img_data[srow + x]
            r_, g_, b_ = palette[idx]
            o = drow + x * 4
            px[o:o+4] = bytes((r_, g_, b_, 0 if idx == trans_index else 255))
    # apply region mask: pixels outside rects become transparent
    if rects and not (len(rects) == 1 and rects[0][2] >= w and rects[0][3] >= h):
        mask = bytearray(w * h)
        for (rx, ry, rw, rh) in rects:
            for y in range(max(0, ry), min(h, ry + rh)):
                for x in range(max(0, rx), min(w, rx + rw)):
                    mask[y * w + x] = 1
        for i in range(w * h):
            if not mask[i]:
                px[i * 4 + 3] = 0
    img = Image.frombytes('RGBA', (w, h), bytes(px))
    return img, r.p

def main(acs_path, outdir):
    data = open(acs_path, 'rb').read()
    r = R(data)
    sig = r.u32()
    assert sig == 0xABCDABC3, f'bad sig {sig:#x}'
    loc_char, loc_anim, loc_img, loc_audio = r.locator(), r.locator(), r.locator(), r.locator()
    os.makedirs(outdir, exist_ok=True)
    os.makedirs(os.path.join(outdir, 'sprites'), exist_ok=True)
    os.makedirs(os.path.join(outdir, 'audio'), exist_ok=True)

    # ---- character info
    r = R(data, loc_char[0])
    minor, major = r.u16(), r.u16()
    loc_localized = r.locator()
    guid = r.guid()
    cw, ch = r.u16(), r.u16()
    trans_idx = r.u8()
    flags = r.u32()
    anm_maj, anm_min = r.u16(), r.u16()
    # VOICEINFO
    voice_guid1 = r.guid(); voice_guid2 = r.guid()
    speed = r.u32(); pitch = r.u16(); voice_extra = r.bool()
    if voice_extra:
        r.u16(); r.string(); r.u16(); r.u16(); r.string()
    # BALLOONINFO
    balloon = {}
    if flags & (1 << 4) or True:
        pass
    lines = r.u8(); cpl = r.u8()
    fg = data[r.p:r.p+4]; r.p += 4
    bg = data[r.p:r.p+4]; r.p += 4
    bd = data[r.p:r.p+4]; r.p += 4
    font = r.string(); fheight = r.i32(); fweight = r.i32()
    italic = r.bool(); unk = r.u8()
    balloon = dict(lines=lines, chars_per_line=cpl, fg=list(fg[:3]), bg=list(bg[:3]),
                   border=list(bd[:3]), font=font, font_height=fheight, font_weight=fweight)
    # palette
    npal = r.u32()
    palette = []
    for _ in range(npal):
        palette.append(tuple(data[r.p:r.p+3])); r.p += 4
    # tray icon
    tray_enabled = r.bool()
    if tray_enabled:
        sz = r.u32(); r.p += sz
        sz = r.u32(); r.p += sz
    # states
    nstates = r.u16()
    states = []
    for _ in range(nstates):
        name = r.string()
        nanm = r.u16()
        anms = [r.string() for _ in range(nanm)]
        states.append({'state': name, 'animations': anms})
    # localized info
    rl = R(data, loc_localized[0])
    nloc = rl.u16()
    localized = []
    for _ in range(nloc):
        lang = rl.u16(); nm = rl.string(); desc = rl.string(); extra = rl.string()
        localized.append({'langid': lang, 'name': nm, 'description': desc})

    # ---- images
    images = []
    r = R(data, loc_img[0])
    nimg = r.u32()
    img_locs = [(r.locator(), r.u32()) for _ in range(nimg)]
    for i, ((off, size), _ck) in enumerate(img_locs):
        img, _ = decode_image(data, off, palette, trans_idx)
        images.append(img)
    print(f'{nimg} images decoded')

    # ---- audio
    r = R(data, loc_audio[0])
    naud = r.u32()
    aud_locs = [(r.locator(), r.u32()) for _ in range(naud)]
    for i, ((off, size), _ck) in enumerate(aud_locs):
        blob = data[off:off+size]
        if blob[:4] == b'RIFF':
            open(os.path.join(outdir, 'audio', f'audio_{i:03d}.wav'), 'wb').write(blob)
    print(f'{naud} audio blocks')

    # ---- animations
    r = R(data, loc_anim[0])
    nanm = r.u32()
    anims = []
    for _ in range(nanm):
        name = r.string()
        loc = r.locator()
        anims.append((name, loc))
    out_anims = {}
    for name, (off, size) in anims:
        ra = R(data, off)
        aname = ra.string()
        transition = ra.u8()
        ret = ra.string()
        nframes = ra.u16()
        frames = []
        for fi in range(nframes):
            # ACSFRAMEIMAGE list
            nim = ra.u16()
            imgs = []
            for _ in range(nim):
                idx = ra.u32(); xo = ra.i16(); yo = ra.i16()
                imgs.append((idx, xo, yo))
            audio_idx = ra.u16()
            duration = ra.u16()
            exit_frame = ra.i16()
            nbr = ra.u8()
            branches = [(ra.u16(), ra.u16()) for _ in range(nbr)]
            novl = ra.u8()
            overlays = []
            for _ in range(novl):
                otype = ra.u8(); replace = ra.bool(); oidx = ra.u16()
                ra.u8()  # unknown
                has_region = ra.bool()
                xo = ra.i16(); yo = ra.i16()
                ow = ra.u16() * 2; oh = ra.u16() * 2
                if has_region:
                    rd_size = ra.u32()
                    ra.p += rd_size
                overlays.append((otype, replace, oidx, xo, yo))
            frames.append(dict(imgs=imgs, audio=audio_idx, duration=duration,
                               exit=exit_frame, branches=branches, overlays=overlays))
        # composite frames
        adir = os.path.join(outdir, 'sprites', aname)
        os.makedirs(adir, exist_ok=True)
        frame_files = []
        for fi, fr in enumerate(frames):
            canvas = Image.new('RGBA', (cw, ch), (0, 0, 0, 0))
            for (idx, xo, yo) in reversed(fr['imgs']):
                canvas.alpha_composite(images[idx], (xo, yo))
            base_file = f'frame_{fi:03d}.png'
            canvas.save(os.path.join(adir, base_file))
            variants = {}
            for (otype, replace, oidx, xo, yo) in fr['overlays']:
                v = canvas.copy()
                v.alpha_composite(images[oidx], (xo, yo))
                vf = f'frame_{fi:03d}_mouth{otype}.png'
                v.save(os.path.join(adir, vf))
                variants[otype] = vf
            frame_files.append(dict(file=base_file, duration_10ms=fr['duration'],
                                    audio=fr['audio'], mouth=variants))
        out_anims[aname] = dict(transition=transition, return_anim=ret, frames=frame_files)
        print(f'  {aname}: {len(frames)} frames')

    json.dump(out_anims, open(os.path.join(outdir, 'animations.json'), 'w'), indent=1)
    json.dump(dict(width=cw, height=ch, transparent_index=trans_idx, flags=flags,
                   palette=palette, states=states, localized=localized, balloon=balloon,
                   version=[major, minor]),
              open(os.path.join(outdir, 'character.json'), 'w'), indent=1)
    print('done')

if __name__ == '__main__':
    main(sys.argv[1], sys.argv[2])
