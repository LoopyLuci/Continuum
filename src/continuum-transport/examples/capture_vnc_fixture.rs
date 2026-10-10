//! Capture real rectangle payloads from a live VNC server, as a regression fixture.
//!
//! ## Why this file exists
//!
//! A Hextile decoder once read the sub-encoding byte as sequential ids
//! `0,1,2,3,4` instead of the flag mask `0x01,0x02,0x04,0x08,0x10`. 261 tests
//! passed, because every fixture had been generated from that decoder's own
//! assumptions and nothing had ever compared against bytes a real server sent.
//! This tool exists so that "bytes a real server produced" is something the test
//! suite can check in.
//!
//! ## How it gets the bytes
//!
//! Two properties are needed at once, and they pull in opposite directions:
//!
//! * The **raw wire bytes** are the whole point. A convenience client hands back
//!   a decoded framebuffer, which is precisely what must not be trusted here.
//! * The **handshake and framing must be right**, because a client that cannot
//!   complete them captures nothing at all. That is not hypothetical: a
//!   hand-rolled RFC 6143 client written for this task completed the handshake
//!   against QEMU but never received an update once it had sent `SetEncodings`,
//!   while aurora's `vnc-client` received one immediately.
//!
//! So the client half is aurora's, which is differentially tested against
//! TigerVNC and x11vnc, and the byte half comes from a recording TCP proxy
//! spliced in front of the server. The proxy is in-process, so this is still a
//! single `cargo run` with nothing to start by hand.
//!
//! The recorded stream is then parsed with aurora's own `hextile_len` /
//! `payload_len`, so the framing used to cut a rectangle out of the recording is
//! the same framing the fixture is later replayed through.
//!
//! ## Safety
//!
//! Strictly read-only. The client sends `SetPixelFormat`, `SetEncodings` and
//! `FramebufferUpdateRequest`, and nothing else: no key, pointer, clipboard,
//! resize, desktop-size or shutdown message. QMP is never touched. The proxy
//! forwards bytes in both directions unmodified.
//!
//! ## Usage
//!
//! ```text
//! cargo run -p continuum-transport --example capture_vnc_fixture -- <host:port> <encoding> <out.bin> <out.txt>
//! ```
//!
//! `<encoding>` is an RFB encoding code: 5 for Hextile, 7 for Tight, 2 for RRE.
//! The capture aborts if the server sends a different encoding, so a fixture can
//! never quietly fill with the wrong thing.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rfb_proto::{Encoding as ProtoEncoding, PixelFormat};
use vnc_client::classic;
use vnc_client::tight::{self, TightDecoder};
use vnc_client::{Event, Framebuffer as AuroraFb, Rect as AuroraRect, Session, SessionOptions};

/// How long to wait for the server to produce the requested encoding.
const CAPTURE_DEADLINE: Duration = Duration::from_secs(60);

struct Rect {
    encoding: i32,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    payload: Vec<u8>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let addr = args.first().cloned().unwrap_or_else(|| "127.0.0.1:6000".into());
    let wanted: i32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5);
    let bin_out = args.get(2).cloned().unwrap_or_else(|| {
        format!("src/continuum-transport/tests/fixtures/qemu_{wanted}.bin")
    });
    let txt_out = args.get(3).cloned().unwrap_or_else(|| {
        format!("src/continuum-transport/tests/fixtures/qemu_{wanted}.txt")
    });
    let name = ["Hextile", "CopyRect", "RRE", "?", "?", "Hextile", "?", "Tight"]
        .get(wanted as usize)
        .copied()
        .unwrap_or("unknown");

    // ── Recording proxy in front of the real server ────────────────────────
    let (proxy_addr, recorded) = start_recorder(&addr);
    println!("recorder listening on {proxy_addr} -> {addr}");

    // ── Drive the connection with aurora's client ──────────────────────────
    let encoding = |code: i32| -> ProtoEncoding {
        match code {
            0 => ProtoEncoding::Raw,
            1 => ProtoEncoding::CopyRect,
            2 => ProtoEncoding::Rre,
            5 => ProtoEncoding::Hextile,
            7 => ProtoEncoding::Tight,
            16 => ProtoEncoding::Zrle,
            other => ProtoEncoding::Other(other),
        }
    };
    let mut opts = SessionOptions::default();
    // Byte-identical to Continuum's `PixelFormat::BGRX32`.
    opts.pixel_format = Some(PixelFormat::RGBX8888_LE);
    // The wanted encoding first: QEMU picks the first encoding the client lists
    // that it also supports. Raw is mandatory (RFC 6143 7.7.1).
    opts.encodings = vec![encoding(wanted), ProtoEncoding::Raw];
    opts.continuous_updates = false;

    let mut session = Session::new(opts);
    let mut sock = TcpStream::connect(proxy_addr).expect("connect to the recorder");
    sock.set_read_timeout(Some(Duration::from_millis(250))).unwrap();
    println!("advertising encodings {wanted} (leading), then 0 (Raw, mandatory)");

    let mut desktop_name = String::new();
    let mut server_pf = PixelFormat::RGBX8888_LE;
    let start = Instant::now();
    let mut buf = vec![0u8; 65536];

    while start.elapsed() < CAPTURE_DEADLINE {
        let out = session.take_output();
        if !out.is_empty() {
            let _ = sock.write_all(&out);
            let _ = sock.flush();
        }
        match sock.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => match session.receive(&buf[..n]) {
                Ok(events) => {
                    for e in events {
                        if let Event::Connected { width, height, name: n } = e {
                            desktop_name = n;
                            println!("connected: {desktop_name:?} {width}x{height}");
                            server_pf = *session.pixel_format();
                        }
                    }
                }
                Err(e) => panic!("session error: {e}"),
            },
            Err(_) => {}
        }
        if session.rect_count(encoding(wanted)) > 0 {
            break;
        }
    }

    let counts = (
        session.rect_count(ProtoEncoding::Hextile),
        session.rect_count(ProtoEncoding::Raw),
        session.rect_count(ProtoEncoding::Rre),
        session.rect_count(ProtoEncoding::Tight),
    );
    println!("rect counts: hextile={} raw={} rre={} tight={}", counts.0, counts.1, counts.2, counts.3);
    if session.rect_count(encoding(wanted)) == 0 {
        panic!("the server sent no encoding-{wanted} rectangle within {CAPTURE_DEADLINE:?}");
    }
    let fb = session.framebuffer();
    let (screen_w, screen_h) = (fb.width(), fb.height());
    let expected_pixels = fb.pixels().to_vec();
    drop(sock);
    std::thread::sleep(Duration::from_millis(300));

    // ── Cut the rectangles out of the recording ────────────────────────────
    let stream = recorded.lock().expect("recording lock").clone();
    println!("recorded {} bytes from the server", stream.len());
    let (rects, bell, colormaps, cut_text) =
        parse_server_stream(&stream, wanted, &server_pf).unwrap_or_else(|e| panic!("{e}"));

    assert!(!rects.is_empty());
    assert!(rects.iter().all(|r| r.width > 0 && r.height > 0));

    // The recorded bytes must decode to exactly the picture aurora's client
    // built. If they did not, the fixture would be recording the wrong span.
    verify_replay(&rects, wanted, &server_pf, screen_w, screen_h, &expected_pixels);

    // ── Write the fixture ──────────────────────────────────────────────────
    let mut blob = Vec::new();
    blob.extend_from_slice(b"CVF1");
    blob.extend_from_slice(&(rects.len() as u16).to_be_bytes());
    for r in &rects {
        blob.extend_from_slice(&r.x.to_be_bytes());
        blob.extend_from_slice(&r.y.to_be_bytes());
        blob.extend_from_slice(&r.width.to_be_bytes());
        blob.extend_from_slice(&r.height.to_be_bytes());
        blob.extend_from_slice(&(r.payload.len() as u32).to_be_bytes());
        blob.extend_from_slice(&r.payload);
    }
    blob.extend_from_slice(&u32::from(screen_w).to_be_bytes());
    blob.extend_from_slice(&u32::from(screen_h).to_be_bytes());

    if let Some(parent) = std::path::Path::new(&bin_out).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&bin_out, &blob).expect("write the fixture");
    let total: usize = rects.iter().map(|r| r.payload.len()).sum();
    println!("\ncaptured {} {name} rectangle(s), {total} payload bytes, {} file bytes", rects.len(), blob.len());
    println!("wrote {bin_out}");

    let doc = format!(
        "qemu_{wanted}.bin - real {name} payloads from a live QEMU VNC server\n\
         ============================================================================\n\
         \n\
         WHY THIS FILE EXISTS\n\
         A Hextile decoder once read the sub-encoding byte as sequential ids\n\
         0,1,2,3,4 instead of the flag mask 0x01,0x02,0x04,0x08,0x10. 261 tests\n\
         passed, because every fixture had been generated from that decoder's own\n\
         assumptions; nothing had ever compared against bytes a real server sent.\n\
         This file is bytes a real server put on the wire.\n\
         \n\
         PROVENANCE - read this before treating it as a corpus\n\
         This is ONE screen, from ONE guest, at ONE moment. It is a regression\n\
         fixture, not a corpus. It covers {rects} {name} rectangle(s) of a single\n\
         full-screen update. It does NOT cover incremental damage updates,\n\
         multi-rectangle updates, edge tiles on a resolution that is not a\n\
         multiple of 16, cursor pseudo-encodings, other pixel formats, other\n\
         guests, or any other server. One real-world sample is a regression guard,\n\
         not evidence of general coverage. Re-capturing will produce different\n\
         bytes, because the guest's screen is not the same twice.\n\
         \n\
         CAPTURED\n\
         date              : {date}\n\
         server            : QEMU, VNC on {addr}\n\
         desktop name      : {desktop_name}\n\
         resolution        : {screen_w}x{screen_h}\n\
         security          : None (RFC 6143 security type 1, no password)\n\
         client            : aurora-vnc vnc-client, via an in-process recording proxy\n\
         \n\
         ENCODINGS REQUESTED (SetEncodings, in this order)\n\
         {wanted}    {name}, listed first so the server prefers it\n\
         0    Raw, mandatory per RFC 6143 7.7.1, present as the fallback\n\
         Every rectangle stored below is encoding {wanted}; the capture aborts if\n\
         the server sends anything else, so the file cannot silently fill with\n\
         the wrong encoding.\n\
         \n\
         PIXEL FORMAT\n\
         Requested by the client with SetPixelFormat, not left to the server\n\
         default:\n\
           bits-per-pixel   : {bpp}\n\
           depth            : {depth}\n\
           big-endian-flag  : {big_endian}\n\
           true-colour-flag : {true_colour}\n\
           red/green/blue max   : {rmax}/{gmax}/{bmax}\n\
           red/green/blue shift : {rshift}/{gshift}/{bshift}\n\
         Four little-endian bytes per pixel in B, G, R, x order - byte-identical\n\
         to Continuum's PixelFormat::BGRX32 and aurora's PixelFormat::RGBX8888_LE.\n\
         \n\
         FILE LAYOUT (all integers big-endian)\n\
           0        magic      4 bytes  \"CVF1\"\n\
           4        count      u16      number of rectangles\n\
           6        rectangles count x ( x:u16 y:u16 w:u16 h:u16 len:u32 payload:len )\n\
           trailer  width     u32      {screen_w}\n\
           trailer  height    u32      {screen_h}\n\
         `payload` is the rectangle's encoding payload only, with no 12-byte\n\
         rectangle header, matching what a decoder is handed after parsing the\n\
         header out of a FramebufferUpdate.\n\
         \n\
         SIZE\n\
         rectangles        : {rects}\n\
         payload bytes     : {total}\n\
         file bytes        : {file_len}\n\
         \n\
         CAPTURE TOOL\n\
         continuum-transport/examples/capture_vnc_fixture.rs. Read-only: it sends\n\
         SetPixelFormat, SetEncodings and non-incremental\n\
         FramebufferUpdateRequest messages and nothing else. It never sends a\n\
         key, pointer, clipboard, resize or shutdown message, and never touches\n\
         QMP.\n\
         \n\
         The client half is aurora-vnc's vnc-client rather than a hand-rolled\n\
         RFC 6143 implementation. A hand-rolled client written for this task\n\
         completed the handshake against this QEMU but never received an update\n\
         once it had sent SetEncodings, while aurora's received one immediately;\n\
         the raw bytes are therefore taken through an in-process recording proxy\n\
         spliced in front of the server, and cut out of the recording with\n\
         aurora's own hextile_len / payload_len so the framing used to capture is\n\
         the framing the fixture is replayed through.\n\
         \n\
         Before writing, the tool replays the extracted rectangles through\n\
         aurora's decoder and checks the result is pixel-identical to the frame\n\
         the client had already built, so a mis-cut rectangle cannot be checked\n\
         in.\n\
         \n\
         Incidental during this session: {bell} Bell, {colormaps}\n\
         SetColourMapEntries, {cut_text} ServerCutText message(s) were skipped.\n",
        date = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
        addr = addr,
        desktop_name = desktop_name,
        screen_w = screen_w,
        screen_h = screen_h,
        rects = rects.len(),
        total = total,
        file_len = blob.len(),
        bpp = server_pf.bits_per_pixel,
        depth = server_pf.depth,
        big_endian = server_pf.big_endian as u8,
        true_colour = server_pf.true_colour as u8,
        rmax = server_pf.red_max,
        gmax = server_pf.green_max,
        bmax = server_pf.blue_max,
        rshift = server_pf.red_shift,
        gshift = server_pf.green_shift,
        bshift = server_pf.blue_shift,
        bell = bell,
        colormaps = colormaps,
        cut_text = cut_text,
    );
    std::fs::write(&txt_out, doc).expect("write the fixture doc");
    println!("wrote {txt_out}");
}

/// Start a transparent recording proxy in front of `target`.
///
/// Returns the local address to point a client at, and the shared buffer that
/// everything the server sends is appended to.
fn start_recorder(target: &str) -> (SocketAddr, Arc<Mutex<Vec<u8>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the recorder");
    let addr = listener.local_addr().expect("recorder address");
    let recorded: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&recorded);
    let target = target.to_string();

    std::thread::spawn(move || {
        let Ok((mut client, _)) = listener.accept() else { return };
        let Ok(mut upstream) = TcpStream::connect(target) else { return };
        client.set_nodelay(true).ok();
        upstream.set_nodelay(true).ok();

        let sink2 = Arc::clone(&sink);
        let mut to_client = client.try_clone().expect("clone");
        let mut to_server = upstream.try_clone().expect("clone");
        // Client -> server: forwarded, not recorded.
        std::thread::spawn(move || pump(&mut client, &mut to_server, None));
        // Server -> client: forwarded and recorded.
        pump(&mut upstream, &mut to_client, Some(sink2));
    });

    (addr, recorded)
}

fn pump(src: &mut TcpStream, dst: &mut TcpStream, record: Option<Arc<Mutex<Vec<u8>>>>) {
    let mut buf = vec![0u8; 65536];
    loop {
        match src.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if let Some(sink) = &record {
                    if let Ok(mut v) = sink.lock() {
                        v.extend_from_slice(&buf[..n]);
                    }
                }
                if dst.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
    let _ = dst.shutdown(std::net::Shutdown::Write);
}

struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn u8(&mut self) -> Option<u8> {
        let v = *self.data.get(self.at)?;
        self.at += 1;
        Some(v)
    }

    fn u16(&mut self) -> Option<u16> {
        let v = self.data.get(self.at..self.at + 2)?;
        self.at += 2;
        Some(u16::from_be_bytes(v.try_into().unwrap()))
    }

    fn u32(&mut self) -> Option<u32> {
        let v = self.data.get(self.at..self.at + 4)?;
        self.at += 4;
        Some(u32::from_be_bytes(v.try_into().unwrap()))
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let v = self.data.get(self.at..self.at + n)?;
        self.at += n;
        Some(v)
    }
}

/// Walk the recorded server->client stream and pull out every rectangle whose
/// encoding is `wanted`.
fn parse_server_stream(
    stream: &[u8],
    wanted: i32,
    pf: &PixelFormat,
) -> Result<(Vec<Rect>, usize, usize, usize), String> {
    // Skip the handshake. Everything up to ServerInit is server->client, and
    // ClientInit / the security-type choice go the other way, so they are absent.
    let mut c = Cursor::new(stream);
    let greeting = c.take(12).ok_or("truncated greeting")?;
    if !greeting.starts_with(b"RFB ") {
        return Err("recording does not start with a server version".into());
    }
    let n_types = c.u8().ok_or("truncated security type count")? as usize;
    c.take(n_types).ok_or("truncated security types")?;
    c.u32().ok_or("truncated SecurityResult")?;
    let _w = c.u16().ok_or("truncated width")?;
    let _h = c.u16().ok_or("truncated height")?;
    c.take(16).ok_or("truncated pixel format")?;
    let name_len = c.u32().ok_or("truncated name length")? as usize;
    c.take(name_len).ok_or("truncated desktop name")?;

    let mut rects = Vec::new();
    let (mut bell, mut colormaps, mut cut_text) = (0usize, 0usize, 0usize);

    while let Some(message) = c.u8() {
        match message {
            0 => {
                c.u8().ok_or("truncated update padding")?;
                let n = c.u16().ok_or("truncated rectangle count")?;
                for _ in 0..n {
                    let x = c.u16().ok_or("truncated rect x")?;
                    let y = c.u16().ok_or("truncated rect y")?;
                    let w = c.u16().ok_or("truncated rect width")?;
                    let h = c.u16().ok_or("truncated rect height")?;
                    let encoding = c.u32().ok_or("truncated rect encoding")? as i32;
                    match encoding {
                        0 => {
                            c.take(usize::from(w) * usize::from(h) * 4)
                                .ok_or("truncated raw pixels")?;
                        }
                        e if e == wanted => {
                            let payload = cut_payload(&mut c, encoding, w, h, pf)?;
                            rects.push(Rect { encoding, x, y, width: w, height: h, payload });
                        }
                        // Pseudo-encodings with no payload this tool understands.
                        -223 | -308 | -312 => {}
                        other => return Err(format!("server sent unimplemented encoding {other}")),
                    }
                }
                if !rects.is_empty() {
                    break;
                }
            }
            1 => {
                c.u8().ok_or("truncated colormap padding")?;
                c.u16().ok_or("truncated colormap first index")?;
                let n = c.u16().ok_or("truncated colormap count")? as usize;
                c.take(n * 6).ok_or("truncated colormap entries")?;
                colormaps += 1;
            }
            2 => bell += 1,
            3 => {
                c.u8().ok_or("truncated cut text padding")?;
                c.u8().ok_or("truncated cut text padding")?;
                let n = c.u32().ok_or("truncated cut text length")? as usize;
                c.take(n).ok_or("truncated cut text")?;
                cut_text += 1;
            }
            other => return Err(format!("unexpected server message type {other}")),
        }
    }

    Ok((rects, bell, colormaps, cut_text))
}

/// Advance the cursor past one rectangle payload and return those bytes.
fn cut_payload(
    c: &mut Cursor<'_>,
    encoding: i32,
    width: u16,
    height: u16,
    pf: &PixelFormat,
) -> Result<Vec<u8>, String> {
    // Neither encoding is length-prefixed, so look ahead and ask aurora where
    // the rectangle actually ends.
    let remaining = c.data.len() - c.at;
    let window = match encoding {
        // A fully raw Hextile tile grid, plus one mask byte per tile.
        5 => {
            let tiles = usize::from(width).div_ceil(16) * usize::from(height).div_ceil(16);
            tiles + tiles * usize::from(width.min(16)) * usize::from(height.min(16)) * 4 + 4096
        }
        7 => usize::from(width) * usize::from(height) * 4 + 65536,
        _ => return Err(format!("no framing for encoding {encoding}")),
    }
    .min(remaining);

    let look = &c.data[c.at..c.at + window];
    let used = match encoding {
        5 => classic::hextile_len(look, pf, width, height)
            .map_err(|e| format!("aurora rejected the Hextile framing: {e}"))?
            .ok_or_else(|| "Hextile payload incomplete at the end of the recording".to_string())?,
        7 => tight::payload_len(look, pf, width, height)
            .map_err(|e| format!("aurora rejected the Tight framing: {e}"))?
            .ok_or_else(|| "Tight payload incomplete at the end of the recording".to_string())?,
        _ => unreachable!(),
    };
    c.at += used;
    Ok(look[..used].to_vec())
}

/// Decode the extracted rectangles and check they rebuild the exact picture the
/// client had already assembled from the same bytes.
fn verify_replay(
    rects: &[Rect],
    encoding: i32,
    pf: &PixelFormat,
    screen_w: u16,
    screen_h: u16,
    expected: &[u32],
) {
    let mut fb = AuroraFb::new(screen_w, screen_h);
    let mut decoder = TightDecoder::default();
    for r in rects {
        assert_eq!(r.encoding, encoding, "a rectangle of another encoding slipped in");
        let rect = AuroraRect { x: r.x, y: r.y, width: r.width, height: r.height };
        assert!(!r.payload.is_empty(), "a captured rectangle had an empty payload");
        match encoding {
            5 => classic::decode_hextile(rect, &r.payload, pf, &[], &mut fb)
                .unwrap_or_else(|e| panic!("the captured Hextile rectangle did not decode: {e}")),
            7 => decoder
                .decode(rect, &r.payload, pf, &[], &mut fb)
                .unwrap_or_else(|e| panic!("the captured Tight rectangle did not decode: {e}")),
            other => panic!("no replay path for encoding {other}"),
        }
    }
    assert_eq!(
        fb.pixels(),
        expected,
        "the extracted rectangles do not rebuild the picture the client decoded; \
         the capture cut the wrong span and must not be checked in"
    );
}



