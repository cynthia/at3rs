pub struct BitReader<'a> {
    data: &'a [u8],
    bit_ptr: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, bit_ptr: 0 }
    }

    pub fn read_bits(&mut self, n: usize) -> u32 {
        let mut val = 0u32;
        for _ in 0..n {
            let byte_idx = self.bit_ptr / 8;
            let bit_idx = 7 - (self.bit_ptr % 8);
            if byte_idx < self.data.len() {
                let bit = (self.data[byte_idx] >> bit_idx) & 1;
                val = (val << 1) | (bit as u32);
            }
            self.bit_ptr += 1;
        }
        val
    }
}

pub struct BitWriter {
    pub data: Vec<u8>,
    bit_ptr: usize,
}

impl BitWriter {
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            bit_ptr: 0,
        }
    }

    pub fn write_bits(&mut self, val: u32, n: usize) {
        for i in 0..n {
            let bit_idx = 7 - (self.bit_ptr % 8);
            let byte_idx = self.bit_ptr / 8;

            if byte_idx >= self.data.len() {
                self.data.push(0);
            }

            let bit = (val >> (n - 1 - i)) & 1;
            if bit == 1 {
                self.data[byte_idx] |= 1 << bit_idx;
            }
            self.bit_ptr += 1;
        }
    }

    pub fn flush(&self) -> &[u8] {
        &self.data
    }

    pub fn bits_written(&self) -> usize {
        self.bit_ptr
    }
}

pub struct VlcEntry {
    pub symbol: i16,
    pub len: u8,
    pub code: u32,
}

pub struct VlcTable {
    pub entries: Vec<VlcEntry>,
}

impl VlcTable {
    pub fn new(raw_tabs: &[(i16, u8)]) -> Self {
        let mut entries: Vec<VlcEntry> = raw_tabs
            .iter()
            .map(|&(s, l)| VlcEntry {
                symbol: s,
                len: l,
                code: 0,
            })
            .collect();

        let mut current_code = 0u32;
        let mut last_len = 0u8;

        for entry in entries.iter_mut() {
            if entry.len > 0 {
                current_code <<= entry.len - last_len;
                entry.code = current_code;
                current_code += 1;
                last_len = entry.len;
            }
        }
        Self { entries }
    }

    pub fn from_codes(raw_tabs: &[(u32, u8)]) -> Self {
        let entries = raw_tabs
            .iter()
            .enumerate()
            .map(|(symbol, &(code, len))| VlcEntry {
                symbol: symbol as i16,
                len,
                code,
            })
            .collect();
        Self { entries }
    }

    pub fn decode(&self, reader: &mut BitReader) -> Option<i16> {
        let mut code = 0u32;
        let start_bit = reader.bit_ptr;

        for bit_len in 1..=16 {
            reader.bit_ptr = start_bit;
            code = reader.read_bits(bit_len as usize);

            for entry in &self.entries {
                if entry.len == bit_len && entry.code == code {
                    return Some(entry.symbol);
                }
            }
        }
        None
    }

    pub fn encode(&self, symbol: i16, writer: &mut BitWriter) -> bool {
        for entry in &self.entries {
            if entry.symbol == symbol {
                writer.write_bits(entry.code, entry.len as usize);
                return true;
            }
        }
        false
    }

    pub fn bit_len(&self, symbol: i16) -> Option<usize> {
        self.entries
            .iter()
            .find(|entry| entry.symbol == symbol)
            .map(|entry| entry.len as usize)
    }
}

pub const HUFF_TAB_SIZES: [usize; 7] = [9, 5, 7, 9, 15, 31, 63];

pub const ATRAC3_HUFFTABS_RAW: &[(i16, u8)] = &[
    /* Table 0 (9 entries) */
    (31, 1),
    (32, 3),
    (33, 3),
    (34, 4),
    (35, 4),
    (36, 5),
    (37, 5),
    (38, 5),
    (39, 5),
    /* Table 1 (5 entries) */
    (31, 1),
    (32, 3),
    (30, 3),
    (33, 3),
    (29, 3),
    /* Table 2 (7 entries) */
    (31, 1),
    (32, 3),
    (30, 3),
    (33, 4),
    (29, 4),
    (34, 4),
    (28, 4),
    /* Table 3 (9 entries) */
    (31, 1),
    (32, 3),
    (30, 3),
    (33, 4),
    (29, 4),
    (34, 5),
    (28, 5),
    (35, 5),
    (27, 5),
    /* Table 4 (15 entries) */
    (31, 2),
    (32, 3),
    (30, 3),
    (33, 4),
    (29, 4),
    (34, 4),
    (28, 4),
    (38, 4),
    (24, 4),
    (35, 5),
    (27, 5),
    (36, 6),
    (26, 6),
    (37, 6),
    (25, 6),
    /* Table 5 (31 entries) */
    (31, 3),
    (32, 4),
    (30, 4),
    (33, 4),
    (29, 4),
    (34, 4),
    (28, 4),
    (46, 4),
    (16, 4),
    (35, 5),
    (27, 5),
    (36, 5),
    (26, 5),
    (37, 5),
    (25, 5),
    (38, 6),
    (24, 6),
    (39, 6),
    (23, 6),
    (40, 6),
    (22, 6),
    (41, 6),
    (21, 6),
    (42, 7),
    (20, 7),
    (43, 7),
    (19, 7),
    (44, 7),
    (18, 7),
    (45, 7),
    (17, 7),
    /* Table 6 (63 entries) */
    (31, 3),
    (62, 4),
    (0, 4),
    (32, 5),
    (30, 5),
    (33, 5),
    (29, 5),
    (34, 5),
    (28, 5),
    (35, 5),
    (27, 5),
    (36, 5),
    (26, 5),
    (37, 6),
    (25, 6),
    (38, 6),
    (24, 6),
    (39, 6),
    (23, 6),
    (40, 6),
    (22, 6),
    (41, 6),
    (21, 6),
    (42, 6),
    (20, 6),
    (43, 6),
    (19, 6),
    (44, 6),
    (18, 6),
    (45, 7),
    (17, 7),
    (46, 7),
    (16, 7),
    (47, 7),
    (15, 7),
    (48, 7),
    (14, 7),
    (49, 7),
    (13, 7),
    (50, 7),
    (12, 7),
    (51, 7),
    (11, 7),
    (52, 8),
    (10, 8),
    (53, 8),
    (9, 8),
    (54, 8),
    (8, 8),
    (55, 8),
    (7, 8),
    (56, 8),
    (6, 8),
    (57, 8),
    (5, 8),
    (58, 8),
    (4, 8),
    (59, 8),
    (3, 8),
    (60, 8),
    (2, 8),
    (61, 8),
    (1, 8),
];

pub const ATRAC3_HUFF_CODES: [&[(u32, u8)]; 7] = [
    &[
        (0x0, 1),
        (0x4, 3),
        (0x5, 3),
        (0xC, 4),
        (0xD, 4),
        (0x1C, 5),
        (0x1D, 5),
        (0x1E, 5),
        (0x1F, 5),
    ],
    &[(0x0, 1), (0x4, 3), (0x5, 3), (0x6, 3), (0x7, 3)],
    &[
        (0x0, 1),
        (0x4, 3),
        (0x5, 3),
        (0xC, 4),
        (0xD, 4),
        (0xE, 4),
        (0xF, 4),
    ],
    &[
        (0x0, 1),
        (0x4, 3),
        (0x5, 3),
        (0xC, 4),
        (0xD, 4),
        (0x1C, 5),
        (0x1D, 5),
        (0x1E, 5),
        (0x1F, 5),
    ],
    &[
        (0x0, 2),
        (0x2, 3),
        (0x3, 3),
        (0x8, 4),
        (0x9, 4),
        (0xA, 4),
        (0xB, 4),
        (0x1C, 5),
        (0x1D, 5),
        (0x3C, 6),
        (0x3D, 6),
        (0x3E, 6),
        (0x3F, 6),
        (0xC, 4),
        (0xD, 4),
    ],
    &[
        (0x0, 3),
        (0x2, 4),
        (0x3, 4),
        (0x4, 4),
        (0x5, 4),
        (0x6, 4),
        (0x7, 4),
        (0x14, 5),
        (0x15, 5),
        (0x16, 5),
        (0x17, 5),
        (0x18, 5),
        (0x19, 5),
        (0x34, 6),
        (0x35, 6),
        (0x36, 6),
        (0x37, 6),
        (0x38, 6),
        (0x39, 6),
        (0x3A, 6),
        (0x3B, 6),
        (0x78, 7),
        (0x79, 7),
        (0x7A, 7),
        (0x7B, 7),
        (0x7C, 7),
        (0x7D, 7),
        (0x7E, 7),
        (0x7F, 7),
        (0x8, 4),
        (0x9, 4),
    ],
    &[
        (0x0, 3),
        (0x8, 5),
        (0x9, 5),
        (0xA, 5),
        (0xB, 5),
        (0xC, 5),
        (0xD, 5),
        (0xE, 5),
        (0xF, 5),
        (0x10, 5),
        (0x11, 5),
        (0x24, 6),
        (0x25, 6),
        (0x26, 6),
        (0x27, 6),
        (0x28, 6),
        (0x29, 6),
        (0x2A, 6),
        (0x2B, 6),
        (0x2C, 6),
        (0x2D, 6),
        (0x2E, 6),
        (0x2F, 6),
        (0x30, 6),
        (0x31, 6),
        (0x32, 6),
        (0x33, 6),
        (0x68, 7),
        (0x69, 7),
        (0x6A, 7),
        (0x6B, 7),
        (0x6C, 7),
        (0x6D, 7),
        (0x6E, 7),
        (0x6F, 7),
        (0x70, 7),
        (0x71, 7),
        (0x72, 7),
        (0x73, 7),
        (0x74, 7),
        (0x75, 7),
        (0xEC, 8),
        (0xED, 8),
        (0xEE, 8),
        (0xEF, 8),
        (0xF0, 8),
        (0xF1, 8),
        (0xF2, 8),
        (0xF3, 8),
        (0xF4, 8),
        (0xF5, 8),
        (0xF6, 8),
        (0xF7, 8),
        (0xF8, 8),
        (0xF9, 8),
        (0xFA, 8),
        (0xFB, 8),
        (0xFC, 8),
        (0xFD, 8),
        (0xFE, 8),
        (0xFF, 8),
        (0x2, 4),
        (0x3, 4),
    ],
];

lazy_static::lazy_static! {
    pub static ref SPECTRAL_VLC: Vec<VlcTable> = {
        ATRAC3_HUFF_CODES.iter().map(|table| VlcTable::from_codes(table)).collect()
    };
}
