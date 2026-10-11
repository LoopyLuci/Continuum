//! Cross-version selection. Both sides list what they can do, most preferred
//! first, and [`select`] picks the first local choice the peer also offered.
//!
//! Unknown security types and encodings stay as `Other` and are skipped, so a
//! future peer advertising a new method does not break an older Aurora build.
//! Raw is always last among frame encodings: every VNC server since 3.3 can
//! send it.

use crate::types::{Encoding, SecurityType};

/// Security types Aurora can complete, strongest interoperable first.
/// `None` is absent: a peer that offers only `None` is refused unless the
/// caller adds it.
pub const SECURITY_LADDER: &[SecurityType] = &[SecurityType::VeNCrypt, SecurityType::VncAuth];

/// Frame encodings a viewer should advertise. Pseudo-encodings are not here;
/// callers append the ones they implement.
pub const ENCODING_LADDER: &[Encoding] = &[Encoding::Tight, Encoding::Zrle, Encoding::CopyRect, Encoding::Raw];

/// First item of `prefer` that `offered` contains.
pub fn select<T: Copy + PartialEq>(prefer: &[T], offered: &[T]) -> Option<T> {
    prefer.iter().copied().find(|t| offered.contains(t))
}

/// Security type to use. `local` is this side's capability list.
pub fn select_security(local: &[SecurityType], offered: &[SecurityType]) -> Option<SecurityType> {
    select(local, offered)
}

/// Encodings to advertise: the ladder, then any extra pseudo-encodings, Raw
/// kept, duplicates dropped.
pub fn advertise_encodings(extra: &[Encoding]) -> Vec<Encoding> {
    let mut out = Vec::new();
    for e in ENCODING_LADDER.iter().chain(extra) {
        if !out.contains(e) {
            out.push(*e);
        }
    }
    if !out.contains(&Encoding::Raw) {
        out.push(Encoding::Raw);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_vencrypt_but_falls_back_to_classic() {
        let offered = [SecurityType::VncAuth, SecurityType::VeNCrypt];
        assert_eq!(select_security(SECURITY_LADDER, &offered), Some(SecurityType::VeNCrypt));
        let old = [SecurityType::VncAuth];
        assert_eq!(select_security(SECURITY_LADDER, &old), Some(SecurityType::VncAuth));
    }

    #[test]
    fn unknown_security_is_skipped() {
        let offered = [SecurityType::Other(30), SecurityType::VncAuth];
        assert_eq!(select_security(SECURITY_LADDER, &offered), Some(SecurityType::VncAuth));
        assert_eq!(select_security(SECURITY_LADDER, &[SecurityType::Other(17)]), None);
    }

    #[test]
    fn raw_is_always_advertised() {
        let ads = advertise_encodings(&[Encoding::Cursor]);
        assert_eq!(*ads.last().unwrap(), Encoding::Cursor);
        assert!(ads.contains(&Encoding::Raw));
        assert_eq!(ads.iter().filter(|e| **e == Encoding::Raw).count(), 1);
    }
}
