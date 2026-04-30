use std::f32::consts::PI;

const QMF_48TAP_HALF: [f32; 24] = [
    -0.00001461907,
    -0.00009205479,
    -0.000056157569,
    0.00030117269,
    0.0002422519,
    -0.00085293897,
    -0.0005205574,
    0.0020340169,
    0.00078333891,
    -0.0042153862,
    -0.00075614988,
    0.0078402944,
    -0.000061169922,
    -0.01344162,
    0.0024626821,
    0.021736089,
    -0.007801671,
    -0.034090221,
    0.01880949,
    0.054326009,
    -0.043596379,
    -0.099384367,
    0.13207909,
    0.46424159,
];

lazy_static::lazy_static! {
    pub static ref QMF_WINDOW: [f32; 48] = {
        let mut w = [0.0; 48];
        for i in 0..24 {
            let s = QMF_48TAP_HALF[i] * 2.0;
            w[i] = s;
            w[47 - i] = s;
        }
        w
    };

    pub static ref SF_TABLE: [f32; 64] = {
        let mut table = [0.0; 64];
        for i in 0..64 {
            table[i] = 2.0_f32.powf(i as f32 / 3.0 - 21.0);
        }
        table
    };

    pub static ref MDCT_WINDOW: [f32; 512] = {
        let mut w = [0.0; 512];
        for i in 0..256 {
            let s = ((i as f32 + 0.5) * (PI / 512.0)).sin();
            w[i] = s;
            w[511 - i] = s;
        }
        w
    };

    pub static ref ENCODE_WINDOW: [f32; 256] = {
        let mut w = [0.0; 256];
        for i in 0..256 {
            w[i] = ((((i as f32 + 0.5) / 256.0) - 0.5) * PI).sin() + 1.0;
        }
        w
    };

    pub static ref DECODE_WINDOW: [f32; 256] = {
        let mut w = [0.0; 256];
        for i in 0..256 {
            let a = ENCODE_WINDOW[i];
            let b = ENCODE_WINDOW[255 - i];
            w[i] = 2.0 * a / (a * a + b * b);
        }
        w
    };

    pub static ref ATRAC3_LOUDNESS_CURVE: [f32; 1024] = {
        let mut curve = [0.0; 1024];
        for i in 0..1024 {
            let f = (i as f32 + 3.0) * 0.5 * 44_100.0 / 1024.0;
            let mut t = f.log10() - 3.5;
            t = -10.0 * t * t + 3.0 - f / 3000.0;
            curve[i] = 10.0_f32.powf(0.1 * t);
        }
        curve
    };

    pub static ref ATRAC3_ATH_BFU: [f32; 32] = {
        let ath_spec = calc_ath_spec(1024, 44_100);
        let mut ath = [0.0; 32];
        for block in 0..32 {
            let start = ATRAC3_SUBBAND_TAB[block];
            let end = ATRAC3_SUBBAND_TAB[block + 1];
            let mut min_db = 999.0f32;
            for &v in &ath_spec[start..end] {
                min_db = min_db.min(v);
            }
            ath[block] = 10.0_f32.powf(0.1 * min_db);
        }
        ath
    };
}

pub const ATRAC3_SUBBAND_TAB: [usize; 33] = [
    0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256, 288, 320,
    352, 384, 416, 448, 480, 512, 576, 640, 704, 768, 896, 1024,
];

pub const ATRAC3_FIXED_ALLOC_TABLE: [f32; 32] = [
    4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 2.0,
    2.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0,
];

pub const ATRAC3_SELECTOR_SHAPE_HINT: [f32; 32] = [
    7.0, 7.0, 7.0, 7.0, 7.0, 7.0, 7.0, 7.0, 7.0, 6.0, 5.0, 4.0, 4.0, 4.0, 4.0, 3.0, 3.0, 3.0, 2.0,
    2.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0,
];

pub const ATRAC3_BLOCKS_PER_BAND: [usize; 5] = [0, 18, 26, 30, 32];
pub const ATRAC3_ANALYSIS_SPECTRUM_SCALE: f32 = 0.235;
pub const ATRAC3_LOUDNESS_FACTOR: f32 = 0.006;
pub const ATRAC3_ATH_GATE_SCALE: f32 = 1.0e-4;
pub const ATRAC3_GAIN_NEUTRAL_LEVEL: u8 = 4;
pub const ATRAC3_GAIN_LOC_SCALE: usize = 3;
pub const ATRAC3_GAIN_LOC_SIZE: usize = 1 << ATRAC3_GAIN_LOC_SCALE;

fn ath_formula_frank(mut freq_hz: f32) -> f32 {
    const TAB: [i16; 140] = [
        9669, 9669, 9626, 9512, 9353, 9113, 8882, 8676, 8469, 8243, 7997, 7748, 7492, 7239, 7000,
        6762, 6529, 6302, 6084, 5900, 5717, 5534, 5351, 5167, 5004, 4812, 4638, 4466, 4310, 4173,
        4050, 3922, 3723, 3577, 3451, 3281, 3132, 3036, 2902, 2760, 2658, 2591, 2441, 2301, 2212,
        2125, 2018, 1900, 1770, 1682, 1594, 1512, 1430, 1341, 1260, 1198, 1136, 1057, 998, 943,
        887, 846, 744, 712, 693, 668, 637, 606, 580, 555, 529, 502, 475, 448, 422, 398, 375, 351,
        327, 322, 312, 301, 291, 268, 246, 215, 182, 146, 107, 61, 13, -35, -96, -156, -179, -235,
        -295, -350, -401, -421, -446, -499, -532, -535, -513, -476, -431, -313, -179, 8, 203, 403,
        580, 736, 881, 1022, 1154, 1251, 1348, 1421, 1479, 1399, 1285, 1193, 1287, 1519, 1914,
        2369, 3352, 4352, 5352, 6352, 7352, 8352, 9352, 9999, 9999, 9999, 9999, 9999,
    ];

    freq_hz = freq_hz.clamp(10.0, 29_853.0);
    let freq_log = 40.0 * (0.1 * freq_hz).log10();
    let index = freq_log.floor() as usize;
    let frac = freq_log - index as f32;
    0.01 * (TAB[index] as f32 * (1.0 - frac) + TAB[index + 1] as f32 * frac)
}

fn calc_ath_spec(len: usize, sample_rate: usize) -> Vec<f32> {
    let mut res = vec![0.0f32; len];
    let mf = sample_rate as f32 / 2000.0;
    for (i, out) in res.iter_mut().enumerate() {
        let f_khz = (i as f32 + 1.0) * mf / len as f32;
        let mut trh = ath_formula_frank(1000.0 * f_khz) - 100.0;
        trh -= f_khz * f_khz * 0.015;
        *out = trh;
    }
    res
}
