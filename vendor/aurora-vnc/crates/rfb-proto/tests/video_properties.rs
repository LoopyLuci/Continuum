//! Property tests for Aurora Video (spec/VIDEO.md): random Aurora messages
//! and video rectangle headers round-trip, every proper prefix is
//! Incomplete, tile grids cover the framebuffer exactly, unknown subtypes
//! and appended fields are skipped. (Fuzz targets `client_message`,
//! `server_message` and `video_rect` cover arbitrary bytes.)

use rfb_proto::video::{self, tile_format, AuroraClientMessage, AuroraServerMessage, VideoRectHeader};
use rfb_proto::*;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const CASES: u64 = 500;

fn caps(rng: &mut Rng) -> VideoCaps {
    let n = rng.below(6) as usize;
    VideoCaps {
        version: rng.next() as u16,
        flags: rng.next() as u16,
        max_frames_in_flight: rng.next() as u8,
        max_bitrate_kbps: rng.next() as u32,
        max_pixel_rate_kpps: rng.next() as u32,
        codecs: (0..n)
            .map(|_| CodecCap {
                codec: Codec(rng.below(5) as u8),
                profile: rng.next() as u8,
                level: rng.next() as u8,
                flags: rng.next() as u8,
                max_width: rng.next() as u16,
                max_height: rng.next() as u16,
                max_fps: rng.next() as u16,
                max_tiles: rng.next() as u16,
            })
            .collect(),
    }
}

fn config(rng: &mut Rng) -> VideoConfig {
    loop {
        let (w, h) = (1 + rng.below(8192) as u16, 1 + rng.below(8192) as u16);
        let (cols, rows) = (1 + rng.below(8) as usize, 1 + rng.below(8) as usize);
        let align = [1u16, 16, 64, 128][rng.below(4) as usize];
        let (Some(cw), Some(rh)) = (VideoConfig::split(w, cols, align), VideoConfig::split(h, rows, align)) else {
            continue;
        };
        return VideoConfig {
            config_id: 1 + rng.below(u64::from(u32::MAX)) as u32,
            codec: Codec(1 + rng.below(3) as u8),
            profile: rng.next() as u8,
            level: rng.next() as u8,
            flags: rng.next() as u8,
            colour_primaries: 1,
            transfer_characteristics: 13,
            matrix_coefficients: 1,
            full_range: true,
            fps: 1 + rng.below(480) as u16,
            frames_in_flight: rng.next() as u8,
            width: w,
            height: h,
            col_widths: cw,
            row_heights: rh,
        };
    }
}

fn check_prefixes(b: &[u8], decode: impl Fn(&[u8]) -> Result<usize, DecodeError>) {
    for cut in 0..b.len() {
        match decode(&b[..cut]) {
            Err(DecodeError::Incomplete { needed }) => assert!(needed > cut),
            other => panic!("{cut}-byte prefix of {} gave {other:?}", b.len()),
        }
    }
}

#[test]
fn client_messages_round_trip() {
    let limits = Limits::default();
    let mut rng = Rng(0x5EED_0001);
    for _ in 0..CASES {
        let m = match rng.below(6) {
            0 => AuroraClientMessage::VideoCaps(caps(&mut rng)),
            1 => AuroraClientMessage::KeyframeRequest {
                config_id: rng.next() as u32,
                tile: rng.next() as u16,
                reason: rng.next() as u8,
            },
            2 => AuroraClientMessage::FrameAck {
                config_id: rng.next() as u32,
                frame: rng.next() as u32,
                latency_us: rng.next() as u32,
                decode_us: rng.next() as u32,
            },
            3 => AuroraClientMessage::TimeProbe { client_time_us: rng.next() },
            4 => AuroraClientMessage::DecodePerf(DecodePerf {
                max_tiles_in_flight: rng.next() as u16,
                refresh_mhz: rng.next() as u32,
                tiers: (0..rng.below(5))
                    .map(|_| TierPerf {
                        codec: Codec(rng.next() as u8),
                        format: rng.next() as u8,
                        threads: rng.next() as u16,
                        tile_kpps: rng.next() as u32,
                        total_kpps: rng.next() as u32,
                    })
                    .collect(),
            }),
            _ => AuroraClientMessage::Unknown {
                subtype: 7 + rng.below(240) as u8,
                body: (0..rng.below(40)).map(|_| rng.next() as u8).collect(),
            },
        };
        let msg = ClientMessage::Aurora(m);
        let mut b = Vec::new();
        msg.encode(&mut b);
        assert_eq!(ClientMessage::decode(&b, &limits).unwrap(), (msg, b.len()));
        check_prefixes(&b, |p| ClientMessage::decode(p, &limits).map(|(_, n)| n));
    }
}

#[test]
fn server_messages_round_trip_and_grids_cover() {
    let limits = Limits::default();
    let mut rng = Rng(0x5EED_0002);
    for _ in 0..CASES {
        let c = config(&mut rng);
        c.validate(&limits).unwrap();
        // Every pixel is in exactly one tile.
        let mut area = 0u64;
        for i in 0..c.tile_count() {
            let (x, y, w, h) = c.tile_rect(i).unwrap();
            assert!(u32::from(x) + u32::from(w) <= u32::from(c.width));
            assert!(u32::from(y) + u32::from(h) <= u32::from(c.height));
            assert_eq!(c.tile_at(x, y, w, h), Some(i));
            area += u64::from(w) * u64::from(h);
        }
        assert_eq!(area, u64::from(c.width) * u64::from(c.height));
        let msg = ServerMessage::Aurora(AuroraServerMessage::VideoConfig(c));
        let mut b = Vec::new();
        msg.encode(&mut b);
        assert_eq!(ServerMessage::decode(&b, &limits).unwrap(), (msg, b.len()));
        check_prefixes(&b, |p| ServerMessage::decode(p, &limits).map(|(_, n)| n));
    }
}

#[test]
fn bad_grids_are_refused() {
    let limits = Limits::default();
    let mut rng = Rng(0x5EED_0003);
    for _ in 0..CASES {
        let mut c = config(&mut rng);
        match rng.below(4) {
            0 => c.width = c.width.wrapping_add(1),
            1 => c.row_heights[0] = 0,
            2 => c.config_id = 0,
            _ => {
                c.col_widths = vec![1; 9];
                c.row_heights = vec![1; 8];
                c.width = 9;
                c.height = 8; // 72 tiles > 64
            }
        }
        let mut b = Vec::new();
        ServerMessage::Aurora(AuroraServerMessage::VideoConfig(c)).encode(&mut b);
        assert!(matches!(ServerMessage::decode(&b, &limits), Err(DecodeError::Invalid(_))));
    }
}

#[test]
fn appended_fields_and_unknown_subtypes_are_skipped() {
    let limits = Limits::default();
    let mut b = Vec::new();
    ClientMessage::Aurora(AuroraClientMessage::TimeProbe { client_time_us: 42 }).encode(&mut b);
    // A later version appends 4 bytes to TimeProbe.
    b[7] += 4;
    b.extend_from_slice(&[9, 9, 9, 9]);
    b.extend_from_slice(&[4, 1, 0, 0, 0, 0]); // next message: KeyEvent, still there
    let (m, n) = ClientMessage::decode(&b, &limits).unwrap();
    assert_eq!(m, ClientMessage::Aurora(AuroraClientMessage::TimeProbe { client_time_us: 42 }));
    assert_eq!(n, 8 + 12);
    // Unknown subtypes keep their body and the stream stays in sync.
    let mut u = vec![65, 99, 0, 0, 0, 0, 0, 2, 0xaa, 0xbb];
    u.extend_from_slice(&[2, 0, 0, 0]);
    let (m, n) = ServerMessage::decode(&u, &limits).unwrap();
    assert_eq!(m, ServerMessage::Aurora(AuroraServerMessage::Unknown { subtype: 99, body: vec![0xaa, 0xbb] }));
    assert_eq!(n, 10);
}

#[test]
fn over_limit_lengths_are_refused_before_allocating() {
    let limits = Limits::default();
    let mut b = vec![65, 1, 0, 0];
    b.extend_from_slice(&(limits.max_aurora_msg_len + 1).to_be_bytes());
    assert!(matches!(ClientMessage::decode(&b, &limits), Err(DecodeError::Invalid(_))));
    let mut h = Vec::new();
    VideoRectHeader {
        config_id: 1,
        frame: 0,
        capture_time_us: 0,
        tile: 0,
        seq: 0,
        tiles_in_frame: 1,
        flags: 0,
        codec: Codec::AV1,
        format: 0,
        length: limits.max_video_payload + 1,
    }
    .encode(&mut h);
    assert!(matches!(VideoRectHeader::decode(&h, &limits), Err(DecodeError::Invalid(_))));
}

#[test]
fn video_rects_round_trip() {
    let limits = Limits::default();
    let mut rng = Rng(0x5EED_0004);
    for _ in 0..CASES {
        let c = config(&mut rng);
        let tiles_in_frame = 1 + rng.below(c.tile_count() as u64) as u16;
        let chroma = rng.below(2) as u8;
        let fmt = chroma
            | if rng.below(2) == 0 { tile_format::DEPTH_10 } else { 0 }
            | if chroma == tile_format::CHROMA_444 && rng.below(2) == 0 {
                tile_format::RGB | tile_format::LOSSLESS
            } else {
                0
            };
        let data: Vec<u8> = (0..rng.below(200)).map(|_| rng.next() as u8).collect();
        let h = VideoRectHeader {
            config_id: c.config_id,
            frame: rng.next() as u32,
            capture_time_us: rng.next(),
            tile: rng.below(c.tile_count() as u64) as u16,
            seq: rng.below(u64::from(tiles_in_frame)) as u16,
            tiles_in_frame,
            flags: rng.next() as u16 & 3,
            codec: c.codec,
            format: fmt,
            length: 0,
        };
        let mut b = Vec::new();
        video::encode_rect(&c, &h, &data, &mut b);
        let (rh, n) = RectHeader::decode(&b).unwrap();
        assert_eq!(rh.encoding, Encoding::AuroraVideo);
        assert_eq!(c.tile_at(rh.x, rh.y, rh.width, rh.height), Some(usize::from(h.tile)));
        let (back, d, used) = VideoRectHeader::decode(&b[n..], &limits).unwrap();
        assert_eq!(back, VideoRectHeader { length: data.len() as u32, ..h });
        assert_eq!((d, n + used), (&data[..], b.len()));
        check_prefixes(&b[n..], |p| VideoRectHeader::decode(p, &limits).map(|(_, _, u)| u));
    }
}

#[test]
fn bad_tile_headers_are_refused() {
    let limits = Limits::default();
    let base = VideoRectHeader {
        config_id: 1,
        frame: 0,
        capture_time_us: 0,
        tile: 0,
        seq: 0,
        tiles_in_frame: 1,
        flags: 0,
        codec: Codec::AV1,
        format: 0,
        length: 0,
    };
    for h in [
        VideoRectHeader { seq: 1, ..base },
        VideoRectHeader { tiles_in_frame: 0, ..base },
        VideoRectHeader { tiles_in_frame: 65, seq: 0, ..base },
        VideoRectHeader { tile: 64, ..base },
        VideoRectHeader { format: 2, ..base },
        VideoRectHeader { format: tile_format::RGB, ..base },
    ] {
        let mut b = Vec::new();
        h.encode(&mut b);
        assert!(matches!(VideoRectHeader::decode(&b, &limits), Err(DecodeError::Invalid(_))), "{h:?}");
    }
}

#[test]
fn clock_offset_bounds_error_by_half_rtt() {
    let mut rng = Rng(0x5EED_0005);
    for _ in 0..CASES {
        let true_offset = rng.below(1 << 40) as i64 - (1 << 39);
        let t0 = (1u64 << 41) + rng.below(1 << 30);
        let up = rng.below(5000);
        let down = rng.below(5000);
        let s = (t0 as i64 + up as i64 + true_offset) as u64;
        let t1 = t0 + up + down;
        let (off, rtt) = video::clock_offset(t0, s, t1);
        assert_eq!(rtt, up + down);
        assert!((off - true_offset).unsigned_abs() <= rtt / 2 + 1);
    }
}
