//! Framebuffer, damage tracking, and the rectangle encoders.
//!
//! ## Damage tracking is the whole point
//!
//! A [`Framebuffer`] is the server's picture of the guest. Before each capture
//! it is compared with the new frame by [`Framebuffer::damage`], which returns
//! only the rectangles whose pixels actually changed. Those rectangles are what
//! goes on the wire. A 1920x1080 Raw send is 8.3 MB; the damage from one
//! character typed into a terminal is a few kilobytes.
//!
//! [`Framebuffer::damage`] coalesces changed row spans vertically when they line
//! up, so the common case — a cursor, a caret, a status bar — comes back as
//! exactly one rectangle covering the changed region.
//!
//! [`Framebuffer::detect_scroll`] additionally recognises that the screen
//! *moved* rather than changed. That is the one case where re-sending pixels is
//! pure waste: a scrolled frame becomes a `CopyRect` header plus only the newly
//! exposed strip.
//!
//! ## Encodings implemented
//!
//! | Encoding    | Code  | What it is used for here |
//! |-------------|-------|--------------------------|
//! | Raw         | 0     | The correctness floor. Always permitted. |
//! | CopyRect    | 1     | Scrolling: sixteen bytes for a whole screen. |
//! | RRE         | 2     | A solid region: eight bytes for a whole screen. |
//! | Hextile     | 5     | UI-scale damage. A flat 16x16 tile is one byte. |
//! | Tight       | 7     | zlib over `TPIXEL`s. The best general-purpose ratio. |
//! | DesktopSize | -223  | Announcing a resolution change. |
//!
//! ### What "Tight" means here, precisely
//!
//! Tight is implemented as **BasicCompression with the CopyFilter**: control
//! byte `0x01` (compression type 0, stream 0, no filter-id byte, reset stream
//! 0), then a one-to-three byte compact length, then a complete zlib stream.
//! Three details are load-bearing and all three are easy to get wrong:
//!
//! * **The compact length is one to three bytes, not one to five.** The
//!   encoding is `0xxxxxxx` for 0..127, `1xxxxxxx 0yyyyyyy` for 128..16383 and
//!   `1xxxxxxx 1yyyyyyy zzzzzzzz` for 16384..4194303, with the low bits first.
//!   There is no five-byte form, so a 4K rectangle cannot be length-prefixed
//!   the way a five-byte form would allow.
//! * **`TPIXEL` is three bytes for 32bpp/24-depth pixels, in R, G, B order**,
//!   not four bytes in B, G, R, x order like a `PIXEL`. Tight drops the padding
//!   byte *and* reorders. Sending `PIXEL`s here is the single most common way to
//!   get a Tight client to render blue and orange.
//! * **The rectangle may not be wider than 2048 pixels.** The specification
//!   reduces implementation complexity by forbidding it, so a wide rectangle is
//!   split by [`split_width`] before encoding rather than sent whole.
//!
//! Bit 7 of the control byte (FillCompression) and the JPEG and PNG compression
//! types are **not** implemented: FillCompression's framing is described only
//! loosely in the specification and disagrees between implementations, and a
//! client that mis-parses it loses stream synchronisation rather than showing a
//! glitch.
//!
//! ## Encodings deliberately *not* implemented
//!
//! * **ZRLE (16), Zlib (6), zlibhex (8), CoRRE (4), TRLE (15), JRLE (22) and the
//!   rest.** ZRLE would win over Tight on photographic content, but it wraps zlib
//!   in a pixel-run-length format where a subtle error corrupts a rectangle in a
//!   way that reads as a rendering bug rather than a protocol error. Tight with
//!   zlib already captures most of the win on desktop UI content, which is what
//!   a guest screen actually is.
//! * **Tight's PaletteFilter and GradientFilter.** These are where Tight's
//!   remaining win lives, and both are bit-level packing formats with no
//!   self-evident correctness check. The CopyFilter is the documented default
//!   (`bit 6 clear` means CopyFilter, per the specification) and is what is
//!   implemented.
//! * **Every pseudo-encoding**, including Cursor, ExtendedDesktopSize and the
//!   JPEG-quality-level hints. A client that advertises them simply never
//!   receives them, which is legal: [`Encoding::from_code`] classifies unknown
//!   codes so that a client advertising twenty pseudo-encodings is not
//!   disconnected for it.
//!
//! ## The one thing Hextile gets wrong if you are careless
//!
//! A Hextile tile's sub-encoding byte is a *mask*, and bit 2
//! (`BackgroundSpecified`) means "a pixel value follows, setting this tile's
//! background" — not "this tile is the background". A solid tile is encoded as
//! that bit plus the pixel, with `AnySubrects` clear meaning there are no
//! sub-rectangles and therefore the whole tile is that one colour. The
//! background may only be carried over from the previous tile if that tile was
//! *not* raw, so [`Framebuffer::write_hextile_payload`] tracks that explicitly.

use std::io::Write;

use flate2::write::ZlibEncoder;
use flate2::Compression;

use crate::rfb::proto::{CopyRect, PixelFormat, Rect};

/// Raw encoding.
pub const ENCODING_RAW: i32 = 0;
/// CopyRect encoding.
pub const ENCODING_COPY_RECT: i32 = 1;
/// RRE encoding.
pub const ENCODING_RRE: i32 = 2;
/// Hextile encoding.
pub const ENCODING_HEXTILE: i32 = 5;
/// Tight encoding.
pub const ENCODING_TIGHT: i32 = 7;
/// DesktopSize pseudo-encoding.
pub const ENCODING_DESKTOP_SIZE: i32 = -223;

/// Hextile tile edge, in pixels. Fixed by the specification at 16.
pub const MAX_HEXTILE_TILE: u32 = 16;

/// Widest rectangle the Tight specification permits.
pub const MAX_TIGHT_RECT_WIDTH: u32 = 2048;

/// Most rectangles in one `FramebufferUpdate` before another message is needed.
pub const MAX_RECTS_PER_UPDATE: usize = 0xFFFF;

/// Hextile sub-encoding mask bits.
pub mod hextile {
    /// The tile is raw `width * height` pixels.
    pub const RAW: u8 = 1;
    /// A pixel value follows, setting this tile's background colour.
    pub const BACKGROUND_SPECIFIED: u8 = 2;
    /// A pixel value follows, setting the foreground for every sub-rectangle.
    pub const FOREGROUND_SPECIFIED: u8 = 4;
    /// A sub-rectangle count byte follows.
    pub const ANY_SUBRECTS: u8 = 8;
    /// Each sub-rectangle is preceded by its own pixel value.
    pub const SUBRECTS_COLOURED: u8 = 16;
}

/// A rectangle of changed pixels.
///
/// Deliberately encoding-agnostic: damage is computed once and the encoder
/// decides per rectangle how best to express it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangedRegion {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl ChangedRegion {
    /// The same rectangle moved to a new origin, for tests that need to describe
    /// a sub-rectangle of a frame.
    pub fn with_origin(&self, x: u16, y: u16) -> ChangedRegion {
        ChangedRegion {
            x,
            y,
            ..*self
        }
    }

    fn to_rect(self, encoding: i32) -> Rect {
        Rect {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            encoding,
        }
    }

    pub fn pixel_count(&self) -> u64 {
        self.width as u64 * self.height as u64
    }

    pub fn bounds(&self) -> (u16, u16, u16, u16) {
        (self.x, self.y, self.width, self.height)
    }

    fn right(&self) -> u32 {
        u32::from(self.x) + u32::from(self.width)
    }

    fn bottom(&self) -> u32 {
        u32::from(self.y) + u32::from(self.height)
    }

    /// Whether `other` lies entirely inside this rectangle.
    pub fn contains(&self, other: &ChangedRegion) -> bool {
        u32::from(self.x) <= u32::from(other.x)
            && u32::from(self.y) <= u32::from(other.y)
            && self.right() >= other.right()
            && self.bottom() >= other.bottom()
    }
}

/// A detected scroll: this frame is the previous frame shifted by these pixels.
///
/// The convention, which [`Framebuffer::scroll`] and `CopyRect` both follow, is
/// `after(x, y) == before(x + dx, y + dy)`. It is the one `CopyRect` encodes
/// directly: copying source `(dx, dy)` to destination `(0, 0)` gives exactly
/// that, which is why only non-negative offsets are representable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scroll {
    pub dx: i16,
    pub dy: i16,
}

/// A rectangle encoding this server can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Encoding {
    Raw,
    CopyRect,
    Rre,
    Hextile,
    Tight,
    /// Recognised so that a client advertising it is understood, never emitted.
    DesktopSize,
    /// A negative code this server does not implement.
    UnsupportedPseudo(i32),
    /// A positive code this server does not implement.
    Unsupported(i32),
}

impl Encoding {
    /// Classify a code a client advertised in `SetEncodings`.
    pub fn from_code(code: i32) -> Encoding {
        match code {
            ENCODING_RAW => Encoding::Raw,
            ENCODING_COPY_RECT => Encoding::CopyRect,
            ENCODING_RRE => Encoding::Rre,
            ENCODING_HEXTILE => Encoding::Hextile,
            ENCODING_TIGHT => Encoding::Tight,
            ENCODING_DESKTOP_SIZE => Encoding::DesktopSize,
            other if other < 0 => Encoding::UnsupportedPseudo(other),
            other => Encoding::Unsupported(other),
        }
    }

    pub fn code(&self) -> i32 {
        match self {
            Encoding::Raw => ENCODING_RAW,
            Encoding::CopyRect => ENCODING_COPY_RECT,
            Encoding::Rre => ENCODING_RRE,
            Encoding::Hextile => ENCODING_HEXTILE,
            Encoding::Tight => ENCODING_TIGHT,
            Encoding::DesktopSize => ENCODING_DESKTOP_SIZE,
            Encoding::UnsupportedPseudo(code) | Encoding::Unsupported(code) => *code,
        }
    }

    /// Whether this server can put this encoding on the wire.
    pub fn is_producible(&self) -> bool {
        matches!(
            self,
            Encoding::Raw
                | Encoding::CopyRect
                | Encoding::Rre
                | Encoding::Hextile
                | Encoding::Tight
                | Encoding::DesktopSize
        )
    }

    /// Whether the encoding carries pixels.
    ///
    /// `CopyRect` and `DesktopSize` do not, which is why they are only ever
    /// chosen deliberately and never by the per-rectangle heuristic.
    pub fn carries_pixels(&self) -> bool {
        self.is_producible() && !matches!(self, Encoding::CopyRect | Encoding::DesktopSize)
    }
}

/// What a client will accept, and how.
#[derive(Debug, Clone)]
pub struct EncodingPreferences {
    encodings: Vec<Encoding>,
}

impl EncodingPreferences {
    /// Build from the codes in a `SetEncodings` message.
    ///
    /// Duplicates collapse. The *order* the client sent is preserved only for
    /// [`EncodingPreferences::advertised`]: negotiation order is the fixed
    /// preference in [`EncodingPreferences::choose`], because a client listing
    /// Tight before Hextile is describing what it can decode, not what it
    /// prefers this particular frame to use.
    pub fn new(codes: &[i32]) -> Self {
        let mut encodings = Vec::with_capacity(codes.len());
        for code in codes {
            let encoding = Encoding::from_code(*code);
            if !encodings.contains(&encoding) {
                encodings.push(encoding);
            }
        }
        if !encodings.contains(&Encoding::Raw) {
            // Raw is mandatory: a server may use it whether or not it was listed.
            encodings.push(Encoding::Raw);
        }
        Self { encodings }
    }

    pub fn supports(&self, encoding: Encoding) -> bool {
        self.encodings.contains(&encoding)
    }

    pub fn advertised(&self) -> &[Encoding] {
        &self.encodings
    }

    /// Pick the best encoding for one rectangle of `pixel_count` pixels.
    ///
    /// `solid` is the shortcut that decides RRE: a single-colour rectangle has
    /// an exact eight-byte encoding, and no general-purpose encoder beats that.
    pub fn choose(&self, pixel_count: u64, solid: bool) -> Encoding {
        if solid && self.supports(Encoding::Rre) {
            return Encoding::Rre;
        }
        // Below this size zlib's header and Huffman tables cost more than the
        // pixels they would compress.
        if self.supports(Encoding::Tight) && pixel_count >= 64 {
            return Encoding::Tight;
        }
        if self.supports(Encoding::Hextile) && pixel_count >= 256 {
            return Encoding::Hextile;
        }
        Encoding::Raw
    }
}

/// The server's picture of the guest, always in BGRX32.
///
/// One canonical format internally regardless of what any client asked for, so a
/// `SetPixelFormat` from one client cannot corrupt another's pixels. Conversion
/// to a client's format happens per rectangle on the way out.
#[derive(Clone, PartialEq, Eq)]
pub struct Framebuffer {
    width: u16,
    height: u16,
    pixels: Vec<u8>,
}

impl std::fmt::Debug for Framebuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Framebuffer")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.pixels.len())
            .finish()
    }
}

impl Framebuffer {
    /// Bytes one BGRX32 pixel occupies.
    pub const BYTES_PER_PIXEL: usize = 4;

    /// Allocate a framebuffer filled with opaque black.
    pub fn new(width: u16, height: u16) -> Self {
        let mut fb = Self {
            width,
            height,
            pixels: Vec::new(),
        };
        fb.resize(width, height);
        fb
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn pixel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// Bytes in one row.
    pub fn stride(&self) -> usize {
        self.width as usize * Self::BYTES_PER_PIXEL
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Mutable access to the pixels, for building a frame from a capture.
    ///
    /// Exposed because the capture path decodes a JPEG into exactly this
    /// layout. Every mutation through it is the caller's responsibility: the
    /// damage invariants only hold if the framebuffer is compared with
    /// [`Framebuffer::damage`] before being used as a scroll source.
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    /// Build from raw BGRX32 bytes, rejecting a length that disagrees with the
    /// declared geometry.
    ///
    /// The length is checked rather than trusted: a mismatch here would make
    /// every subsequent `row()` read out of bounds, and the geometry comes from
    /// a decoded image.
    pub fn from_bgra32(width: u16, height: u16, pixels: Vec<u8>) -> Result<Self, String> {
        let expected = width as usize * height as usize * Self::BYTES_PER_PIXEL;
        if pixels.len() != expected {
            return Err(format!(
                "a {width}x{height} BGRX32 framebuffer is {expected} bytes, got {}",
                pixels.len()
            ));
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    /// Row `y` as BGRX32 bytes.
    pub fn row(&self, y: u16) -> &[u8] {
        let start = y as usize * self.stride();
        &self.pixels[start..start + self.stride()]
    }

    /// Resize, discarding contents.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        let length = width as usize * height as usize * Self::BYTES_PER_PIXEL;
        self.pixels.clear();
        self.pixels.resize(length, 0);
        // Alpha is set on every pixel: a PNG decoded from a screendump can carry
        // anything there, and a client that honours alpha would render the whole
        // desktop translucent.
        for pixel in self.pixels.chunks_exact_mut(4) {
            pixel[3] = 0xff;
        }
    }

    /// Replace the contents from a framebuffer of identical geometry.
    pub fn copy_from(&mut self, other: &Framebuffer) {
        debug_assert_eq!((self.width, self.height), (other.width, other.height));
        self.pixels.copy_from_slice(&other.pixels);
    }

    /// Whether `region` lies entirely inside this framebuffer.
    pub fn contains(&self, region: &ChangedRegion) -> bool {
        let (x, y, w, h) = region.bounds();
        if w == 0 || h == 0 {
            return false;
        }
        u32::from(x) + u32::from(w) <= u32::from(self.width)
            && u32::from(y) + u32::from(h) <= u32::from(self.height)
    }

    /// The rectangles that differ between `self` and `current`.
    ///
    /// An empty result means nothing changed, which is the case the update loop
    /// cares about most: a static desktop must cost zero bytes.
    pub fn damage(&self, current: &Framebuffer) -> Vec<ChangedRegion> {
        if self.width != current.width || self.height != current.height {
            // A geometry change is not damage. The caller resizes and sends a
            // DesktopSize instead; reporting the old framebuffer as damaged
            // would send pixels at the wrong resolution.
            return Vec::new();
        }
        if self.pixels == current.pixels {
            return Vec::new();
        }

        let mut regions: Vec<ChangedRegion> = Vec::new();
        // A run of consecutive rows that all share the same changed span.
        let mut open: Option<RowRun> = None;

        for y in 0..current.height {
            match changed_span(self, current, y) {
                None => {
                    if let Some(run) = open.take() {
                        push_region(&mut regions, run);
                    }
                }
                Some(span) => match open {
                    // Same span on the very next row: extend the run. This is
                    // what turns a 200-row repaint into one rectangle.
                    Some(previous) if previous.y1 + 1 == y && previous.span == span => {
                        open = Some(RowRun {
                            span,
                            y0: previous.y0,
                            y1: y,
                        });
                    }
                    Some(_) => {
                        if let Some(run) = open.take() {
                            push_region(&mut regions, run);
                        }
                        open = Some(RowRun { span, y0: y, y1: y });
                    }
                    None => open = Some(RowRun { span, y0: y, y1: y }),
                },
            }
        }
        if let Some(run) = open {
            push_region(&mut regions, run);
        }
        merge_regions(&mut regions);
        regions
    }

    /// Copy `regions` out of `current` into `self`.
    ///
    /// Called with rectangles [`Framebuffer::damage`] produced, which makes the
    /// two framebuffers identical afterwards by construction. That invariant is
    /// what licenses `CopyRect`: the client provably holds the source pixels
    /// because this server has just guaranteed it.
    pub fn apply(&mut self, current: &Framebuffer, regions: &[ChangedRegion]) {
        for region in regions {
            let (x, y, w, h) = region.bounds();
            let len = w as usize * Self::BYTES_PER_PIXEL;
            for row in 0..h {
                let start = (y + row) as usize * self.stride() + x as usize * Self::BYTES_PER_PIXEL;
                self.pixels[start..start + len]
                    .copy_from_slice(&current.pixels[start..start + len]);
            }
        }
    }

    /// Shift the framebuffer so that `after(x, y) == before(x + dx, y + dy)`.
    ///
    /// Only non-negative offsets are accepted, because `CopyRect`'s source is an
    /// unsigned coordinate inside the framebuffer: a scroll whose content came
    /// from above the top edge is not expressible. `copy_within` is used because
    /// source and destination overlap on any shift smaller than the frame, and
    /// iterating rows upwards is what makes the in-place copy safe — row `y` is
    /// written from row `y + dy`, which has not been touched yet.
    pub fn scroll(&mut self, dx: i16, dy: i16) -> bool {
        if dx < 0 || dy < 0 || (dx == 0 && dy == 0) {
            return false;
        }
        let (dx, dy) = (dx as usize, dy as usize);
        if dx >= self.width as usize || dy >= self.height as usize {
            return false;
        }
        let shift = dx * Self::BYTES_PER_PIXEL;
        let stride = self.stride();
        for y in 0..self.height as usize - dy {
            let source = (y + dy) * stride + shift;
            let destination = y * stride;
            self.pixels
                .copy_within(source..source + stride - shift, destination);
        }
        true
    }

    /// Try to explain `current` as `self` scrolled by some offset.
    ///
    /// Only worth attempting when the damage covers nearly the whole screen: a
    /// scroll rewrites almost every pixel, whereas a handful of changed pixels
    /// that happen to look like a shifted copy is not a scroll. The candidate is
    /// then *verified* at sampled points across the frame before being believed,
    /// because a false positive here produces a `CopyRect` the client applies to
    /// pixels it does not hold — silent corruption, not a glitch.
    ///
    /// A uniform frame is never reported as a scroll. Every offset "explains" a
    /// solid rectangle, so the verification would pass on a frame that actually
    /// just changed colour, and the client would be left showing the old colour.
    pub fn detect_scroll(&self, current: &Framebuffer) -> Option<Scroll> {
        if self.width != current.width || self.height != current.height {
            return None;
        }
        let (width, height) = (self.width as usize, self.height as usize);
        let full = full_frame(self.width, self.height);
        if self.is_solid(&full) || current.is_solid(&full) {
            return None;
        }

        let stride = self.stride();
        let before = self.pixels.as_slice();
        let after = current.pixels.as_slice();
        // A leading slice is enough to identify a candidate: 64 bytes is far more
        // than enough to be unambiguous on a frame that is not uniform.
        let prefix = width.min(16) * Self::BYTES_PER_PIXEL;

let mut candidate = None;
        // dy == 0 is a horizontal scroll and must be in the search; only the
        // do-nothing offset is excluded.
        'search: for dy in 0..height {
            let row = dy * stride;
            for dx in 0..=width.saturating_sub(prefix / Self::BYTES_PER_PIXEL) {
                let offset = row + dx * Self::BYTES_PER_PIXEL;
                if after[..prefix] == before[offset..offset + prefix] {
                    candidate = Some((dx, dy));
                    break 'search;
                }
            }
        }
        let (dx, dy) = candidate?;
        if dx == 0 && dy == 0 {
            // Not a scroll: it is the same frame.
            return None;
        }

        // Verify. Twelve spread samples is not a proof, but the bar here is
        // "almost certainly a scroll" rather than "probably".
        let mut checked = 0usize;
        let step_y = (height / 4).max(1);
        let step_x = (width / 4).max(1);
        for y in (0..height).step_by(step_y) {
            for x in (0..width).step_by(step_x) {
                let (sx, sy) = (x + dx, y + dy);
                if sx >= width || sy >= height {
                    continue;
                }
                let a = y * stride + x * Self::BYTES_PER_PIXEL;
                let b = sy * stride + sx * Self::BYTES_PER_PIXEL;
                if after[a..a + 4] != before[b..b + 4] {
                    return None;
                }
                checked += 1;
            }
        }
        if checked < 4 {
            return None;
        }
        let dx = i16::try_from(dx).ok()?;
        let dy = i16::try_from(dy).ok()?;
        Some(Scroll { dx, dy })
    }

    /// The rectangles a scroll cannot supply, which must be sent as pixels.
    ///
    /// With the `after(x, y) == before(x + dx, y + dy)` convention the newly
    /// arrived pixels are on the right and the bottom.
    pub fn exposed_by_scroll(&self, dx: i16, dy: i16) -> Vec<ChangedRegion> {
        let mut regions = Vec::new();
        let (width, height) = (u32::from(self.width), u32::from(self.height));
        let dx = dx.unsigned_abs() as u32;
        let dy = dy.unsigned_abs() as u32;
        if dx > 0 && dx < width {
            regions.push(ChangedRegion {
                x: (width - dx) as u16,
                y: 0,
                width: dx as u16,
                height: self.height,
            });
        }
        if dy > 0 && dy < height {
            regions.push(ChangedRegion {
                x: 0,
                y: (height - dy) as u16,
                width: self.width,
                height: dy as u16,
            });
        }
        regions.retain(|r| self.contains(r));
        regions
    }

    /// Whether `region` is a single colour.
    pub fn is_solid(&self, region: &ChangedRegion) -> bool {
        if !self.contains(region) {
            return false;
        }
        let (x, y, w, h) = region.bounds();
        let stride = self.stride();
        let first = pixel_at(&self.pixels, stride, x, y);
        for row in 0..h {
            let start = (y + row) as usize * stride + x as usize * Self::BYTES_PER_PIXEL;
            let len = w as usize * Self::BYTES_PER_PIXEL;
            if self.pixels[start..start + len]
                .chunks_exact(Self::BYTES_PER_PIXEL)
                .any(|pixel| pixel != first)
            {
                return false;
            }
        }
        true
    }

    /// The colour of `region`, assuming [`Framebuffer::is_solid`].
    pub fn solid_colour(&self, region: &ChangedRegion) -> [u8; 4] {
        pixel_at(&self.pixels, self.stride(), region.x, region.y)
    }

    /// Append one rectangle's header and payload in `encoding`.
    ///
    /// A pixel-carrying encoding this server cannot produce must never reach the
    /// wire with an empty body: the client would read the following rectangle's
    /// header as its data and desynchronise the stream permanently. Unimplemented
    /// codes therefore degrade to Raw.
    pub fn write_rect(
        &self,
        out: &mut Vec<u8>,
        region: &ChangedRegion,
        encoding: Encoding,
        format: &PixelFormat,
    ) {
        match encoding {
            Encoding::Raw => {
                region.to_rect(ENCODING_RAW).write_header(out);
                self.write_raw_payload(out, region, format);
            }
            Encoding::Rre => {
                region.to_rect(ENCODING_RRE).write_header(out);
                self.write_rre_payload(out, region, format);
            }
            Encoding::Hextile => {
                region.to_rect(ENCODING_HEXTILE).write_header(out);
                self.write_hextile_payload(out, region, format);
            }
            Encoding::Tight => {
                region.to_rect(ENCODING_TIGHT).write_header(out);
                self.write_tight_payload(out, region, format);
            }
            Encoding::DesktopSize => {
                region.to_rect(ENCODING_DESKTOP_SIZE).write_header(out);
            }
            Encoding::CopyRect | Encoding::Unsupported(_) | Encoding::UnsupportedPseudo(_) => {
                region.to_rect(ENCODING_RAW).write_header(out);
                self.write_raw_payload(out, region, format);
            }
        }
    }

    /// Raw `PIXEL` payload for `region`, converted to `format`.
    pub fn write_raw_payload(&self, out: &mut Vec<u8>, region: &ChangedRegion, format: &PixelFormat) {
        let (x, y, w, h) = region.bounds();
        let stride = self.stride();
        out.reserve(region.pixel_count() as usize * format.bytes_per_pixel());
        for row in 0..h {
            let start = (y + row) as usize * stride + x as usize * Self::BYTES_PER_PIXEL;
            let len = w as usize * Self::BYTES_PER_PIXEL;
            write_pixels(out, &self.pixels[start..start + len], format);
        }
    }

    /// Raw `TPIXEL` payload for `region`.
    ///
    /// The difference from [`Framebuffer::write_raw_payload`] is three bytes per
    /// pixel in R, G, B order when the pixel format is 32bpp/24-depth with 8-bit
    /// components. Everything else is identical to `PIXEL`.
    pub fn write_tpixels(&self, out: &mut Vec<u8>, region: &ChangedRegion, format: &PixelFormat) {
        if format.tpixel_is_rgb_triple() {
            let (x, y, w, h) = region.bounds();
            let stride = self.stride();
            out.reserve(region.pixel_count() as usize * 3);
            for row in 0..h {
                let start = (y + row) as usize * stride + x as usize * Self::BYTES_PER_PIXEL;
                for pixel in self.pixels[start..start + w as usize * Self::BYTES_PER_PIXEL]
                    .chunks_exact(Self::BYTES_PER_PIXEL)
                {
                    out.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
                }
            }
            return;
        }
        self.write_raw_payload(out, region, format);
    }

    /// RRE payload: a background pixel plus the sub-rectangles that differ.
    ///
    /// A solid region is one pixel and no sub-rectangles: eight bytes for a whole
    /// screen. The sub-rectangle scan finds maximal horizontal runs of differing
    /// pixels, which is the textbook O(area) version. It stays correct anywhere,
    /// and RRE is only chosen for regions that are mostly one colour.
    pub fn write_rre_payload(&self, out: &mut Vec<u8>, region: &ChangedRegion, format: &PixelFormat) {
        let (x, y, w, h) = region.bounds();
        let stride = self.stride();
        let background = pixel_at(&self.pixels, stride, x, y);

        let mut subrects: Vec<[u16; 4]> = Vec::new();
        for row in 0..h {
            let py = y + row;
            let mut col = 0u16;
            while col < w {
                if pixel_at(&self.pixels, stride, x + col, py) == background {
                    col += 1;
                    continue;
                }
                // An RRE sub-rectangle is a single flat colour, so the run has
                // to be bounded by the colour *changing* as well as by the
                // background. Stopping only at the background would collapse a
                // whole run of varying colours into one rectangle carrying the
                // first pixel's value, silently discarding every other pixel in
                // it -- which is most of a desktop, where almost nothing is
                // exactly the background colour.
                let run = pixel_at(&self.pixels, stride, x + col, py);
                let start = col;
                while col < w && pixel_at(&self.pixels, stride, x + col, py) == run {
                    col += 1;
                }
                subrects.push([start, row, col - start, 1]);
            }
        }

        out.extend_from_slice(&(subrects.len() as u32).to_be_bytes());
        format.write_pixel(background, out);
        for [sx, sy, sw, sh] in subrects {
            // RFC 6143 7.7.3: a sub-rectangle is the tuple
            // <pixel-value, x-position, y-position, width, height> -- the colour
            // comes *first*. Writing the geometry first desynchronises every
            // real client while still round-tripping against a decoder that
            // makes the same mistake, which is exactly what this file's own
            // integration test used to do.
            let colour = pixel_at(&self.pixels, stride, x + sx, y + sy);
            format.write_pixel(colour, out);
            out.extend_from_slice(&sx.to_be_bytes());
            out.extend_from_slice(&sy.to_be_bytes());
            out.extend_from_slice(&sw.to_be_bytes());
            out.extend_from_slice(&sh.to_be_bytes());
        }
    }

    /// Hextile payload: 16x16 tiles in left-to-right, top-to-bottom order.
    ///
    /// A solid tile is `BackgroundSpecified` plus the pixel, with `AnySubrects`
    /// clear — which means no sub-rectangles, so the tile is entirely that
    /// colour. The one byte per tile after the first in a run is the whole point
    /// of the encoding on a desktop full of flat panels.
    ///
    /// The mixed sub-encodings (`AnySubrects`, `SubrectsColoured`) are not
    /// emitted: a partly-flat tile falls back to Raw, which is correct and only
    /// costs bandwidth on tiles that are half flat.
    pub fn write_hextile_payload(&self, out: &mut Vec<u8>, region: &ChangedRegion, format: &PixelFormat) {
        let (x, y, w, h) = region.bounds();
        let tile = MAX_HEXTILE_TILE as u16;
        let bytes_per_pixel = format.bytes_per_pixel();
        // The background a non-raw tile inherits when it does not restate it.
        // The specification is explicit that this may not be carried over across
        // a raw tile, which `Option` models directly.
        let mut carried: Option<[u8; 4]> = None;

        for tile_y in (0..h).step_by(tile as usize) {
            let rows = (h - tile_y).min(tile);
            for tile_x in (0..w).step_by(tile as usize) {
                let cols = (w - tile_x).min(tile);
                let tile_region = ChangedRegion {
                    x: x + tile_x,
                    y: y + tile_y,
                    width: cols,
                    height: rows,
                };
                if self.is_solid(&tile_region) {
                    let colour = self.solid_colour(&tile_region);
                    match carried {
                        Some(previous) if previous == colour => out.push(0),
                        _ => {
                            out.push(hextile::BACKGROUND_SPECIFIED);
                            format.write_pixel(colour, out);
                        }
                    }
                    carried = Some(colour);
                } else {
                    out.push(hextile::RAW);
                    let mut pixels = Vec::with_capacity(cols as usize * rows as usize * bytes_per_pixel);
                    self.write_raw_payload(&mut pixels, &tile_region, format);
                    out.extend_from_slice(&pixels);
                    carried = None;
                }
            }
        }
    }

    /// Tight payload: BasicCompression with the CopyFilter.
    ///
    /// Control byte `0x01`: compression type 0 (BasicCompression), bits 5-4 = 00
    /// (zlib stream 0), bit 6 clear (no filter-id byte, therefore CopyFilter),
    /// and bit 0 set so the decoder resets stream 0 before inflating — which is
    /// what makes a fresh complete zlib stream per rectangle correct.
    pub fn write_tight_payload(&self, out: &mut Vec<u8>, region: &ChangedRegion, format: &PixelFormat) {
        const CONTROL: u8 = 0x01;

        let mut raw = Vec::with_capacity(region.pixel_count() as usize * format.bytes_per_pixel());
        self.write_tpixels(&mut raw, region, format);

        let mut compressed = Vec::new();
        {
            let mut encoder = ZlibEncoder::new(&mut compressed, Compression::default());
            if encoder.write_all(&raw).is_err() {
                // Not reachable for any input flate2 can hand us, but the writer
                // is fallible and the protocol has no way to report it: emitting
                // the control byte with an empty length is the only safe answer,
                // and a zero-length BasicCompression payload is legal.
                out.push(CONTROL);
                write_compact_length(out, 0);
                return;
            }
        }
        // zlib is always used, never a "send it as is" branch. The specification
        // mentions an under-12-bytes special case whose framing differs between
        // implementations, and an unconditional zlib stream is unambiguous.
        out.push(CONTROL);
        write_compact_length(out, compressed.len());
        out.extend_from_slice(&compressed);
    }
}

/// Write a Tight compact length: one to three bytes.
///
/// The first byte is `0xxxxxxx` for 0..127 and `1xxxxxxx` otherwise. Only then
/// does the *second* byte's high bit discriminate: `0yyyyyyy` for 128..16383,
/// `1yyyyyyy zzzzzzzz` for 16384..4194303. The 7-bit groups run low bits first,
/// so 10000 is `0x90 0x4E`.
fn write_compact_length(out: &mut Vec<u8>, length: usize) {
    if length <= 127 {
        out.push(length as u8);
    } else if length <= 16383 {
        out.push(0x80 | (length & 0x7F) as u8);
        out.push(((length >> 7) & 0x7F) as u8);
    } else {
        out.push(0x80 | (length & 0x7F) as u8);
        out.push(0x80 | ((length >> 7) & 0x7F) as u8);
        out.push(((length >> 14) & 0xFF) as u8);
    }
}

/// One BGRX32 pixel.
fn pixel_at(pixels: &[u8], stride: usize, x: u16, y: u16) -> [u8; 4] {
    let offset = y as usize * stride + x as usize * Framebuffer::BYTES_PER_PIXEL;
    [
        pixels[offset],
        pixels[offset + 1],
        pixels[offset + 2],
        pixels[offset + 3],
    ]
}

/// Convert BGRX32 bytes into `format` as `PIXEL`s, appending to `out`.
fn write_pixels(out: &mut Vec<u8>, line: &[u8], format: &PixelFormat) {
    if format == &PixelFormat::BGRX32 {
        out.extend_from_slice(line);
        return;
    }
    let mut scratch = [0u8; 4];
    for chunk in line.chunks_exact(Framebuffer::BYTES_PER_PIXEL) {
        scratch.copy_from_slice(chunk);
        format.write_pixel(scratch, out);
    }
}

/// A run of consecutive rows sharing one changed column span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowRun {
    span: (u16, u16),
    y0: u16,
    y1: u16,
}

/// The changed column span in row `y`, if the row changed at all.
fn changed_span(previous: &Framebuffer, current: &Framebuffer, y: u16) -> Option<(u16, u16)> {
    let old = previous.row(y);
    let new = current.row(y);
    if old == new {
        return None;
    }
    let mut first: Option<u16> = None;
    let mut last = 0u16;
    for (index, (a, b)) in old
        .chunks_exact(Framebuffer::BYTES_PER_PIXEL)
        .zip(new.chunks_exact(Framebuffer::BYTES_PER_PIXEL))
        .enumerate()
    {
        if a != b {
            let x = u16::try_from(index).ok()?;
            if first.is_none() {
                first = Some(x);
            }
            last = x;
        }
    }
    let first = first?;
    Some((first, last - first + 1))
}

/// Turn a completed row run into a rectangle.
fn push_region(regions: &mut Vec<ChangedRegion>, run: RowRun) {
    let (x, width) = run.span;
    if width == 0 || run.y1 < run.y0 {
        return;
    }
    regions.push(ChangedRegion {
        x,
        y: run.y0,
        width,
        height: run.y1 - run.y0 + 1,
    });
}

/// Merge rectangles that overlap.
///
/// Merging only ever joins overlapping rectangles, so the union is exactly the
/// same set of pixels and no damage is invented.
fn merge_regions(regions: &mut Vec<ChangedRegion>) {
    if regions.len() < 2 {
        return;
    }
    let mut merged: Vec<ChangedRegion> = Vec::with_capacity(regions.len());
    for region in regions.drain(..) {
        match merged
            .iter_mut()
            .find(|existing| overlaps(**existing, region))
        {
            Some(existing) => *existing = union(*existing, region),
            None => merged.push(region),
        }
    }
    *regions = merged;
}

fn overlaps(a: ChangedRegion, b: ChangedRegion) -> bool {
    u32::from(a.x) < b.right() && u32::from(b.x) < a.right() && u32::from(a.y) < b.bottom() && u32::from(b.y) < a.bottom()
}

fn union(a: ChangedRegion, b: ChangedRegion) -> ChangedRegion {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    ChangedRegion {
        x,
        y,
        width: (a.right().max(b.right()) - u32::from(x)) as u16,
        height: (a.bottom().max(b.bottom()) - u32::from(y)) as u16,
    }
}

/// Write a `CopyRect` rectangle: a header plus the unsigned source coordinate.
pub fn write_copy_rect(out: &mut Vec<u8>, copy: &CopyRect) {
    Rect {
        x: copy.dst_x,
        y: copy.dst_y,
        width: copy.width,
        height: copy.height,
        encoding: ENCODING_COPY_RECT,
    }
    .write_header(out);
    out.extend_from_slice(&copy.src_x.to_be_bytes());
    out.extend_from_slice(&copy.src_y.to_be_bytes());
}

/// Write a `DesktopSize` pseudo-rectangle.
pub fn write_desktop_size(out: &mut Vec<u8>, width: u16, height: u16) {
    Rect {
        x: 0,
        y: 0,
        width,
        height,
        encoding: ENCODING_DESKTOP_SIZE,
    }
    .write_header(out);
}

/// Whether a rectangle may legally be sent as one Hextile.
///
/// Hextile tiles are 16x16 and the specification requires the rectangle's width
/// and height to be multiples of 16 unless clipped by the framebuffer edge. A
/// client derives its tile count from the rectangle dimensions, so an unaligned
/// interior rectangle makes it read the tile count from the wrong offset and
/// desynchronise the stream for good. The check therefore lives here rather than
/// being left to the encoder.
pub fn hextile_aligned(region: &ChangedRegion, fb_width: u16, fb_height: u16) -> bool {
    let (x, y, w, h) = region.bounds();
    let clipped_right = u32::from(x) + u32::from(w) >= u32::from(fb_width);
    let clipped_bottom = u32::from(y) + u32::from(h) >= u32::from(fb_height);
    (u32::from(w) % MAX_HEXTILE_TILE == 0 || clipped_right)
        && (u32::from(h) % MAX_HEXTILE_TILE == 0 || clipped_bottom)
}

/// Split a rectangle into pieces no wider than `max_width`.
///
/// Needed for Tight, whose specification caps the width of a rectangle at 2048
/// pixels. Returns the region unchanged when it is already narrow enough.
pub fn split_width(region: &ChangedRegion, max_width: u32) -> Vec<ChangedRegion> {
    if u32::from(region.width) <= max_width {
        return vec![*region];
    }
    let mut pieces = Vec::new();
    let mut offset = 0u32;
    while offset < u32::from(region.width) {
        let width = max_width.min(u32::from(region.width) - offset);
        pieces.push(ChangedRegion {
            x: (u32::from(region.x) + offset) as u16,
            y: region.y,
            width: width as u16,
            height: region.height,
        });
        offset += width;
    }
    pieces
}

/// Whether `region` covers enough of the frame that looking for a scroll pays.
pub fn covers_most_of(region: &ChangedRegion, total_pixels: usize) -> bool {
    total_pixels > 0 && region.pixel_count() * 10 >= total_pixels as u64 * 8
}

/// The whole framebuffer as one region, for a client that must resynchronise.
pub fn full_frame(width: u16, height: u16) -> ChangedRegion {
    ChangedRegion {
        x: 0,
        y: 0,
        width,
        height,
    }
}

/// Total area of a set of regions, ignoring overlap.
pub fn total_area(regions: &[ChangedRegion]) -> u64 {
    regions.iter().map(ChangedRegion::pixel_count).sum()
}

/// Whether every region lies inside `bounds`.
pub fn all_contained(regions: &[ChangedRegion], bounds: &ChangedRegion) -> bool {
    regions.iter().all(|region| bounds.contains(region))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::ZlibDecoder;
    use std::io::Read as _;

    fn solid(width: u16, height: u16, bgra: [u8; 4]) -> Framebuffer {
        let mut fb = Framebuffer::new(width, height);
        for pixel in fb.pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&bgra);
        }
        fb
    }

    fn paint(fb: &mut Framebuffer, x: u16, y: u16, w: u16, h: u16, bgra: [u8; 4]) {
        for row in y..y + h {
            for col in x..x + w {
                let offset = row as usize * fb.stride() + col as usize * 4;
                fb.pixels[offset..offset + 4].copy_from_slice(&bgra);
            }
        }
    }

    /// A frame with enough structure that a false scroll match is detectable.
    fn structured(width: u16, height: u16) -> Framebuffer {
        let mut fb = Framebuffer::new(width, height);
        for y in 0..height {
            for x in 0..width {
                if (u32::from(x) + u32::from(y)) % 7 == 0 {
                    paint(&mut fb, x, y, 1, 1, [x as u8, y as u8, 3, 255]);
                }
            }
        }
        fb
    }

    /// Scroll `source` by `(dx, dy)` under the `after(x,y) == before(x+dx,y+dy)`
    /// convention, filling the exposed strips with a distinct colour.
    fn scrolled(source: &Framebuffer, dx: i16, dy: i16, exposed: [u8; 4]) -> Framebuffer {
        let mut out = Framebuffer::new(source.width(), source.height());
        // after(x, y) == before(x + dx, y + dy): row `y` of the result is row
        // `y + dy` of the source, shifted left by `dx`.
        let retained_width = source.width() as i32 - dx as i32;
        let retained_height = source.height() as i32 - dy as i32;
        let row_bytes = (retained_width.max(0) as usize) * 4;
        for y in 0..retained_height.max(0) as u16 {
            let source_row = y as i32 + dy as i32;
            let start = y as usize * out.stride();
            let source_start = source_row as usize * source.stride() + dx as usize * 4;
            out.pixels_mut()[start..start + row_bytes]
                .copy_from_slice(&source.pixels()[source_start..source_start + row_bytes]);
        }
        let blank = Framebuffer::new(source.width(), source.height());
        for region in blank.exposed_by_scroll(dx, dy) {
            paint(&mut out, region.x, region.y, region.width, region.height, exposed);
        }
        out
    }

    // ── Damage tracking ───────────────────────────────────────────────────

    #[test]
    fn identical_frames_have_no_damage() {
        let a = solid(64, 48, [1, 2, 3, 255]);
        let b = a.clone();
        assert!(a.damage(&b).is_empty(), "an unchanged frame must cost nothing");
    }

    #[test]
    fn one_pixel_produces_a_one_pixel_rect() {
        let previous = solid(64, 48, [1, 2, 3, 255]);
        let mut current = previous.clone();
        paint(&mut current, 10, 20, 1, 1, [9, 9, 9, 255]);

        assert_eq!(
            previous.damage(&current),
            vec![ChangedRegion {
                x: 10,
                y: 20,
                width: 1,
                height: 1
            }]
        );
    }

    #[test]
    fn a_small_change_is_not_a_full_frame() {
        // The headline claim of the module, pinned as an assertion. A 16x16
        // change in a 1920x1080 frame must not come back as the whole frame.
        let previous = solid(1920, 1080, [0, 0, 0, 255]);
        let mut current = previous.clone();
        paint(&mut current, 1000, 500, 16, 16, [255, 255, 255, 255]);

        let regions = previous.damage(&current);
        assert_eq!(regions.len(), 1);
        let sent = total_area(&regions);
        assert_eq!(sent, 256, "only the 16x16 region is damaged");
        let fraction = sent as f64 * 100.0 / previous.pixel_count() as f64;
        assert!(fraction < 0.1, "damage covered {fraction:.3}% of the frame");
    }

    #[test]
    fn the_raw_payload_of_a_small_change_is_actually_small() {
        // The same claim measured in bytes rather than pixels, which is what the
        // screendump path gets wrong: it sends the whole frame every time.
        let previous = solid(1920, 1080, [0, 0, 0, 255]);
        let mut current = previous.clone();
        paint(&mut current, 1000, 500, 16, 16, [255, 255, 255, 255]);

        let regions = previous.damage(&current);
        let mut sent = Vec::new();
        for region in &regions {
            previous.write_rect(&mut sent, region, Encoding::Raw, &PixelFormat::BGRX32);
        }
        let full = 1920 * 1080 * 4;
        assert_eq!(sent.len(), 12 + 16 * 16 * 4, "a 16x16 Raw rect is 16 KB");
        assert!(
            sent.len() * 100 < full,
            "sent {} bytes against a {full}-byte full frame",
            sent.len()
        );
    }

    #[test]
    fn a_contiguous_block_coalesces_into_one_rect() {
        let previous = solid(64, 48, [0, 0, 0, 255]);
        let mut current = previous.clone();
        paint(&mut current, 5, 5, 10, 5, [255, 0, 0, 255]);

        assert_eq!(
            previous.damage(&current),
            vec![ChangedRegion {
                x: 5,
                y: 5,
                width: 10,
                height: 5
            }]
        );
    }

    #[test]
    fn a_tall_narrow_change_coalesces_down_the_rows() {
        let previous = solid(64, 64, [0, 0, 0, 255]);
        let mut current = previous.clone();
        paint(&mut current, 30, 10, 1, 40, [255, 255, 255, 255]);

        let regions = previous.damage(&current);
        assert_eq!(regions.len(), 1, "a caret is one rectangle, not forty");
        assert_eq!(regions[0].width, 1);
        assert_eq!(regions[0].height, 40);
    }

    #[test]
    fn two_separate_changes_stay_two_rects() {
        let previous = solid(64, 48, [0, 0, 0, 255]);
        let mut current = previous.clone();
        paint(&mut current, 2, 2, 4, 4, [255, 0, 0, 255]);
        paint(&mut current, 40, 30, 4, 4, [0, 255, 0, 255]);

        let regions = previous.damage(&current);
        assert_eq!(regions.len(), 2, "far-apart damage should not be merged");
        assert!(all_contained(&regions, &full_frame(64, 48)));
    }

    #[test]
    fn damage_is_empty_when_the_geometry_changes() {
        // Not a full-frame damage report: the caller resizes and sends a
        // DesktopSize instead.
        assert!(solid(64, 48, [0, 0, 0, 255])
            .damage(&solid(32, 24, [0, 0, 0, 255]))
            .is_empty());
    }

    #[test]
    fn apply_makes_two_framebuffers_identical() {
        let previous = solid(64, 48, [0, 0, 0, 255]);
        let mut current = previous.clone();
        paint(&mut current, 3, 4, 20, 15, [1, 2, 3, 255]);
        paint(&mut current, 40, 40, 10, 5, [9, 9, 9, 255]);

        let mut updated = previous.clone();
        updated.apply(&current, &previous.damage(&current));
        assert_eq!(updated, current, "applying damage must fully resynchronise");
    }

    #[test]
    fn a_full_frame_is_sent_once_then_nothing() {
        // The end-to-end property: after the initial full frame a static guest
        // produces no traffic at all.
        let server = solid(320, 240, [30, 60, 90, 255]);
        let mut client_view = Framebuffer::new(320, 240);
        client_view.apply(&server, &[full_frame(320, 240)]);
        assert_eq!(client_view, server);
        assert!(
            client_view.damage(&server).is_empty(),
            "a static frame must produce zero rectangles"
        );
    }

    // ── Scrolling ─────────────────────────────────────────────────────────

    #[test]
    fn a_scroll_is_detected() {
        let previous = structured(64, 48);
        let scrolled = scrolled(&previous, 0, 5, [7, 7, 7, 255]);
        assert_eq!(previous.detect_scroll(&scrolled), Some(Scroll { dx: 0, dy: 5 }));
    }

    #[test]
    fn a_horizontal_scroll_is_detected() {
        let previous = structured(64, 48);
        let current = scrolled(&previous, 3, 0, [7, 7, 7, 255]);
        assert_eq!(
            previous.detect_scroll(&current),
            Some(Scroll { dx: 3, dy: 0 })
        );
    }

    #[test]
    fn a_diagonal_scroll_is_detected() {
        let previous = structured(64, 48);
        let current = scrolled(&previous, 2, 4, [7, 7, 7, 255]);
        assert_eq!(
            previous.detect_scroll(&current),
            Some(Scroll { dx: 2, dy: 4 })
        );
    }

    #[test]
    fn scrolling_leaves_only_the_exposed_strip_as_damage() {
        let previous = structured(64, 48);
        let mut client = previous.clone();
        assert!(client.scroll(0, 5));
        let exposed = client.exposed_by_scroll(0, 5);
        assert_eq!(
            exposed,
            vec![ChangedRegion {
                x: 0,
                y: 43,
                width: 64,
                height: 5
            }]
        );
        assert!(all_contained(&exposed, &full_frame(64, 48)));
    }

    #[test]
    fn scroll_plus_exposed_reproduces_the_new_frame_exactly() {
        // The property `CopyRect` depends on: after scroll + apply(exposed) the
        // server's copy equals what the client will have after applying the
        // CopyRect and the exposed rectangles.
        for (dx, dy) in [(0i16, 5i16), (3, 0), (2, 4), (0, 1)] {
            let previous = structured(64, 48);
            let current = scrolled(&previous, dx, dy, [7, 7, 7, 255]);
            let mut client = previous.clone();
            assert!(client.scroll(dx, dy), "scroll({dx}, {dy}) should be accepted");
            let exposed = client.exposed_by_scroll(dx, dy);
            client.apply(&current, &exposed);
            assert_eq!(client, current, "scroll({dx}, {dy}) did not reproduce the frame");
        }
    }

    #[test]
    fn scroll_rejects_negative_zero_and_oversized_offsets() {
        let original = structured(32, 16);
        let mut fb = original.clone();
        assert!(!fb.scroll(-1, 0), "a negative source cannot be a CopyRect");
        assert!(!fb.scroll(0, -1));
        assert!(!fb.scroll(0, 0), "a zero shift is not a scroll");
        assert!(!fb.scroll(32, 0), "a shift of the whole width copies nothing");
        assert!(!fb.scroll(0, 16));
        assert_eq!(fb, original, "a rejected scroll must not move anything");
    }

    #[test]
    fn a_colour_change_on_a_uniform_frame_is_not_a_scroll() {
        // Every offset "explains" a solid rectangle, so without this guard a
        // repaint would be sent as a CopyRect and the client would keep showing
        // the old colour.
        let previous = solid(64, 48, [10, 10, 10, 255]);
        let current = solid(64, 48, [200, 200, 200, 255]);
        assert_eq!(previous.detect_scroll(&current), None);
    }

    #[test]
    fn a_one_pixel_change_is_not_a_scroll() {
        let previous = structured(64, 48);
        let mut current = previous.clone();
        paint(&mut current, 0, 0, 1, 1, [255, 255, 255, 255]);
        assert_eq!(previous.detect_scroll(&current), None);
    }

    #[test]
    fn a_detected_scroll_is_always_real() {
        // Whatever the search returns, applying it must reproduce the frame.
        let previous = structured(96, 72);
        for (dx, dy) in [(1i16, 1i16), (0, 7), (5, 0), (11, 3), (0, 30)] {
            let current = scrolled(&previous, dx, dy, [200, 100, 50, 255]);
            if let Some(scroll) = previous.detect_scroll(&current) {
                let mut client = previous.clone();
                assert!(client.scroll(scroll.dx, scroll.dy));
                let exposed = client.exposed_by_scroll(scroll.dx, scroll.dy);
                client.apply(&current, &exposed);
                assert_eq!(
                    client, current,
                    "reported scroll ({}, {}) was not the real one",
                    scroll.dx, scroll.dy
                );
            }
        }
    }

    // ── Hextile ───────────────────────────────────────────────────────────

    #[test]
    fn hextile_alignment_follows_the_specifications_edge_rule() {
        let aligned = ChangedRegion {
            x: 4,
            y: 4,
            width: 32,
            height: 32,
        };
        // The specification constrains the rectangle's width and height, not its
        // origin: tiles are cut relative to the rectangle, so an unaligned
        // origin is legal.
        assert!(
            hextile_aligned(&aligned, 1920, 1080),
            "32 is a multiple of 16 wherever the rectangle starts"
        );

        let interior = ChangedRegion {
            x: 4,
            y: 4,
            width: 30,
            height: 32,
        };
        assert!(
            !hextile_aligned(&interior, 1920, 1080),
            "30 is not a multiple of 16 and nothing clips it"
        );

        let edge = ChangedRegion {
            x: 1904,
            y: 1064,
            width: 16,
            height: 16,
        };
        assert!(hextile_aligned(&edge, 1920, 1080), "clipped by the edge is allowed");
        let clipped = ChangedRegion {
            x: 1918,
            y: 1078,
            width: 2,
            height: 2,
        };
        assert!(hextile_aligned(&clipped, 1920, 1080), "clipped by the edge is allowed");
        assert!(!hextile_aligned(&interior, 1910, 1070));
    }

    #[test]
    fn hextile_encodes_a_flat_frame_as_one_byte_per_tile() {
        let fb = solid(64, 64, [0x10, 0x20, 0x30, 0xff]);
        let mut out = Vec::new();
        fb.write_hextile_payload(&mut out, &full_frame(64, 64), &PixelFormat::BGRX32);
        // 4x4 tiles of 16x16. The first restates its background (1 + 4 bytes);
        // the other fifteen inherit it and cost one byte each.
        assert_eq!(out.len(), 1 + 4 + 15);
        assert_eq!(
            out[0],
            hextile::BACKGROUND_SPECIFIED,
            "the first non-raw tile must state its background"
        );
        assert_eq!(&out[1..5], &[0x10, 0x20, 0x30, 0xff]);
        assert!(out[5..].iter().all(|b| *b == 0), "later tiles carry the background over");
    }

#[test]
    fn hextile_restates_the_background_after_a_raw_tile() {
        let mut fb = solid(64, 64, [0x10, 0x20, 0x30, 0xff]);
        // One flat tile, then a non-flat one, then flat again: the third must
        // restate its background, because the specification forbids carrying one
        // over a raw tile.
        paint(&mut fb, 16, 0, 16, 15, [0x99, 0x88, 0x77, 0xff]);
        paint(&mut fb, 16, 16, 1, 1, [1, 2, 3, 0xff]);

        let mut out = Vec::new();
        fb.write_hextile_payload(&mut out, &full_frame(64, 64), &PixelFormat::BGRX32);

        // Tile 0: flat, states its background.
        assert_eq!(out[0], hextile::BACKGROUND_SPECIFIED);
        assert_eq!(&out[1..5], &[0x10, 0x20, 0x30, 0xff]);
        // Tile 1: not flat, so raw.
        assert_eq!(out[5], hextile::RAW);
        // Tile 2 (x=32, y=0) is flat, but tile 1 was raw, so it must state the
        // background rather than assume one.
        let tile2 = 5 + 1 + 16 * 16 * 4;
        assert_eq!(
            out[tile2],
            hextile::BACKGROUND_SPECIFIED,
            "the background may not be carried over a raw tile"
        );
        assert_eq!(&out[tile2 + 1..tile2 + 5], &[0x10, 0x20, 0x30, 0xff]);
    }

    #[test]
    fn hextile_carries_the_background_over_consecutive_flat_tiles() {
        // A 32x32 frame is a 2x2 grid, so the byte count is checkable by hand.
        let mut fb = solid(32, 32, [0x10, 0x20, 0x30, 0xff]);
        paint(&mut fb, 0, 16, 32, 16, [0x99, 0x88, 0x77, 0xff]);
        let mut out = Vec::new();
        fb.write_hextile_payload(&mut out, &full_frame(32, 32), &PixelFormat::BGRX32);

        // Tile 0 states the background; tile 1 inherits it.
        assert_eq!(out[0], hextile::BACKGROUND_SPECIFIED);
        assert_eq!(&out[1..5], &[0x10, 0x20, 0x30, 0xff]);
        assert_eq!(out[5], 0, "the second flat tile costs one byte");
        // Tile 2 is a different colour, so it restates the background; tile 3
        // then inherits that one.
        assert_eq!(out[6], hextile::BACKGROUND_SPECIFIED);
        assert_eq!(&out[7..11], &[0x99, 0x88, 0x77, 0xff]);
        assert_eq!(out[11], 0);
        assert_eq!(out.len(), 12, "two stated backgrounds and two inherited tiles");
    }

    #[test]
    fn hextile_tile_payloads_reconstruct_the_frame() {
        // Decode what the encoder wrote, exactly as a client would, and check it
        // reproduces the source pixels.
        let mut fb = structured(64, 64);
        paint(&mut fb, 0, 0, 32, 32, [40, 50, 60, 255]);
        paint(&mut fb, 33, 20, 5, 3, [200, 10, 10, 255]);

        let region = full_frame(64, 64);
        let format = PixelFormat::BGRX32;
        let mut out = Vec::new();
        fb.write_hextile_payload(&mut out, &region, &format);

        let mut canvas = Framebuffer::new(64, 64);
        let mut cursor = 0usize;
        let mut background = [0u8; 4];
        for tile_y in (0..64u16).step_by(16) {
            for tile_x in (0..64u16).step_by(16) {
                let mask = out[cursor];
                cursor += 1;
                if mask & hextile::RAW != 0 {
                    assert_eq!(mask, hextile::RAW, "raw cancels every other bit");
                    for row in 0..16u16 {
                        for col in 0..16u16 {
                            let at = (tile_y + row) as usize * canvas.stride()
                                + (tile_x + col) as usize * 4;
                            canvas.pixels[at..at + 4]
                                .copy_from_slice(&out[cursor..cursor + 4]);
                            cursor += 4;
                        }
                    }
                    background = [0u8; 4];
                } else {
                    if mask & hextile::BACKGROUND_SPECIFIED != 0 {
                        background.copy_from_slice(&out[cursor..cursor + 4]);
                        cursor += 4;
                    }
                    assert_eq!(mask & hextile::ANY_SUBRECTS, 0, "no sub-rectangles were written");
                    for row in 0..16u16 {
                        for col in 0..16u16 {
                            let at = (tile_y + row) as usize * canvas.stride()
                                + (tile_x + col) as usize * 4;
                            canvas.pixels[at..at + 4].copy_from_slice(&background);
                        }
                    }
                }
            }
        }
        assert_eq!(cursor, out.len(), "every byte written was consumed");
        assert_eq!(canvas, fb, "the Hextile stream did not reproduce the frame");
    }

    // ── RRE ───────────────────────────────────────────────────────────────

    #[test]
    fn a_solid_rectangle_rre_encodes_to_nothing_but_a_background_pixel() {
        let fb = solid(64, 64, [0xaa, 0xbb, 0xcc, 0xff]);
        let mut out = Vec::new();
        fb.write_rre_payload(&mut out, &full_frame(64, 64), &PixelFormat::BGRX32);
        // Four bytes of count, four of background: 4096 pixels in eight bytes.
        assert_eq!(out.len(), 8);
        assert_eq!(&out[..4], &[0, 0, 0, 0]);
        assert_eq!(&out[4..], &[0xaa, 0xbb, 0xcc, 0xff], "the alpha is carried through");
    }

    #[test]
    fn rre_subrectangles_reconstruct_the_region() {
        let mut fb = Framebuffer::new(8, 8);
        paint(&mut fb, 0, 0, 8, 8, [0, 0, 0, 255]);
        paint(&mut fb, 4, 0, 4, 8, [255, 255, 255, 255]);

        let mut out = Vec::new();
        fb.write_rre_payload(&mut out, &full_frame(8, 8), &PixelFormat::BGRX32);
        let count = u32::from_be_bytes(out[..4].try_into().unwrap()) as usize;
        assert_eq!(count, 8, "one sub-rectangle per row of the right half");
        assert_eq!(out.len(), 8 + count * 12, "each sub-rectangle is 4 u16 plus a pixel");

        let background = [out[4], out[5], out[6], out[7]];
        let mut canvas = vec![background; 8 * 8];
        for sub in out[8..].chunks_exact(12) {
            // RFC 6143 7.7.3: <pixel-value, x, y, width, height>.
            let colour = [sub[0], sub[1], sub[2], sub[3]];
            let x = u16::from_be_bytes([sub[4], sub[5]]) as usize;
            let y = u16::from_be_bytes([sub[6], sub[7]]) as usize;
            let w = u16::from_be_bytes([sub[8], sub[9]]) as usize;
            let h = u16::from_be_bytes([sub[10], sub[11]]) as usize;
            for row in y..y + h {
                for col in x..x + w {
                    canvas[row * 8 + col] = colour;
                }
            }
        }
let expected: Vec<[u8; 4]> = (0..8)
            .flat_map(|_row| {
                (0..8).map(move |col| {
                    if col < 4 {
                        [0, 0, 0, 255]
                    } else {
                        [255, 255, 255, 255]
                    }
                })
            })
            .collect();
        assert_eq!(canvas, expected, "RRE sub-rectangles must rebuild the region");
    }

    #[test]
    fn rre_handles_a_single_trailing_pixel_in_a_row() {
        // The case a naive run scanner drops: one differing pixel in the last
        // column, with no following differing pixel to close the run.
        let mut fb = Framebuffer::new(4, 4);
        paint(&mut fb, 0, 0, 4, 4, [0, 0, 0, 255]);
        paint(&mut fb, 3, 1, 1, 1, [255, 0, 0, 255]);

        let mut out = Vec::new();
        fb.write_rre_payload(&mut out, &full_frame(4, 4), &PixelFormat::BGRX32);
        let count = u32::from_be_bytes(out[..4].try_into().unwrap()) as usize;
        assert_eq!(count, 1);
        assert_eq!(&out[8..12], &[255, 0, 0, 255], "the colour, which comes first");
        assert_eq!(&out[12..14], &[0, 3], "x=3");
        assert_eq!(&out[14..16], &[0, 1], "y=1");
        assert_eq!(&out[16..18], &[0, 1], "w=1");
        assert_eq!(&out[18..20], &[0, 1], "h=1");
    }

    // ── Tight ─────────────────────────────────────────────────────────────

    #[test]
    fn tight_writes_the_documented_control_byte() {
        let fb = structured(64, 64);
        let mut out = Vec::new();
        fb.write_tight_payload(&mut out, &full_frame(64, 64), &PixelFormat::BGRX32);
        assert_eq!(
            out[0],
            0x01,
            "BasicCompression, stream 0, no filter-id byte, reset stream 0"
        );
    }

    #[test]
    fn tight_uses_three_byte_rgb_tpixels_not_four_byte_pixels() {
        let mut fb = Framebuffer::new(8, 8);
        paint(&mut fb, 0, 0, 8, 8, [0x11, 0x22, 0x33, 0xff]);
        let mut out = Vec::new();
        fb.write_tight_payload(&mut out, &full_frame(8, 8), &PixelFormat::BGRX32);

        // Control byte, a three-byte compact length, then a zlib stream.
        let (length, used) = read_compact_length(&out[1..]);
        assert!(used >= 1);
        let mut decompressed = Vec::new();
        ZlibDecoder::new(std::io::Cursor::new(&out[1 + used..1 + used + length]))
            .read_to_end(&mut decompressed)
            .expect("the zlib stream should decode");
        // 64 pixels at three bytes each, in R, G, B order.
        assert_eq!(decompressed.len(), 8 * 8 * 3);
        assert_eq!(
            &decompressed[..3],
            &[0x33, 0x22, 0x11],
            "TPIXEL is R, G, B: the padding byte is dropped and the order swapped"
        );
        for pixel in decompressed.chunks_exact(3) {
            assert_eq!(pixel, &[0x33, 0x22, 0x11]);
        }
    }

    #[test]
    fn tight_falls_back_to_the_full_pixel_for_five_five_five() {
        // 16bpp is not a TPIXEL-3 case, so it must stay two bytes per pixel.
        let fb = solid(8, 8, [0x11, 0x22, 0x33, 0xff]);
        let mut out = Vec::new();
        fb.write_tight_payload(&mut out, &full_frame(8, 8), &PixelFormat::RGB555);
        let (length, used) = read_compact_length(&out[1..]);
        let mut decompressed = Vec::new();
        ZlibDecoder::new(std::io::Cursor::new(&out[1 + used..1 + used + length]))
            .read_to_end(&mut decompressed)
            .unwrap();
        assert_eq!(decompressed.len(), 8 * 8 * 2);
    }

    #[test]
    fn tight_zlib_actually_compresses_a_desktop_like_frame() {
        let mut fb = Framebuffer::new(256, 256);
        for y in 0..256u16 {
            for x in 0..256u16 {
                let colour = if (x / 32 + y / 32) % 2 == 0 {
                    [235, 235, 235, 255]
                } else {
                    [40, 42, 54, 255]
                };
                paint(&mut fb, x, y, 1, 1, colour);
            }
        }
        let region = full_frame(256, 256);
        let mut tight = Vec::new();
        fb.write_tight_payload(&mut tight, &region, &PixelFormat::BGRX32);
        let mut raw = Vec::new();
        fb.write_raw_payload(&mut raw, &region, &PixelFormat::BGRX32);
        assert!(
            tight.len() * 20 < raw.len(),
            "tight {} bytes against raw {}",
            tight.len(),
            raw.len()
        );
    }

    #[test]
    fn tight_compact_length_uses_all_three_forms() {
        // Worked from the specification's own worked example: 10000 is 90 4E.
        let mut out = Vec::new();
        write_compact_length(&mut out, 10000);
        assert_eq!(out, vec![0x90, 0x4E]);

        for (length, expected_len) in [
            (0usize, 1usize),
            (127, 1),
            (128, 2),
            (16383, 2),
            (16384, 3),
            (4_194_303, 3),
        ] {
            let mut encoded = Vec::new();
            write_compact_length(&mut encoded, length);
            assert_eq!(encoded.len(), expected_len, "length {length}");
            let (decoded, used) = read_compact_length(&encoded);
            assert_eq!(decoded, length, "round trip of {length}");
            assert_eq!(used, expected_len);
        }
    }

    #[test]
    fn tight_rectangles_wider_than_2048_are_split() {
        let wide = ChangedRegion {
            x: 0,
            y: 0,
            width: 5000,
            height: 10,
        };
        let pieces = split_width(&wide, MAX_TIGHT_RECT_WIDTH);
        assert_eq!(pieces.len(), 3);
        assert!(pieces.iter().all(|p| u32::from(p.width) <= MAX_TIGHT_RECT_WIDTH));
        assert_eq!(total_area(&pieces), total_area(&[wide]), "splitting loses no pixels");
        assert_eq!(pieces[0].width, 2048);
        assert_eq!(pieces[1].x, 2048);
        assert_eq!(pieces[2].width, 5000 - 4096);

        let narrow = ChangedRegion {
            x: 3,
            y: 4,
            width: 100,
            height: 100,
        };
        assert_eq!(split_width(&narrow, MAX_TIGHT_RECT_WIDTH), vec![narrow]);
    }

    // ── Raw, headers and negotiation ──────────────────────────────────────

    #[test]
    fn raw_payload_is_the_exact_pixels_in_the_client_format() {
        let fb = solid(8, 4, [0x11, 0x22, 0x33, 0xff]);
        let region = full_frame(8, 4);
        let mut out = Vec::new();
        fb.write_raw_payload(&mut out, &region, &PixelFormat::BGRX32);
        assert_eq!(out, fb.pixels()[..]);

        let mut out555 = Vec::new();
        fb.write_raw_payload(&mut out555, &region, &PixelFormat::RGB555);
        assert_eq!(out555.len(), 8 * 4 * 2);
    }

    #[test]
    fn encoding_classification_covers_client_advertisements() {
        assert_eq!(Encoding::from_code(0), Encoding::Raw);
        assert_eq!(Encoding::from_code(5), Encoding::Hextile);
        assert_eq!(Encoding::from_code(-223), Encoding::DesktopSize);
        assert_eq!(Encoding::from_code(16), Encoding::Unsupported(16));
        assert_eq!(Encoding::from_code(-239), Encoding::UnsupportedPseudo(-239));
        assert!(Encoding::Hextile.is_producible());
        assert!(!Encoding::Unsupported(16).is_producible());
        assert!(!Encoding::DesktopSize.carries_pixels());
        assert!(Encoding::Raw.carries_pixels());
    }

    #[test]
    fn raw_is_implicitly_available_even_if_not_advertised() {
        let prefs = EncodingPreferences::new(&[5, 7]);
        assert!(prefs.supports(Encoding::Raw));
        assert!(prefs.supports(Encoding::Hextile));
        assert!(!prefs.supports(Encoding::Rre));
    }

    #[test]
    fn a_client_advertising_only_unknown_encodings_still_gets_raw() {
        // Real clients advertise Cursor, ExtendedDesktopSize and friends. A
        // server that errored on those would refuse every TigerVNC connection.
        let prefs = EncodingPreferences::new(&[-239, -308, 16, 224, 250, -313]);
        assert!(prefs.supports(Encoding::Raw));
        assert_eq!(prefs.choose(100_000, false), Encoding::Raw);
        assert_eq!(
            prefs.choose(100_000, true),
            Encoding::Raw,
            "RRE was never advertised, so even a solid region goes out raw"
        );
    }

    #[test]
    fn encoding_choice_prefers_compression_only_when_it_pays() {
        let all = EncodingPreferences::new(&[0, 1, 2, 5, 7]);
        assert_eq!(all.choose(1000, false), Encoding::Tight);
        assert_eq!(all.choose(1000, true), Encoding::Rre, "solid regions are cheaper as RRE");
        assert_eq!(all.choose(16, false), Encoding::Raw, "zlib overhead is not worth 16 pixels");

        let no_tight = EncodingPreferences::new(&[0, 5]);
        assert_eq!(no_tight.choose(1000, false), Encoding::Hextile);
        assert_eq!(no_tight.choose(64, false), Encoding::Raw);

        assert_eq!(EncodingPreferences::new(&[]).choose(10_000, false), Encoding::Raw);
    }

    #[test]
    fn an_unimplemented_encoding_never_ships_an_empty_payload() {
        // If such a code ever reached write_rect, the client would read the next
        // rectangle's header as its data and desynchronise the stream.
        let fb = solid(16, 16, [7, 7, 7, 255]);
        let mut out = Vec::new();
        fb.write_rect(&mut out, &full_frame(16, 16), Encoding::Unsupported(16), &PixelFormat::BGRX32);
        assert_eq!(i32::from_be_bytes(out[8..12].try_into().unwrap()), ENCODING_RAW);
        assert_eq!(out.len(), 12 + 16 * 16 * 4);
    }

    #[test]
    fn copyrect_and_desktopsize_headers_match_the_specification() {
        let mut out = Vec::new();
        write_copy_rect(
            &mut out,
            &CopyRect {
                dst_x: 0,
                dst_y: 0,
                width: 64,
                height: 48,
                src_x: 0,
                src_y: 5,
            },
        );
        assert_eq!(out.len(), 16);
        assert_eq!(i32::from_be_bytes(out[8..12].try_into().unwrap()), ENCODING_COPY_RECT);
        assert_eq!(&out[12..], &[0, 0, 0, 5], "the source coordinate follows the header");

        let mut size = Vec::new();
        write_desktop_size(&mut size, 800, 600);
        assert_eq!(size.len(), 12);
        assert_eq!(i32::from_be_bytes(size[8..12].try_into().unwrap()), ENCODING_DESKTOP_SIZE);
        assert_eq!(&size[..8], &[0, 0, 0, 0, 0x03, 0x20, 0x02, 0x58], "x, y, width, height");
    }

    #[test]
    fn from_bgra32_rejects_a_length_that_lies_about_the_geometry() {
        assert!(Framebuffer::from_bgra32(4, 4, vec![0u8; 64]).is_ok());
        assert!(
           Framebuffer::from_bgra32(4, 4, vec![0u8; 63]).is_err(),
            "a short buffer would make every row read out of bounds"
        );
        assert!(Framebuffer::from_bgra32(4, 4, vec![0u8; 65]).is_err());
    }

    #[test]
    fn a_resize_discards_contents_but_stays_well_formed() {
        let mut fb = solid(64, 48, [1, 2, 3, 255]);
        fb.resize(32, 16);
        assert_eq!((fb.width(), fb.height()), (32, 16));
        assert_eq!(fb.pixels().len(), 32 * 16 * 4);
        assert!(
            fb.pixels().chunks_exact(4).all(|p| p[3] == 0xff),
            "alpha must be restored, or a resized desktop renders translucent"
        );
    }

    #[test]
    fn regions_outside_the_framebuffer_are_rejected() {
        let fb = solid(16, 16, [0, 0, 0, 255]);
        assert!(!fb.contains(&ChangedRegion { x: 15, y: 0, width: 2, height: 1 }));
        assert!(!fb.contains(&ChangedRegion { x: 0, y: 0, width: 0, height: 1 }));
        assert!(fb.contains(&ChangedRegion { x: 15, y: 15, width: 1, height: 1 }));
    }

    #[test]
    fn solid_detection_is_exact() {
        let mut fb = solid(8, 8, [1, 2, 3, 255]);
        assert!(fb.is_solid(&full_frame(8, 8)));
        assert_eq!(fb.solid_colour(&full_frame(8, 8)), [1, 2, 3, 255]);
        paint(&mut fb, 7, 7, 1, 1, [4, 5, 6, 255]);
        assert!(!fb.is_solid(&full_frame(8, 8)), "the far corner counts");
        assert!(fb.is_solid(&ChangedRegion { x: 0, y: 0, width: 7, height: 8 }));
    }

    #[test]
    fn covers_most_of_is_a_ratio() {
        let region = ChangedRegion {
            x: 0,
            y: 0,
            width: 1000,
            height: 1000,
        };
        assert!(covers_most_of(&region, 1_000_000), "100% covers most of it");
        assert!(!covers_most_of(&region, 10_000_000), "1% does not");
        assert!(!covers_most_of(&region, 0), "an empty frame is not a scroll");
    }

    /// Read a Tight compact length, returning the value and the bytes consumed.
///
/// The discriminator is the *second* byte's high bit, not the first: the first
/// byte of a two-byte form and of a three-byte form are both `1xxxxxxx`.
fn read_compact_length(bytes: &[u8]) -> (usize, usize) {
    let first = bytes[0];
    if first & 0x80 == 0 {
        return (first as usize, 1);
    }
    let low = (first as usize & 0x7F) | ((bytes[1] as usize & 0x7F) << 7);
    if bytes[1] & 0x80 == 0 {
        (low, 2)
    } else {
        (low | ((bytes[2] as usize) << 14), 3)
    }
}
}