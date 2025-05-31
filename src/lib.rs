//! Granular Tukey-window effect (Python-compatible, nih-plug 0.11 + rand 0.9)

use nih_plug::prelude::*;
use rand::{rng, Rng};
use std::{num::NonZeroU32, sync::Arc};

/*──────────────────── 0. Parameters ────────────────────*/
// A hand-written, EMPTY Params implementation – no UI-visible parameters
pub struct NoParams;
unsafe impl Params for NoParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        Vec::new()            // <- tells nih-plug we have zero parameters
    }
}

/*──────────────────── 1. Constants (match Python) ──────*/
const RING_SEC:      f32   = 5.0;
const MAX_GRAINS:    usize = 25;
const TRIGGER_PROB:  f32   = 0.2;
const MIN_MS:        f32   = 20.0;
const MAX_MS:        f32   = 500.0;
const TUKEY_ALPHA:   f32   = 0.2;

/*──────────────────── 2. Internal structs ──────────────*/
struct Grain { buf: Vec<f32>, pos: usize }
impl Grain { #[inline] fn done(&self)->bool { self.pos>=self.buf.len() } }

struct Granular {
    params: Arc<NoParams>,
    ring:   Vec<f32>,
    wr:     usize,
    grains: Vec<Grain>,
    sr:     f32,
}

impl Default for Granular {
    fn default() -> Self {
        Self {
            params: Arc::new(NoParams),
            ring:   Vec::new(),
            wr:     0,
            grains: Vec::new(),
            sr:     48_000.0,
        }
    }
}

/*──────────────────── 3. Plugin implementation ────────*/
impl Plugin for Granular {
    const NAME:    &'static str = "Granular (Tukey)";
    const VENDOR:  &'static str = "Daishi Suzuki";
    const URL:     &'static str = "https://example.com";
    const EMAIL:   &'static str = "zukky.rikugame@gmail.com";
    const VERSION: &'static str = "0.2.0";

    const AUDIO_IO_LAYOUTS:&'static[AudioIOLayout]=&[AudioIOLayout{
        main_input_channels:  NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    const MIDI_INPUT:  MidiConfig = MidiConfig::None;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::None;

    type SysExMessage   = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> { self.params.clone() }

    fn initialize(&mut self,_:&AudioIOLayout,cfg:&BufferConfig,_:&mut impl InitContext<Self>)
      -> bool {
        self.sr   = cfg.sample_rate as f32;
        self.ring = vec![0.0; (RING_SEC*self.sr) as usize];
        true
    }

    fn reset(&mut self) {
        self.wr=0; self.grains.clear(); self.ring.fill(0.0);
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _ctx: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let mut rng = rng();
        let n_ch    = buffer.channels() as usize;
        let min_len = ((MIN_MS / 1_000.0) * self.sr) as usize;
        let max_len = ((MAX_MS / 1_000.0) * self.sr) as usize;

        // ── ① グレイン生成判定 (ブロックごと) ──
        if self.grains.len() < MAX_GRAINS && rng.random::<f32>() < TRIGGER_PROB {
            if self.ring.len() >= max_len {
                let len   = rng.random_range(min_len..=max_len);
                let start = rng.random_range(0..self.ring.len() - len);
                let mut data: Vec<f32> = (0..len)
                    .map(|i| self.ring[(start + i) % self.ring.len()])
                    .collect();
                apply_tukey(&mut data, TUKEY_ALPHA);
                self.grains.push(Grain { buf: data, pos: 0 });
            }
        }

        // ── ② フレーム単位ループ ──
        for mut frame in buffer.iter_samples() {
            // a. いったん「全チャンネルを足し合わせて」モノラル化
            let mut mono_input = 0.0;
            for ch in 0..n_ch {
                mono_input += *frame.get_mut(ch).unwrap();
            }
            // （必要なら平均を取りたければ `/ n_ch` しますが、
            //  Python版と同じ「足し合わせ」のままにするにはこのまま。）

            // b. モノラル化したサンプルをリングバッファへ書き込み
            self.ring[self.wr] = mono_input;
            self.wr = (self.wr + 1) % self.ring.len();

            // c. このフレーム用のグレイン合成
            let mut mix = 0.0;
            for g in &mut self.grains {
                if let Some(&v) = g.buf.get(g.pos) {
                    mix += v;
                }
            }

            // d. 合成したモノラル・グラニュラー音を全チャンネルに同じだけ加算
            for ch in 0..n_ch {
                *frame.get_mut(ch).unwrap() += mix;
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
fn apply_tukey(x:&mut[f32],alpha:f32){
    let n=x.len()as f32;
    let edge=(alpha*(n-1.0)*0.5).floor();
    for(i,v)in x.iter_mut().enumerate(){
        let k=i as f32;
        let w=if k<edge{
            0.5*(1.0-(2.0*std::f32::consts::PI*k/(alpha*(n-1.0))).cos())
        }else if k>n-edge-1.0{
            let k2=n-k-1.0;
            0.5*(1.0-(2.0*std::f32::consts::PI*k2/(alpha*(n-1.0))).cos())
        }else{1.0};
        *v*=w;
    }
}

/*──────────────────── 5. CLAP / VST3 export ───────────*/
impl ClapPlugin for Granular {
    const CLAP_ID:          &'static str = "com.zukky.granular";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Granular effect with Tukey window");
    const CLAP_MANUAL_URL:  Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES:&'static[ClapFeature]=&[ClapFeature::AudioEffect];
}
impl Vst3Plugin for Granular {
    const VST3_CLASS_ID:[u8;16]=*b"GranTukeyRustPl\0";   // exactly 16 bytes (with trailing null)
    const VST3_SUBCATEGORIES:&'static[Vst3SubCategory]=&[Vst3SubCategory::Fx];
}

nih_export_clap!(Granular);
nih_export_vst3!(Granular);
