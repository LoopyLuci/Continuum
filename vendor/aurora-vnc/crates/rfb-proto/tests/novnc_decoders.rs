//! Cross-check against an independent implementation: noVNC's ZRLE, Tight,
//! CopyRect and Raw decoders (JavaScript, run under node) must rebuild the
//! exact framebuffer our encoders describe, with the ZRLE and Tight zlib
//! streams carried across many rectangles. Skips cleanly without node or
//! noVNC (set AURORA_NOVNC_DIR if it is not in /usr/share/novnc).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use rfb_proto::encodings::{copyrect, raw, tight, TightEncoder, ZrleEncoder};
use rfb_proto::{Encoding, PixelFormat};

const HARNESS: &str = r#"
import './browser-shim.js';
import fs from 'fs';
import ZRLEDecoder from './core/decoders/zrle.js';
import TightDecoder from './core/decoders/tight.js';
import CopyRectDecoder from './core/decoders/copyrect.js';
import RawDecoder from './core/decoders/raw.js';

const [streamPath, rectsPath, outPath, W, H] = process.argv.slice(2);
const width = +W, height = +H;
const data = new Uint8Array(fs.readFileSync(streamPath));
const rects = JSON.parse(fs.readFileSync(rectsPath, 'utf8'));
let pos = 0;
const sock = {
    rQwait(_msg, num) { return data.length - pos < num; },
    rQshift8() { return data[pos++]; },
    rQshift16() { const v = (data[pos] << 8) | data[pos + 1]; pos += 2; return v; },
    rQshift32() { const v = data[pos] * 2 ** 24 + (data[pos + 1] << 16) + (data[pos + 2] << 8) + data[pos + 3]; pos += 4; return v; },
    rQshiftBytes(len, copy = true) { const s = data.subarray(pos, pos + len); pos += len; return copy ? s.slice() : s; },
    rQshiftTo(target, len) { target.set(data.subarray(pos, pos + len)); pos += len; },
    rQpeek8() { return data[pos]; },
    rQskipBytes(n) { pos += n; },
    rQpeekBytes(len, copy = true) { const s = data.subarray(pos, pos + len); return copy ? s.slice() : s; },
    // Older noVNC releases poke at the queue directly.
    get rQ() { return data; },
    get rQi() { return pos; },
    set rQi(v) { pos = v; },
    get rQlen() { return data.length - pos; },
};
const fb = new Uint8Array(width * height * 4);
for (let i = 3; i < fb.length; i += 4) fb[i] = 255;
const display = {
    fillRect(x, y, w, h, c) {
        for (let j = y; j < y + h; j++) for (let i = x; i < x + w; i++) {
            const o = (j * width + i) * 4; fb[o] = c[0]; fb[o + 1] = c[1]; fb[o + 2] = c[2]; fb[o + 3] = 255;
        }
    },
    blitImage(x, y, w, h, arr, off) {
        for (let j = 0; j < h; j++) for (let i = 0; i < w; i++) {
            const s = off + (j * w + i) * 4, o = ((y + j) * width + x + i) * 4;
            fb[o] = arr[s]; fb[o + 1] = arr[s + 1]; fb[o + 2] = arr[s + 2]; fb[o + 3] = 255;
        }
    },
    copyImage(sx, sy, x, y, w, h) {
        const copy = fb.slice();
        for (let j = 0; j < h; j++) for (let i = 0; i < w; i++) {
            const s = ((sy + j) * width + sx + i) * 4, o = ((y + j) * width + x + i) * 4;
            fb[o] = copy[s]; fb[o + 1] = copy[s + 1]; fb[o + 2] = copy[s + 2]; fb[o + 3] = 255;
        }
    },
    imageRect() { throw new Error('JPEG was not expected'); },
};
const decoders = { 16: new ZRLEDecoder(), 7: new TightDecoder(), 1: new CopyRectDecoder(), 0: new RawDecoder() };
for (const [x, y, w, h, enc] of rects) {
    if (!decoders[enc].decodeRect(x, y, w, h, sock, display, 24)) {
        throw new Error(`decoder ${enc} wanted more data at rect ${[x, y, w, h]}`);
    }
}
if (pos !== data.length) throw new Error(`decoders consumed ${pos} of ${data.length} bytes`);
fs.writeFileSync(outPath, fb);
"#;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()))?;
        } else {
            fs::copy(&p, to.join(e.file_name()))?;
        }
    }
    Ok(())
}

fn setup() -> Option<PathBuf> {
    let novnc = std::env::var_os("AURORA_NOVNC_DIR").map(PathBuf::from).unwrap_or_else(|| "/usr/share/novnc".into());
    if !novnc.join("core/decoders/zrle.js").exists() {
        skip(format_args!("noVNC not found at {} (set AURORA_NOVNC_DIR)", novnc.display()));
        return None;
    }
    if !Command::new("node").arg("--version").output().is_ok_and(|o| o.status.success()) {
        skip(format_args!("node not installed"));
        return None;
    }
    let dir = std::env::temp_dir().join(format!("aurora-novnc-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    copy_dir(&novnc.join("core"), &dir.join("core")).expect("copy noVNC core/");
    copy_dir(&novnc.join("vendor"), &dir.join("vendor")).expect("copy noVNC vendor/");
    fs::write(dir.join("package.json"), r#"{"type":"module"}"#).expect("write harness files");
    fs::write(dir.join("harness.js"), HARNESS).expect("write harness files");
    // noVNC's logging module expects a browser `window`; it is imported first.
    fs::write(dir.join("browser-shim.js"), "globalThis.window = globalThis;\n").expect("write harness files");
    Some(dir)
}

/// The pixel format noVNC always asks for: 32 bpp, depth 24, R in byte 0.
const NOVNC: PixelFormat = PixelFormat {
    bits_per_pixel: 32,
    depth: 24,
    big_endian: false,
    true_colour: true,
    red_max: 255,
    green_max: 255,
    blue_max: 255,
    red_shift: 0,
    green_shift: 8,
    blue_shift: 16,
};

fn picture(rng: &mut Rng, w: usize, h: usize) -> Vec<u32> {
    let colours: Vec<u32> =
        (0..[1, 2, 4, 16, 40, 200, 100_000][rng.below(7) as usize]).map(|_| rng.next() as u32 & 0xffffff).collect();
    match rng.below(4) {
        0 => (0..w * h).map(|_| colours[rng.below(colours.len() as u64) as usize]).collect(),
        1 => {
            let mut px = Vec::new();
            while px.len() < w * h {
                let v = colours[rng.below(colours.len() as u64) as usize];
                px.extend(std::iter::repeat_n(v, (1 + rng.below(300) as usize).min(w * h - px.len())));
            }
            px
        }
        2 => (0..w * h).map(|i| (((i % w) * 255 / w.max(1)) | (((i / w) * 255 / h.max(1)) << 8)) as u32).collect(),
        _ => (0..w * h).map(|_| rng.next() as u32 & 0xffffff).collect(),
    }
}

#[test]
fn novnc_decodes_our_zrle_tight_copyrect_and_raw_exactly() {
    let Some(dir) = setup() else { return };
    let (fw, fh) = (300usize, 200usize);
    let mut fb = vec![0u32; fw * fh];
    let mut rng = Rng(7);
    let mut zrle = ZrleEncoder::new(6);
    let mut tight_enc = TightEncoder::new(6);
    let mut stream = Vec::new();
    let mut rects = Vec::new();
    for i in 0..160 {
        let w = 1 + rng.below(fw as u64 - 1) as usize;
        let h = 1 + rng.below(fh as u64 - 1) as usize;
        let (x, y) = (rng.below((fw - w + 1) as u64) as usize, rng.below((fh - h + 1) as u64) as usize);
        let enc = [Encoding::Zrle, Encoding::Tight, Encoding::CopyRect, Encoding::Raw][i % 4];
        if enc == Encoding::CopyRect {
            let (sx, sy) = (rng.below((fw - w + 1) as u64) as usize, rng.below((fh - h + 1) as u64) as usize);
            copyrect::encode(sx as u16, sy as u16, &mut stream);
            rects.push(format!("[{x},{y},{w},{h},1]"));
            let old = fb.clone();
            for r in 0..h {
                fb[(y + r) * fw + x..(y + r) * fw + x + w]
                    .copy_from_slice(&old[(sy + r) * fw + sx..(sy + r) * fw + sx + w]);
            }
            continue;
        }
        let px = picture(&mut rng, w, h);
        for r in 0..h {
            fb[(y + r) * fw + x..(y + r) * fw + x + w].copy_from_slice(&px[r * w..(r + 1) * w]);
        }
        match enc {
            Encoding::Zrle => {
                zrle.encode(&NOVNC, w as u16, h as u16, &px, &mut stream);
                rects.push(format!("[{x},{y},{w},{h},16]"));
            }
            Encoding::Tight => {
                for (sx, sy, sw, sh) in tight::split(0, 0, w as u16, h as u16) {
                    let (sx, sy, sw, sh) = (usize::from(sx), usize::from(sy), usize::from(sw), usize::from(sh));
                    let sub: Vec<u32> = (sy..sy + sh).flat_map(|r| px[r * w + sx..r * w + sx + sw].to_vec()).collect();
                    tight_enc.encode(&NOVNC, sw as u16, sh as u16, &sub, &mut stream);
                    rects.push(format!("[{},{},{sw},{sh},7]", x + sx, y + sy));
                }
            }
            _ => {
                raw::encode(&NOVNC, &px, &mut stream);
                rects.push(format!("[{x},{y},{w},{h},0]"));
            }
        }
    }
    fs::write(dir.join("stream.bin"), &stream).unwrap();
    fs::write(dir.join("rects.json"), format!("[{}]", rects.join(","))).unwrap();
    let out = Command::new("node")
        .current_dir(&dir)
        .args(["harness.js", "stream.bin", "rects.json", "fb.bin", &fw.to_string(), &fh.to_string()])
        .output()
        .unwrap();
    assert!(out.status.success(), "noVNC harness failed:\n{}", String::from_utf8_lossy(&out.stderr));
    let got = fs::read(dir.join("fb.bin")).unwrap();
    let want: Vec<u8> = fb.iter().flat_map(|&v| [v as u8, (v >> 8) as u8, (v >> 16) as u8, 255]).collect();
    let first_bad = got.chunks(4).zip(want.chunks(4)).position(|(a, b)| a != b);
    assert!(
        first_bad.is_none(),
        "pixel {} differs: noVNC {:?}, expected {:?}",
        first_bad.unwrap(),
        &got[first_bad.unwrap() * 4..][..4],
        &want[first_bad.unwrap() * 4..][..4]
    );
    eprintln!("noVNC rebuilt {} rects ({} bytes) pixel-exact", rects.len(), stream.len());
    let _ = fs::remove_dir_all(&dir);
}

/// A missing reference tool (or one that fails to start) skips the test,
/// unless `AURORA_REQUIRE_INTEROP=1` (set in GitHub CI), where a skip would
/// hide a broken environment and the test fails instead.
fn skip(reason: std::fmt::Arguments) {
    if std::env::var_os("AURORA_REQUIRE_INTEROP").is_some_and(|v| v == "1") {
        panic!("AURORA_REQUIRE_INTEROP=1 but {reason}");
    }
    eprintln!("SKIP: {reason}");
}
