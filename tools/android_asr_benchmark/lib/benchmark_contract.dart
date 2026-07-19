const int modelManifestSchema = 2;
const int benchmarkCorpusManifestSchema = 3;
const int benchmarkReportSchema = 3;

const String benchmarkCorpusId = 'zh-tw-meijia-synthetic-v1';
const String benchmarkCorpusKind = 'synthetic_tts';
const String benchmarkCorpusSource = 'corpus/zh-tw-synthetic-v1.tsv';
const String benchmarkCorpusSourceSha256 =
    '424fc2e9baf3f40240f7e23ffc7aacd1f00f71a4780074b82c153560873fd621';
const int benchmarkCorpusItems = 50;
const int benchmarkCorpusLongItems = 3;

const String productionEngineSelector = 'streaming-production';
const String offlineCandidateEngineSelector = 'offline-paraformer-candidate';

const String benchmarkModelContract = 'tempestmiku.streaming-paraformer.v1';
const String productionModelCommit = '2a7f71bb58885c1b522ed4e683abd397355d9fc4';
const String productionModelId =
    'csukuangfj/sherpa-onnx-streaming-paraformer-zh@'
    '$productionModelCommit';
const String productionModelRepository =
    'https://huggingface.co/csukuangfj/'
    'sherpa-onnx-streaming-paraformer-zh';

const String offlineCandidateModelContract =
    'tempestmiku.offline-paraformer-candidate.v1';
const String offlineCandidateModelCommit =
    'def027084691107096b5ebba69785756d63de6c5';
const String offlineCandidateModelId =
    'csukuangfj/sherpa-onnx-paraformer-zh-2023-09-14@'
    '$offlineCandidateModelCommit';
const String offlineCandidateModelRepository =
    'https://huggingface.co/csukuangfj/'
    'sherpa-onnx-paraformer-zh-2023-09-14';

const String productionRuntimePackage = 'sherpa_onnx';
const String productionRuntimeVersion = '1.13.4';
const String productionProvider = 'cpu';
const int productionThreads = 2;
const int productionSampleRate = 16000;
const int productionFeatureDimension = 80;
const String productionDecodingMethod = 'greedy_search';
const String productionTransliterator = 'Simplified-Traditional';
const int productionMinimumAndroidSdk = 29;

const int productionChunkSamples = productionSampleRate ~/ 10;
const int productionTailPaddingSamples = productionSampleRate;
const int productionMaxDecodeSteps = 20000;
const int productionModelBytes = 237202501;

const int offlineCandidateModelBytes = 243446974;

const List<BenchmarkModelFileContract> productionModelFiles = [
  BenchmarkModelFileContract(
    role: 'encoder',
    path: 'encoder.int8.onnx',
    bytes: 165462184,
    sha256: '81a70226a8934e6ed92aa1d4fc486b428b5398e2f2619ed4897b7294cab90e9a',
  ),
  BenchmarkModelFileContract(
    role: 'decoder',
    path: 'decoder.int8.onnx',
    bytes: 71664561,
    sha256: 'f3cca9f77bb9d93c8fcbfb63ae617b6b1ee96818df3aa3b151c40658fe38594f',
  ),
  BenchmarkModelFileContract(
    role: 'tokens',
    path: 'tokens.txt',
    bytes: 75756,
    sha256: '59aba8873a2ed1e122c25fee421e25f283b63290efbde85c1f01a853d83cb6e6',
  ),
];

const List<BenchmarkModelFileContract> offlineCandidateModelFiles = [
  BenchmarkModelFileContract(
    role: 'model',
    path: 'model.int8.onnx',
    bytes: 243371218,
    sha256: 'f36a0433bcf096bd6d6f11b80a3ac8bed110bdca632fe0d731df8d1a84475945',
  ),
  BenchmarkModelFileContract(
    role: 'tokens',
    path: 'tokens.txt',
    bytes: 75756,
    sha256: '59aba8873a2ed1e122c25fee421e25f283b63290efbde85c1f01a853d83cb6e6',
  ),
];

const BenchmarkContract productionBenchmarkContract = BenchmarkContract(
  engineSelector: productionEngineSelector,
  manifestContract: benchmarkModelContract,
  inferenceMode: 'streaming',
  modelId: productionModelId,
  repository: productionModelRepository,
  commit: productionModelCommit,
  attribution: 'sherpa-onnx streaming Paraformer Chinese model by csukuangfj',
  totalBytes: productionModelBytes,
  files: productionModelFiles,
);

const BenchmarkContract offlineCandidateBenchmarkContract = BenchmarkContract(
  engineSelector: offlineCandidateEngineSelector,
  manifestContract: offlineCandidateModelContract,
  inferenceMode: 'offline_whole_audio',
  modelId: offlineCandidateModelId,
  repository: offlineCandidateModelRepository,
  commit: offlineCandidateModelCommit,
  attribution: 'sherpa-onnx offline Paraformer Chinese model by csukuangfj',
  totalBytes: offlineCandidateModelBytes,
  files: offlineCandidateModelFiles,
);

final class BenchmarkModelFileContract {
  const BenchmarkModelFileContract({
    required this.role,
    required this.path,
    required this.bytes,
    required this.sha256,
  });

  final String role;
  final String path;
  final int bytes;
  final String sha256;
}

final class BenchmarkContract {
  const BenchmarkContract({
    required this.engineSelector,
    required this.manifestContract,
    required this.inferenceMode,
    required this.modelId,
    required this.repository,
    required this.commit,
    required this.attribution,
    required this.totalBytes,
    required this.files,
  });

  final String engineSelector;
  final String manifestContract;
  final String inferenceMode;
  final String modelId;
  final String repository;
  final String commit;
  final String attribution;
  final int totalBytes;
  final List<BenchmarkModelFileContract> files;

  String get sourceRevisionUrl => '$repository/tree/$commit';
}

BenchmarkContract benchmarkContractForSelector(String selector) =>
    switch (selector) {
      productionEngineSelector => productionBenchmarkContract,
      offlineCandidateEngineSelector => offlineCandidateBenchmarkContract,
      _ => throw FormatException('unknown benchmark engine: $selector'),
    };

final class BenchmarkModelManifest {
  const BenchmarkModelManifest({required this.contract, required this.files});

  final BenchmarkContract contract;
  final Map<String, BenchmarkModelFileContract> files;

  BenchmarkModelFileContract file(String role) =>
      files[role] ?? (throw StateError('missing model role: $role'));

  factory BenchmarkModelManifest.fromJson(Map<String, dynamic> json) {
    _expect(json, 'schema', modelManifestSchema);
    final engine = json['engine'];
    if (engine is! String) {
      throw const FormatException('engine must be a string');
    }
    final contract = benchmarkContractForSelector(engine);
    _expect(json, 'contract', contract.manifestContract);
    _expect(json, 'modelId', contract.modelId);
    _expect(json, 'repository', contract.repository);
    _expect(json, 'commit', contract.commit);
    _expect(json, 'sourceRevisionUrl', contract.sourceRevisionUrl);
    _expect(json, 'licenseName', 'Apache-2.0');
    _expect(json, 'licenseUrl', 'https://www.apache.org/licenses/LICENSE-2.0');
    _expect(json, 'attribution', contract.attribution);
    _expect(json, 'totalBytes', contract.totalBytes);

    final runtime = _object(json, 'runtime');
    _expect(runtime, 'package', productionRuntimePackage);
    _expect(runtime, 'version', productionRuntimeVersion);
    _expect(runtime, 'provider', productionProvider);
    _expect(runtime, 'threads', productionThreads);

    final inference = _object(json, 'inference');
    _expect(inference, 'mode', contract.inferenceMode);
    _expect(inference, 'sampleRate', productionSampleRate);
    _expect(inference, 'featureDimension', productionFeatureDimension);
    _expect(inference, 'decodingMethod', productionDecodingMethod);
    if (contract.engineSelector == productionEngineSelector) {
      _expect(inference, 'chunkSamples', productionChunkSamples);
      _expect(inference, 'tailPaddingSamples', productionTailPaddingSamples);
      _expect(inference, 'maxDecodeSteps', productionMaxDecodeSteps);
      _expect(inference, 'endpointDetection', false);
    } else {
      _expect(inference, 'inputMode', 'whole_audio');
      _expect(inference, 'decodeTrigger', 'after_input_complete');
    }

    final conversion = _object(json, 'conversion');
    _expect(conversion, 'platform', 'android_icu');
    _expect(conversion, 'transliterator', productionTransliterator);
    _expect(conversion, 'minimumAndroidSdk', productionMinimumAndroidSdk);

    final encodedFiles = json['files'];
    if (encodedFiles is! List<Object?> ||
        encodedFiles.length != contract.files.length) {
      throw FormatException(
        'files must contain the exact ${contract.files.length} files',
      );
    }
    final files = <String, BenchmarkModelFileContract>{};
    for (final encoded in encodedFiles) {
      if (encoded is! Map<Object?, Object?>) {
        throw const FormatException('each model file must be an object');
      }
      final role = encoded['role'];
      if (role is! String) {
        throw const FormatException('model file role must be a string');
      }
      final expected =
          contract.files
              .where((candidate) => candidate.role == role)
              .firstOrNull;
      if (expected == null || files.containsKey(role)) {
        throw FormatException('unknown or duplicate model file role: $role');
      }
      _expect(encoded, 'path', expected.path);
      _expect(encoded, 'bytes', expected.bytes);
      _expect(encoded, 'sha256', expected.sha256);
      files[role] = expected;
    }
    if (files.length != contract.files.length) {
      throw const FormatException('model manifest omitted a required role');
    }
    return BenchmarkModelManifest(
      contract: contract,
      files: Map.unmodifiable(files),
    );
  }
}

/// Exact, integrity-bound synthetic corpus contract used by both A/B engines.
///
/// Model files were already digest-bound, but an A/B result is not durable
/// evidence if its references or WAVs can drift independently of the report.
/// Schema 3 therefore requires size and SHA-256 for every app-private input in
/// addition to the pinned source inventory.
final class BenchmarkCorpusManifest {
  const BenchmarkCorpusManifest({required this.metadata, required this.cases});

  final Map<String, dynamic> metadata;
  final List<BenchmarkCorpusCaseContract> cases;

  factory BenchmarkCorpusManifest.fromJson(Map<String, dynamic> json) {
    _expect(json, 'schema', benchmarkCorpusManifestSchema);
    _expect(json, 'id', benchmarkCorpusId);
    _expect(json, 'kind', benchmarkCorpusKind);
    _expect(json, 'locale', 'zh_TW');
    _expect(json, 'transcriptionScript', 'Traditional Chinese');
    _expect(json, 'voice', 'Meijia');
    _expect(json, 'rateWordsPerMinute', 185);
    _expect(json, 'source', benchmarkCorpusSource);
    _expect(json, 'sourceSha256', benchmarkCorpusSourceSha256);
    _expect(json, 'items', benchmarkCorpusItems);
    _expect(json, 'longItems', benchmarkCorpusLongItems);
    _expect(json, 'targetLongDurationSeconds', 59.5);
    final generatedAt = _nonEmptyString(json, 'generatedAt');
    final generated = DateTime.tryParse(generatedAt);
    if (generated == null || !generated.isUtc) {
      throw const FormatException('generatedAt must be an ISO-8601 UTC time');
    }

    final encodedCases = json['cases'];
    if (encodedCases is! List<Object?> ||
        encodedCases.length != benchmarkCorpusItems) {
      throw const FormatException('corpus must contain exactly 50 cases');
    }
    final cases = <BenchmarkCorpusCaseContract>[];
    final audioNames = <String>{};
    final referenceNames = <String>{};
    for (var index = 0; index < encodedCases.length; index += 1) {
      final encoded = encodedCases[index];
      if (encoded is! Map<Object?, Object?>) {
        throw const FormatException('corpus case must be an object');
      }
      final file = _nonEmptyString(encoded, 'file');
      final reference = _nonEmptyString(encoded, 'reference');
      final category = _nonEmptyString(encoded, 'category');
      final expectedPrefix = (index + 1).toString().padLeft(3, '0');
      if (!RegExp(
        '^zh-tw-synth-$expectedPrefix-[a-z-]+\\.wav\$',
      ).hasMatch(file)) {
        throw FormatException('corpus case $index has a non-canonical WAV');
      }
      if (reference != '${file.substring(0, file.length - 4)}.txt') {
        throw FormatException('corpus case $index reference does not match');
      }
      if (!RegExp(r'^[a-z]+(?:-[a-z]+)*$').hasMatch(category) ||
          !file.endsWith('-$category.wav')) {
        throw FormatException('corpus case $index category does not match');
      }
      if (!audioNames.add(file) || !referenceNames.add(reference)) {
        throw FormatException('corpus case $index duplicates an input');
      }
      final isLong = encoded['long'];
      if (isLong is! bool || isLong != (index >= 47)) {
        throw const FormatException(
          'the final three corpus cases must be consecutive long runs',
        );
      }
      final audioBytes = _positiveInt(encoded, 'audioBytes');
      final referenceBytes = _positiveInt(encoded, 'referenceBytes');
      if (audioBytes > 2 * 1024 * 1024) {
        throw FormatException('corpus case $index WAV exceeds 2 MiB');
      }
      if (referenceBytes > 64 * 1024) {
        throw FormatException('corpus case $index reference exceeds 64 KiB');
      }
      cases.add(
        BenchmarkCorpusCaseContract(
          file: file,
          reference: reference,
          category: category,
          expectedLong: isLong,
          audioBytes: audioBytes,
          audioSha256: _sha256(encoded, 'audioSha256'),
          referenceBytes: referenceBytes,
          referenceSha256: _sha256(encoded, 'referenceSha256'),
        ),
      );
    }
    if (cases.where((entry) => entry.category == 'code-switch').isEmpty) {
      throw const FormatException('corpus must contain code-switch cases');
    }
    return BenchmarkCorpusManifest(
      metadata: Map<String, dynamic>.unmodifiable(json),
      cases: List<BenchmarkCorpusCaseContract>.unmodifiable(cases),
    );
  }
}

final class BenchmarkCorpusCaseContract {
  const BenchmarkCorpusCaseContract({
    required this.file,
    required this.reference,
    required this.category,
    required this.expectedLong,
    required this.audioBytes,
    required this.audioSha256,
    required this.referenceBytes,
    required this.referenceSha256,
  });

  final String file;
  final String reference;
  final String category;
  final bool expectedLong;
  final int audioBytes;
  final String audioSha256;
  final int referenceBytes;
  final String referenceSha256;
}

Map<String, dynamic> _object(Map<Object?, Object?> json, String key) {
  final value = json[key];
  if (value is! Map<Object?, Object?>) {
    throw FormatException('$key must be an object');
  }
  return value.map((key, value) => MapEntry(key.toString(), value));
}

void _expect(Map<Object?, Object?> json, String key, Object expected) {
  if (json[key] != expected) {
    throw FormatException('$key must equal $expected');
  }
}

String _nonEmptyString(Map<Object?, Object?> json, String key) {
  final value = json[key];
  if (value is! String || value.trim().isEmpty) {
    throw FormatException('$key must be a non-empty string');
  }
  return value;
}

int _positiveInt(Map<Object?, Object?> json, String key) {
  final value = json[key];
  if (value is! int || value <= 0) {
    throw FormatException('$key must be a positive integer');
  }
  return value;
}

String _sha256(Map<Object?, Object?> json, String key) {
  final value = _nonEmptyString(json, key);
  if (!RegExp(r'^[0-9a-f]{64}$').hasMatch(value)) {
    throw FormatException('$key must be a lowercase SHA-256 digest');
  }
  return value;
}
