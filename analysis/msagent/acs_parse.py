#!/usr/bin/env python3
"""Static parser for Microsoft Agent v2 .ACS character files.
Format per Remy Lebeau, "MSAgent Character Data Specification" v1.3 (2009).
Binary-verified against Merlin.acs (official, MS Agent 2.0 SDK ISO).
"""
import struct, sys, json, hashlib

class R:
    def __init__(self, b): self.b = b; self.p = 0
    def u8(self):  v = self.b[self.p]; self.p += 1; return v
    def u16(self): v = struct.unpack_from('<H', self.b, self.p)[0]; self.p += 2; return v
    def i16(self): v = struct.unpack_from('<h', self.b, self.p)[0]; self.p += 2; return v
    def u32(self): v = struct.unpack_from('<I', self.b, self.p)[0]; self.p += 4; return v
    def i32(self): v = struct.unpack_from('<i', self.b, self.p)[0]; self.p += 4; return v
    def raw(self, n): v = self.b[self.p:self.p+n]; self.p += n; return v
    def guid(self):
        d = self.raw(16)
        a, b, c = struct.unpack('<IHH', d[:8])
        return f"{a:08X}-{b:04X}-{c:04X}-{d[8:10].hex().upper()}-{d[10:16].hex().upper()}"
    def string(self, nullterm=True):
        # STRING: ULONG charcount + WCHAR chars (+ null terminator in ACS)
        n = self.u32()
        s = self.raw(2 * n).decode('utf-16-le', 'replace')
        if nullterm and n > 0:
            t = self.u16()  # terminator (observed 0x0000)
            assert t == 0, f"bad terminator {t:#x} after {s!r}"
        return s

def loc(r):
    return (r.u32(), r.u32())

def parse_voice(r):
    v = {}
    v['tts_engine'] = r.guid()
    v['tts_mode'] = r.guid()
    v['speed'] = r.u32()
    v['pitch'] = r.u16()
    extra = r.u8()
    v['extra_present'] = extra
    if extra:
        v['langid'] = r.u16()
        v['dialect'] = r.string()
        v['gender'] = r.u16()
        v['age'] = r.u16()
        v['style'] = r.string()
    return v

def parse_balloon(r):
    b = {}
    b['lines'] = r.u8()
    b['chars_per_line'] = r.u8()
    def rgb():
        c = r.raw(4); return f"#{c[0]:02X}{c[1]:02X}{c[2]:02X}"
    b['fg'] = rgb(); b['bg'] = rgb(); b['border'] = rgb()
    b['font'] = r.string()
    b['font_height'] = r.i32()
    b['font_weight'] = r.i32()
    b['italic'] = r.u8()
    b['unknown'] = r.u8()
    return b

def parse_stateinfo_list(r):
    states = []
    n = r.u16()
    for _ in range(n):
        name = r.string()
        m = r.u16()
        anims = [r.string() for _ in range(m)]
        states.append({'state': name, 'anims': anims})
    return states

def parse_localized(r):
    n = r.u16()
    out = []
    for _ in range(n):
        out.append({'langid': r.u16(), 'name': r.string(), 'desc': r.string(), 'extra': r.string()})
    return out

def parse_character_info(r, data):
    ci = {}
    ci['minor'] = r.u16(); ci['major'] = r.u16()
    lloc = loc(r)
    ci['guid'] = r.guid()
    ci['width'] = r.u16(); ci['height'] = r.u16()
    ci['transparent_idx'] = r.u8()
    ci['flags'] = r.u32()
    ci['animset_major'] = r.u16(); ci['animset_minor'] = r.u16()
    f = ci['flags']
    # Empirically (Merlin/Genie/Peedy/Robby): bit5 (0x20) => VOICEINFO present,
    # bit9 (0x200) => BALLOONINFO present (spec's "bits 8,9": bit9=enabled is right,
    # bit4 for voice is wrong — voice presence is bit5).
    ci['voice_enabled'] = bool(f & 0x20)
    ci['balloon_enabled'] = bool(f & 0x200)
    ci['balloon_style'] = (f >> 16) & 7
    ci['std_anim_set'] = bool(f & (1 << 20))
    if ci['voice_enabled']:
        ci['voice'] = parse_voice(r)
    if ci['balloon_enabled']:
        ci['balloon'] = parse_balloon(r)
    # palette
    npal = r.u32()
    ci['palette_colors'] = npal
    r.raw(npal * 4)
    tray = r.u8()
    ci['tray_icon'] = tray
    if tray:
        # TRAYICON: ULONG mono size + ICONIMAGE + ULONG color size + ICONIMAGE
        s1 = r.u32(); r.raw(s1)
        s2 = r.u32(); r.raw(s2)
        ci['tray_icon_sizes'] = (s1, s2)
    ci['states'] = parse_stateinfo_list(r)
    ci['localized'] = parse_localized(r)
    return ci

def parse_anim_info(r, data):
    # per-animation info block at locator
    a = {}
    a['name'] = r.string()
    a['transition'] = r.u8()
    a['return_anim'] = r.string()
    nframes = r.u16()
    frames = []
    for _ in range(nframes):
        fr = {}
        nimg = r.u16()
        imgs = []
        for _ in range(nimg):
            idx = r.u32(); x = r.i16(); y = r.i16()
            imgs.append({'img': idx, 'x': x, 'y': y})
        fr['imgs'] = imgs
        fr['audio'] = r.u16()
        fr['duration'] = r.u16()  # 1/100 s
        fr['exit_to'] = r.i16()
        nbr = r.u8()   # binary-verified: branch count is u8 (spec said u16)
        fr['branches'] = [{'frame': r.u16(), 'prob': r.u16()} for _ in range(nbr)]
        nov = r.u8()
        ovs = []
        for _ in range(nov):
            # Binary-verified 14-byte overlay record (Merlin 73/73 anims):
            # img u16, type u8, replace u8, x i16, y i16, w u16, h u16, unk u8,
            # regionflag u8 @13 (+ ULONG size + RGNDATA if set).
            ov = {}
            ov['img'] = r.u16(); ov['type'] = r.u8(); ov['replace'] = r.u8()
            ov['x'] = r.i16(); ov['y'] = r.i16(); ov['w2'] = r.u16(); ov['h2'] = r.u16()
            ov['unk'] = r.u8(); ov['region'] = r.u8()
            if ov['region']:
                sz = r.u32(); r.raw(sz)
            ovs.append(ov)
        fr['overlays'] = ovs
        frames.append(fr)
    a['frames'] = frames
    return a

def main(path):
    data = open(path, 'rb').read()
    r = R(data)
    magic = r.u32()
    assert magic == 0xABCDABC3, f"not ACS: {magic:#x}"
    ci_loc = loc(r); an_loc = loc(r); im_loc = loc(r); au_loc = loc(r)
    out = {'file': path.split('/')[-1], 'size': len(data),
           'sha256': hashlib.sha256(data).hexdigest()}
    r.p = ci_loc[0]
    out['character'] = parse_character_info(r, data)
    # animations
    r.p = an_loc[0]
    nan = r.u32()
    anims = []
    for _ in range(nan):
        name = r.string()
        o, s = loc(r)
        anims.append({'name': name, 'off': o, 'size': s})
    out['num_animations'] = nan
    out['image_list_size'] = im_loc[1]
    out['audio_list_size'] = au_loc[1]
    # image/audio list counts: ULONG count then entries of 12 bytes (locator+checksum)
    r.p = im_loc[0]; nim = r.u32()
    r.p = au_loc[0]; nau = r.u32()
    out['num_images'] = nim; out['num_audios'] = nau
    detail = []
    for a in anims:
        r.p = a['off']
        d = parse_anim_info(r, data)
        dur = sum(f['duration'] for f in d['frames'])
        detail.append({
            'name': a['name'], 'transition': d['transition'], 'return': d['return_anim'],
            'frames': len(d['frames']), 'duration_cs': dur,
            'sounds': sorted({f['audio'] for f in d['frames'] if f['audio'] != 0xFFFF and f['audio'] != 0}),
            'has_overlays': any(f['overlays'] for f in d['frames']),
            'branches': any(f['branches'] for f in d['frames']),
            'exit_to': sorted({f['exit_to'] for f in d['frames'] if f['exit_to'] >= 0}),
        })
    out['animations'] = detail
    return out

if __name__ == '__main__':
    res = main(sys.argv[1])
    if len(sys.argv) > 2 and sys.argv[2] == '--full':
        print(json.dumps(res, indent=1))
    else:
        c = res['character']
        print(f"{res['file']} size={res['size']} sha256={res['sha256'][:16]}…")
        print(f"  ver {c['major']}.{c['minor']} guid={c['guid']} {c['width']}x{c['height']} flags={c['flags']:#x} voice={c['voice_enabled']} balloon={c.get('balloon')} ")
        if 'voice' in c: print(f"  voice: {c['voice']}")
        print(f"  anims={res['num_animations']} images={res['num_images']} audios={res['num_audios']}")
        for s in c['states']:
            print(f"  STATE {s['state']}: {s['anims']}")
        print(f"  localized: {c['localized']}")
