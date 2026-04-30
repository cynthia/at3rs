use crate::dsp;
use crate::gha;
use crate::psychoacoustic;

pub enum CodecType {
    Atrac3,
    Atrac3Plus,
}

pub struct AtracContext {
    pub codec_type: CodecType,
    pub channels: u16,
    pub sample_rate: u32,
    pub bitrate: u32,
    pub block_align: usize,

    pub gha_ctx: gha::GhaContext,
    pub dsp_state: dsp::DspState,
}

impl AtracContext {
    pub fn new(codec_type: CodecType, channels: u16, sample_rate: u32, bitrate: u32) -> Self {
        let block_align = match codec_type {
            CodecType::Atrac3 => (bitrate * 1024 / 8) as usize,
            CodecType::Atrac3Plus => ((bitrate * 1000) / (sample_rate / 2048) / 8) as usize,
        };

        Self {
            codec_type,
            channels,
            sample_rate,
            bitrate,
            block_align,
            gha_ctx: gha::GhaContext::new(2048),
            dsp_state: dsp::DspState::new(),
        }
    }

    pub fn encode_frame(&mut self, pcm_in: &[i16], bitstream_out: &mut [u8]) -> usize {
        let mut planar_pcm = vec![vec![0.0f32; 2048]; self.channels as usize];

        for ch in 0..self.channels as usize {
            for i in 0..2048 {
                planar_pcm[ch][i] = pcm_in[i * self.channels as usize + ch] as f32;
            }
        }

        for ch in 0..self.channels as usize {
            let mut tones = vec![gha::GhaInfo::default(); 16];

            self.gha_ctx
                .extract_many(&mut planar_pcm[ch], &mut tones, 16);

            let mut subbands = [0.0f32; 2048];
            dsp::qmf_mdct_forward(&planar_pcm[ch], &mut subbands, &mut self.dsp_state);

            let mut bit_alloc = [0u8; 32];
            psychoacoustic::analyze_frame(&subbands, &tones, &mut bit_alloc);

            dsp::pack_bitstream(&subbands, &tones, &bit_alloc, bitstream_out);
        }

        self.block_align
    }

    pub fn decode_frame(&mut self, _bitstream_in: &[u8], pcm_out: &mut [i16]) {
        let mut planar_pcm = vec![vec![0.0f32; 2048]; self.channels as usize];

        for ch in 0..self.channels as usize {
            let mut subbands = [0.0f32; 2048];
            let mut tones = vec![gha::GhaInfo::default(); 16];

            // Bitstream unpacking is not implemented in this legacy path.
            // dsp::unpack_bitstream(bitstream_in, &mut subbands, &mut tones);

            dsp::qmf_mdct_inverse(&subbands, &mut planar_pcm[ch], &mut self.dsp_state);

            self.gha_ctx.synthesize_many(&mut planar_pcm[ch], &tones);
        }

        for ch in 0..self.channels as usize {
            for i in 0..2048 {
                let sample = planar_pcm[ch][i].clamp(-32768.0, 32767.0);
                pcm_out[i * self.channels as usize + ch] = sample as i16;
            }
        }
    }
}
