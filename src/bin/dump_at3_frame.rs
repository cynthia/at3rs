use std::env;
use std::fs;
use std::io;

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn read(&mut self, n: usize) -> u32 {
        let mut out = 0u32;
        for _ in 0..n {
            let byte = self.pos / 8;
            let bit = 7 - (self.pos % 8);
            let v = self.data.get(byte).map(|b| (b >> bit) & 1).unwrap_or(0);
            out = (out << 1) | v as u32;
            self.pos += 1;
        }
        out
    }

    fn pos(&self) -> usize {
        self.pos
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_mul(8).saturating_sub(self.pos)
    }

    fn read_signed(&mut self, n: usize) -> i16 {
        let raw = self.read(n);
        let shift = 32 - n;
        (((raw << shift) as i32) >> shift) as i16
    }
}

const ATRAC3_HUFF_CODES: [&[(u32, u8)]; 7] = [
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

const ATRAC3_SUBBAND_TAB: [usize; 33] = [
    0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256, 288, 320,
    352, 384, 416, 448, 480, 512, 576, 640, 704, 768, 896, 1024,
];

fn selector_quant_limit(selector: u32) -> i16 {
    match selector {
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 7,
        6 => 15,
        7 => 31,
        _ => 0,
    }
}

fn clc_bits(selector: u32) -> usize {
    match selector {
        1 => 4,
        2 | 3 => 3,
        4 | 5 => 4,
        6 => 5,
        7 => 6,
        _ => 0,
    }
}

fn clc_pair_value(code: u32) -> i16 {
    match code {
        0 => 0,
        1 => 1,
        2 => -2,
        3 => -1,
        _ => 0,
    }
}

fn skip_vlc_value(br: &mut Bits<'_>, selector: u32) -> bool {
    let Some(table) = selector
        .checked_sub(1)
        .and_then(|idx| ATRAC3_HUFF_CODES.get(idx as usize))
    else {
        return false;
    };

    let start = br.pos();
    let mut code = 0u32;
    for len in 1..=16 {
        code = (code << 1) | br.read(1);
        if table
            .iter()
            .any(|&(entry_code, entry_len)| entry_len as usize == len && entry_code == code)
        {
            return true;
        }
    }
    eprintln!(
        "warning: failed to decode VLC selector={} at bit {}",
        selector, start
    );
    false
}

fn skip_clc_values(br: &mut Bits<'_>, selector: u32, count: usize) -> bool {
    if selector == 0 {
        return true;
    }
    if selector == 1 {
        let pairs = (count + 1) / 2;
        br.read(4 * pairs);
        return true;
    }
    let bits = clc_bits(selector);
    if bits == 0 {
        return false;
    }
    br.read(bits * count);
    true
}

fn parse_tonal_components(br: &mut Bits<'_>, qmf_bands: usize, tonal_groups: u32) -> bool {
    let tonal_mode = br.read(2);
    println!("tonal_mode={} tonal_groups={}", tonal_mode, tonal_groups);

    for group in 0..tonal_groups {
        let mut qmf_flags = Vec::with_capacity(qmf_bands);
        for _ in 0..qmf_bands {
            qmf_flags.push(br.read(1) != 0);
        }
        let coded_values = br.read(3) as usize + 1;
        let quant_selector = br.read(3);
        println!(
            "tonal_group {:02} qmf_flags={:?} coded_values={} quant_selector={}",
            group, qmf_flags, coded_values, quant_selector
        );

        let mut total_components = 0usize;
        for (qmf_band, active) in qmf_flags.iter().copied().enumerate() {
            if !active {
                continue;
            }
            for local_block in 0..4 {
                let spec_block = qmf_band * 4 + local_block;
                let component_count = br.read(3) as usize;
                total_components += component_count;
                if component_count != 0 {
                    println!(
                        "  spec_block {:02} components={}",
                        spec_block, component_count
                    );
                }
                for component in 0..component_count {
                    let sf_idx = br.read(6);
                    let rel_pos = br.read(6);
                    println!(
                        "    component {:02} sf_idx={} rel_pos={} abs_pos={}",
                        component,
                        sf_idx,
                        rel_pos,
                        spec_block * 64 + rel_pos as usize
                    );

                    let component_mode = if tonal_mode == 3 {
                        br.read(1)
                    } else {
                        tonal_mode
                    };
                    let skipped = match component_mode {
                        0 => (0..coded_values).all(|_| skip_vlc_value(br, quant_selector)),
                        1 => skip_clc_values(br, quant_selector, coded_values),
                        other => {
                            eprintln!("warning: unsupported tonal component coding mode {}", other);
                            false
                        }
                    };
                    if !skipped {
                        return false;
                    }
                }
            }
        }
        println!(
            "tonal_group {:02} total_components={} bit_pos={}",
            group,
            total_components,
            br.pos()
        );
    }

    true
}

fn find_data_chunk(bytes: &[u8]) -> io::Result<&[u8]> {
    if bytes.len() >= 0x60 && &bytes[0..3] == b"EA3" {
        return Ok(&bytes[0x60..]);
    }

    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected RIFF/WAVE or EA3/OMA input",
        ));
    }

    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let start = pos + 8;
        let end = start.saturating_add(size);
        if end > bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "truncated chunk",
            ));
        }
        if id == b"data" {
            return Ok(&bytes[start..end]);
        }
        pos = end + (size & 1);
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "missing data chunk",
    ))
}

fn dump_unit(unit: &[u8], ch: usize) {
    let dump_mantissas = env::var("AT3RS_DUMP_MANTISSAS").ok().as_deref() == Some("1");
    let mut br = Bits::new(unit);
    let unit_id = br.read(6);
    let subbands = br.read(2) as usize;
    let qmf_bands = subbands + 1;
    let mut gains: Vec<Vec<(u32, u32)>> = Vec::with_capacity(qmf_bands);
    for _ in 0..qmf_bands {
        let count = br.read(3);
        let mut points = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let level = br.read(4);
            let location = br.read(5);
            points.push((level, location));
        }
        gains.push(points);
    }
    let tonal_groups = br.read(5);
    let gain_counts: Vec<usize> = gains.iter().map(Vec::len).collect();

    if tonal_groups != 0 {
        println!(
            "ch {} unit_id=0x{:02x} subbands={} gain_counts={:?} gains={:?} tonal_groups={}",
            ch, unit_id, subbands, gain_counts, gains, tonal_groups
        );
        if !parse_tonal_components(&mut br, qmf_bands, tonal_groups) {
            println!(
                "tonal_parse=failed bit_pos={} bits_remaining={}",
                br.pos(),
                br.remaining()
            );
            return;
        }
    }

    let num_bfu = br.read(5) as usize + 1;
    let coding_mode = br.read(1);
    let mut selectors = Vec::with_capacity(num_bfu);
    for _ in 0..num_bfu {
        selectors.push(br.read(3));
    }

    let mut sf = Vec::with_capacity(num_bfu);
    for &selector in &selectors {
        if selector == 0 {
            sf.push(None);
        } else {
            sf.push(Some(br.read(6)));
        }
    }

    println!(
        "ch {} unit_id=0x{:02x} subbands={} gain_counts={:?} gains={:?} num_bfu={} coding_mode={}",
        ch, unit_id, subbands, gain_counts, gains, num_bfu, coding_mode
    );
    println!("selectors={:?}", selectors);
    println!("sf_idx={:?}", sf);

    if coding_mode == 1 {
        for (block, &selector) in selectors.iter().enumerate() {
            if selector == 0 {
                continue;
            }
            let len = ATRAC3_SUBBAND_TAB[block + 1] - ATRAC3_SUBBAND_TAB[block];
            let limit = selector_quant_limit(selector);
            let mut max_abs = 0i16;
            let mut saturated = 0usize;
            let mut nonzero = 0usize;
            let mut mantissas = Vec::with_capacity(len);

            if selector == 1 {
                for _ in 0..(len / 2) {
                    let packed = br.read(4);
                    let a = clc_pair_value((packed >> 2) & 3);
                    let b = clc_pair_value(packed & 3);
                    for q in [a, b] {
                        max_abs = max_abs.max(q.abs());
                        saturated += usize::from(q == -2 || q == 1);
                        nonzero += usize::from(q != 0);
                        mantissas.push(q);
                    }
                }
            } else {
                let bits = clc_bits(selector);
                for _ in 0..len {
                    let q = br.read_signed(bits);
                    max_abs = max_abs.max(q.abs());
                    saturated += usize::from(q.abs() >= limit);
                    nonzero += usize::from(q != 0);
                    mantissas.push(q);
                }
            }

            println!(
                "bfu {:02} len={} selector={} sf={:?} nonzero={} max_abs={} saturated={}",
                block, len, selector, sf[block], nonzero, max_abs, saturated
            );
            if dump_mantissas {
                println!("bfu {:02} mantissas={:?}", block, mantissas);
            }
        }
    } else {
        println!("mantissa_stats=skipped_non_clc");
    }
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: dump_at3_frame <file.at3> [block_align] [frame]");
        std::process::exit(2);
    }

    let block_align = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(384usize);
    let frame = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0usize);
    let bytes = fs::read(&args[1])?;
    let data = find_data_chunk(&bytes)?;
    let frame_start = frame * block_align;
    let frame_end = frame_start + block_align;
    if frame_end > data.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame is outside data chunk",
        ));
    }

    let frame_data = &data[frame_start..frame_end];
    let unit_size = block_align / 2;
    for ch in 0..2 {
        let start = ch * unit_size;
        let end = start + unit_size;
        dump_unit(&frame_data[start..end], ch);
    }
    Ok(())
}
