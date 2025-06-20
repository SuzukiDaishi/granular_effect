//! Granular Tukey-window effect (Python-compatible, nih-plug 0.11 + rand 0.9)

use nih_plug::prelude::*;
use rand::{rng, Rng};
use std::{num::NonZeroU32, sync::Arc};

/*──────────────────── 0. Parameters ────────────────────*/
// A hand-written, EMPTY Params implementation – no UI-visible parameters
pub struct NoParams;
unsafe impl Params for NoParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        Vec::new() // <- tells nih-plug we have zero parameters
    }
}

/*──────────────────── 1. Constants (match Python) ──────*/
const RING_SEC: f32 = 5.0;
const MAX_GRAINS: usize = 25;
const TRIGGER_PROB: f32 = 0.2;
const MIN_MS: f32 = 20.0;
const MAX_MS: f32 = 500.0;
const TUKEY_ALPHA: f32 = 0.2;

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
    params: Arc<NoParams>,
    ring: Vec<f32>,
    wr: usize,
    grains: Vec<Grain>,
    sr: f32,
}

impl Default for Granular {
    fn default() -> Self {
        Self {
            params: Arc::new(NoParams),
            ring: Vec::new(),
            wr: 0,
            grains: Vec::new(),
            sr: 48_000.0,
        }
    }
}

/*──────────────────── 3. Plugin implementation ────────*/
impl Plugin for Granular {
    const NAME: &'static str = "Granular (Tukey)";
    const VENDOR: &'static str = "Daishi Suzuki";
    const URL: &'static str = "https://example.com";
    const EMAIL: &'static str = "zukky.rikugame@gmail.com";
    const VERSION: &'static str = "0.2.0";

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

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
        let min_len = ((MIN_MS / 1_000.0) * self.sr) as usize;
        let max_len = ((MAX_MS / 1_000.0) * self.sr) as usize;

        // ── ① グレイン生成判定 (ブロックごと) ──
        if self.grains.len() < MAX_GRAINS && rng.random::<f32>() < TRIGGER_PROB {
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
            // a. 入力をモノラル化（全チャンネル平均）
            let mut mono_input = 0.0;
            for ch in 0..n_ch {
                mono_input += *frame.get_mut(ch).unwrap();
            }
            mono_input /= n_ch as f32;

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

            // d. 合成したモノラル・グラニュラー音をチャンネル別に加算
            for ch in 0..n_ch {
                *frame.get_mut(ch).unwrap() += mixes[ch];
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
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Granular effect with Tukey window");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect];
}
impl Vst3Plugin for Granular {
    const VST3_CLASS_ID: [u8; 16] = *b"GranTukeyRustPl\0"; // exactly 16 bytes (with trailing null)
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
}
