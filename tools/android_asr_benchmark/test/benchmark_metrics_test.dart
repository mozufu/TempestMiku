import 'package:flutter_test/flutter_test.dart';
import 'package:tm_asr_benchmark/benchmark_contract.dart';
import 'package:tm_asr_benchmark/benchmark_metrics.dart';

void main() {
  test('CER normalizes case, spaces, and punctuation', () {
    expect(characterErrorRate('Hello, 世界！', 'hello世界'), 0);
    expect(characterErrorRate('咪庫', '咪咕'), closeTo(0.5, 0.0001));
    expect(characterErrorRate('', ''), 0);
    expect(characterErrorRate('', 'extra'), 1);
  });

  test('CER normalization is reusable for lexical schema checks', () {
    expect(normalizeForCharacterErrorRate(' Hello—世界，！？ '), 'hello世界');
    expect(normalizeForCharacterErrorRate('，。！？…… -- '), isEmpty);
    expect(characterErrorRate(' Hello—世界，！？ ', 'hello世界'), 0);
  });

  test('LCS coverage detects wholesale truncation without length padding', () {
    expect(referenceLcsCoverage('甲乙丙丁', '甲乙丙丁'), 1);
    expect(referenceLcsCoverage('甲乙丙丁', '甲乙很多無關文字'), 0.5);
    expect(referenceLcsCoverage('甲乙丙丁', '甲乙'), 0.5);
    expect(referenceLcsCoverage('', ''), 1);
    const reference = '甲乙丙丁戊己庚辛壬癸子丑';
    const truncatedAndPadded = '甲乙丙丁戊己庚辛壬無關無關無關';
    expect(referenceLcsCoverage(reference, truncatedAndPadded), 0.75);
    expect(
      referenceTailLcsCoverage(reference, truncatedAndPadded),
      closeTo(0.25, 0.0001),
    );
    expect(referenceTailLcsCoverage(reference, reference), 1);
  });

  test('gates expose every failed dimension', () {
    expect(
      evaluateGates(
        ranOnAndroidDevice: true,
        modelBytes: maxModelBytes,
        peakRssKiB: maxPeakRssKiB,
        maxInferenceMilliseconds: maxInferenceDurationMilliseconds,
        realTimeFactor: maxRealTimeFactor,
        thermalStatusAvailable: true,
        corpusIntegrityBound: true,
        corpusItems: minimumCorpusItems,
        longRuns: requiredLongRuns,
        completedItems: minimumCorpusItems,
        failedItems: 0,
        emptyRawItems: 0,
        emptyConvertedItems: 0,
        completedLongRuns: requiredLongRuns,
        nonTruncatedLongRuns: requiredLongRuns,
        convertedMeanCer: maxConvertedMeanCer,
        codeSwitchConvertedMeanCer: maxCodeSwitchConvertedMeanCer,
      ),
      <String, Object>{'passed': true, 'failures': <String>[]},
    );
    expect(
      evaluateGates(
        ranOnAndroidDevice: false,
        modelBytes: maxModelBytes + 1,
        peakRssKiB: maxPeakRssKiB + 1,
        maxInferenceMilliseconds: maxInferenceDurationMilliseconds + 1,
        realTimeFactor: maxRealTimeFactor + 0.01,
        thermalStatusAvailable: false,
        corpusIntegrityBound: false,
        corpusItems: minimumCorpusItems - 1,
        longRuns: requiredLongRuns - 1,
        completedItems: minimumCorpusItems - 2,
        failedItems: 1,
        emptyRawItems: 1,
        emptyConvertedItems: 1,
        completedLongRuns: requiredLongRuns - 1,
        nonTruncatedLongRuns: requiredLongRuns - 1,
        convertedMeanCer: maxConvertedMeanCer + 0.01,
        codeSwitchConvertedMeanCer: maxCodeSwitchConvertedMeanCer + 0.01,
      )['failures'],
      <String>[
        'android_device_execution',
        'model_bytes',
        'peak_rss_kib',
        'inference_timeout',
        'max_rtf',
        'thermal_status_unavailable',
        'corpus_integrity',
        'corpus_items',
        'long_runs',
        'completion',
        'empty_raw_output',
        'empty_converted_output',
        'long_run_completion',
        'long_run_reference_coverage',
        'converted_mean_cer',
        'code_switch_converted_mean_cer',
      ],
    );
  });

  test('zero RSS is unavailable evidence, not a passing measurement', () {
    final gates = evaluateGates(
      ranOnAndroidDevice: true,
      modelBytes: maxModelBytes,
      peakRssKiB: 0,
      maxInferenceMilliseconds: maxInferenceDurationMilliseconds,
      realTimeFactor: maxRealTimeFactor,
      thermalStatusAvailable: true,
      corpusIntegrityBound: true,
      corpusItems: minimumCorpusItems,
      longRuns: requiredLongRuns,
      completedItems: minimumCorpusItems,
      failedItems: 0,
      emptyRawItems: 0,
      emptyConvertedItems: 0,
      completedLongRuns: requiredLongRuns,
      nonTruncatedLongRuns: requiredLongRuns,
      convertedMeanCer: maxConvertedMeanCer,
      codeSwitchConvertedMeanCer: maxCodeSwitchConvertedMeanCer,
    );

    expect(gates['passed'], isFalse);
    expect(gates['failures'], <String>['peak_rss_unavailable']);
  });

  test('production manifest accepts only the exact Paraformer contract', () {
    final manifest = _manifest(productionBenchmarkContract);
    final parsed = BenchmarkModelManifest.fromJson(manifest);
    expect(parsed.contract.engineSelector, productionEngineSelector);
    expect(parsed.file('encoder').path, 'encoder.int8.onnx');
    expect(parsed.file('decoder').bytes, 71664561);
    expect(parsed.file('tokens').sha256, hasLength(64));

    final drifted = _manifest(productionBenchmarkContract);
    (drifted['runtime']! as Map<String, dynamic>)['threads'] = 4;
    expect(
      () => BenchmarkModelManifest.fromJson(drifted),
      throwsA(isA<FormatException>()),
    );
  });

  test('offline candidate manifest pins whole-audio model and provenance', () {
    final manifest = _manifest(offlineCandidateBenchmarkContract);
    final parsed = BenchmarkModelManifest.fromJson(manifest);

    expect(parsed.contract.engineSelector, offlineCandidateEngineSelector);
    expect(parsed.contract.inferenceMode, 'offline_whole_audio');
    expect(
      parsed.contract.sourceRevisionUrl,
      endsWith(offlineCandidateModelCommit),
    );
    expect(parsed.file('model').bytes, 243371218);
    expect(
      parsed.file('model').sha256,
      'f36a0433bcf096bd6d6f11b80a3ac8bed110bdca632fe0d731df8d1a84475945',
    );
    expect(parsed.file('tokens').bytes, 75756);

    final drifted = _manifest(offlineCandidateBenchmarkContract);
    (drifted['inference']! as Map<String, dynamic>)['decodeTrigger'] =
        'while_streaming';
    expect(
      () => BenchmarkModelManifest.fromJson(drifted),
      throwsA(isA<FormatException>()),
    );
  });

  test('unknown engine and model digest fail closed', () {
    final unknown = _manifest(productionBenchmarkContract);
    unknown['engine'] = 'host-only-experiment';
    expect(
      () => BenchmarkModelManifest.fromJson(unknown),
      throwsA(isA<FormatException>()),
    );

    final drifted = _manifest(offlineCandidateBenchmarkContract);
    final files = drifted['files']! as List<Map<String, dynamic>>;
    files.firstWhere((file) => file['role'] == 'model')['sha256'] =
        List<String>.filled(64, '0').join();
    expect(
      () => BenchmarkModelManifest.fromJson(drifted),
      throwsA(isA<FormatException>()),
    );
  });

  test('corpus manifest binds the canonical inventory and every input', () {
    final manifest = _corpusManifest();
    final parsed = BenchmarkCorpusManifest.fromJson(manifest);

    expect(parsed.cases, hasLength(benchmarkCorpusItems));
    expect(parsed.cases.where((entry) => entry.expectedLong), hasLength(3));
    expect(parsed.cases.last.category, 'companion');
    expect(parsed.cases.first.audioBytes, 32044);
    expect(parsed.cases.first.audioSha256, _digest('a'));

    final driftedAudio = _corpusManifest();
    (driftedAudio['cases']! as List<Map<String, dynamic>>).first.remove(
      'audioSha256',
    );
    expect(
      () => BenchmarkCorpusManifest.fromJson(driftedAudio),
      throwsA(isA<FormatException>()),
    );

    final reorderedLongRun = _corpusManifest();
    (reorderedLongRun['cases']! as List<Map<String, dynamic>>)[46]['long'] =
        true;
    expect(
      () => BenchmarkCorpusManifest.fromJson(reorderedLongRun),
      throwsA(isA<FormatException>()),
    );

    final driftedSource = _corpusManifest()..['sourceSha256'] = _digest('0');
    expect(
      () => BenchmarkCorpusManifest.fromJson(driftedSource),
      throwsA(isA<FormatException>()),
    );
  });
}

Map<String, dynamic> _corpusManifest() => <String, dynamic>{
  'schema': benchmarkCorpusManifestSchema,
  'id': benchmarkCorpusId,
  'kind': benchmarkCorpusKind,
  'locale': 'zh_TW',
  'transcriptionScript': 'Traditional Chinese',
  'voice': 'Meijia',
  'rateWordsPerMinute': 185,
  'source': benchmarkCorpusSource,
  'sourceSha256': benchmarkCorpusSourceSha256,
  'generatedAt': '2026-07-18T00:00:00Z',
  'items': benchmarkCorpusItems,
  'longItems': benchmarkCorpusLongItems,
  'targetLongDurationSeconds': 59.5,
  'cases': <Map<String, dynamic>>[
    for (var index = 0; index < benchmarkCorpusItems; index += 1)
      <String, dynamic>{
        'file':
            'zh-tw-synth-${(index + 1).toString().padLeft(3, '0')}-'
            '${_category(index)}.wav',
        'reference':
            'zh-tw-synth-${(index + 1).toString().padLeft(3, '0')}-'
            '${_category(index)}.txt',
        'category': _category(index),
        'long': index >= 47,
        'audioBytes': 32044,
        'audioSha256': _digest('a'),
        'referenceBytes': 24,
        'referenceSha256': _digest('b'),
      },
  ],
};

String _category(int index) {
  if (index >= 47) {
    return switch (index) {
      47 => 'daily',
      48 => 'engineering',
      _ => 'companion',
    };
  }
  return index >= 34 && index <= 43 ? 'code-switch' : 'daily';
}

String _digest(String character) => List<String>.filled(64, character).join();

Map<String, dynamic> _manifest(BenchmarkContract contract) => <String, dynamic>{
  'schema': modelManifestSchema,
  'engine': contract.engineSelector,
  'contract': contract.manifestContract,
  'modelId': contract.modelId,
  'repository': contract.repository,
  'commit': contract.commit,
  'sourceRevisionUrl': contract.sourceRevisionUrl,
  'licenseName': 'Apache-2.0',
  'licenseUrl': 'https://www.apache.org/licenses/LICENSE-2.0',
  'attribution': contract.attribution,
  'totalBytes': contract.totalBytes,
  'runtime': <String, dynamic>{
    'package': productionRuntimePackage,
    'version': productionRuntimeVersion,
    'provider': productionProvider,
    'threads': productionThreads,
  },
  'inference': <String, dynamic>{
    'mode': contract.inferenceMode,
    'sampleRate': productionSampleRate,
    'featureDimension': productionFeatureDimension,
    'decodingMethod': productionDecodingMethod,
    if (contract.engineSelector ==
        productionEngineSelector) ...<String, dynamic>{
      'chunkSamples': productionChunkSamples,
      'tailPaddingSamples': productionTailPaddingSamples,
      'maxDecodeSteps': productionMaxDecodeSteps,
      'endpointDetection': false,
    } else ...<String, dynamic>{
      'inputMode': 'whole_audio',
      'decodeTrigger': 'after_input_complete',
    },
  },
  'conversion': <String, dynamic>{
    'platform': 'android_icu',
    'transliterator': productionTransliterator,
    'minimumAndroidSdk': productionMinimumAndroidSdk,
  },
  'files': <Map<String, dynamic>>[
    for (final file in contract.files)
      <String, dynamic>{
        'role': file.role,
        'path': file.path,
        'bytes': file.bytes,
        'sha256': file.sha256,
      },
  ],
};
