import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:crypto/crypto.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:tm_asr_benchmark/benchmark_contract.dart';
import 'package:tm_asr_benchmark/benchmark_metrics.dart';
import 'package:tm_asr_benchmark/report_verification.dart';

void main() {
  final corpus = _corpusEvidence();
  final apkSha256 = _digest('a');

  test('loads and re-hashes every local corpus input', () async {
    final fixture = await _writeCorpusFixture();
    try {
      final evidence = await BenchmarkCorpusEvidence.load(fixture.manifest);
      expect(evidence.references, hasLength(benchmarkCorpusItems));
      expect(evidence.durationSeconds, hasLength(benchmarkCorpusItems));
      expect(
        evidence.durationSeconds.values.where((value) => value >= 59),
        hasLength(3),
      );
      expect(evidence.manifestSha256, hasLength(64));

      await fixture.firstReference.writeAsString('drifted reference');
      expect(
        () => BenchmarkCorpusEvidence.load(fixture.manifest),
        throwsFormatException,
      );
    } finally {
      await fixture.root.delete(recursive: true);
    }
  });

  test('independently recomputes a complete passing report', () {
    final report = _report(productionBenchmarkContract, corpus, apkSha256);

    final verified = verifyBenchmarkReport(
      report: report,
      expectedEngine: productionEngineSelector,
      corpusEvidence: corpus,
      expectedBenchmarkApkSha256: apkSha256,
      sourceSha256: _digest('b'),
    );

    expect(verified.metrics['mean_converted_cer'], 0);
    expect(verified.metrics['thermal_snapshots_complete'], isTrue);
    expect(verified.benchmarkApkSha256, apkSha256);
  });

  test('rejects missing RSS, thermal snapshots, and tampered metrics', () {
    final missingRss = _report(productionBenchmarkContract, corpus, apkSha256);
    (missingRss['metrics']! as Map<String, dynamic>)['peak_rss_kib'] = 0;
    expect(() => _verify(missingRss, corpus, apkSha256), throwsFormatException);

    final missingThermal = _report(
      productionBenchmarkContract,
      corpus,
      apkSha256,
    );
    ((missingThermal['cases']! as List<Map<String, dynamic>>).first).remove(
      'thermal_after',
    );
    expect(
      () => _verify(missingThermal, corpus, apkSha256),
      throwsFormatException,
    );

    final tamperedCer = _report(productionBenchmarkContract, corpus, apkSha256);
    ((tamperedCer['cases']! as List<Map<String, dynamic>>).first)['raw_cer'] =
        0.01;
    expect(
      () => _verify(tamperedCer, corpus, apkSha256),
      throwsFormatException,
    );

    final tamperedAggregate = _report(
      productionBenchmarkContract,
      corpus,
      apkSha256,
    );
    (tamperedAggregate['metrics']!
            as Map<String, dynamic>)['mean_converted_cer'] =
        0.01;
    expect(
      () => _verify(tamperedAggregate, corpus, apkSha256),
      throwsFormatException,
    );

    final tamperedDuration = _report(
      productionBenchmarkContract,
      corpus,
      apkSha256,
    );
    ((tamperedDuration['cases']! as List<Map<String, dynamic>>)
            .first)['duration_seconds'] =
        59.5;
    expect(
      () => _verify(tamperedDuration, corpus, apkSha256),
      throwsFormatException,
    );

    final impossibleTiming = _report(
      productionBenchmarkContract,
      corpus,
      apkSha256,
    );
    final firstTiming =
        (impossibleTiming['cases']! as List<Map<String, dynamic>>).first;
    firstTiming['decode_ms'] = 91;
    firstTiming['conversion_ms'] = 10;
    expect(
      () => _verify(impossibleTiming, corpus, apkSha256),
      throwsFormatException,
    );
  });

  test('binds report corpus and benchmark APK to host-side inputs', () {
    final wrongCorpus = _report(productionBenchmarkContract, corpus, apkSha256);
    (wrongCorpus['corpus']!
        as Map<String, dynamic>)['manifestSha256'] = _digest('d');
    expect(
      () => _verify(wrongCorpus, corpus, apkSha256),
      throwsFormatException,
    );

    final wrongApk = _report(productionBenchmarkContract, corpus, apkSha256);
    expect(
      () => _verify(wrongApk, corpus, _digest('e')),
      throwsFormatException,
    );
  });

  test('emits a machine-readable exact-engine same-device A/B report', () {
    final production = _report(productionBenchmarkContract, corpus, apkSha256);
    final candidate = _report(
      offlineCandidateBenchmarkContract,
      corpus,
      apkSha256,
    );

    final pair = verifyBenchmarkPair(
      productionReport: production,
      productionReportSha256: _digest('1'),
      candidateReport: candidate,
      candidateReportSha256: _digest('2'),
      corpusEvidence: corpus,
      expectedBenchmarkApkSha256: apkSha256,
      verifiedAt: DateTime.utc(2026, 7, 19),
    );

    expect(pair['schema'], benchmarkPairReportSchema);
    expect(pair['passed'], isTrue);
    expect(
      (pair['production']! as Map<String, dynamic>)['engine'],
      productionEngineSelector,
    );
    expect(
      (pair['candidate']! as Map<String, dynamic>)['engine'],
      offlineCandidateEngineSelector,
    );

    (candidate['device']! as Map<String, dynamic>)['benchmarkInstallationId'] =
        '00000000-0000-4000-8000-000000000002';
    expect(
      () => verifyBenchmarkPair(
        productionReport: production,
        productionReportSha256: _digest('1'),
        candidateReport: candidate,
        candidateReportSha256: _digest('2'),
        corpusEvidence: corpus,
        expectedBenchmarkApkSha256: apkSha256,
      ),
      throwsFormatException,
    );
  });
}

VerifiedBenchmarkReport _verify(
  Map<String, dynamic> report,
  BenchmarkCorpusEvidence corpus,
  String apkSha256,
) => verifyBenchmarkReport(
  report: report,
  expectedEngine: productionEngineSelector,
  corpusEvidence: corpus,
  expectedBenchmarkApkSha256: apkSha256,
  sourceSha256: _digest('b'),
);

BenchmarkCorpusEvidence _corpusEvidence() {
  final cases = <BenchmarkCorpusCaseContract>[];
  final references = <String, String>{};
  final durationSeconds = <String, double>{};
  for (var index = 0; index < benchmarkCorpusItems; index += 1) {
    final number = (index + 1).toString().padLeft(3, '0');
    final category = index == 0 ? 'code-switch' : 'general';
    final file = 'zh-tw-synth-$number-$category.wav';
    final reference = 'zh-tw-synth-$number-$category.txt';
    cases.add(
      BenchmarkCorpusCaseContract(
        file: file,
        reference: reference,
        category: category,
        expectedLong: index >= 47,
        audioBytes: 1,
        audioSha256: _digest('a'),
        referenceBytes: 1,
        referenceSha256: _digest('b'),
      ),
    );
    references[reference] = '第${index + 1}句唯一精確參考文字';
    durationSeconds[file] = index >= 47 ? 59.5 : 1.0;
  }
  final metadata = <String, dynamic>{
    'schema': benchmarkCorpusManifestSchema,
    'id': benchmarkCorpusId,
    'kind': benchmarkCorpusKind,
    'locale': 'zh_TW',
    'transcriptionScript': 'Traditional Chinese',
    'voice': 'Meijia',
    'rateWordsPerMinute': 185,
    'source': benchmarkCorpusSource,
    'sourceSha256': benchmarkCorpusSourceSha256,
    'items': benchmarkCorpusItems,
    'longItems': benchmarkCorpusLongItems,
    'targetLongDurationSeconds': 59.5,
    'generatedAt': '2026-07-19T00:00:00.000Z',
  };
  return BenchmarkCorpusEvidence(
    contract: BenchmarkCorpusManifest(metadata: metadata, cases: cases),
    manifestSha256: _digest('c'),
    references: references,
    durationSeconds: durationSeconds,
  );
}

Map<String, dynamic> _report(
  BenchmarkContract contract,
  BenchmarkCorpusEvidence corpus,
  String apkSha256,
) {
  final cases = <Map<String, dynamic>>[];
  for (var index = 0; index < corpus.contract.cases.length; index += 1) {
    final corpusCase = corpus.contract.cases[index];
    final duration = corpus.durationSeconds[corpusCase.file]!;
    final reference = corpus.references[corpusCase.reference]!;
    cases.add(<String, dynamic>{
      'file': corpusCase.file,
      'category': corpusCase.category,
      'status': 'completed',
      'long_run': corpusCase.expectedLong,
      'duration_seconds': duration,
      'inference_ms': 100,
      'decode_ms': 90,
      'conversion_ms': 10,
      'rtf': 0.1 / duration,
      'raw_text': reference,
      'converted_text': reference,
      'reference': reference,
      'raw_cer': 0.0,
      'converted_cer': 0.0,
      'reference_lcs_coverage': 1.0,
      'tail_reference_lcs_coverage': 1.0,
      'thermal_before': <String, dynamic>{
        'available': true,
        'status': 0,
        'label': 'none',
      },
      'thermal_after': <String, dynamic>{
        'available': true,
        'status': 0,
        'label': 'none',
      },
    });
  }
  final metrics = <String, dynamic>{
    'model_load_ms': 10,
    'peak_rss_kib': 500000,
    'maximum_thermal_status': 0,
    'thermal_snapshots_complete': true,
    'max_inference_ms': 100,
    'max_rtf': 0.1,
    'mean_raw_cer': 0.0,
    'mean_converted_cer': 0.0,
    'code_switch_mean_converted_cer': 0.0,
    'corpus_items': benchmarkCorpusItems,
    'completed_items': benchmarkCorpusItems,
    'failed_items': 0,
    'empty_raw_items': 0,
    'empty_converted_items': 0,
    'long_runs': benchmarkCorpusLongItems,
    'completed_long_runs': benchmarkCorpusLongItems,
    'non_truncated_long_runs': benchmarkCorpusLongItems,
    'corpus_integrity_bound': true,
    'minimum_long_run_reference_lcs_coverage': 1.0,
    'minimum_long_run_tail_reference_lcs_coverage': 1.0,
  };
  return <String, dynamic>{
    'schema': benchmarkReportSchema,
    'completed_at': '2026-07-19T00:00:00.000Z',
    'passed': true,
    'engine': contract.engineSelector,
    'evidence_scope': 'android_device',
    'host_results_qualify_as_device_evidence': false,
    'device': <String, dynamic>{
      'manufacturer': 'Vendor',
      'model': 'Phone',
      'device': 'phone',
      'sdk': 35,
      'fingerprint': 'vendor/phone/phone:15/build/id:user/release-keys',
      'buildId': 'BUILD',
      'securityPatch': '2026-07-01',
      'hardware': 'soc',
      'board': 'board',
      'product': 'product',
      'supportedAbis': <String>['arm64-v8a'],
      'physicalDevice': true,
      'benchmarkInstallationId': '00000000-0000-4000-8000-000000000001',
      'benchmarkApkSha256': apkSha256,
    },
    'model': <String, dynamic>{
      'id': contract.modelId,
      'repository': contract.repository,
      'commit': contract.commit,
      'source_revision_url': contract.sourceRevisionUrl,
      'bytes': contract.totalBytes,
      'attribution': contract.attribution,
      'license': <String, dynamic>{
        'name': 'Apache-2.0',
        'url': 'https://www.apache.org/licenses/LICENSE-2.0',
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
    },
    'runtime': <String, dynamic>{
      'engine': contract.engineSelector,
      'inference_mode': contract.inferenceMode,
      'sherpa_onnx_version': productionRuntimeVersion,
      'sherpa_onnx_git_sha': 'abc123',
      'sherpa_onnx_git_date': '2026-07-19',
      'provider': productionProvider,
      'threads': productionThreads,
      'sample_rate': productionSampleRate,
      'feature_dimension': productionFeatureDimension,
      'decoding_method': productionDecodingMethod,
      if (contract.engineSelector == productionEngineSelector) ...{
        'chunk_samples': productionChunkSamples,
        'tail_padding_samples': productionTailPaddingSamples,
        'max_decode_steps': productionMaxDecodeSteps,
        'endpoint_detection': false,
      } else ...{
        'input_mode': 'whole_audio',
        'decode_trigger': 'after_input_complete',
      },
      'traditional_conversion': 'Android ICU $productionTransliterator',
      'os': 'Android 15',
      'processors': 8,
    },
    'corpus': <String, dynamic>{
      ...corpus.contract.metadata,
      'manifestSha256': corpus.manifestSha256,
    },
    'limits': <String, dynamic>{
      'model_bytes': maxModelBytes,
      'peak_rss_kib': maxPeakRssKiB,
      'inference_ms': maxInferenceDurationMilliseconds,
      'max_rtf': maxRealTimeFactor,
      'minimum_corpus_items': minimumCorpusItems,
      'required_long_runs': requiredLongRuns,
      'integrity_bound_corpus_required': true,
      'converted_mean_cer': maxConvertedMeanCer,
      'code_switch_converted_mean_cer': maxCodeSwitchConvertedMeanCer,
      'minimum_long_run_reference_lcs_coverage':
          minimumLongRunReferenceCoverage,
      'minimum_long_run_tail_reference_lcs_coverage':
          minimumLongRunTailReferenceCoverage,
    },
    'metrics': metrics,
    'gates': <String, dynamic>{'passed': true, 'failures': <String>[]},
    'cases': cases,
  };
}

String _digest(String character) => List.filled(64, character).join();

Future<_CorpusFixture> _writeCorpusFixture() async {
  final root = await Directory.systemTemp.createTemp('tm-asr-corpus-');
  final corpusDirectory = await Directory('${root.path}/corpus').create();
  final cases = <Map<String, dynamic>>[];
  File? firstReference;
  for (var index = 0; index < benchmarkCorpusItems; index += 1) {
    final number = (index + 1).toString().padLeft(3, '0');
    final category = index == 0 ? 'code-switch' : 'general';
    final base = 'zh-tw-synth-$number-$category';
    final audioBytes = _pcm16MonoWav(index >= 47 ? 59.5 : 1.0);
    final referenceBytes = utf8.encode('第${index + 1}句參考文字');
    final audio = File('${corpusDirectory.path}/$base.wav');
    final reference = File('${corpusDirectory.path}/$base.txt');
    await audio.writeAsBytes(audioBytes);
    await reference.writeAsBytes(referenceBytes);
    firstReference ??= reference;
    cases.add(<String, dynamic>{
      'file': '$base.wav',
      'reference': '$base.txt',
      'category': category,
      'long': index >= 47,
      'audioBytes': audioBytes.length,
      'audioSha256': sha256.convert(audioBytes).toString(),
      'referenceBytes': referenceBytes.length,
      'referenceSha256': sha256.convert(referenceBytes).toString(),
    });
  }
  final manifest = File('${root.path}/corpus-manifest.json');
  await manifest.writeAsString(
    jsonEncode(<String, dynamic>{
      'schema': benchmarkCorpusManifestSchema,
      'id': benchmarkCorpusId,
      'kind': benchmarkCorpusKind,
      'locale': 'zh_TW',
      'transcriptionScript': 'Traditional Chinese',
      'voice': 'Meijia',
      'rateWordsPerMinute': 185,
      'source': benchmarkCorpusSource,
      'sourceSha256': benchmarkCorpusSourceSha256,
      'items': benchmarkCorpusItems,
      'longItems': benchmarkCorpusLongItems,
      'targetLongDurationSeconds': 59.5,
      'generatedAt': '2026-07-19T00:00:00.000Z',
      'cases': cases,
    }),
  );
  return _CorpusFixture(
    root: root,
    manifest: manifest,
    firstReference: firstReference!,
  );
}

final class _CorpusFixture {
  const _CorpusFixture({
    required this.root,
    required this.manifest,
    required this.firstReference,
  });

  final Directory root;
  final File manifest;
  final File firstReference;
}

Uint8List _pcm16MonoWav(double durationSeconds) {
  final samples = (productionSampleRate * durationSeconds).round();
  final sampleBytes = samples * 2;
  final bytes = Uint8List(44 + sampleBytes);
  final data = ByteData.sublistView(bytes);

  void writeAscii(int offset, String value) {
    bytes.setRange(offset, offset + value.length, ascii.encode(value));
  }

  writeAscii(0, 'RIFF');
  data.setUint32(4, bytes.length - 8, Endian.little);
  writeAscii(8, 'WAVE');
  writeAscii(12, 'fmt ');
  data.setUint32(16, 16, Endian.little);
  data.setUint16(20, 1, Endian.little);
  data.setUint16(22, 1, Endian.little);
  data.setUint32(24, productionSampleRate, Endian.little);
  data.setUint32(28, productionSampleRate * 2, Endian.little);
  data.setUint16(32, 2, Endian.little);
  data.setUint16(34, 16, Endian.little);
  writeAscii(36, 'data');
  data.setUint32(40, sampleBytes, Endian.little);
  return bytes;
}
