//! A small, fast, deterministic colour table (insertion order is kept, so
//! encoder output is reproducible byte for byte).

pub(crate) struct Palette {
    keys: [u32; 512],
    idx: [u16; 512], // 0 = empty, else index + 1
    pub colours: Vec<u32>,
    cap: usize,
}

impl Palette {
    /// Tracks at most `cap` (<= 256) colours.
    pub fn new(cap: usize) -> Self {
        Palette { keys: [0; 512], idx: [0; 512], colours: Vec::with_capacity(cap), cap: cap.min(256) }
    }

    fn slot(v: u32) -> usize {
        (v.wrapping_mul(0x9E37_79B1) >> 23) as usize
    }

    /// Index of `v`, inserting it. `None` once there are more than `cap`
    /// colours (the table is then "full" and stays so).
    pub fn insert(&mut self, v: u32) -> Option<u8> {
        if self.colours.len() > self.cap {
            return None;
        }
        let mut s = Self::slot(v);
        loop {
            match self.idx[s] {
                0 => {
                    if self.colours.len() == self.cap {
                        self.colours.push(v); // marks overflow
                        return None;
                    }
                    self.keys[s] = v;
                    self.colours.push(v);
                    self.idx[s] = self.colours.len() as u16;
                    return Some((self.colours.len() - 1) as u8);
                }
                i if self.keys[s] == v => return Some((i - 1) as u8),
                _ => s = (s + 1) & 511,
            }
        }
    }

    pub fn get(&self, v: u32) -> u8 {
        let mut s = Self::slot(v);
        loop {
            if self.idx[s] != 0 && self.keys[s] == v {
                return (self.idx[s] - 1) as u8;
            }
            s = (s + 1) & 511;
        }
    }

    pub fn overflowed(&self) -> bool {
        self.colours.len() > self.cap
    }

    pub fn len(&self) -> usize {
        self.colours.len()
    }
}
