import 'package:flutter_test/flutter_test.dart';
import 'package:tm_asr_benchmark/benchmark_metrics.dart';

void main() {
  test('CER normalizes case, spaces, and punctuation', () {
    expect(characterErrorRate('Hello, 世界！', 'hello世界'), 0);
    expect(characterErrorRate('咪庫', '咪咕'), closeTo(0.5, 0.0001));
    expect(characterErrorRate('', ''), 0);
    expect(characterErrorRate('', 'extra'), 1);
  });

  test('gates expose every failed dimension', () {
    expect(
      evaluateGates(
        modelBytes: maxModelBytes,
        peakRssKiB: maxPeakRssKiB,
        maxInferenceMilliseconds: maxInferenceDurationMilliseconds,
        realTimeFactor: maxRealTimeFactor,
        corpusItems: minimumCorpusItems,
        longRuns: requiredLongRuns,
      ),
      <String, Object>{'passed': true, 'failures': <String>[]},
    );
    expect(
      evaluateGates(
        modelBytes: maxModelBytes + 1,
        peakRssKiB: maxPeakRssKiB + 1,
        maxInferenceMilliseconds: maxInferenceDurationMilliseconds + 1,
        realTimeFactor: maxRealTimeFactor + 0.01,
        corpusItems: minimumCorpusItems - 1,
        longRuns: requiredLongRuns - 1,
      )['failures'],
      <String>[
        'model_bytes',
        'peak_rss_kib',
        'inference_timeout',
        'max_rtf',
        'corpus_items',
        'long_runs',
      ],
    );
  });
}
