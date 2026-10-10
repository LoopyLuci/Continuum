//! Replay real QEMU VNC bytes, captured from a live server, as a regression test.
//!
//! ## The gap this closes
//!
//! Every other test of Continuum's Hextile, RRE and Tight encoders checks them
//! against Continuum's own understanding of RFB, or against aurora's. Both are
//! code in this repository's orbit. Nothing checked the encoders against bytes a
//! **real server** produced.
//!
//! That is not a theoretical gap. A Hextile decoder once read the sub-encoding
//! byte as sequential ids `0,1,2,3,4` instead of the flag mask
//! `0x01,0x02,0x04,0x08,0x10`, and 261 tests passed because every fixture had
//! been generated from that decoder's own assumptions. The fixtures below were
//! captured from a running QEMU guest for exactly that reason. Provenance,
//! including how little a single capture covers, is in the `.txt` file beside
//! each `.bin`.
//!
//! ## What these tests can and cannot say
//!
//! Continuum is a server: it has rectangle *encoders* and no rectangle
//! *decoders*. `src/rfb/` contains no Hextile, RRE or Tight decode path; the only
//! decoders there parse client messages. So there is no "Continuum decoder" to
//! replay these bytes through, and the reverse direction of a cross-validation
//! is not available. [`continuum_ships_no_rectangle_decoders`] pins that
//! absence so it is a recorded fact rather than an assumption.
//!
//! What *is* available, and is what these tests do, is the direction that
//! matters: put real server content through Continuum's encoders and require
//! aurora's independent decoders to accept the result and reproduce the picture
//! exactly. That is how the RRE defect described in `encoder.rs` was found, and
//! `real_qemu_picture_round_trips_through_every_continuum_encoder` is its
//! regression test on content nobody in this repository invented.

use std::collections::BTreeMap;

use continuum_transport::rfb::encoder::{hextile, full_frame, ChangedRegion, Framebuffer};
use continuum_transport::rfb::PixelFormat as ContinuumPf;
use rfb_proto::PixelFormat as AuroraPf;
use sha2::{Digest, Sha256};
use vnc_client::classic;
use vnc_client::tight::{self, TightDecoder};
use vnc_client::{Framebuffer as AuroraFb, Rect as AuroraRect};

// ---------------------------------------------------------------------------
// Fixture format
// ---------------------------------------------------------------------------

/// One rectangle from a fixture file.
struct FixtureRect {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    payload: Vec<u8>,
}

struct Fixture {
    rects: Vec<FixtureRect>,
    width: u16,
    height: u16,
}

/// `magic "CVF1" | u16 count | count * (x,y,w,h u16; len u32; payload) | w u32 | h u32`
fn parse_fixture(bytes: &[u8]) -> Fixture {
    assert!(bytes.len() > 6, "fixture is too short to contain a header");
    assert_eq!(&bytes[..4], b"CVF1", "fixture magic");
    let count = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
    let mut at = 6usize;
    let mut rects = Vec::with_capacity(count);
    for _ in 0..count {
        let take_u16 = |at: &mut usize| {
            let v = u16::from_be_bytes([bytes[*at], bytes[*at + 1]]);
            *at += 2;
            v
        };
        let x = take_u16(&mut at);
        let y = take_u16(&mut at);
        let width = take_u16(&mut at);
        let height = take_u16(&mut at);
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        at += 4;
        let payload = bytes[at..at + len].to_vec();
        at += len;
        rects.push(FixtureRect { x, y, width, height, payload });
    }
    let width = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as u16;
    let height = u32::from_be_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as u16;
    assert_eq!(at + 8, bytes.len(), "the fixture has trailing bytes after its trailer");
    Fixture { rects, width, height }
}

const HEXTILE: &[u8] = include_bytes!("fixtures/qemu_hextile.bin");
const TIGHT: &[u8] = include_bytes!("fixtures/qemu_tight.bin");

/// The wire format both fixtures were captured in: four little-endian bytes per
/// pixel in B, G, R, x order. Continuum calls it `BGRX32`, aurora `RGBX8888_LE`.
fn aurora_pf() -> AuroraPf {
    AuroraPf::RGBX8888_LE
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A hash of a framebuffer's pixels, so the expectation can be a constant
/// instead of a wall of hex.
fn pixels_digest(pixels: &[u32]) -> String {
    let mut hasher = Sha256::new();
    for p in pixels {
        hasher.update(p.to_be_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// aurora's `0x00RRGGBB` picture as a Continuum BGRX32 framebuffer.
fn continuum_from_rgb(pixels: &[u32], width: u16, height: u16) -> Framebuffer {
    let mut fb = Framebuffer::new(width, height);
    for (dst, src) in fb
        .pixels_mut()
        .chunks_exact_mut(4)
        .zip(pixels.iter())
    {
        // 0x00RRGGBB -> [B, G, R, A]
        dst[0] = (src & 0x0000_00ff) as u8;
        dst[1] = ((src >> 8) & 0xff) as u8;
        dst[2] = ((src >> 16) & 0xff) as u8;
        dst[3] = 0xff;
    }
    fb
}

/// The region of an aurora picture, as aurora's `0x00RRGGBB`.
fn region_of(pixels: &[u32], width: u16, r: &FixtureRect) -> Vec<u32> {
    (0..r.height)
        .flat_map(|row| {
            (0..r.width).map(move |col| {
                pixels[(u32::from(r.y + row) * u32::from(width) + u32::from(r.x + col)) as usize]
            })
        })
        .collect()
}

#[track_caller]
fn assert_pixels(label: &str, got: &[u32], want: &[u32]) {
    assert_eq!(got.len(), want.len(), "{label}: pixel count differs");
    for (i, (a, b)) in got.iter().zip(want.iter()).enumerate() {
        assert_eq!(a, b, "{label}: pixel {i} is {a:06x}, expected {b:06x}");
    }
}

// ---------------------------------------------------------------------------
// The fixtures are well formed
// ---------------------------------------------------------------------------

#[test]
fn both_fixtures_describe_a_full_1280x800_screen() {
    for (name, bytes) in [("hextile", HEXTILE), ("tight", TIGHT)] {
        let fixture = parse_fixture(bytes);
        assert_eq!(
            (fixture.width, fixture.height),
            (1280, 800),
            "{name}: captured resolution"
        );
        assert!(!fixture.rects.is_empty(), "{name}: no rectangles");
        for r in &fixture.rects {
            assert!(!r.payload.is_empty(), "{name}: a rectangle has no payload");
            assert!(
                u32::from(r.x) + u32::from(r.width) <= u32::from(fixture.width)
                    && u32::from(r.y) + u32::from(r.height) <= u32::from(fixture.height),
                "{name}: rectangle {}x{} at ({},{}) falls outside the screen",
                r.width,
                r.height,
                r.x,
                r.y
            );
        }
        let payload: usize = fixture.rects.iter().map(|r| r.payload.len()).sum();
        println!("{name:8} {:>3} rectangle(s), {payload:>6} payload bytes", fixture.rects.len());
    }
}

// ---------------------------------------------------------------------------
// Hextile
// ---------------------------------------------------------------------------

/// Decode the captured Hextile with aurora, requiring exact consumption.
///
/// Neither Hextile nor Tight is length-prefixed, so "how many bytes was this
/// rectangle" is answered by walking it. If Continuum's tile-mask reading were
/// wrong, the walk would stop in the wrong place, and RFB has no
/// resynchronisation: the rest of the session would be garbage.
#[test]
fn captured_hextile_is_consumed_exactly() {
    let fixture = parse_fixture(HEXTILE);
    let pf = aurora_pf();
    let mut total = 0usize;
    for r in &fixture.rects {
        let walked = classic::hextile_len(&r.payload, &pf, r.width, r.height)
            .unwrap_or_else(|e| panic!("aurora rejected a captured Hextile rectangle: {e}"));
        assert_eq!(
            walked,
            Some(r.payload.len()),
            "aurora consumed {walked:?} of {} bytes; a short or long read means the \
             tile mask was misread",
            r.payload.len()
        );
        total += r.payload.len();
    }
    println!("hextile   {total} bytes consumed exactly, 0 leftover");
}

/// The captured screen must decode to the picture that was on the guest.
#[test]
fn captured_hextile_decodes_to_the_recorded_picture() {
    let fixture = parse_fixture(HEXTILE);
    let pf = aurora_pf();
    let mut fb = AuroraFb::new(fixture.width, fixture.height);
    for r in &fixture.rects {
        let rect = AuroraRect { x: r.x, y: r.y, width: r.width, height: r.height };
        classic::decode_hextile(rect, &r.payload, &pf, &[], &mut fb)
            .unwrap_or_else(|e| panic!("a captured Hextile rectangle did not decode: {e}"));
    }
    // The stored expectation. Captured from a live QEMU guest on the date in
    // fixtures/qemu_hextile.txt; it changes if the fixture is re-captured,
    // which is intended: the point is to pin *these* bytes.
    const EXPECTED: &str = "57430200c508ebd9bc659443754f0e1e7cec40127ce50d90c625d3898553d974";
    assert_eq!(
        pixels_digest(fb.pixels()),
        EXPECTED,
        "the captured Hextile screen decoded to a different picture"
    );
    println!("hextile   screen digest {}", pixels_digest(fb.pixels()));
}

/// Walk the captured tiles using **Continuum's** mask constants, and require
/// that this consumes exactly what aurora consumed.
///
/// This is the direct guard against the class of bug the fixture exists for: a
/// Hextile decoder reading the sub-encoding byte as something other than the
/// specification's flag mask. Continuum's own round-trip tests cannot catch a
/// wrong mask, because they would be wrong in the same way. Real bytes can.
#[test]
fn continuums_hextile_mask_reads_these_real_tiles_exactly() {
    let fixture = parse_fixture(HEXTILE);
    let pf = aurora_pf();
    let bpp = pf.bytes_per_pixel();
    let mut masks: BTreeMap<u8, usize> = BTreeMap::new();

    for r in &fixture.rects {
        let payload = &r.payload;
        let mut at = 0usize;
        let mut y = 0u16;
        while y < r.height {
            let rows = (r.height - y).min(16);
            let mut x = 0u16;
            while x < r.width {
                let cols = (r.width - x).min(16);
                let mask = payload[at];
                at += 1;
                *masks.entry(mask).or_default() += 1;

                // Continuum's constants, used exactly as `hextile::*` declares.
                assert!(
                    mask & !0b1_1111 == 0,
                    "tile mask {mask:#04x} sets a bit RFC 6143 7.7.4 does not define"
                );
                if mask & hextile::RAW != 0 {
                    // If the Raw bit is set the other bits are irrelevant.
                    at += usize::from(cols) * usize::from(rows) * bpp;
                } else {
                    if mask & hextile::BACKGROUND_SPECIFIED != 0 {
                        at += bpp;
                    }
                    if mask & hextile::FOREGROUND_SPECIFIED != 0 {
                        at += bpp;
                    }
                    if mask & hextile::ANY_SUBRECTS != 0 {
                        let n = payload[at] as usize;
                        at += 1;
                        let each = if mask & hextile::SUBRECTS_COLOURED != 0 {
                            bpp + 2
                        } else {
                            2
                        };
                        at += n * each;
                    }
                }
                x += cols;
            }
            y += rows;
        }
        assert_eq!(
            at,
            payload.len(),
            "reading the mask with Continuum's constants consumed {at} of {} bytes",
            payload.len()
        );
    }

    println!("hextile   observed tile masks:");
    for (mask, count) in &masks {
        let mut bits = Vec::new();
        for (bit, name) in [
            (hextile::RAW, "Raw"),
            (hextile::BACKGROUND_SPECIFIED, "BackgroundSpecified"),
            (hextile::FOREGROUND_SPECIFIED, "ForegroundSpecified"),
            (hextile::ANY_SUBRECTS, "AnySubrects"),
            (hextile::SUBRECTS_COLOURED, "SubrectsColoured"),
        ] {
            if mask & bit != 0 {
                bits.push(name);
            }
        }
        println!("           {mask:#04x} x{count:<5} {}", bits.join("|"));
    }

    // The capture is only worth keeping if it exercises something Continuum's
    // own encoder never produces. Continuum emits only Raw and
    // BackgroundSpecified tiles, so a fixture of nothing but those would add
    // nothing this repository could not already generate.
    let saw_any_subrects = masks.keys().any(|m| m & hextile::ANY_SUBRECTS != 0);
    let saw_coloured = masks.keys().any(|m| m & hextile::SUBRECTS_COLOURED != 0);
    let saw_foreground = masks.keys().any(|m| m & hextile::FOREGROUND_SPECIFIED != 0);
    assert!(
        saw_any_subrects || saw_coloured || saw_foreground,
        "the captured fixture no longer exercises any Hextile path Continuum's own \
         encoder cannot generate, so it has stopped being worth anything"
    );
}

// ---------------------------------------------------------------------------
// The direction that matters: real content through Continuum's encoders
// ---------------------------------------------------------------------------

/// Feed the real captured screen through every Continuum encoder and require
/// aurora's decoders to reproduce it exactly.
///
/// This is the regression test for the RRE defect. The encoder used to emit one
/// sub-rectangle per maximal run of pixels differing from the background, filled
/// with the first pixel's colour, which silently destroyed every other colour in
/// that run; it also wrote sub-rectangles as `x, y, w, h, colour` where RFC 6143
/// 7.7.3 specifies `<v, x, y, w, h>`, and used framebuffer-absolute rather than
/// rectangle-relative coordinates. None of that showed up against Continuum's
/// own decoder, which was wrong in the same three ways at once.
#[test]
fn real_qemu_picture_round_trips_through_every_continuum_encoder() {
    // Recover the real picture from the captured Hextile bytes.
    let fixture = parse_fixture(HEXTILE);
    let pf = aurora_pf();
    let mut aurora_fb = AuroraFb::new(fixture.width, fixture.height);
    for r in &fixture.rects {
        classic::decode_hextile(
            AuroraRect { x: r.x, y: r.y, width: r.width, height: r.height },
            &r.payload,
            &pf,
            &[],
            &mut aurora_fb,
        )
        .expect("the captured Hextile screen decodes");
    }
    let source = aurora_fb.pixels().to_vec();
    let fb = continuum_from_rgb(&source, fixture.width, fixture.height);

    // Regions chosen to cover the interesting cases on real content: a solid
    // block, a region with many colours, an edge-clipped region, and the whole
    // screen.
    let regions = [
        ChangedRegion { x: 0, y: 0, width: 1280, height: 800 },
        ChangedRegion { x: 0, y: 0, width: 16, height: 16 },
        ChangedRegion { x: 100, y: 40, width: 640, height: 480 },
        ChangedRegion { x: 1264, y: 784, width: 16, height: 16 },
        ChangedRegion { x: 37, y: 91, width: 501, height: 333 },
    ];

    for region in regions {
        let expected = region_of(&source, fixture.width, &FixtureRect {
            x: region.x,
            y: region.y,
            width: region.width,
            height: region.height,
            payload: Vec::new(),
        });

        // --- Hextile ---
        let mut payload = Vec::new();
        fb.write_hextile_payload(&mut payload, &region, &ContinuumPf::BGRX32);
        let walked = classic::hextile_len(&payload, &pf, region.width, region.height)
            .expect("aurora accepted Continuum's Hextile framing");
        assert_eq!(walked, Some(payload.len()), "hextile leftover for {region:?}");
        let mut out = AuroraFb::new(fixture.width, fixture.height);
        classic::decode_hextile(
            AuroraRect { x: region.x, y: region.y, width: region.width, height: region.height },
            &payload,
            &pf,
            &[],
            &mut out,
        )
        .expect("aurora decoded Continuum's Hextile");
        assert_pixels(
            &format!("hextile {region:?}"),
            &region_of(out.pixels(), out.width(), &FixtureRect {
                x: region.x,
                y: region.y,
                width: region.width,
                height: region.height,
                payload: Vec::new(),
            }),
            &expected,
        );

        // --- RRE ---
        let mut payload = Vec::new();
        fb.write_rre_payload(&mut payload, &region, &ContinuumPf::BGRX32);
        let walked = classic::rre_len(&payload, &pf).expect("aurora accepted Continuum's RRE framing");
        assert_eq!(walked, Some(payload.len()), "rre leftover for {region:?}");
        let mut out = AuroraFb::new(fixture.width, fixture.height);
        classic::decode_rre(
            AuroraRect { x: region.x, y: region.y, width: region.width, height: region.height },
            &payload,
            &pf,
            &[],
            &mut out,
        )
        .expect("aurora decoded Continuum's RRE");
        assert_pixels(
            &format!("rre {region:?}"),
            &region_of(out.pixels(), out.width(), &FixtureRect {
                x: region.x,
                y: region.y,
                width: region.width,
                height: region.height,
                payload: Vec::new(),
            }),
            &expected,
        );

        // --- Tight ---
        let mut payload = Vec::new();
        fb.write_tight_payload(&mut payload, &region, &ContinuumPf::BGRX32);
        let walked = tight::payload_len(&payload, &pf, region.width, region.height)
            .expect("aurora accepted Continuum's Tight framing");
        assert_eq!(walked, Some(payload.len()), "tight leftover for {region:?}");
        let mut out = AuroraFb::new(fixture.width, fixture.height);
        let mut decoder = TightDecoder::default();
        decoder
            .decode(
                AuroraRect { x: region.x, y: region.y, width: region.width, height: region.height },
                &payload,
                &pf,
                &[],
                &mut out,
            )
            .expect("aurora decoded Continuum's Tight");
        assert_pixels(
            &format!("tight {region:?}"),
            &region_of(out.pixels(), out.width(), &FixtureRect {
                x: region.x,
                y: region.y,
                width: region.width,
                height: region.height,
                payload: Vec::new(),
            }),
            &expected,
        );

        println!(
            "real QEMU content {region:?}: hextile {} B, rre {} B, tight {} B, all pixel-identical",
            payload.len(),
            payload.len(),
            payload.len()
        );
    }
}

/// The RRE defect specifically, on real content: a region whose non-background
/// pixels are not all one colour.
///
/// Before the fix this produced a payload that decoded without error and showed
/// the wrong picture, which is the worst possible failure mode for an encoder.
#[test]
fn real_qemu_content_keeps_every_colour_through_rre() {
    let fixture = parse_fixture(HEXTILE);
    let pf = aurora_pf();
    let mut aurora_fb = AuroraFb::new(fixture.width, fixture.height);
    for r in &fixture.rects {
        classic::decode_hextile(
            AuroraRect { x: r.x, y: r.y, width: r.width, height: r.height },
            &r.payload,
            &pf,
            &[],
            &mut aurora_fb,
        )
        .expect("the captured Hextile screen decodes");
    }
    let source = aurora_fb.pixels().to_vec();
    let fb = continuum_from_rgb(&source, fixture.width, fixture.height);

    // Count the distinct colours in a window of the real screen, so the test
    // cannot quietly become a solid-block test that RRE trivially passes.
    let region = ChangedRegion { x: 200, y: 150, width: 240, height: 180 };
    let expected = region_of(
        &source,
        fixture.width,
        &FixtureRect { x: region.x, y: region.y, width: region.width, height: region.height, payload: Vec::new() },
    );
    let mut distinct: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for p in &expected {
        distinct.insert(*p);
    }
    assert!(
        distinct.len() > 2,
        "the chosen window has only {} distinct colours, so it no longer exercises RRE",
        distinct.len()
    );

    let mut payload = Vec::new();
    fb.write_rre_payload(&mut payload, &region, &ContinuumPf::BGRX32);
    let mut out = AuroraFb::new(fixture.width, fixture.height);
    classic::decode_rre(
        AuroraRect { x: region.x, y: region.y, width: region.width, height: region.height },
        &payload,
        &pf,
        &[],
        &mut out,
    )
    .expect("aurora decoded Continuum's RRE");
    let got = region_of(
        out.pixels(),
        out.width(),
        &FixtureRect { x: region.x, y: region.y, width: region.width, height: region.height, payload: Vec::new() },
    );
    assert_pixels("rre on real multi-colour content", &got, &expected);
    println!(
        "rre       {}x{} window with {} distinct colours survived intact ({} payload bytes)",
        region.width,
        region.height,
        distinct.len(),
        payload.len()
    );
}

// ---------------------------------------------------------------------------
// Tight
// ---------------------------------------------------------------------------

#[test]
fn captured_tight_is_consumed_exactly_and_decodes_to_the_recorded_picture() {
    let fixture = parse_fixture(TIGHT);
    let pf = aurora_pf();
    let mut fb = AuroraFb::new(fixture.width, fixture.height);
    let mut decoder = TightDecoder::default();
    let mut total = 0usize;

    for r in &fixture.rects {
        let walked = tight::payload_len(&r.payload, &pf, r.width, r.height)
            .unwrap_or_else(|e| panic!("aurora rejected a captured Tight rectangle: {e}"));
        assert_eq!(
            walked,
            Some(r.payload.len()),
            "aurora framed {walked:?} of {} Tight bytes; the compact length or the \
             control byte was misread",
            r.payload.len()
        );
        decoder
            .decode(
                AuroraRect { x: r.x, y: r.y, width: r.width, height: r.height },
                &r.payload,
                &pf,
                &[],
                &mut fb,
            )
            .unwrap_or_else(|e| panic!("a captured Tight rectangle did not decode: {e}"));
        total += r.payload.len();
    }
    println!("tight     {total} bytes consumed exactly, 0 leftover");

    const EXPECTED: &str = "57430200c508ebd9bc659443754f0e1e7cec40127ce50d90c625d3898553d974";
    assert_eq!(
        pixels_digest(fb.pixels()),
        EXPECTED,
        "the captured Tight screen decoded to a different picture"
    );
    println!("tight     screen digest {}", pixels_digest(fb.pixels()));
}

// ---------------------------------------------------------------------------
// Why there is no reverse direction
// ---------------------------------------------------------------------------

/// Continuum ships no rectangle decoders, so "encode with aurora, decode with
/// Continuum" has nothing to run.
///
/// Recorded as a test rather than a comment so that adding a decoder is a
/// deliberate act: whoever adds one inherits the obligation to cross-validate
/// it against these fixtures, which is the whole point of having them.
#[test]
fn continuum_ships_no_rectangle_decoders() {
    let rfb = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("rfb");
    let mut found_decode = Vec::new();
    for entry in std::fs::read_dir(&rfb).expect("src/rfb is readable") {
        let path = entry.expect("directory entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("source is readable");
        for needle in ["fn decode_hextile", "fn decode_rre", "fn decode_tight"] {
            // Ignore the test module, which is where a fixture-replay decoder
            // would legitimately live.
            if let Some(at) = text.find(needle) {
                let before_tests = text[..at].find("#[cfg(test)]").is_none();
                if before_tests {
                    found_decode.push(format!("{}: {needle}", path.display()));
                }
            }
        }
    }
    assert!(
        found_decode.is_empty(),
        "a rectangle decoder appeared in src/rfb: {found_decode:?}. Cross-validate it \
         against tests/fixtures before trusting it."
    );
    println!("confirmed: no Hextile, RRE or Tight decoder in src/rfb");
}

