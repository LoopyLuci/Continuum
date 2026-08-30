use rand::Rng;
use std::net::SocketAddr;

/// STUN message class
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StunClass {
    Request = 0b00,
    Indication = 0b01,
    ResponseSuccess = 0b10,
    ResponseError = 0b11,
}

impl StunClass {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val & 0b11 {
            0b00 => Some(Self::Request),
            0b01 => Some(Self::Indication),
            0b10 => Some(Self::ResponseSuccess),
            0b11 => Some(Self::ResponseError),
            _ => None,
        }
    }
    
    pub fn as_u16(self) -> u16 {
        self as u16
    }
}

/// STUN method (Binding only)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StunMethod {
    Binding = 0b0000_0000_0001,
}

impl StunMethod {
    pub fn from_u16(val: u16) -> Option<Self> {
        if val == 0x0001 {
            Some(Self::Binding)
        } else {
            None
        }
    }

    pub fn as_u16(self) -> u16 {
        self as u16
    }
}

/// STUN attribute
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StunAttribute {
    MappedAddress(SocketAddr),
    XorMappedAddress(SocketAddr),
    SourceAddress(SocketAddr),
    ChangedAddress(SocketAddr),
    Software(String),
    Unknown(u16, Vec<u8>),
}

/// Mapped address returned by STUN server
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappedAddress {
    pub address: SocketAddr,
}

/// STUN message (RFC 5389)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StunMessage {
    pub class: StunClass,
    pub method: StunMethod,
    pub transaction_id: [u8; 12],
    pub attributes: Vec<StunAttribute>,
}

impl StunMessage {
    /// Create a new Binding Request
    pub fn binding_request() -> Self {
        let mut rng = rand::thread_rng();
        let mut transaction_id = [0u8; 12];
        rng.fill(&mut transaction_id);
        Self {
            class: StunClass::Request,
            method: StunMethod::Binding,
            transaction_id,
            attributes: Vec::new(),
        }
    }

    /// Serialize to bytes for transmission
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(2048);
        let msg_type = self.encode_message_type();
        buf.extend_from_slice(&msg_type.to_be_bytes());
        let len_pos = buf.len();
        buf.extend_from_slice(&[0, 0]);
        buf.extend_from_slice(&[0x21, 0x12, 0xA4, 0x42]);
        buf.extend_from_slice(&self.transaction_id);

        let mut attr_len: u16 = 0;
        for attr in &self.attributes {
            let attr_bytes = Self::serialize_attribute(attr);
            attr_len += attr_bytes.len() as u16;
            buf.extend_from_slice(&attr_bytes);
        }

        buf[len_pos..len_pos + 2].copy_from_slice(&attr_len.to_be_bytes());
        buf
    }

    fn encode_message_type(&self) -> u16 {
        let m = self.method.as_u16();
        let c = self.class.as_u16();
        (m & 0x000F)
            | ((m & 0x0070) << 1)
            | ((m & 0x0F80) << 2)
            | ((c & 0x01) << 4)
            | ((c & 0x02) << 7)
    }

    /// Parse from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self, StunError> {
        if data.len() < 20 {
            return Err(StunError::TooShort);
        }

        let msg_type = u16::from_be_bytes([data[0], data[1]]);
        if msg_type & 0xC000 != 0 {
            return Err(StunError::InvalidMessageType);
        }

        // Decode class: bits at positions 4 and 7
        let class = StunClass::from_u8(
            ((msg_type >> 4) & 0x01) as u8 | ((msg_type >> 7) & 0x02) as u8,
        )
        .ok_or(StunError::InvalidClass)?;
        
        // Decode method: bits at positions 0-3, 5-6, 8-13
        let method_bits =
            (msg_type & 0x000F) | ((msg_type >> 1) & 0x0070) | ((msg_type >> 2) & 0x0F80);
        let method = StunMethod::from_u16(method_bits).ok_or(StunError::InvalidMethod)?;

        let msg_len = u16::from_be_bytes([data[2], data[3]]) as usize;
        if data.len() < 20 + msg_len {
            return Err(StunError::Incomplete);
        }

        if data[4..8] != [0x21, 0x12, 0xA4, 0x42] {
            return Err(StunError::InvalidMagicCookie);
        }

        let mut transaction_id = [0u8; 12];
        transaction_id.copy_from_slice(&data[8..20]);

        let mut attributes = Vec::new();
        let mut pos = 20;
        while pos < 20 + msg_len {
            if data.len() < pos + 4 {
                break;
            }
            let attr_type = u16::from_be_bytes([data[pos], data[pos + 1]]);
            let attr_len = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;
            let padded_len = attr_len + ((4 - (attr_len % 4)) % 4);

            if data.len() < pos + 4 + padded_len {
                break;
            }

            let attr_data = &data[pos + 4..pos + 4 + attr_len];
            if let Some(attr) = Self::parse_attribute(attr_type, attr_data) {
                attributes.push(attr);
            }

            pos += 4 + padded_len;
        }

        Ok(Self {
            class,
            method,
            transaction_id,
            attributes,
        })
    }

    fn serialize_attribute(attr: &StunAttribute) -> Vec<u8> {
        let (type_id, value) = match attr {
            StunAttribute::MappedAddress(addr) => (0x0001u16, Self::serialize_address(addr)),
            StunAttribute::XorMappedAddress(addr) => (0x0020u16, Self::serialize_address(addr)),
            StunAttribute::SourceAddress(addr) => (0x0004u16, Self::serialize_address(addr)),
            StunAttribute::ChangedAddress(addr) => (0x0005u16, Self::serialize_address(addr)),
            StunAttribute::Software(s) => (0x8022u16, s.as_bytes().to_vec()),
            StunAttribute::Unknown(_, _) => return Vec::new(),
        };

        let mut buf = Vec::new();
        buf.extend_from_slice(&type_id.to_be_bytes());
        buf.extend_from_slice(&(value.len() as u16).to_be_bytes());
        buf.extend_from_slice(&value);
        let pad = (4 - (value.len() % 4)) % 4;
        buf.extend(std::iter::repeat(0).take(pad));
        buf
    }

    fn serialize_address(addr: &SocketAddr) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(0x00);
        buf.push(0x01);
        match addr {
            SocketAddr::V4(v4) => {
                buf.extend_from_slice(&u16::to_be_bytes(v4.port()));
                buf.extend_from_slice(&v4.ip().octets());
            }
            SocketAddr::V6(_) => {
                // TODO: IPv6 support
            }
        }
        buf
    }

    fn parse_attribute(type_id: u16, data: &[u8]) -> Option<StunAttribute> {
        match type_id {
            0x0001 if data.len() >= 8 => {
                let port = u16::from_be_bytes([data[2], data[3]]);
                let ip = std::net::Ipv4Addr::new(data[4], data[5], data[6], data[7]);
                Some(StunAttribute::MappedAddress(SocketAddr::new(ip.into(), port)))
            }
            0x0020 if data.len() >= 8 => {
                let port = u16::from_be_bytes([data[2], data[3]]) ^ 0x2112;
                let ip_bytes = [
                    data[4] ^ 0x21,
                    data[5] ^ 0x12,
                    data[6] ^ 0xA4,
                    data[7] ^ 0x42,
                ];
                let ip =
                    std::net::Ipv4Addr::new(ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3]);
                Some(StunAttribute::XorMappedAddress(SocketAddr::new(
                    ip.into(),
                    port,
                )))
            }
            0x0004 if data.len() >= 8 => {
                let port = u16::from_be_bytes([data[2], data[3]]);
                let ip = std::net::Ipv4Addr::new(data[4], data[5], data[6], data[7]);
                Some(StunAttribute::SourceAddress(SocketAddr::new(ip.into(), port)))
            }
            0x0005 if data.len() >= 8 => {
                let port = u16::from_be_bytes([data[2], data[3]]);
                let ip = std::net::Ipv4Addr::new(data[4], data[5], data[6], data[7]);
                Some(StunAttribute::ChangedAddress(SocketAddr::new(
                    ip.into(),
                    port,
                )))
            }
            0x8022 => Some(StunAttribute::Software(
                String::from_utf8_lossy(data).to_string(),
            )),
            _ => Some(StunAttribute::Unknown(type_id, data.to_vec())),
        }
    }

    /// Get the mapped address from a response
    pub fn get_mapped_address(&self) -> Option<SocketAddr> {
        for attr in &self.attributes {
            match attr {
                StunAttribute::MappedAddress(addr) => return Some(*addr),
                StunAttribute::XorMappedAddress(addr) => return Some(*addr),
                _ => {}
            }
        }
        None
    }
}

/// STUN errors
#[derive(Debug, thiserror::Error)]
pub enum StunError {
    #[error("STUN message too short")]
    TooShort,
    #[error("Invalid message type")]
    InvalidMessageType,
    #[error("Invalid class")]
    InvalidClass,
    #[error("Invalid method")]
    InvalidMethod,
    #[error("Incomplete message")]
    Incomplete,
    #[error("Invalid magic cookie")]
    InvalidMagicCookie,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binding_request_serialization() {
        let msg = StunMessage::binding_request();
        let bytes = msg.to_bytes();
        assert_eq!(bytes.len(), 20);
        assert_eq!(bytes[0] & 0xC0, 0);
        assert_eq!(&bytes[4..8], &[0x21, 0x12, 0xA4, 0x42]);
    }

    #[test]
    fn test_parse_binding_response() {
        // Binding Success Response (0x0101)
        let mut data = vec![0x01, 0x01, 0x00, 0x0C, 0x21, 0x12, 0xA4, 0x42];
        data.extend_from_slice(&[0u8; 12]); // Transaction ID
        // XOR-MAPPED-ADDRESS: port 5000 ^ 0x2112 = 0x329A, IP 127.0.0.1 ^ magic = 0x5E12A443
        data.extend_from_slice(&[
            0x00, 0x20, 0x00, 0x08, 0x00, 0x01, 0x32, 0x9A, 0x5E, 0x12, 0xA4, 0x43,
        ]);

        let msg = StunMessage::from_bytes(&data).unwrap();
        assert_eq!(msg.class, StunClass::ResponseSuccess);
        assert_eq!(msg.method, StunMethod::Binding);
        let addr = msg.get_mapped_address().unwrap();
        assert_eq!(addr, "127.0.0.1:5000".parse().unwrap());
    }

    #[test]
    fn test_roundtrip() {
        let msg = StunMessage::binding_request();
        let bytes = msg.to_bytes();
        let parsed = StunMessage::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.class, msg.class);
        assert_eq!(parsed.method, msg.method);
        assert_eq!(parsed.transaction_id, msg.transaction_id);
    }

    #[test]
    fn test_invalid_stun() {
        let data = vec![0xC0, 0x00, 0x00, 0x00, 0x21, 0x12, 0xA4, 0x42];
        let result = StunMessage::from_bytes(&data);
        assert!(result.is_err());
    }
}
