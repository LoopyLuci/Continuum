//! Encoder/decoder tests: deterministic property tests over many pixel
//! formats and image shapes, hand-computed tile bytes, and byte-exact
//! vectors (`vectors/encodings.txt`; regenerate with AURORA_BLESS=1).

use rfb_proto::encodings::zlib::Inflater;
use rfb_proto::encodings::{
    compress_level, copyrect, raw, tight, PixelCodec, TightDecoder, TightEncoder, ZrleDecoder, ZrleEncoder,
};
use rfb_proto::{Encoding, PixelFormat};

/// SplitMix64: a tiny deterministic PRNG, so property tests need no deps.
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

fn tc(bpp: u8, depth: u8, be: bool, max: [u16; 3], shift: [u8; 3]) -> PixelFormat {
    PixelFormat {
        bits_per_pixel: bpp,
        depth,
        big_endian: be,
        true_colour: true,
        red_max: max[0],
        green_max: max[1],
        blue_max: max[2],
        red_shift: shift[0],
        green_shift: shift[1],
        blue_shift: shift[2],
    }
}

fn formats() -> Vec<(&'static str, PixelFormat)> {
    vec![
        ("rgbx8888le", PixelFormat::RGBX8888_LE),
        ("rgbx8888be", tc(32, 24, true, [255; 3], [16, 8, 0])),
        ("xbgr8888le-novnc", tc(32, 24, false, [255; 3], [0, 8, 16])),
        ("rgbx8888le-high", tc(32, 24, false, [255; 3], [24, 16, 8])),
        ("rgbx8888be-high", tc(32, 24, true, [255; 3], [24, 16, 8])),
        ("r10g10b10", tc(32, 30, false, [1023; 3], [20, 10, 0])),
        ("rgb565le", tc(16, 16, false, [31, 63, 31], [11, 5, 0])),
        ("rgb565be", tc(16, 16, true, [31, 63, 31], [11, 5, 0])),
        ("bgr233", tc(8, 8, false, [7, 7, 3], [0, 3, 6])),
        ("colourmap8", PixelFormat { true_colour: false, ..tc(8, 8, false, [0; 3], [0; 3]) }),
    ]
}

fn random_value(rng: &mut Rng, pf: &PixelFormat) -> u32 {
    if !pf.true_colour {
        return rng.below(1 << pf.bits_per_pixel) as u32;
    }
    let ch = |rng: &mut Rng, max: u16, shift: u8| (rng.below(u64::from(max) + 1) as u32) << shift;
    ch(rng, pf.red_max, pf.red_shift) | ch(rng, pf.green_max, pf.green_shift) | ch(rng, pf.blue_max, pf.blue_shift)
}

/// Images that hit every subencoding and filter: solid, few colours, runs,
/// many colours, noise, and mixtures.
fn image(rng: &mut Rng, pf: &PixelFormat, w: usize, h: usize, kind: u64) -> Vec<u32> {
    let n = w * h;
    let colours: Vec<u32> =
        (0..[1, 2, 3, 5, 16, 17, 127, 200, 300, 4096][rng.below(10) as usize]).map(|_| random_value(rng, pf)).collect();
    match kind % 5 {
        0 => vec![colours[0]; n],
        1 => (0..n).map(|_| colours[rng.below(colours.len() as u64) as usize]).collect(),
        2 => {
            let mut px = Vec::with_capacity(n);
            while px.len() < n {
                let v = colours[rng.below(colours.len() as u64) as usize];
                let len = [1, 2, 7, 255, 256, 600][rng.below(6) as usize];
                px.extend(std::iter::repeat_n(v, len.min(n - px.len())));
            }
            px
        }
        3 => (0..n).map(|_| random_value(rng, pf)).collect(),
        _ => {
            // Half text-like (2 colours), half noise.
            (0..n)
                .map(
                    |i| {
                        if (i % w) < w / 2 {
                            colours[(i * 7 / 3) % colours.len().min(2)]
                        } else {
                            random_value(rng, pf)
                        }
                    },
                )
                .collect()
        }
    }
}

#[test]
fn zrle_round_trips_every_format_shape_and_level() {
    let mut rng = Rng(1);
    for (name, pf) in formats() {
        let mut enc = ZrleEncoder::new((rng.below(10)) as u8);
        let mut dec = ZrleDecoder::new();
        for case in 0..40u64 {
            let (w, h) = [(1, 1), (64, 64), (65, 63), (130, 70), (3, 200), (200, 3), (128, 128)][(case % 7) as usize];
            let px = image(&mut rng, &pf, w, h, case);
            let mut out = Vec::new();
            enc.encode(&pf, w as u16, h as u16, &px, &mut out);
            // A truncated payload is Incomplete and changes nothing...
            assert!(matches!(
                dec.decode(&pf, w as u16, h as u16, &out[..out.len() - 1]),
                Err(rfb_proto::DecodeError::Incomplete { .. })
            ));
            // ...so the full one still decodes on the same stream.
            let (back, used) =
                dec.decode(&pf, w as u16, h as u16, &out).unwrap_or_else(|e| panic!("{name} case {case}: {e}"));
            assert_eq!(used, out.len());
            assert!(back == px, "{name} case {case} {w}x{h}");
        }
    }
}

#[test]
fn tight_round_trips_every_format_shape_and_level() {
    let mut rng = Rng(2);
    for (name, pf) in formats() {
        let mut enc = TightEncoder::new(rng.below(10) as u8);
        let mut dec = TightDecoder::new();
        for case in 0..40u64 {
            let (w, h) = [(1, 1), (2, 2), (64, 64), (9, 3), (130, 70), (2048, 2), (256, 256)][(case % 7) as usize];
            let px = image(&mut rng, &pf, w, h, case);
            let mut out = Vec::new();
            enc.encode(&pf, w as u16, h as u16, &px, &mut out);
            assert!(matches!(
                dec.decode(&pf, w as u16, h as u16, &out[..out.len() - 1]),
                Err(rfb_proto::DecodeError::Incomplete { .. })
            ));
            let (back, used) =
                dec.decode(&pf, w as u16, h as u16, &out).unwrap_or_else(|e| panic!("{name} case {case}: {e}"));
            assert_eq!(used, out.len());
            assert!(back == px, "{name} case {case} {w}x{h}");
        }
    }
}

#[test]
fn raw_and_copyrect_round_trip() {
    let mut rng = Rng(3);
    for (_, pf) in formats() {
        let px = image(&mut rng, &pf, 17, 5, 3);
        let mut out = Vec::new();
        raw::encode(&pf, &px, &mut out);
        assert_eq!(out.len(), 17 * 5 * pf.bytes_per_pixel());
        assert_eq!(raw::decode(&pf, 17, 5, &out).unwrap(), (px, out.len()));
    }
    let mut out = Vec::new();
    copyrect::encode(0x0102, 0x0304, &mut out);
    assert_eq!(out, [1, 2, 3, 4]);
    assert_eq!(copyrect::decode(&out).unwrap(), ((0x0102, 0x0304), 4));
}

#[test]
fn tight_split_covers_rectangles_within_limits() {
    let mut rng = Rng(4);
    for _ in 0..200 {
        let (x, y) = (rng.below(1000) as u16, rng.below(1000) as u16);
        let (w, h) = (1 + rng.below(6000) as u16, 1 + rng.below(3000) as u16);
        let parts = tight::split(x, y, w, h);
        let mut area = 0usize;
        for &(px, py, pw, ph) in &parts {
            assert!(pw <= tight::MAX_WIDTH && usize::from(pw) * usize::from(ph) <= tight::MAX_PIXELS);
            assert!(px >= x && py >= y && px + pw <= x + w && py + ph <= y + h);
            area += usize::from(pw) * usize::from(ph);
        }
        assert_eq!(area, usize::from(w) * usize::from(h)); // in bounds + same area = exact cover (bands don't overlap)
    }
}

#[test]
fn cpixel_and_tpixel_forms_follow_the_specs() {
    let f = formats();
    let c = |i: usize| PixelCodec::new(&f[i].1);
    let mut out = Vec::new();
    // RGBX LE: CPIXEL = 3 low bytes little-endian; TPIXEL = R, G, B.
    c(0).put_cpixel(0x00a1b2c3, &mut out);
    c(0).put_tpixel(0x00a1b2c3, &mut out);
    assert_eq!(out, [0xc3, 0xb2, 0xa1, 0xa1, 0xb2, 0xc3]);
    out.clear();
    // Big-endian, low bytes: CPIXEL is the low three bytes in big-endian order.
    c(1).put_cpixel(0x00a1b2c3, &mut out);
    assert_eq!(out, [0xa1, 0xb2, 0xc3]);
    out.clear();
    // Channels in the high bytes: CPIXEL is the high three bytes.
    c(3).put_cpixel(0xa1b2c300, &mut out);
    c(4).put_cpixel(0xa1b2c300, &mut out);
    assert_eq!(out, [0xc3, 0xb2, 0xa1, 0xa1, 0xb2, 0xc3]);
    // Depth 30 and 16/8 bpp keep the full PIXEL.
    assert_eq!([c(5).cpixel_len(), c(6).cpixel_len(), c(8).cpixel_len(), c(9).cpixel_len()], [4, 2, 1, 1]);
    assert_eq!([c(2).tpixel_len(), c(5).tpixel_len(), c(6).tpixel_len()], [3, 4, 2]);
    // Depth 32, or channels straddling both ends, keep the full 4 bytes.
    assert_eq!(PixelCodec::new(&tc(32, 32, false, [255; 3], [16, 8, 0])).cpixel_len(), 4);
    assert_eq!(PixelCodec::new(&tc(32, 24, false, [255, 255, 255], [24, 8, 0])).cpixel_len(), 4);
    // Top-byte layouts: the colour bytes are kept, the empty low byte dropped,
    // in the format's byte order (red survives).
    let mut out = Vec::new();
    PixelCodec::new(&tc(32, 24, false, [255; 3], [8, 16, 24])).put_cpixel(0x00_00_00_ff << 8, &mut out);
    assert_eq!(out, [0xff, 0, 0]);
    // Hostile shifts must not panic and fall back to the full form.
    let odd = PixelCodec::new(&tc(32, 24, false, [255; 3], [200, 8, 0]));
    assert_eq!((odd.cpixel_len(), odd.tpixel_len()), (4, 4));
}

#[test]
fn compress_level_pseudo_encodings() {
    assert_eq!(compress_level(&[Encoding::Raw]), None);
    assert_eq!(compress_level(&[Encoding::Other(-256)]), Some(0));
    assert_eq!(compress_level(&[Encoding::Other(-250), Encoding::Zrle, Encoding::Other(-247)]), Some(9));
}

fn inflate_payload(zlib_with_len: &[u8]) -> Vec<u8> {
    let len = u32::from_be_bytes(zlib_with_len[..4].try_into().unwrap()) as usize;
    assert_eq!(len, zlib_with_len.len() - 4);
    Inflater::new().inflate(&zlib_with_len[4..], 1 << 20).unwrap()
}

/// The uncompressed ZRLE tile stream, written out by hand from RFC 6143.
#[test]
fn zrle_tile_bytes_match_the_rfc_by_hand() {
    let pf = PixelFormat::RGBX8888_LE;
    let (a, b, c) = (0x00ff0000u32, 0x0000ff00u32, 0x000000ffu32);
    let tile = |w: u16, h: u16, px: &[u32]| {
        let mut out = Vec::new();
        ZrleEncoder::new(0).encode(&pf, w, h, px, &mut out);
        inflate_payload(&out)
    };
    // Solid: subencoding 1, one CPIXEL (3 bytes, little-endian).
    assert_eq!(tile(4, 2, &[a; 8]), [1, 0x00, 0x00, 0xff]);
    // Two colours, 8x2: packed palette (2), palette, 1 bit per pixel MSB first.
    let px = [a, b, a, b, a, b, a, b, b, b, b, b, a, a, a, a];
    assert_eq!(tile(8, 2, &px), [2, 0, 0, 0xff, 0, 0xff, 0, 0b0101_0101, 0b1111_0000]);
    // Three colours, 4x1: 2 bits per pixel (10 bytes beats raw's 12).
    assert_eq!(tile(4, 1, &[a, b, c, a]), [3, 0, 0, 0xff, 0, 0xff, 0, 0xff, 0, 0, 0b0001_1000]);
    // ...but for 3 pixels raw (9 bytes) is smaller than the palette (10).
    assert_eq!(tile(3, 1, &[a, b, c]), [0, 0, 0, 0xff, 0, 0xff, 0, 0xff, 0, 0]);
    // Two long runs, 64x64: plain RLE (128) is smallest: CPIXEL then
    // (length-1) as 255-continuation bytes (999 = 3*255 + 234).
    let mut px = vec![a; 1000];
    px.extend(vec![b; 4096 - 1000]);
    let mut want = vec![128, 0, 0, 0xff, 255, 255, 255, 234, 0, 0xff, 0];
    want.extend([255; 12]);
    want.push(35); // 3095 = 12*255 + 35
    assert_eq!(tile(64, 64, &px), want);
    // 17 colours (too many to pack) in runs of 2: palette RLE (128 + 17),
    // each run as index|128 then (length-1).
    let px: Vec<u32> = (0..128).map(|i| (i / 2) % 17).collect();
    let mut want = vec![128 + 17];
    (0..17u8).for_each(|i| want.extend([i, 0, 0]));
    (0..64u8).for_each(|r| want.extend([(r % 17) | 0x80, 1]));
    assert_eq!(tile(64, 2, &px), want);
    // Two tiles: a 65x1 rectangle is a 64x1 tile then a 1x1 tile.
    let mut px = vec![c; 64];
    px.push(a);
    assert_eq!(tile(65, 1, &px), [1, 0xff, 0, 0, 1, 0, 0, 0xff]);
}

/// Tight payloads written out by hand from the rfbproto Tight description.
#[test]
fn tight_bytes_match_the_spec_by_hand() {
    let pf = PixelFormat::RGBX8888_LE;
    let (a, b) = (0x00ff0000u32, 0x0000ff00u32);
    let enc = |w: u16, h: u16, px: &[u32]| {
        let mut out = Vec::new();
        TightEncoder::new(0).encode(&pf, w, h, px, &mut out);
        out
    };
    // Fill: 0x80 then TPIXEL (R, G, B).
    assert_eq!(enc(5, 5, &[a; 25]), [0x80, 0xff, 0, 0]);
    // Two colours, 4x2: stream 1 + explicit filter (0x50), palette filter,
    // 2 colours, then 2 data bytes (< 12, so no zlib).
    assert_eq!(enc(4, 2, &[a, b, a, b, b, b, a, a]), [0x50, 1, 1, 0xff, 0, 0, 0, 0xff, 0, 0b0101_0000, 0b1100_0000]);
    // 2x1 with two colours is still a mono palette.
    assert_eq!(enc(2, 1, &[a, b]), [0x50, 1, 1, 0xff, 0, 0, 0, 0xff, 0, 0b0100_0000]);
    // Three colours in 3 pixels: full colour, copy filter implied (0x00),
    // 9 bytes of TPIXELs (< 12, so no zlib).
    let c = 0x000000ffu32;
    assert_eq!(enc(3, 1, &[a, b, c]), [0x00, 0xff, 0, 0, 0, 0xff, 0, 0, 0, 0xff]);
}

// ---- byte-exact vectors -------------------------------------------------

fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

fn vector_cases() -> Vec<(String, String)> {
    let mut rng = Rng(42);
    let mut cases = Vec::new();
    let pick = [0usize, 1, 3, 6, 9];
    let f = formats();
    for &fi in &pick {
        let (fname, pf) = (f[fi].0, f[fi].1);
        for (kind, (w, h)) in [(0u64, (8, 8)), (1, (16, 4)), (2, (66, 3)), (3, (5, 4)), (4, (12, 6))] {
            let px = image(&mut rng, &pf, w, h, kind);
            let pixels: Vec<String> = px.iter().map(|v| format!("{v:x}")).collect();
            let mut z = Vec::new();
            ZrleEncoder::new(0).encode(&pf, w as u16, h as u16, &px, &mut z);
            let mut t = Vec::new();
            for (x, y, cw, ch) in tight::split(0, 0, w as u16, h as u16) {
                let sub: Vec<u32> = (y..y + ch)
                    .flat_map(|yy| (x..x + cw).map(move |xx| (xx, yy)))
                    .map(|(xx, yy)| px[usize::from(yy) * w + usize::from(xx)])
                    .collect();
                TightEncoder::new(0).encode(&pf, cw, ch, &sub, &mut t);
            }
            let mut r = Vec::new();
            raw::encode(&pf, &px, &mut r);
            let name = format!("{fname}-kind{kind}-{w}x{h}");
            let body = format!(
                "format = {pf:?}\nsize = {w}x{h}\npixels = {}\nraw = {}\nzrle-level0 = {}\ntight-level0 = {}\n",
                pixels.join(" "),
                hex(&r),
                hex(&z),
                hex(&t)
            );
            cases.push((name, body));
        }
    }
    cases
}

#[test]
fn byte_exact_vectors() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vectors/encodings.txt");
    let mut text = String::from(
        "# Byte-exact encoder vectors. Pixel values are in the stated format (hex).\n\
         # zrle-level0 is the full ZRLE payload (u32 length + zlib stream at level 0,\n\
         # i.e. stored blocks, one sync flush); tight-level0 is the Tight payload(s)\n\
         # with fresh streams at level 0. Regenerate: AURORA_BLESS=1 cargo test -p rfb-proto.\n",
    );
    for (name, body) in vector_cases() {
        text.push_str(&format!("\n[{name}]\n{body}"));
    }
    if std::env::var_os("AURORA_BLESS").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    let want = std::fs::read_to_string(&path).expect("vectors/encodings.txt (run with AURORA_BLESS=1 to create)");
    assert!(want == text, "encoder output changed; inspect and re-bless vectors/encodings.txt");
}
