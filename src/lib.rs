//! Granular Tukey-window effect (Python-compatible, nih-plug 0.11 + rand 0.9)

use nih_plug::prelude::*;
use rand::{rng, Rng};
use std::{num::NonZeroU32, sync::Arc};

/*──────────────────── 0. Parameters ────────────────────*/
// ４つの FloatParam パラメータを持つ struct を定義する。
// - density: グレイン生成確率 (0.0=生成なし, 1.0=毎ブロック必ず生成)
// - min_ms: グレインの最小長 (ミリ秒単位)
// - max_ms: グレインの最大長 (ミリ秒単位)
// - mix: ウェット／ドライ比率 (0.0=ドライのみ, 1.0=100% ウェット)
#[derive(Params)]
pub struct GranularParams {
    /// グレインを生成する確率 (0.0=生成なし, 1.0=毎ブロック必ず生成)
    #[id = "density"]
    pub density: FloatParam,

    /// グレインの最小長 (ミリ秒単位)
    #[id = "min_ms"]
    pub min_ms: FloatParam,

    /// グレインの最大長 (ミリ秒単位)
    #[id = "max_ms"]
    pub max_ms: FloatParam,

    /// ウェット／ドライ比率 (0.0=ドライのみ, 1.0=100% ウェット)
    #[id = "mix"]
    pub mix: FloatParam,
}

impl Default for GranularParams {
    fn default() -> Self {
        Self {
            density: FloatParam::new("Density", 0.2, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_smoother(SmoothingStyle::Linear(0.01)),

            min_ms: FloatParam::new(
                "Min Length (ms)",
                20.0,
                FloatRange::Linear {
                    min: 1.0,
                    max: 1000.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(1.0)),

            max_ms: FloatParam::new(
                "Max Length (ms)",
                500.0,
                FloatRange::Linear {
                    min: 1.0,
                    max: 1000.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(1.0)),

            mix: FloatParam::new("Mix", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_smoother(SmoothingStyle::Linear(0.01)),
        }
    }
}

/*──────────────────── 1. Constants (match Python) ──────*/
const RING_SEC: f32 = 5.0; // リングバッファの長さ (秒)
const MAX_GRAINS: usize = 25; // 同時に立ち上がるグレイン数上限
                              // TRIGGER_PROB は「density」パラメータで置き換え
                              // MIN_MS / MAX_MS は「min_ms」「max_ms」パラメータで置き換え
const TUKEY_ALPHA: f32 = 0.2; // Tukey 窓の形状

/*──────────────────── 2. Internal structs ──────────────*/
struct Grain {
    buf: Vec<f32>,
    pos: usize,
    ch: usize,
}
impl Grain {
    #[inline]
    fn done(&self) -> bool {
        self.pos >= self.buf.len()
    }
}

struct Granular {
    params: Arc<GranularParams>,
    ring: Vec<f32>,
    wr: usize,
    grains: Vec<Grain>,
    sr: f32,
}

impl Default for Granular {
    fn default() -> Self {
        Self {
            params: Arc::new(GranularParams::default()),
            ring: Vec::new(),
            wr: 0,
            grains: Vec::new(),
            sr: 0.0,
        }
    }
}

/*──────────────────── 3. Plugin implementation ────────*/
impl Plugin for Granular {
    const NAME: &'static str = "Granular";
    const VENDOR: &'static str = "Daishi Suzuki";
    const URL: &'static str = "https://example.com";
    const EMAIL: &'static str = "zukky.rikugame@gmail.com";
    const VERSION: &'static str = "0.3.0";

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
    ];

    const MIDI_INPUT: MidiConfig = MidiConfig::None;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::None;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn initialize(
        &mut self,
        _: &AudioIOLayout,
        cfg: &BufferConfig,
        _: &mut impl InitContext<Self>,
    ) -> bool {
        self.sr = cfg.sample_rate as f32;
        self.ring = vec![0.0; (RING_SEC * self.sr) as usize];
        true
    }

    fn reset(&mut self) {
        self.wr = 0;
        self.grains.clear();
        self.ring.fill(0.0);
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _ctx: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let mut rng = rng();
        let n_ch = buffer.channels() as usize;

        // ── ① パラメータ値を取得 ──
        let density = self.params.density.smoothed.next();
        let min_len_ms = self.params.min_ms.smoothed.next().max(1.0);
        let max_len_ms = self.params.max_ms.smoothed.next().max(min_len_ms);
        let min_len = ((min_len_ms / 1_000.0) * self.sr) as usize;
        let max_len = ((max_len_ms / 1_000.0) * self.sr) as usize;
        let mix = self.params.mix.smoothed.next().clamp(0.0, 1.0);

        // ── ① グレイン生成判定 (ブロックごと) ──
        if self.grains.len() < MAX_GRAINS && rng.random::<f32>() < density {
            if self.ring.len() >= max_len {
                let len = rng.random_range(min_len..=max_len);
                let start = rng.random_range(0..self.ring.len() - len);
                let mut data: Vec<f32> = (0..len)
                    .map(|i| self.ring[(start + i) % self.ring.len()])
                    .collect();
                apply_tukey(&mut data, TUKEY_ALPHA);
                let ch = rng.random_range(0..n_ch);
                self.grains.push(Grain {
                    buf: data,
                    pos: 0,
                    ch,
                });
            }
        }

        // ── ② フレーム単位ループ ──
        for mut frame in buffer.iter_samples() {
            // a. モノラル化してリングバッファへ書き込む
            let mut mono_input = 0.0;
            for ch in 0..n_ch {
                mono_input += *frame.get_mut(ch).unwrap();
            }

            // b. モノラル化したサンプルをリングバッファへ書き込み
            self.ring[self.wr] = mono_input;
            self.wr = (self.wr + 1) % self.ring.len();

            // c. このフレーム用のグレイン合成
            // 各チャンネル用のミックス値を初期化
            let mut mixes = vec![0.0f32; n_ch];
            for g in &mut self.grains {
                if let Some(&v) = g.buf.get(g.pos) {
                    mixes[g.ch % n_ch] += v;
                }
            }

            // d. ドライ成分とウェット成分を mix でミックス
            for ch in 0..n_ch {
                let dry = *frame.get_mut(ch).unwrap();
                let out = dry * (1.0 - mix) + mixes[ch] * mix;
                *frame.get_mut(ch).unwrap() = out;
            }

            // e. グレイン再生位置を進める
            for g in &mut self.grains {
                if g.pos < g.buf.len() {
                    g.pos += 1;
                }
            }
        }

        // ── ③ 終了したグレインを除去 ──
        self.grains.retain(|g| !g.done());

        ProcessStatus::Normal
    }
}

/*──────────────────── 4. Tukey window ─────────────────*/
fn apply_tukey(x: &mut [f32], alpha: f32) {
    let n = x.len() as f32;
    let edge = (alpha * (n - 1.0) * 0.5).floor();
    for (i, v) in x.iter_mut().enumerate() {
        let k = i as f32;
        let w = if k < edge {
            0.5 * (1.0 - (2.0 * std::f32::consts::PI * k / (alpha * (n - 1.0))).cos())
        } else if k > n - edge - 1.0 {
            let k2 = n - k - 1.0;
            0.5 * (1.0 - (2.0 * std::f32::consts::PI * k2 / (alpha * (n - 1.0))).cos())
        } else {
            1.0
        };
        *v *= w;
    }
}

/*──────────────────── 5. CLAP / VST3 export ───────────*/
impl ClapPlugin for Granular {
    const CLAP_ID: &'static str = "com.zukky.granular";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Granular effect with Tukey window + parameters");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect];
}
impl Vst3Plugin for Granular {
    const VST3_CLASS_ID: [u8; 16] = *b"Granular!!!!!!!!";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx];
}

nih_export_clap!(Granular);
nih_export_vst3!(Granular);

/*──────────────────── Tests ───────────────────────────*/
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tukey_window_symmetry_and_edges() {
        let mut data = vec![1.0f32; 10];
        apply_tukey(&mut data, 0.5);

        let n = data.len();
        assert!(data[0].abs() < 1e-6);
        assert!(data[n - 1].abs() < 1e-6);

        for i in 0..n {
            let j = n - 1 - i;
            assert!(
                (data[i] - data[j]).abs() < 1e-6,
                "window not symmetric at {i}"
            );
            assert!(data[i] <= 1.0 + 1e-6);
        }
    }

    #[test]
    fn grain_done_checks_bounds() {
        let g = Grain {
            buf: vec![0.0, 1.0],
            pos: 2,
            ch: 0,
        };
        assert!(g.done());
        let g = Grain {
            buf: vec![0.0, 1.0],
            pos: 1,
            ch: 0,
        };
        assert!(!g.done());
    }

    #[test]
    fn tukey_alpha_zero_is_rectangular() {
        let mut data = vec![1.0f32; 16];
        let orig = data.clone();
        apply_tukey(&mut data, 0.0);
        assert_eq!(data, orig);
    }

    #[test]
    fn plugin_initializes_ring_size() {
        let layout = Granular::AUDIO_IO_LAYOUTS[0];
        let cfg = BufferConfig {
            sample_rate: 44100.0,
            min_buffer_size: None,
            max_buffer_size: 64,
            process_mode: ProcessMode::Realtime,
        };

        struct DummyInit;
        impl InitContext<Granular> for DummyInit {
            fn plugin_api(&self) -> PluginApi {
                PluginApi::Clap
            }
            fn execute(&self, _task: ()) {}
            fn set_latency_samples(&self, _samples: u32) {}
            fn set_current_voice_capacity(&self, _capacity: u32) {}
        }

        let mut plugin = Granular::default();
        assert!(plugin.initialize(&layout, &cfg, &mut DummyInit));

        plugin
            .params
            .density
            .smoothed
            .reset(plugin.params.density.value());
        plugin
            .params
            .min_ms
            .smoothed
            .reset(plugin.params.min_ms.value());
        plugin
            .params
            .max_ms
            .smoothed
            .reset(plugin.params.max_ms.value());
        plugin.params.mix.smoothed.reset(plugin.params.mix.value());
        let expected = (RING_SEC * cfg.sample_rate) as usize;
        assert_eq!(plugin.ring.len(), expected);
    }

    #[test]
    fn process_handles_multiple_channels() {
        let layout = Granular::AUDIO_IO_LAYOUTS[0];
        let cfg = BufferConfig {
            sample_rate: 48000.0,
            min_buffer_size: None,
            max_buffer_size: 64,
            process_mode: ProcessMode::Realtime,
        };

        struct DummyInit;
        impl InitContext<Granular> for DummyInit {
            fn plugin_api(&self) -> PluginApi {
                PluginApi::Clap
            }
            fn execute(&self, _task: ()) {}
            fn set_latency_samples(&self, _samples: u32) {}
            fn set_current_voice_capacity(&self, _capacity: u32) {}
        }

        struct DummyCtx {
            transport: Transport,
        }
        impl DummyCtx {
            fn new(sr: f32) -> Self {
                let mut t: Transport = unsafe { std::mem::zeroed() };
                t.sample_rate = sr;
                Self { transport: t }
            }
        }
        impl ProcessContext<Granular> for DummyCtx {
            fn plugin_api(&self) -> PluginApi {
                PluginApi::Clap
            }
            fn execute_background(&self, _task: ()) {}
            fn execute_gui(&self, _task: ()) {}
            fn transport(&self) -> &Transport {
                &self.transport
            }
            fn next_event(&mut self) -> Option<PluginNoteEvent<Granular>> {
                None
            }
            fn send_event(&mut self, _event: PluginNoteEvent<Granular>) {}
            fn set_latency_samples(&self, _samples: u32) {}
            fn set_current_voice_capacity(&self, _capacity: u32) {}
        }

        let mut plugin = Granular::default();
        assert!(plugin.initialize(&layout, &cfg, &mut DummyInit));

        // four channels of silence
        let frames = 32;
        let mut real = vec![vec![0.0f32; frames]; 4];
        let mut buffer = Buffer::default();
        unsafe {
            buffer.set_slices(frames, |s| {
                *s = real.iter_mut().map(|c| c.as_mut_slice()).collect();
            });
        }
        let mut aux_inputs: [Buffer; 0] = [];
        let mut aux_outputs: [Buffer; 0] = [];
        let mut aux = AuxiliaryBuffers {
            inputs: &mut aux_inputs,
            outputs: &mut aux_outputs,
        };
        let mut ctx = DummyCtx::new(cfg.sample_rate);
        plugin.process(&mut buffer, &mut aux, &mut ctx);

        assert_eq!(buffer.channels(), 4);
        assert_eq!(buffer.samples(), frames);
    }

    #[test]
    fn grains_mix_to_correct_channels() {
        let layout = Granular::AUDIO_IO_LAYOUTS[0];
        let cfg = BufferConfig {
            sample_rate: 48000.0,
            min_buffer_size: None,
            max_buffer_size: 64,
            process_mode: ProcessMode::Realtime,
        };

        struct DummyInit;
        impl InitContext<Granular> for DummyInit {
            fn plugin_api(&self) -> PluginApi {
                PluginApi::Clap
            }
            fn execute(&self, _task: ()) {}
            fn set_latency_samples(&self, _samples: u32) {}
            fn set_current_voice_capacity(&self, _capacity: u32) {}
        }

        struct DummyCtx {
            transport: Transport,
        }
        impl DummyCtx {
            fn new(sr: f32) -> Self {
                let mut t: Transport = unsafe { std::mem::zeroed() };
                t.sample_rate = sr;
                Self { transport: t }
            }
        }
        impl ProcessContext<Granular> for DummyCtx {
            fn plugin_api(&self) -> PluginApi {
                PluginApi::Clap
            }
            fn execute_background(&self, _task: ()) {}
            fn execute_gui(&self, _task: ()) {}
            fn transport(&self) -> &Transport {
                &self.transport
            }
            fn next_event(&mut self) -> Option<PluginNoteEvent<Granular>> {
                None
            }
            fn send_event(&mut self, _event: PluginNoteEvent<Granular>) {}
            fn set_latency_samples(&self, _samples: u32) {}
            fn set_current_voice_capacity(&self, _capacity: u32) {}
        }

        let mut plugin = Granular::default();
        assert!(plugin.initialize(&layout, &cfg, &mut DummyInit));

        plugin
            .params
            .density
            .smoothed
            .reset(plugin.params.density.value());
        plugin
            .params
            .min_ms
            .smoothed
            .reset(plugin.params.min_ms.value());
        plugin
            .params
            .max_ms
            .smoothed
            .reset(plugin.params.max_ms.value());
        plugin.params.mix.smoothed.reset(plugin.params.mix.value());

        plugin.grains.clear();
        plugin.grains.push(Grain {
            buf: vec![1.0],
            pos: 0,
            ch: 0,
        });
        plugin.grains.push(Grain {
            buf: vec![0.5],
            pos: 0,
            ch: 1,
        });
        while plugin.grains.len() < MAX_GRAINS {
            plugin.grains.push(Grain {
                buf: Vec::new(),
                pos: 0,
                ch: 0,
            });
        }

        let frames = 1;
        let mut real = vec![vec![0.0f32; frames]; 2];
        let mut buffer = Buffer::default();
        unsafe {
            buffer.set_slices(frames, |s| {
                *s = real.iter_mut().map(|c| c.as_mut_slice()).collect();
            });
        }
        let mut aux_inputs: [Buffer; 0] = [];
        let mut aux_outputs: [Buffer; 0] = [];
        let mut aux = AuxiliaryBuffers {
            inputs: &mut aux_inputs,
            outputs: &mut aux_outputs,
        };
        let mut ctx = DummyCtx::new(cfg.sample_rate);
        plugin.process(&mut buffer, &mut aux, &mut ctx);

        let out = buffer.as_slice();
        assert!((out[0][0] - 1.0).abs() < 1e-6);
        assert!((out[1][0] - 0.5).abs() < 1e-6);
    }
}
