import os
import numpy as np
from pedalboard import Pedalboard, load_plugin

PLUGIN_PATH = os.path.join(os.path.dirname(__file__), '..', 'target', 'bundled', 'Granular Effect.vst3')

SAMPLE_RATES = [44100, 48000]
CHANNELS = [1, 2, 4]
DURATION = 0.05  # seconds


def run():
    for sr in SAMPLE_RATES:
        frames = int(sr * DURATION)
        for ch in CHANNELS:
            audio = np.zeros((ch, frames), dtype=np.float32)
            plugin = load_plugin(PLUGIN_PATH)
            board = Pedalboard([plugin])
            try:
                processed = board(audio, sr)
                assert processed.shape == audio.shape
            except ValueError as e:
                # plugin supports stereo only
                assert ch != 2
    print('pedalboard plugin tests passed')


if __name__ == '__main__':
    run()
