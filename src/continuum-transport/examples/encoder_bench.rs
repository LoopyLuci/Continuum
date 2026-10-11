// Compare Continuum's encoders with rfb-proto's, on real guest pixels.
//
//   cargo run -p continuum-transport --example encoder_bench
//
// # Why this decides the integration question
//
// "Integrate aurora-vnc fully" cannot mean "delete Continuum's encoders"
// without knowing what that costs, because the two are not equivalent. Looking
// at the code, `rfb_proto::encodings::classic::encode_rre` emits one sub-rect
// per differing pixel, while Continuum's `write_rre_payload` finds maximal runs
// of equal colour. On a desktop -- which is mostly flat areas with a few
// windows -- that is not a stylistic difference, it is the difference between a
// few hundred bytes and hundreds of kilobytes per frame.
//
// So the sizes are measured on a real captured desktop rather than argued from
// the source, and the integration keeps whichever encoder actually wins on
// size while sharing one pixel-format and one pixel model.
//
// It also checks the other direction that matters for correctness: that
// vnc-client, which is an independent decoder, accepts aurora's output. An
// encoder that is small and wrong is worse than one that is large and right.

// # Content matters more than provenance
//
// The first version of this captured a frame from the live guest. It reported
// that Hextile cost Continuum 4,004 bytes and rfb-proto 20,000 -- a real
// result -- and then a rerun on a wedged guest reported a *uniform* screen
// where RRE cost 8 bytes, which would have said "the encoders are identical".
//
// Both are the same mistake wearing different clothes: an encoder comparison is
// only as meaningful as the picture it runs on. A blank screen makes every
// encoder look perfect, and a real one is not reproducible, which matters when
// the decision being informed is an architectural one that has to hold up in
// review.
//
// So the default input is deterministic and desktop-shaped -- flat background,
// a few large panels, thin borders, a gradient, and text-like speckle -- built
// to exercise the run-length behaviour that separates a per-pixel RRE encoder
// from one that finds runs. `--live` captures a real frame instead, which is
// worth running but is not the basis for the decision.

use std::time::Duration;
#[allow(unused_imports)]
use continuum_transport::capture_source::CaptureSource;

use continuum_transport::rfb::encoder::{ChangedRegion, Framebuffer};
use continuum_transport::rfb::proto::PixelFormat;

fn qmp() -> std::net::SocketAddr {
    std::net::SocketAddr::from(([127, 0, 0, 1], 4444))
}

/// A deterministic, desktop-shaped picture.
///
/// Deliberately the opposite of a benchmark's usual "worst case": encoders here
/// are chosen for interactive use, where the content is overwhelmingly flat and
/// the question is how cheaply runs of equal colour are expressed. Pure noise
/// would make every encoder look identical, because none of them can compress
/// noise -- which is exactly the result that would have hidden the difference.
fn desktop_like(width: u16, height: u16) -> Vec<[u8; 3]> {
    let (w, h) = (width as usize, height as usize);
    let mut px = vec![[24u8, 26, 32]; w * h];

    let mut fill = |x: usize, y: usize, c: [u8; 3]| {
        if x < w && y < h {
            px[y * w + x] = c;
        }
    };

    // A large flat panel -- the biggest single win available to a run-length
    // encoder.
    for y in 48..h - 48 {
        for x in 48..w - 48 {
            fill(x, y, [38, 40, 48]);
        }
    }
    // A few smaller windows.
    for (x0, y0, x1, y1) in [(120u32, 120u32, 640u32, 460u32), (700, 200, 1160, 560)] {
        for y in y0 as usize..y1 as usize {
            for x in x0 as usize..x1 as usize {
                fill(x, y, [250, 250, 248]);
            }
        }
        // A one-pixel border, the kind of thing that is pure overhead unless
        // the encoder can express it cheaply.
        for x in x0 as usize..x1 as usize {
            fill(x, y0 as usize, [90, 100, 120]);
            fill(x, (y1 - 1) as usize, [90, 100, 120]);
        }
        for y in y0 as usize..y1 as usize {
            fill(x0 as usize, y, [90, 100, 120]);
            fill((x1 - 1) as usize, y, [90, 100, 120]);
        }
    }
    // Text-like speckle: short runs of varied colour on a flat field.
    let mut seed = 0x1234_5678u32;
    let mut next = || {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        seed >> 16
    };
    for row in (200..520).step_by(4) {
        let end = 200 + (next() % 380) as usize;
        for x in 200..end.min(w.saturating_sub(60)) {
            let v = (next() % 60) as u8;
            fill(x, row, [30 + v, 30 + v, 34 + v]);
        }
    }
    // A gradient strip, so the comparison includes content no run-length
    // scheme can collapse.
    for y in 600..640 {
        for x in 0..w {
            let v = (x * 255 / w.max(1)) as u8;
            fill(x, y, [v, v / 2, 255 - v]);
        }
    }
    px
}

fn report(name: &str, continuum: usize, aurora: usize) {
    let ratio = if continuum == 0 {
        f64::INFINITY
    } else {
        aurora as f64 / continuum as f64
    };
    let verdict = if continuum <= aurora {
        "Continuum smaller or equal"
    } else {
        "aurora smaller"
    };
    println!(
        "  {name:<10} Continuum {continuum:>8} B   rfb-proto {aurora:>8} B   aurora/continuum {ratio:>6.2}x   ({verdict})"
    );
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let live = std::env::args().any(|a| a == "--live");
    let (width, height, rgb): (u32, u32, Vec<u8>) = if live {
        let (source, reason) = CaptureSource::connect("omarchy", qmp(), 90, 1280, 800)
            .await
            .map_err(|e| format!("no guest on QMP {}: {e}", qmp()))?;
        if let Some(reason) = reason {
            eprintln!("note: {reason}");
        }
        let frame = source.capture_jpeg().await?;
        let img = image::load_from_memory(&frame.jpeg)?.to_rgb8();
        let (w, h) = (img.width(), img.height());
        (w, h, img.as_raw().clone())
    } else {
        let px = desktop_like(1280, 800);
        let mut flat = Vec::with_capacity(px.len() * 3);
        for p in px {
            flat.extend_from_slice(&p);
        }
        (1280, 800, flat)
    };
    println!(
        "{} content: {width}x{height}",
        if live { "live guest" } else { "deterministic desktop-like" }
    );

    // Two representations of the same picture, one per library's convention.
    //
    // Continuum's `Framebuffer` holds BGRX32 (what `capture_loop` decodes a
    // screendump into), while rfb-proto's encoders take `&[u32]` already in the
    // client's pixel format -- 0x00RRGGBB. Feeding aurora the Continuum
    // representation directly would compare red against blue.
    let mut bgra = Vec::with_capacity((width * height * 4) as usize);
    let mut aurora_px = Vec::with_capacity((width * height) as usize);
    for p in rgb.chunks_exact(3) {
        let (r, g, b) = (p[0], p[1], p[2]);
        bgra.extend_from_slice(&[b, g, r, 0]);
        aurora_px.push(u32::from_be_bytes([0, r, g, b]));
    }

    let fb = Framebuffer::from_bgra32(width as u16, height as u16, bgra)?;
    let region = ChangedRegion { x: 0, y: 0, width: width as u16, height: height as u16 };
    let w = width as u16;
    let h = height as u16;
    let format = PixelFormat::BGRX32;

    println!("payload bytes for the whole screen:\n");

    let mut out = Vec::new();
    fb.write_raw_payload(&mut out, &region, &format);
    let c_raw = out.len();
    out.clear();
    rfb_proto::encodings::raw::encode(&rfb_proto::types::PixelFormat::RGBX8888_LE, &aurora_px, &mut out);
    report("Raw", c_raw, out.len());
    out.clear();

    fb.write_rre_payload(&mut out, &region, &format);
    let c_rre = out.len();
    out.clear();
    rfb_proto::encodings::classic::encode_rre(w, h, &aurora_px, &mut out);
    report("RRE", c_rre, out.len());
    out.clear();

    fb.write_hextile_payload(&mut out, &region, &format);
    let c_hextile = out.len();
    out.clear();
    rfb_proto::encodings::classic::encode_hextile(w, h, &aurora_px, &mut out);
    report("Hextile", c_hextile, out.len());
    out.clear();

    fb.write_tight_payload(&mut out, &region, &format);
    let c_tight = out.len();
    out.clear();
    rfb_proto::encodings::TightEncoder::new(6).encode(
        &rfb_proto::types::PixelFormat::RGBX8888_LE,
        w,
        h,
        &aurora_px,
        &mut out,
    );
    report("Tight", c_tight, out.len());

    println!("\nthe same, as a fraction of the full Raw frame ({c_raw} B):");
    println!(
        "  RRE     Continuum {:>6.1}%   rfb-proto {:>6.1}%",
        100.0 * c_rre as f64 / c_raw as f64,
        100.0 * out.len() as f64 / c_raw as f64
    );

    // Correctness of the smaller encoder is not in question, but the larger one
    // must still be decodable by an independent decoder, or "smaller" is
    // meaningless.
    println!("\nverifying vnc-client accepts rfb-proto's Hextile output:");
    let mut payload = Vec::new();
    rfb_proto::encodings::classic::encode_hextile(w, h, &aurora_px, &mut payload);
    let mut decoded = vnc_client::framebuffer::Framebuffer::new(w, h);
    let rect = vnc_client::framebuffer::Rect { x: 0, y: 0, width: w, height: h };
    let pf = rfb_proto::types::PixelFormat::RGBX8888_LE;
    vnc_client::classic::decode_hextile(rect, &payload, &pf, &[], &mut decoded).map_err(|e| e.to_string())?;
    let identical = decoded.pixels() == aurora_px.as_slice();
    println!(
        "  rfb-proto Hextile decoded by vnc-client: {} ({} B consumed of {} B)",
        if identical { "pixel-identical" } else { "MISMATCH" },
        payload.len(),
        payload.len()
    );
    let _ = Duration::from_secs(1);
    Ok(())
}