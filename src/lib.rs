//! Granular Tukey-window effect (Python-compatible, nih-plug 0.11 + rand 0.9)
//! サンプルレートが変わっても動作する設計になっている例

use nih_plug::prelude::*;               // プラグイン用前提クレート
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
            // ── FloatParam::new の正しい呼び出し順序 ──
            //   (name: &str, default: f32, range: FloatRange)
            density: FloatParam::new(
                "Density",                       // parameter name
                0.2,                             // default value
                FloatRange::Linear { min: 0.0, max: 1.0 }, // allowed range
            )
            .with_smoother(SmoothingStyle::Linear(0.01)),

            min_ms: FloatParam::new(
                "Min Length (ms)",               // parameter name
                20.0,                            // default value
                FloatRange::Linear { min: 1.0, max: 1000.0 }, // allowed range
            )
            .with_smoother(SmoothingStyle::Linear(1.0)),

            max_ms: FloatParam::new(
                "Max Length (ms)",               // parameter name
                500.0,                           // default value
                FloatRange::Linear { min: 1.0, max: 1000.0 }, // allowed range
            )
            .with_smoother(SmoothingStyle::Linear(1.0)),

            mix: FloatParam::new(
                "Mix",                           // parameter name
                1.0,                             // default value
                FloatRange::Linear { min: 0.0, max: 1.0 }, // allowed range
            )
            .with_smoother(SmoothingStyle::Linear(0.01)),
        }
    }
}

/*──────────────────── 1. Constants ───────────────────*/
// サンプルレートに依存しない定数はそのまま利用
const RING_SEC:      f32   = 5.0;   // リングバッファの長さ (秒)
const MAX_GRAINS:    usize = 25;   // 同時に立ち上がるグレイン数上限
// TRIGGER_PROB は「density」パラメータで置き換え
// MIN_MS / MAX_MS は「min_ms」「max_ms」パラメータで置き換え
const TUKEY_ALPHA:   f32   = 0.2;   // Tukey 窓の形状

/*──────────────────── 2. Internal structs ──────────────*/
struct Grain {
    buf: Vec<f32>,
    pos: usize,
}
impl Grain {
    #[inline]
    fn done(&self) -> bool {
        self.pos >= self.buf.len()
    }
}

struct Granular {
    params: Arc<GranularParams>,
    ring:   Vec<f32>, // リングバッファ本体 (サンプル数 = RING_SEC * sr)
    wr:     usize,    // リングバッファへの書き込み位置
    grains: Vec<Grain>,
    sr:     f32,      // サンプルレート (Hz)、initialize() で設定される
}

impl Default for Granular {
    fn default() -> Self {
        Self {
            params: Arc::new(GranularParams::default()),
            ring:   Vec::new(),
            wr:     0,
            grains: Vec::new(),
            sr:     0.0,  // initialize() 呼び出し時に上書き
        }
    }
}

/*──────────────────── 3. Plugin implementation ────────*/
impl Plugin for Granular {
    const NAME:    &'static str = "Granular (Tukey, Parameters)";
    const VENDOR:  &'static str = "Daishi Suzuki";
    const URL:     &'static str = "https://example.com";
    const EMAIL:   &'static str = "zukky.rikugame@gmail.com";
    const VERSION: &'static str = "0.3.0";

    // モノラル／ステレオの両方をサポート
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels:  NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels:  NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
    ];

    const MIDI_INPUT:  MidiConfig = MidiConfig::None;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::None;

    type SysExMessage   = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    /// 初期化時に呼び出される。サンプルレートを保持し、リングバッファを確保。
    fn initialize(
        &mut self,
        _: &AudioIOLayout,
        cfg: &BufferConfig,
        _: &mut impl InitContext<Self>,
    ) -> bool {
        // ホストから渡されるサンプルレートを保持 (例: 48000.0)
        self.sr = cfg.sample_rate as f32;
        // RING_SEC 秒分のサンプルを保持するサイズを確保
        self.ring = vec![0.0; (RING_SEC * self.sr) as usize];
        true
    }

    /// リセット時に呼び出される。リングバッファやグレインキューをクリア。
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
        let n_ch    = buffer.channels() as usize;

        // ── ① パラメータ値を取得 ──
        // density: グレイン生成確率 (0.0～1.0)
        let density = self.params.density.smoothed.next();
        // min_ms, max_ms: ミリ秒 → サンプル数に変換
        let min_len_ms = self.params.min_ms.smoothed.next().max(1.0);
        let max_len_ms = self.params.max_ms.smoothed.next().max(min_len_ms);
        let min_len = ((min_len_ms / 1_000.0) * self.sr) as usize;
        let max_len = ((max_len_ms / 1_000.0) * self.sr) as usize;
        // mix: ウェット／ドライ比率 (0.0～1.0)
        let mix = self.params.mix.smoothed.next().clamp(0.0, 1.0);

        // ── ② グレイン生成判定 (バッファ単位) ──
        if self.grains.len() < MAX_GRAINS && rng.random::<f32>() < density {
            // リングバッファに max_len サンプル分以上の履歴があるか
            if self.ring.len() >= max_len && max_len > 0 {
                // ランダムにグレイン長を決定 (min_len..=max_len)
                let len = rng.random_range(min_len..=max_len);
                // ランダムに開始位置を決定
                let start = rng.random_range(0..(self.ring.len() - len));
                // リングバッファから len サンプルを切り出す
                let mut data: Vec<f32> = (0..len)
                    .map(|i| self.ring[(start + i) % self.ring.len()])
                    .collect();
                // Tukey 窓をかけてフェードイン・アウト
                apply_tukey(&mut data, TUKEY_ALPHA);
                // 新しいグレインとしてキューに追加
                self.grains.push(Grain { buf: data, pos: 0 });
            }
        }

        // ── ③ フレーム単位ループ ──
        for mut frame in buffer.iter_samples() {
            // a. モノラル化してリングバッファへ書き込む
            let mut mono_input = 0.0;
            for ch in 0..n_ch {
                mono_input += *frame.get_mut(ch).unwrap();
            }
            self.ring[self.wr] = mono_input;
            self.wr = (self.wr + 1) % self.ring.len();

            // b. アクティブなグレインを1サンプルずつミックス
            let mut wet_mix = 0.0;
            for g in &mut self.grains {
                if let Some(&v) = g.buf.get(g.pos) {
                    wet_mix += v;
                }
            }

            // c. ドライ成分 (入力) と ウェット成分 (グレイン合成) を mix でミックス
            for ch in 0..n_ch {
                let dry = *frame.get_mut(ch).unwrap();
                let out = dry * (1.0 - mix) + wet_mix * mix;
                *frame.get_mut(ch).unwrap() = out;
            }

            // d. グレインの再生位置を 1 サンプル進める
            for g in &mut self.grains {
                if g.pos < g.buf.len() {
                    g.pos += 1;
                }
            }
        }

        // ── ④ 終了したグレインを除去 ──
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
    const CLAP_ID:          &'static str = "com.zukky.granular";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Granular effect with Tukey window + parameters");
    const CLAP_MANUAL_URL:  Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES:    &'static [ClapFeature] = &[ClapFeature::AudioEffect];
}

impl Vst3Plugin for Granular {
    // 16 バイト固定のクラスID (ヌル終端なし)
    const VST3_CLASS_ID:       [u8; 16]            = *b"GranTukParamRust";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx];
}

nih_export_clap!(Granular);
nih_export_vst3!(Granular);
