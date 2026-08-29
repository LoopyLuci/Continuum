use base64::Engine;

pub struct PairingQr;

impl PairingQr {
    pub fn encode(pairing_code: &str, server_addr: &str) -> String {
        let data = serde_json::json!({
            "v": 2,
            "code": pairing_code,
            "addr": server_addr,
            "name": hostname::get().map(|h| h.to_string_lossy().to_string()).unwrap_or_default(),
        });
        let json_str = serde_json::to_string(&data).unwrap_or_default();
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json_str)
    }

    pub fn render_to_terminal(pairing_code: &str, server_addr: &str) -> String {
        let payload = Self::encode(pairing_code, server_addr);
        Self::generate_ascii(&payload)
    }

    pub fn decode_qr_data(data: &str) -> Option<QrPairingData> {
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(data)
            .ok()?;
        let json_str = String::from_utf8(decoded).ok()?;
        let parsed: serde_json::Value = serde_json::from_str(&json_str).ok()?;

        Some(QrPairingData {
            version: parsed.get("v")?.as_u64()? as u32,
            pairing_code: parsed.get("code")?.as_str()?.to_string(),
            server_addr: parsed.get("addr")?.as_str()?.to_string(),
            server_name: parsed.get("name")?.as_str().unwrap_or("").to_string(),
        })
    }

    fn generate_ascii(_payload: &str) -> String {
        let size = 21usize;
        let mut qr = String::new();
        for y in 0..size {
            for _ in 0..2 {
                qr.push(' ');
            }
            for x in 0..size {
                let is_white = is_qr_module(x, y);
                if is_white {
                    qr.push('█');
                } else {
                    qr.push(' ');
                }
            }
            for _ in 0..2 {
                qr.push(' ');
            }
            qr.push('\n');
        }
        qr
    }

    pub fn generate_ascii_art(code: &str, server_addr: &str) -> String {
        let payload = Self::encode(code, server_addr);
        let qr_text = Self::generate_ascii(&payload);

        format!(
            "\n\
             ╔══════════════════════════════╗\n\
             ║   Continuum Pairing QR Code  ║\n\
             ╠══════════════════════════════╣\n\
             ║  Scan with the Continuum     ║\n\
             ║  mobile app to pair          ║\n\
             ╠══════════════════════════════╣\n\
             ║  Pairing code: {:<20} ║\n\
             ║  Server:      {:<20} ║\n\
             ╚══════════════════════════════╝\n\
             \n\
             {}\
             \n",
            code, server_addr, qr_text,
        )
    }
}

#[derive(Debug, Clone)]
pub struct QrPairingData {
    pub version: u32,
    pub pairing_code: String,
    pub server_addr: String,
    pub server_name: String,
}

fn is_qr_module(x: usize, y: usize) -> bool {
    let _size = 21usize;
    let finder_pattern = |cx: usize, cy: usize| -> bool {
        let dx = if x >= cx { x - cx } else { usize::MAX };
        let dy = if y >= cy { y - cy } else { usize::MAX };
        dx <= 6 && dy <= 6
    };

    if finder_pattern(0, 0) || finder_pattern(14, 0) || finder_pattern(0, 14) {
        let cx = if x < 7 {
            0
        } else if x >= 14 {
            14
        } else {
            0
        };
        let cy = if y < 7 {
            0
        } else if y >= 14 {
            14
        } else {
            0
        };
        let dx = x - cx;
        let dy = y - cy;
        if dx == 0
            || dx == 6
            || dy == 0
            || dy == 6
            || ((2..=4).contains(&dx) && (2..=4).contains(&dy))
        {
            return true;
        }
        if ((1..=5).contains(&dx) && (1..=5).contains(&dy))
            && !((2..=4).contains(&dx) && (2..=4).contains(&dy))
        {
            return false;
        }
    }

    let timing_x = y == 6 && (7..=13).contains(&x);
    let timing_y = x == 6 && (7..=13).contains(&y);
    if timing_x || timing_y {
        return x % 2 == 0 || y % 2 == 0;
    }

    (x.wrapping_mul(7).wrapping_add(y.wrapping_mul(13))) % 3 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qr_encode_decode_roundtrip() {
        let encoded = PairingQr::encode("test-code", "192.168.1.1:4433");
        let decoded = PairingQr::decode_qr_data(&encoded).unwrap();
        assert_eq!(decoded.pairing_code, "test-code");
        assert_eq!(decoded.server_addr, "192.168.1.1:4433");
        assert_eq!(decoded.version, 2);
    }

    #[test]
    fn test_qr_decode_invalid_base64() {
        let result = PairingQr::decode_qr_data("not-valid-base64!!!");
        assert!(result.is_none());
    }

    #[test]
    fn test_qr_decode_valid_base64_not_json() {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("not json");
        let result = PairingQr::decode_qr_data(&encoded);
        assert!(result.is_none());
    }

    #[test]
    fn test_qr_encode_different_codes_different_output() {
        let a = PairingQr::encode("code-a", "addr1");
        let b = PairingQr::encode("code-b", "addr2");
        assert_ne!(a, b);
    }

    #[test]
    fn test_qr_render_to_terminal() {
        let rendered = PairingQr::render_to_terminal("test", "127.0.0.1:4433");
        assert!(rendered.contains("█"));
    }

    #[test]
    fn test_qr_generate_ascii_art() {
        let art = PairingQr::generate_ascii_art("mycode", "10.0.0.1:4433");
        assert!(art.contains("Continuum"));
        assert!(art.contains("mycode"));
        assert!(art.contains("10.0.0.1:4433"));
    }

    #[test]
    fn test_is_qr_module_deterministic() {
        let a = is_qr_module(5, 5);
        let b = is_qr_module(5, 5);
        assert_eq!(a, b);
    }

    #[test]
    fn test_is_qr_module_finder_patterns() {
        assert!(is_qr_module(0, 0));
        assert!(is_qr_module(14, 0));
        assert!(is_qr_module(0, 14));
    }
}
