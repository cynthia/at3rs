/// Public encoder tuning surface.
///
/// Defaults are deterministic and do not read environment variables. The
/// builder methods below are the supported controls; additional crate-visible
/// fields are reserved for in-tree experiments while quality work is ongoing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncoderQuality {
    Standard,
    High,
}

#[derive(Clone, Debug)]
pub struct EncoderConfig {
    pub(crate) stage_debug: bool,
    pub(crate) force_clc: bool,
    pub(crate) disable_vlc: bool,
    pub(crate) analysis_scale: f32,
    pub(crate) ath_gate_scale: f32,
    pub(crate) disable_sf_search: bool,
    pub(crate) experimental_sf_write_offset: i32,
    pub(crate) disable_tonal_quant_boost: bool,
    pub(crate) tonal_quant_boost_factor: f32,
    pub(crate) experimental_gain: bool,
    pub(crate) experimental_gain_v2: bool,
    pub(crate) experimental_tonal_components: bool,
    pub(crate) vlc_bit_safety: usize,
    pub(crate) enable_tail_prune: bool,
    pub(crate) experimental_concentrate_bits: bool,
    pub(crate) experimental_tonal_prune: bool,
    pub(crate) enable_tail_sf_smooth: bool,
    pub(crate) disable_reference_shape: bool,
    pub(crate) experimental_rebalance_bfus: bool,
    pub(crate) cap_high_bfus: bool,
    pub(crate) static_qmf_bands: bool,
    pub(crate) vlc_max_selector: Option<usize>,
}

impl EncoderConfig {
    /// Apply a named quality preset.
    pub fn with_quality(mut self, quality: EncoderQuality) -> Self {
        match quality {
            EncoderQuality::Standard => {}
            EncoderQuality::High => {
                self.experimental_gain_v2 = true;
                self.analysis_scale = 0.205;
                self.vlc_bit_safety = 8;
            }
        }
        self
    }

    /// Enable per-frame debug logging from the encoder pipeline.
    pub fn with_stage_debug(mut self, enabled: bool) -> Self {
        self.stage_debug = enabled;
        self
    }

    /// Force fixed-length mantissa coding instead of allowing VLC.
    pub fn with_force_clc(mut self, enabled: bool) -> Self {
        self.force_clc = enabled;
        self
    }

    /// Enable the second experimental ATRAC3 gain-control detector.
    pub fn with_experimental_gain_v2(mut self, enabled: bool) -> Self {
        self.experimental_gain_v2 = enabled;
        self
    }

    /// Enable experimental tonal-component syntax emission.
    pub fn with_experimental_tonal_components(mut self, enabled: bool) -> Self {
        self.experimental_tonal_components = enabled;
        self
    }

    /// Override the MDCT analysis scale; non-positive values are ignored.
    pub fn with_analysis_scale(mut self, value: f32) -> Self {
        if value > 0.0 {
            self.analysis_scale = value;
        }
        self
    }

    /// Override the absolute-threshold gate scale; non-positive values are ignored.
    pub fn with_ath_gate_scale(mut self, value: f32) -> Self {
        if value > 0.0 {
            self.ath_gate_scale = value;
        }
        self
    }

    pub fn vlc_enabled(&self) -> bool {
        !self.force_clc && !self.disable_vlc
    }

    pub fn scale_factor_search_enabled(&self) -> bool {
        !self.disable_sf_search
    }

    pub fn tonal_quant_boost_enabled(&self) -> bool {
        !self.disable_tonal_quant_boost
    }
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            stage_debug: false,
            force_clc: false,
            disable_vlc: false,
            analysis_scale: super::ATRAC3_ANALYSIS_SPECTRUM_SCALE,
            ath_gate_scale: super::ATRAC3_ATH_GATE_SCALE,
            disable_sf_search: false,
            experimental_sf_write_offset: 0,
            disable_tonal_quant_boost: false,
            tonal_quant_boost_factor: 0.5 / super::ATRAC3_ANALYSIS_SPECTRUM_SCALE,
            experimental_gain: false,
            experimental_gain_v2: false,
            experimental_tonal_components: false,
            vlc_bit_safety: 32,
            enable_tail_prune: false,
            experimental_concentrate_bits: false,
            experimental_tonal_prune: false,
            enable_tail_sf_smooth: false,
            disable_reference_shape: false,
            experimental_rebalance_bfus: false,
            cap_high_bfus: false,
            static_qmf_bands: false,
            vlc_max_selector: None,
        }
    }
}
