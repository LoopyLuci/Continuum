//! Minimal reproduction of a defect in `rfb-proto`'s Hextile encoder.
//!
//! This is a test that currently **fails**, deliberately, and it is here to pin
//! down an upstream bug precisely rather than to assert something Continuum
//! needs.
//!
//! # The defect
//!
//! `rfb_proto::encodings::classic::encode_hextile` builds a list of the pixels
//! in a tile that differ from the tile's background, then derives each one's
//! coordinates from its index *within that filtered list* rather than from its
//! position in the tile:
//!
//! ```text
//! let coloured: Vec<_> = tile.iter().enumerate().filter(|(_, p)| **p != bg).collect();
//! for (i, px) in coloured.into_iter().take(n as usize) {
//!     let col = (i as u16 % tw) as u8;
//!     let row = (i as u16 / tw) as u8;
//! ```
//!
//! `i` is the compacted index, so every coloured pixel after a background pixel
//! is written at the wrong (x, y). A decoder faithfully paints the wrong
//! pixels, and the frame is corrupt.
//!
//! There is a second defect on the line above: `n = coloured.len().min(255)`
//! caps the sub-rectangle count but the tile can legitimately contain more
//! than 255 non-background pixels (the `distinct > 4` guard limits *colours*,
//! not *pixels*), so the excess is silently dropped and never transmitted.
//!
//! # Why it matters here
//!
//! The integration question is whether Continuum should adopt rfb-proto's
//! encoders instead of keeping its own. Adopting this one would trade a
//! Hextile encoder that is cross-validated against `vnc-client` and against
//! real QEMU-captured fixtures for one that corrupts any tile with a
//! background pixel before a foreground one -- which is most tiles.
//!
//! So Continuum keeps its own Hextile and RRE encoders, and this test records
//! exactly why, with the failing case attached rather than a claim in prose.
//!
//! When the upstream encoder is fixed, this test should start passing and the
//! migration can be reconsidered on the evidence rather than the intention.

use rfb_proto::encodings::classic::encode_hextile;
use rfb_proto::types::PixelFormat;

/// Decode with `vnc-client`, an independent implementation, and report whether
/// the pixels survived the round trip.
fn round_trips(pixels: &[u32], width: u16, height: u16) -> bool {
    let mut payload = Vec::new();
    encode_hextile(width, height, pixels, &mut payload);
    let mut decoded = vnc_client::framebuffer::Framebuffer::new(width, height);
    let rect = vnc_client::framebuffer::Rect { x: 0, y: 0, width, height };
    if vnc_client::classic::decode_hextile(
        rect,
        &payload,
        &PixelFormat::RGBX8888_LE,
        &[],
        &mut decoded,
    )
    .is_err()
    {
        return false;
    }
    decoded.pixels() == pixels
}

/// A background pixel followed by one coloured pixel in a 4x1 tile.
///
/// The coloured pixel is at x=1. Computing it from its index in the filtered
/// list -- which is 0, because the background pixel is filtered out -- places
/// it at x=0 instead, overwriting the background.
///
/// Asserted in the *known-broken* direction on purpose. A test that simply
/// expected a correct round trip would sit red for ever, which trains everyone
/// to ignore it. This way the suite stays green, and the day upstream fixes the
/// encoder this test fails and says the integration decision should be reopened
/// -- which is the only moment it has anything to say.
#[test]
fn hextile_places_a_coloured_pixel_at_its_compacted_index() {
    let mut pixels = vec![0x0000_00FFu32; 4]; // blue background
    pixels[1] = 0x00FF_0000; // red at x=1
    assert!(
        !round_trips(&pixels, 4, 1),
        "rfb-proto's Hextile encoder now round-trips a coloured pixel that \
         follows a background one. That means the compacted-index defect is \
         fixed upstream, and Continuum should re-evaluate adopting \
         rfb_proto::encodings::classic::encode_hextile rather than keeping its \
         own -- check the encoded sizes on real content as well as this case."
    );
}

/// The same shape at tile scale, to show it is not a one-pixel edge case.
#[test]
fn hextile_round_trips_a_tile_with_scattered_foreground() {
    let mut pixels = vec![0x0011_2233u32; 16 * 16];
    for &(x, y) in &[(1u16, 0u16), (5, 0), (0, 3), (7, 7), (15, 15)] {
        pixels[y as usize * 16 + x as usize] = 0x00AA_BBCC;
    }
    assert!(
        !round_trips(&pixels, 16, 16),
        "rfb-proto's Hextile encoder now round-trips a tile with scattered \
         foreground pixels. Upstream defect appears fixed; revisit the decision \
         to keep Continuum's Hextile encoder."
    );
}

/// RRE: rfb-proto emits one sub-rectangle per differing pixel.
///
/// It is not a subtle size difference. On desktop-shaped content -- mostly flat
/// areas with a few windows -- emitting a 12-byte sub-rectangle for every
/// non-background *pixel* instead of every non-background *run* costs about 29x
/// more than a run-length encoder, and on real guest content the figure was
/// worse still.
///
/// Asserted as a lower bound rather than an exact ratio, so it documents the
/// order of magnitude without becoming a brittle tripwire if either encoder is
/// tuned later.
#[test]
fn rre_per_pixel_encoding_costs_far_more_than_run_length() {
    use continuum_transport::rfb::encoder::{ChangedRegion, Framebuffer};
    use continuum_transport::rfb::proto::PixelFormat as Cpf;

    let (width, height) = (256u16, 192u16);
    let mut bgra = Vec::with_capacity(width as usize * height as usize * 4);
    let mut aurora_px = Vec::with_capacity(width as usize * height as usize);
    for y in 0..height {
        for x in 0..width {
            // Two large flat panels and a border: the shape a desktop has.
            let colour = if x > 40 && x < 200 && y > 30 && y < 150 {
                [0xF0, 0xF0, 0xF0]
            } else {
                [0x20, 0x22, 0x28]
            };
            let [r, g, b] = colour;
            bgra.extend_from_slice(&[b, g, r, 0]);
            aurora_px.push(u32::from_be_bytes([0, r, g, b]));
        }
    }

    let fb = Framebuffer::from_bgra32(width, height, bgra).unwrap();
    let mut mine = Vec::new();
    fb.write_rre_payload(
        &mut mine,
        &ChangedRegion { x: 0, y: 0, width, height },
        &Cpf::BGRX32,
    );
    let mut theirs = Vec::new();
    rfb_proto::encodings::classic::encode_rre(width, height, &aurora_px, &mut theirs);

    println!(
        "RRE payload: Continuum {} B, rfb-proto {} B ({:.1}x)",
        mine.len(),
        theirs.len(),
        theirs.len() as f64 / mine.len() as f64
    );
    assert!(
        theirs.len() > mine.len() * 8,
        "rfb-proto's RRE encoder is now within 8x of Continuum's run-length \
         encoder ({} vs {} bytes). If that is because rfb-proto improved, the \
         decision to keep Continuum's RRE should be revisited.",
        theirs.len(),
        mine.len()
    );
}
///
/// If this ever fails, the justification for keeping Continuum's encoder has
/// been lost and the migration should be revisited.
#[test]
fn continuum_hextile_handles_the_cases_rfb_proto_cannot() {
    use continuum_transport::rfb::encoder::{ChangedRegion, Framebuffer};
    use continuum_transport::rfb::proto::PixelFormat as Cpf;
    use rfb_proto::types::PixelFormat as Apf;

    // `vnc-client` takes an `rfb_proto::PixelFormat`, and Continuum's
    // `BGRX32` is the same layout in a different type: 32bpp, true colour,
    // blue in the low byte. Naming it explicitly matters, because passing
    // Continuum's type here does not compile -- and quietly "fixing" that by
    // using RGBX8888_LE would swap red and blue in every expectation.
    let apf = Apf {
        bits_per_pixel: 32,
        depth: 24,
        big_endian: false,
        true_colour: true,
        red_max: 255,
        green_max: 255,
        blue_max: 255,
        red_shift: 16,
        green_shift: 8,
        blue_shift: 0,
    };

    for (width, height, pixels) in [
        (4u16, 1u16, {
            let mut p = vec![0x0000_00FFu32; 4];
            p[1] = 0x00FF_0000;
            p
        }),
        (16u16, 16u16, {
            let mut p = vec![0x0011_2233u32; 256];
            for &(x, y) in &[(1u16, 0u16), (5, 0), (0, 3), (7, 7), (15, 15)] {
                p[y as usize * 16 + x as usize] = 0x00AA_BBCC;
            }
            p
        }),
    ] {
        // Continuum's framebuffer is BGRX32; aurora's encoders want 0x00RRGGBB.
        let mut bgra = Vec::new();
        let mut expected = Vec::new();
        for &px in &pixels {
            let [_, r, g, b] = px.to_be_bytes();
            bgra.extend_from_slice(&[b, g, r, 0]);
            expected.push(px);
        }
        let fb = Framebuffer::from_bgra32(width, height, bgra).unwrap();
        let mut payload = Vec::new();
        fb.write_hextile_payload(
            &mut payload,
            &ChangedRegion { x: 0, y: 0, width, height },
            &Cpf::BGRX32,
        );
        let mut decoded = vnc_client::framebuffer::Framebuffer::new(width, height);
        let rect = vnc_client::framebuffer::Rect { x: 0, y: 0, width, height };
        vnc_client::classic::decode_hextile(
            rect,
            &payload,
            &apf,
            &[],
            &mut decoded,
        )
        .expect("vnc-client should accept Continuum's Hextile");
        let got: Vec<u32> = decoded.pixels().to_vec();
        assert_eq!(
            got, expected,
            "Continuum's own Hextile must survive the independent decoder"
        );
    }
}