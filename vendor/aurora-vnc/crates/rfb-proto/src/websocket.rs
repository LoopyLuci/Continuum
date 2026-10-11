//! WebSocket binary framing for an RFB byte stream (RFC 6455).
//!
//! The handshake is the HTTP upgrade. After that, RFB bytes are binary
//! frames. This module does not open sockets.

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// `Sec-WebSocket-Accept` for a client key (RFC 6455 section 1.3).
pub fn accept_key(client_key: &str) -> String {
    b64(&sha1(format!("{client_key}{GUID}").as_bytes()))
}

fn sha1(data: &[u8]) -> [u8; 20] {
    fn rol(x: u32, n: u32) -> u32 {
        x.rotate_left(n)
    }
    let mut msg = data.to_vec();
    let bits = (data.len() as u64) * 8;
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    let mut h = [0x67452301u32, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = rol(w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16], 1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = rol(a, 5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(w[i]);
            e = d;
            d = c;
            c = rol(b, 30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8) | bytes[i + 2] as u32;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(T[((n >> 6) & 63) as usize] as char);
        out.push(T[(n & 63) as usize] as char);
        i += 3;
    }
    if bytes.len() - i == 1 {
        let n = (bytes[i] as u32) << 16;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push('=');
        out.push('=');
    } else if bytes.len() - i == 2 {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8);
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(T[((n >> 6) & 63) as usize] as char);
        out.push('=');
    }
    out
}

pub fn client_key(request: &str) -> Option<&str> {
    request.lines().find_map(|l| l.split_once("Sec-WebSocket-Key:").map(|(_, v)| v.trim()))
}

pub fn accept_upgrade(request: &str, accept_key: &str) -> Option<String> {
    if !request.contains("Upgrade: websocket") && !request.contains("Upgrade: WebSocket") {
        return None;
    }
    Some(format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept_key}\r\n\r\n"
    ))
}

/// Unmasked server binary frame.
pub fn encode_binary(payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0x82];
    if payload.len() < 126 {
        out.push(payload.len() as u8);
    } else if payload.len() <= u16::MAX as usize {
        out.push(126);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        out.push(127);
        out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    out.extend_from_slice(payload);
    out
}

pub fn decode_binary(frame: &[u8]) -> Option<&[u8]> {
    if frame.len() < 2 || frame[0] & 0x0f != 2 {
        return None;
    }
    let len = frame[1] & 0x7f;
    let (n, rest) = if len < 126 {
        (len as usize, 2)
    } else if len == 126 && frame.len() >= 4 {
        (u16::from_be_bytes([frame[2], frame[3]]) as usize, 4)
    } else {
        return None;
    };
    frame.get(rest..rest + n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_binary_frame() {
        let frame = encode_binary(b"RFB");
        assert_eq!(decode_binary(&frame), Some(&b"RFB"[..]));
    }

    #[test]
    fn upgrade_requires_websocket() {
        assert!(accept_upgrade("GET / HTTP/1.1\r\nUpgrade: websocket\r\n\r\n", "abc").is_some());
        assert!(accept_upgrade("GET / HTTP/1.1\r\n\r\n", "abc").is_none());
    }

    #[test]
    fn rfc_accept_key() {
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }
}
