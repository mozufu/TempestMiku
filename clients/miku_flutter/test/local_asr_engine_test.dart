import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/asr/local_asr_engine.dart';

void main() {
  test('accepts the exact P6.6 PCM sample and duration bounds', () {
    final audio = LocalAsrAudio(
      samples: Float32List(localAsrMaxSamples),
      sampleRate: localAsrSampleRate,
    );

    expect(audio.durationSeconds, localAsrMaxDurationSeconds);
    expect(localAsrMaxPcm16Bytes, 1920000);
  });

  test('rejects empty, wrong-rate, and oversized audio', () {
    expect(
      () => LocalAsrAudio(
        samples: Float32List(0),
        sampleRate: localAsrSampleRate,
      ),
      throwsArgumentError,
    );
    expect(
      () => LocalAsrAudio(samples: Float32List(1), sampleRate: 48000),
      throwsArgumentError,
    );
    expect(
      () => LocalAsrAudio(
        samples: Float32List(localAsrMaxSamples + 1),
        sampleRate: localAsrSampleRate,
      ),
      throwsArgumentError,
    );
  });
}
