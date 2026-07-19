import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:crypto/crypto.dart';
import 'package:tm_asr_benchmark/benchmark_contract.dart';
import 'package:tm_asr_benchmark/benchmark_metrics.dart';

const String benchmarkPairReportSchema = 'tempestmiku.p6-6.android-asr-ab.v1';

final _uuidV4Pattern = RegExp(
  r'^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$',
);

/// Integrity-bound local corpus inputs used to verify a device report.
///
/// Loading this object verifies the manifest itself and every referenced WAV
/// and text file. The host verifier can therefore compare report text and the
/// report's manifest digest against the exact local inputs instead of trusting
/// the report's own summaries.
final class BenchmarkCorpusEvidence {
  const BenchmarkCorpusEvidence({
    required this.contract,
    required this.manifestSha256,
    required this.references,
    required this.durationSeconds,
  });

  final BenchmarkCorpusManifest contract;
  final String manifestSha256;
  final Map<String, String> references;
  final Map<String, double> durationSeconds;

  static Future<BenchmarkCorpusEvidence> load(File manifestFile) async {
    final manifestBytes = await manifestFile.readAsBytes();
    final decoded = jsonDecode(utf8.decode(manifestBytes));
    if (decoded is! Map<String, dynamic>) {
      throw const FormatException('corpus manifest must be a JSON object');
    }
    final contract = BenchmarkCorpusManifest.fromJson(decoded);
    final corpusDirectory = Directory(
      '${manifestFile.parent.path}${Platform.pathSeparator}corpus',
    );
    final references = <String, String>{};
    final durationSeconds = <String, double>{};
    for (final corpusCase in contract.cases) {
      final audio = File(
        '${corpusDirectory.path}${Platform.pathSeparator}${corpusCase.file}',
      );
      final reference = File(
        '${corpusDirectory.path}${Platform.pathSeparator}'
        '${corpusCase.reference}',
      );
      await _verifyFile(
        audio,
        expectedBytes: corpusCase.audioBytes,
        expectedSha256: corpusCase.audioSha256,
      );
      await _verifyFile(
        reference,
        expectedBytes: corpusCase.referenceBytes,
        expectedSha256: corpusCase.referenceSha256,
      );
      final duration = await _pcm16MonoWavDurationSeconds(audio);
      if ((duration >= longRunMinimumSeconds) != corpusCase.expectedLong) {
        throw FormatException(
          '${corpusCase.file} long-run identity did not match its WAV',
        );
      }
      durationSeconds[corpusCase.file] = duration;
      references[corpusCase.reference] =
          (await reference.readAsString()).trim();
    }
    return BenchmarkCorpusEvidence(
      contract: contract,
      manifestSha256: sha256.convert(manifestBytes).toString(),
      references: Map<String, String>.unmodifiable(references),
      durationSeconds: Map<String, double>.unmodifiable(durationSeconds),
    );
  }
}

final class VerifiedBenchmarkReport {
  const VerifiedBenchmarkReport({
    required this.source,
    required this.contract,
    required this.device,
    required this.metrics,
    required this.completedAt,
    required this.corpusManifestSha256,
    required this.sourceSha256,
  });

  final Map<String, dynamic> source;
  final BenchmarkContract contract;
  final Map<String, dynamic> device;
  final Map<String, dynamic> metrics;
  final DateTime completedAt;
  final String corpusManifestSha256;
  final String sourceSha256;

  String get benchmarkInstallationId =>
      device['benchmarkInstallationId']! as String;

  String get benchmarkApkSha256 => device['benchmarkApkSha256']! as String;
}

/// Verifies one physical-Android report without trusting its aggregate fields.
///
/// Every quality/runtime aggregate is recalculated from the case records. RSS
/// is the one process-level value that cannot be reconstructed per case, so it
/// is required to be a positive measured value and checked directly against the
/// bound. The supplied corpus and APK digests are computed by the host CLIs.
VerifiedBenchmarkReport verifyBenchmarkReport({
  required Map<String, dynamic> report,
  required String expectedEngine,
  required BenchmarkCorpusEvidence corpusEvidence,
  required String expectedBenchmarkApkSha256,
  required String sourceSha256,
}) {
  _requireSha256(sourceSha256, 'source report SHA-256');
  _expect(report, 'schema', benchmarkReportSchema);
  final engine = _string(report, 'engine');
  if (engine != expectedEngine) {
    throw FormatException(
      'report engine was $engine, expected $expectedEngine',
    );
  }
  final contract = benchmarkContractForSelector(expectedEngine);
  _expect(report, 'evidence_scope', 'android_device');
  _expect(report, 'host_results_qualify_as_device_evidence', false);
  final completedAt = _utcTime(report, 'completed_at');

  final device = _object(report, 'device');
  _verifyDevice(device, expectedBenchmarkApkSha256);
  _verifyModel(_object(report, 'model'), contract);
  _verifyRuntime(_object(report, 'runtime'), contract);
  _verifyLimits(_object(report, 'limits'));
  _verifyCorpusIdentity(_object(report, 'corpus'), corpusEvidence);

  final claimedMetrics = _object(report, 'metrics');
  final peakRssKiB = _integer(claimedMetrics, 'peak_rss_kib');
  if (peakRssKiB <= 0) {
    throw const FormatException('peak_rss_kib was not a positive measurement');
  }
  final modelLoadMilliseconds = _integer(claimedMetrics, 'model_load_ms');
  if (modelLoadMilliseconds < 0) {
    throw const FormatException('model_load_ms must be non-negative');
  }

  final encodedCases = _list(report, 'cases');
  final expectedCases = corpusEvidence.contract.cases;
  if (encodedCases.length != expectedCases.length) {
    throw FormatException(
      'report contained ${encodedCases.length} cases; '
      'expected ${expectedCases.length}',
    );
  }

  var maxInferenceMilliseconds = 0;
  var maxRtf = 0.0;
  var maximumThermalStatus = 0;
  var longRuns = 0;
  var completedLongRuns = 0;
  var nonTruncatedLongRuns = 0;
  var rawCerTotal = 0.0;
  var convertedCerTotal = 0.0;
  var codeSwitchCerTotal = 0.0;
  var codeSwitchCerCount = 0;
  double? minimumLongRunCoverage;
  double? minimumLongRunTailCoverage;

  for (var index = 0; index < expectedCases.length; index += 1) {
    final encoded = encodedCases[index];
    if (encoded is! Map<Object?, Object?>) {
      throw FormatException('report case $index was not an object');
    }
    final deviceCase = encoded.map(
      (key, value) => MapEntry(key.toString(), value),
    );
    final expected = expectedCases[index];
    _expect(deviceCase, 'file', expected.file);
    _expect(deviceCase, 'category', expected.category);
    _expect(deviceCase, 'status', 'completed');

    final durationSeconds = _finiteNumber(deviceCase, 'duration_seconds');
    final expectedDuration = corpusEvidence.durationSeconds[expected.file];
    if (expectedDuration == null) {
      throw FormatException('${expected.file} has no host-derived duration');
    }
    if ((durationSeconds - expectedDuration).abs() > 1e-9) {
      throw FormatException(
        '${expected.file} duration did not match the hash-bound WAV',
      );
    }
    if (expectedDuration <= 0 || expectedDuration > 60) {
      throw FormatException(
        '${expected.file} duration was outside 0 < seconds <= 60',
      );
    }
    final isLong = expectedDuration >= longRunMinimumSeconds;
    if (isLong != expected.expectedLong) {
      throw FormatException('${expected.file} long-run identity drifted');
    }
    _expect(deviceCase, 'long_run', expected.expectedLong);
    if (isLong) longRuns += 1;

    final inferenceMilliseconds = _integer(deviceCase, 'inference_ms');
    final decodeMilliseconds = _integer(deviceCase, 'decode_ms');
    final conversionMilliseconds = _integer(deviceCase, 'conversion_ms');
    if (inferenceMilliseconds < 0 ||
        decodeMilliseconds < 0 ||
        conversionMilliseconds < 0) {
      throw FormatException('${expected.file} reported a negative duration');
    }
    if (inferenceMilliseconds < decodeMilliseconds + conversionMilliseconds) {
      throw FormatException(
        '${expected.file} inference_ms was shorter than decode plus conversion',
      );
    }
    final rtf = _finiteNumber(deviceCase, 'rtf');
    if (rtf < 0) {
      throw FormatException('${expected.file} reported a negative RTF');
    }
    final roundedRtf =
        inferenceMilliseconds /
        Duration.millisecondsPerSecond /
        expectedDuration;
    final timingTolerance = (0.001 / expectedDuration) + 1e-9;
    if ((rtf - roundedRtf).abs() > timingTolerance) {
      throw FormatException(
        '${expected.file} RTF was inconsistent with inference_ms',
      );
    }
    maxInferenceMilliseconds = math.max(
      maxInferenceMilliseconds,
      inferenceMilliseconds,
    );
    maxRtf = math.max(maxRtf, rtf);

    final rawText = _string(deviceCase, 'raw_text', allowEmpty: false);
    final convertedText = _string(
      deviceCase,
      'converted_text',
      allowEmpty: false,
    );
    final reference = _string(deviceCase, 'reference', allowEmpty: false);
    final expectedReference = corpusEvidence.references[expected.reference];
    if (expectedReference == null || reference != expectedReference) {
      throw FormatException(
        '${expected.file} reference did not match the local corpus',
      );
    }

    final rawCer = characterErrorRate(reference, rawText);
    final convertedCer = characterErrorRate(reference, convertedText);
    final referenceCoverage = referenceLcsCoverage(reference, convertedText);
    final tailReferenceCoverage = referenceTailLcsCoverage(
      reference,
      convertedText,
    );
    _expectClose(deviceCase, 'raw_cer', rawCer);
    _expectClose(deviceCase, 'converted_cer', convertedCer);
    _expectClose(deviceCase, 'reference_lcs_coverage', referenceCoverage);
    _expectClose(
      deviceCase,
      'tail_reference_lcs_coverage',
      tailReferenceCoverage,
    );
    rawCerTotal += rawCer;
    convertedCerTotal += convertedCer;
    if (expected.category == 'code-switch') {
      codeSwitchCerTotal += convertedCer;
      codeSwitchCerCount += 1;
    }
    if (isLong) {
      completedLongRuns += 1;
      minimumLongRunCoverage = _minimumNullable(
        minimumLongRunCoverage,
        referenceCoverage,
      );
      minimumLongRunTailCoverage = _minimumNullable(
        minimumLongRunTailCoverage,
        tailReferenceCoverage,
      );
      if (referenceCoverage >= minimumLongRunReferenceCoverage &&
          tailReferenceCoverage >= minimumLongRunTailReferenceCoverage) {
        nonTruncatedLongRuns += 1;
      }
    }

    final thermalBefore = _thermalSnapshot(
      deviceCase,
      'thermal_before',
      expected.file,
    );
    final thermalAfter = _thermalSnapshot(
      deviceCase,
      'thermal_after',
      expected.file,
    );
    maximumThermalStatus = math.max(
      maximumThermalStatus,
      math.max(thermalBefore, thermalAfter),
    );
  }

  if (codeSwitchCerCount == 0) {
    throw const FormatException('report contained no code-switch cases');
  }
  final corpusItems = expectedCases.length;
  final meanRawCer = rawCerTotal / corpusItems;
  final meanConvertedCer = convertedCerTotal / corpusItems;
  final codeSwitchMeanConvertedCer = codeSwitchCerTotal / codeSwitchCerCount;
  final recomputedMetrics = <String, dynamic>{
    'model_load_ms': modelLoadMilliseconds,
    'peak_rss_kib': peakRssKiB,
    'maximum_thermal_status': maximumThermalStatus,
    'thermal_snapshots_complete': true,
    'max_inference_ms': maxInferenceMilliseconds,
    'max_rtf': maxRtf,
    'mean_raw_cer': meanRawCer,
    'mean_converted_cer': meanConvertedCer,
    'code_switch_mean_converted_cer': codeSwitchMeanConvertedCer,
    'corpus_items': corpusItems,
    'completed_items': corpusItems,
    'failed_items': 0,
    'empty_raw_items': 0,
    'empty_converted_items': 0,
    'long_runs': longRuns,
    'completed_long_runs': completedLongRuns,
    'non_truncated_long_runs': nonTruncatedLongRuns,
    'corpus_integrity_bound': true,
    'minimum_long_run_reference_lcs_coverage': minimumLongRunCoverage,
    'minimum_long_run_tail_reference_lcs_coverage': minimumLongRunTailCoverage,
  };
  _verifyClaimedMetrics(claimedMetrics, recomputedMetrics);

  final recomputedGates = evaluateGates(
    ranOnAndroidDevice: true,
    modelBytes: contract.totalBytes,
    peakRssKiB: peakRssKiB,
    maxInferenceMilliseconds: maxInferenceMilliseconds,
    realTimeFactor: maxRtf,
    thermalStatusAvailable: true,
    corpusIntegrityBound: true,
    corpusItems: corpusItems,
    longRuns: longRuns,
    completedItems: corpusItems,
    failedItems: 0,
    emptyRawItems: 0,
    emptyConvertedItems: 0,
    completedLongRuns: completedLongRuns,
    nonTruncatedLongRuns: nonTruncatedLongRuns,
    convertedMeanCer: meanConvertedCer,
    codeSwitchConvertedMeanCer: codeSwitchMeanConvertedCer,
  );
  if (recomputedGates['passed'] != true) {
    throw FormatException(
      'recomputed Android gates failed: ${recomputedGates['failures']}',
    );
  }
  _expect(report, 'passed', true);
  final claimedGates = _object(report, 'gates');
  _expect(claimedGates, 'passed', true);
  final claimedFailures = _list(claimedGates, 'failures');
  if (claimedFailures.isNotEmpty) {
    throw const FormatException('passing report claimed gate failures');
  }

  return VerifiedBenchmarkReport(
    source: Map<String, dynamic>.unmodifiable(report),
    contract: contract,
    device: Map<String, dynamic>.unmodifiable(device),
    metrics: Map<String, dynamic>.unmodifiable(recomputedMetrics),
    completedAt: completedAt,
    corpusManifestSha256: corpusEvidence.manifestSha256,
    sourceSha256: sourceSha256,
  );
}

Map<String, dynamic> verifyBenchmarkPair({
  required Map<String, dynamic> productionReport,
  required String productionReportSha256,
  required Map<String, dynamic> candidateReport,
  required String candidateReportSha256,
  required BenchmarkCorpusEvidence corpusEvidence,
  required String expectedBenchmarkApkSha256,
  DateTime? verifiedAt,
}) {
  _requireSha256(productionReportSha256, 'production report SHA-256');
  _requireSha256(candidateReportSha256, 'candidate report SHA-256');
  final production = verifyBenchmarkReport(
    report: productionReport,
    expectedEngine: productionEngineSelector,
    corpusEvidence: corpusEvidence,
    expectedBenchmarkApkSha256: expectedBenchmarkApkSha256,
    sourceSha256: productionReportSha256,
  );
  final candidate = verifyBenchmarkReport(
    report: candidateReport,
    expectedEngine: offlineCandidateEngineSelector,
    corpusEvidence: corpusEvidence,
    expectedBenchmarkApkSha256: expectedBenchmarkApkSha256,
    sourceSha256: candidateReportSha256,
  );

  const identityFields = <String>[
    'benchmarkInstallationId',
    'benchmarkApkSha256',
    'manufacturer',
    'model',
    'device',
    'sdk',
    'fingerprint',
    'buildId',
    'securityPatch',
    'hardware',
    'board',
    'product',
    'supportedAbis',
    'physicalDevice',
  ];
  for (final field in identityFields) {
    if (jsonEncode(production.device[field]) !=
        jsonEncode(candidate.device[field])) {
      throw FormatException(
        'A/B reports came from different device/build field: $field',
      );
    }
  }
  if (production.corpusManifestSha256 != candidate.corpusManifestSha256) {
    throw const FormatException('A/B reports used different corpus manifests');
  }

  final productionMean = production.metrics['mean_converted_cer']! as double;
  final candidateMean = candidate.metrics['mean_converted_cer']! as double;
  final productionCodeSwitch =
      production.metrics['code_switch_mean_converted_cer']! as double;
  final candidateCodeSwitch =
      candidate.metrics['code_switch_mean_converted_cer']! as double;
  return <String, dynamic>{
    'schema': benchmarkPairReportSchema,
    'verified_at':
        (verifiedAt ?? DateTime.now().toUtc()).toUtc().toIso8601String(),
    'passed': true,
    'evidence_scope': 'physical_android_same_device_ab',
    'device': <String, dynamic>{
      for (final field in identityFields) field: production.device[field],
    },
    'corpus': <String, dynamic>{
      'id': benchmarkCorpusId,
      'source_sha256': benchmarkCorpusSourceSha256,
      'manifest_sha256': corpusEvidence.manifestSha256,
    },
    'benchmark_apk_sha256': expectedBenchmarkApkSha256,
    'production': _pairEngineSummary(production),
    'candidate': _pairEngineSummary(candidate),
    'comparison': <String, dynamic>{
      'candidate_minus_production_mean_converted_cer':
          candidateMean - productionMean,
      'candidate_minus_production_code_switch_converted_cer':
          candidateCodeSwitch - productionCodeSwitch,
      'candidate_minus_production_peak_rss_kib':
          (candidate.metrics['peak_rss_kib']! as int) -
          (production.metrics['peak_rss_kib']! as int),
      'candidate_minus_production_max_rtf':
          (candidate.metrics['max_rtf']! as double) -
          (production.metrics['max_rtf']! as double),
    },
    'gates': const <String, dynamic>{
      'production_report_verified': true,
      'candidate_report_verified': true,
      'same_physical_installation': true,
      'same_android_build': true,
      'same_benchmark_apk': true,
      'same_corpus_manifest': true,
    },
  };
}

Future<String> sha256File(File file) async =>
    (await sha256.bind(file.openRead()).first).toString();

Map<String, dynamic> _pairEngineSummary(VerifiedBenchmarkReport report) =>
    <String, dynamic>{
      'engine': report.contract.engineSelector,
      'completed_at': report.completedAt.toUtc().toIso8601String(),
      'report_sha256': report.sourceSha256,
      'model_id': report.contract.modelId,
      'model_bytes': report.contract.totalBytes,
      'metrics': report.metrics,
    };

void _verifyDevice(
  Map<String, dynamic> device,
  String expectedBenchmarkApkSha256,
) {
  _expect(device, 'physicalDevice', true);
  final sdk = _integer(device, 'sdk');
  if (sdk < productionMinimumAndroidSdk) {
    throw FormatException('device SDK $sdk is below Android 10');
  }
  for (final field in const <String>[
    'manufacturer',
    'model',
    'device',
    'fingerprint',
    'buildId',
    'securityPatch',
    'hardware',
    'board',
    'product',
  ]) {
    _string(device, field, allowEmpty: false);
  }
  final supportedAbis = _list(device, 'supportedAbis');
  if (!supportedAbis.every((value) => value is String) ||
      !supportedAbis.contains('arm64-v8a')) {
    throw const FormatException('device did not report arm64-v8a support');
  }
  final installationId = _string(
    device,
    'benchmarkInstallationId',
    allowEmpty: false,
  );
  if (!_uuidV4Pattern.hasMatch(installationId)) {
    throw const FormatException('benchmarkInstallationId was not UUIDv4');
  }
  _expect(
    device,
    'benchmarkApkSha256',
    _requireSha256(expectedBenchmarkApkSha256, 'benchmark APK SHA-256'),
  );
}

void _verifyModel(Map<String, dynamic> model, BenchmarkContract contract) {
  _expect(model, 'id', contract.modelId);
  _expect(model, 'repository', contract.repository);
  _expect(model, 'commit', contract.commit);
  _expect(model, 'source_revision_url', contract.sourceRevisionUrl);
  _expect(model, 'bytes', contract.totalBytes);
  _expect(model, 'attribution', contract.attribution);
  final license = _object(model, 'license');
  _expect(license, 'name', 'Apache-2.0');
  _expect(license, 'url', 'https://www.apache.org/licenses/LICENSE-2.0');
  final files = _list(model, 'files');
  if (files.length != contract.files.length) {
    throw const FormatException('report model file count drifted');
  }
  for (final expected in contract.files) {
    final matching =
        files.where((encoded) {
          return encoded is Map<Object?, Object?> &&
              encoded['role'] == expected.role;
        }).toList();
    if (matching.length != 1) {
      throw FormatException('model role ${expected.role} was not exact');
    }
    final file = matching.single as Map<Object?, Object?>;
    _expect(file, 'path', expected.path);
    _expect(file, 'bytes', expected.bytes);
    _expect(file, 'sha256', expected.sha256);
  }
}

void _verifyRuntime(Map<String, dynamic> runtime, BenchmarkContract contract) {
  _expect(runtime, 'engine', contract.engineSelector);
  _expect(runtime, 'inference_mode', contract.inferenceMode);
  _expect(runtime, 'sherpa_onnx_version', productionRuntimeVersion);
  _string(runtime, 'sherpa_onnx_git_sha', allowEmpty: false);
  _string(runtime, 'sherpa_onnx_git_date', allowEmpty: false);
  _expect(runtime, 'provider', productionProvider);
  _expect(runtime, 'threads', productionThreads);
  _expect(runtime, 'sample_rate', productionSampleRate);
  _expect(runtime, 'feature_dimension', productionFeatureDimension);
  _expect(runtime, 'decoding_method', productionDecodingMethod);
  _expect(
    runtime,
    'traditional_conversion',
    'Android ICU $productionTransliterator',
  );
  _string(runtime, 'os', allowEmpty: false);
  if (_integer(runtime, 'processors') <= 0) {
    throw const FormatException('runtime processor count was not positive');
  }
  if (contract.engineSelector == productionEngineSelector) {
    _expect(runtime, 'chunk_samples', productionChunkSamples);
    _expect(runtime, 'tail_padding_samples', productionTailPaddingSamples);
    _expect(runtime, 'max_decode_steps', productionMaxDecodeSteps);
    _expect(runtime, 'endpoint_detection', false);
  } else {
    _expect(runtime, 'input_mode', 'whole_audio');
    _expect(runtime, 'decode_trigger', 'after_input_complete');
  }
}

void _verifyLimits(Map<String, dynamic> limits) {
  _expect(limits, 'model_bytes', maxModelBytes);
  _expect(limits, 'peak_rss_kib', maxPeakRssKiB);
  _expect(limits, 'inference_ms', maxInferenceDurationMilliseconds);
  _expect(limits, 'max_rtf', maxRealTimeFactor);
  _expect(limits, 'minimum_corpus_items', minimumCorpusItems);
  _expect(limits, 'required_long_runs', requiredLongRuns);
  _expect(limits, 'integrity_bound_corpus_required', true);
  _expect(limits, 'converted_mean_cer', maxConvertedMeanCer);
  _expect(
    limits,
    'code_switch_converted_mean_cer',
    maxCodeSwitchConvertedMeanCer,
  );
  _expect(
    limits,
    'minimum_long_run_reference_lcs_coverage',
    minimumLongRunReferenceCoverage,
  );
  _expect(
    limits,
    'minimum_long_run_tail_reference_lcs_coverage',
    minimumLongRunTailReferenceCoverage,
  );
}

void _verifyCorpusIdentity(
  Map<String, dynamic> reported,
  BenchmarkCorpusEvidence evidence,
) {
  final local = evidence.contract.metadata;
  for (final key in const <String>[
    'schema',
    'id',
    'kind',
    'locale',
    'transcriptionScript',
    'voice',
    'rateWordsPerMinute',
    'source',
    'sourceSha256',
    'items',
    'longItems',
    'targetLongDurationSeconds',
    'generatedAt',
  ]) {
    _expect(reported, key, local[key]);
  }
  _expect(reported, 'manifestSha256', evidence.manifestSha256);
}

void _verifyClaimedMetrics(
  Map<String, dynamic> claimed,
  Map<String, dynamic> recomputed,
) {
  for (final key in const <String>[
    'model_load_ms',
    'peak_rss_kib',
    'maximum_thermal_status',
    'max_inference_ms',
    'corpus_items',
    'completed_items',
    'failed_items',
    'empty_raw_items',
    'empty_converted_items',
    'long_runs',
    'completed_long_runs',
    'non_truncated_long_runs',
    'corpus_integrity_bound',
  ]) {
    _expect(claimed, key, recomputed[key]);
  }
  _expect(claimed, 'thermal_snapshots_complete', true);
  for (final key in const <String>[
    'max_rtf',
    'mean_raw_cer',
    'mean_converted_cer',
    'code_switch_mean_converted_cer',
    'minimum_long_run_reference_lcs_coverage',
    'minimum_long_run_tail_reference_lcs_coverage',
  ]) {
    final expected = recomputed[key];
    if (expected is! num) {
      throw FormatException('recomputed metric $key was unavailable');
    }
    _expectClose(claimed, key, expected.toDouble());
  }
}

int _thermalSnapshot(Map<String, dynamic> deviceCase, String key, String file) {
  final thermal = _object(deviceCase, key);
  _expect(thermal, 'available', true);
  final status = _integer(thermal, 'status');
  if (status < 0 || status > 6) {
    throw FormatException('$file $key thermal status was outside 0..6');
  }
  const labels = <String>[
    'none',
    'light',
    'moderate',
    'severe',
    'critical',
    'emergency',
    'shutdown',
  ];
  _expect(thermal, 'label', labels[status]);
  return status;
}

Future<void> _verifyFile(
  File file, {
  required int expectedBytes,
  required String expectedSha256,
}) async {
  if (!await file.exists()) {
    throw FormatException('missing local corpus input: ${file.path}');
  }
  if (await file.length() != expectedBytes) {
    throw FormatException('local corpus size drifted: ${file.path}');
  }
  if (await sha256File(file) != expectedSha256) {
    throw FormatException('local corpus SHA-256 drifted: ${file.path}');
  }
}

Future<double> _pcm16MonoWavDurationSeconds(File file) async {
  final bytes = await file.readAsBytes();
  if (bytes.length < 44 ||
      ascii.decode(bytes.sublist(0, 4), allowInvalid: true) != 'RIFF' ||
      ascii.decode(bytes.sublist(8, 12), allowInvalid: true) != 'WAVE') {
    throw FormatException('${file.path} is not a RIFF/WAVE file');
  }
  final data = ByteData.sublistView(bytes);
  if (data.getUint32(4, Endian.little) + 8 != bytes.length) {
    throw FormatException('${file.path} RIFF length did not match the file');
  }
  var offset = 12;
  int? sampleRate;
  int? blockAlign;
  int? sampleBytes;
  while (offset + 8 <= bytes.length) {
    final chunkId = ascii.decode(
      bytes.sublist(offset, offset + 4),
      allowInvalid: true,
    );
    final chunkBytes = data.getUint32(offset + 4, Endian.little);
    final contentOffset = offset + 8;
    final contentEnd = contentOffset + chunkBytes;
    if (contentEnd > bytes.length) {
      throw FormatException('${file.path} contains a truncated WAV chunk');
    }
    if (chunkId == 'fmt ') {
      if (sampleRate != null || chunkBytes < 16) {
        throw FormatException('${file.path} has an invalid fmt chunk');
      }
      final audioFormat = data.getUint16(contentOffset, Endian.little);
      final channels = data.getUint16(contentOffset + 2, Endian.little);
      sampleRate = data.getUint32(contentOffset + 4, Endian.little);
      final byteRate = data.getUint32(contentOffset + 8, Endian.little);
      blockAlign = data.getUint16(contentOffset + 12, Endian.little);
      final bitsPerSample = data.getUint16(contentOffset + 14, Endian.little);
      if (audioFormat != 1 ||
          channels != 1 ||
          sampleRate != productionSampleRate ||
          blockAlign != 2 ||
          bitsPerSample != 16 ||
          byteRate != sampleRate * blockAlign) {
        throw FormatException('${file.path} is not mono 16 kHz PCM16');
      }
    } else if (chunkId == 'data') {
      if (sampleBytes != null) {
        throw FormatException('${file.path} has duplicate data chunks');
      }
      sampleBytes = chunkBytes;
    }
    offset = contentEnd + (chunkBytes.isOdd ? 1 : 0);
  }
  if (offset != bytes.length ||
      sampleRate == null ||
      blockAlign == null ||
      sampleBytes == null ||
      sampleBytes == 0 ||
      sampleBytes % blockAlign != 0) {
    throw FormatException('${file.path} has an incomplete PCM WAV contract');
  }
  return sampleBytes / blockAlign / sampleRate;
}

Map<String, dynamic> _object(Map<Object?, Object?> json, String key) {
  final value = json[key];
  if (value is! Map<Object?, Object?>) {
    throw FormatException('$key must be an object');
  }
  return value.map((key, value) => MapEntry(key.toString(), value));
}

List<Object?> _list(Map<Object?, Object?> json, String key) {
  final value = json[key];
  if (value is! List<Object?>) {
    throw FormatException('$key must be a list');
  }
  return value;
}

String _string(
  Map<Object?, Object?> json,
  String key, {
  bool allowEmpty = true,
}) {
  final value = json[key];
  if (value is! String || (!allowEmpty && value.trim().isEmpty)) {
    throw FormatException(
      '$key must be ${allowEmpty ? 'a string' : 'non-empty'}',
    );
  }
  return value;
}

int _integer(Map<Object?, Object?> json, String key) {
  final value = json[key];
  if (value is! int) throw FormatException('$key must be an integer');
  return value;
}

double _finiteNumber(Map<Object?, Object?> json, String key) {
  final value = json[key];
  if (value is! num || !value.toDouble().isFinite) {
    throw FormatException('$key must be a finite number');
  }
  return value.toDouble();
}

DateTime _utcTime(Map<Object?, Object?> json, String key) {
  final encoded = _string(json, key, allowEmpty: false);
  final parsed = DateTime.tryParse(encoded);
  if (parsed == null || !parsed.isUtc) {
    throw FormatException('$key must be an ISO-8601 UTC time');
  }
  return parsed;
}

void _expect(Map<Object?, Object?> json, String key, Object? expected) {
  if (json[key] != expected) {
    throw FormatException('$key must equal $expected');
  }
}

void _expectClose(Map<Object?, Object?> json, String key, double expected) {
  final actual = _finiteNumber(json, key);
  final tolerance = math.max(1e-12, expected.abs() * 1e-10);
  if ((actual - expected).abs() > tolerance) {
    throw FormatException('$key was $actual; recomputed value was $expected');
  }
}

double? _minimumNullable(double? current, double value) =>
    current == null || value < current ? value : current;

String _requireSha256(String value, String label) {
  if (!RegExp(r'^[0-9a-f]{64}$').hasMatch(value)) {
    throw FormatException('$label must be a lowercase SHA-256 digest');
  }
  return value;
}
