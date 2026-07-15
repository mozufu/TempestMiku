import 'dart:convert';
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:path_provider/path_provider.dart';
import 'package:sherpa_onnx/sherpa_onnx.dart' as sherpa_onnx;
import 'package:tm_asr_benchmark/benchmark_metrics.dart';

const int _sampleRate = 16000;
const int _maxSamples = _sampleRate * 60;
const String _manifestName = 'model-manifest.json';
const String _corpusManifestName = 'corpus-manifest.json';
const String _reportName = 'last-result.json';
const MethodChannel _deviceChannel = MethodChannel(
  'org.mozufu.tempestmiku.asrbenchmark/device',
);

void main() {
  runApp(const AsrBenchmarkApp());
}

class AsrBenchmarkApp extends StatelessWidget {
  const AsrBenchmarkApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'TempestMiku ASR Benchmark',
      theme: ThemeData(colorSchemeSeed: Colors.teal, useMaterial3: true),
      home: const BenchmarkPage(),
    );
  }
}

class BenchmarkPage extends StatefulWidget {
  const BenchmarkPage({super.key});

  @override
  State<BenchmarkPage> createState() => _BenchmarkPageState();
}

class _BenchmarkPageState extends State<BenchmarkPage> {
  String? _rootPath;
  Map<String, dynamic>? _report;
  String _status = 'Locating app-private benchmark directory…';
  bool _running = false;

  @override
  void initState() {
    super.initState();
    _refresh();
  }

  Future<void> _refresh() async {
    try {
      final support = await getApplicationSupportDirectory();
      final root = Directory('${support.path}/asr-benchmark');
      await root.create(recursive: true);
      final reportFile = File('${root.path}/$_reportName');
      Map<String, dynamic>? report;
      if (await reportFile.exists()) {
        report =
            jsonDecode(await reportFile.readAsString()) as Map<String, dynamic>;
      }
      if (!mounted) return;
      setState(() {
        _rootPath = root.path;
        _report = report;
        _status =
            report == null
                ? 'Push a pinned model manifest and WAV corpus, then run.'
                : 'Loaded the most recent report.';
      });
    } catch (error) {
      if (!mounted) return;
      setState(() => _status = 'Setup failed: $error');
    }
  }

  Future<void> _run() async {
    final rootPath = _rootPath;
    if (rootPath == null || _running) return;
    setState(() {
      _running = true;
      _status = 'Running native inference on a worker isolate…';
    });
    final device =
        await _deviceChannel.invokeMapMethod<String, dynamic>(
          'getDeviceInfo',
        ) ??
        <String, dynamic>{};
    final report = await compute(runBenchmark, <String, dynamic>{
      'rootPath': rootPath,
      'device': device,
    });
    if (!mounted) return;
    setState(() {
      _running = false;
      _report = report;
      final passed = report['passed'] == true;
      _status =
          passed
              ? 'All P6.6 benchmark gates passed.'
              : 'Benchmark finished; one or more gates remain open.';
    });
  }

  @override
  Widget build(BuildContext context) {
    final report = _report;
    final error = report?['error'];
    final metrics = report?['metrics'] as Map<String, dynamic>?;
    final gates = report?['gates'] as Map<String, dynamic>?;
    return Scaffold(
      appBar: AppBar(title: const Text('On-device ASR benchmark')),
      body: ListView(
        padding: const EdgeInsets.all(20),
        children: [
          const Text(
            'Standalone debug-only harness. It has no microphone, network, '
            'session, or message-send path.',
          ),
          const SizedBox(height: 16),
          SelectableText('Private data: ${_rootPath ?? '…'}'),
          const SizedBox(height: 12),
          Text(_status),
          if (error != null) ...[
            const SizedBox(height: 12),
            SelectableText('Error: $error'),
          ],
          if (metrics != null) ...[
            const SizedBox(height: 16),
            Text('Model load: ${metrics['model_load_ms']} ms'),
            Text('Peak RSS: ${metrics['peak_rss_kib']} KiB'),
            Text('Max RTF: ${metrics['max_rtf']}'),
            Text('Mean strict CER: ${metrics['mean_cer'] ?? 'n/a'}'),
            Text('Corpus items: ${metrics['corpus_items']}'),
          ],
          if (gates != null) ...[
            const SizedBox(height: 12),
            SelectableText('Open gates: ${gates['failures']}'),
          ],
          const SizedBox(height: 20),
          FilledButton(
            onPressed: _running || _rootPath == null ? null : _run,
            child: Text(_running ? 'Running…' : 'Run benchmark'),
          ),
          TextButton(
            onPressed: _running ? null : _refresh,
            child: const Text('Refresh'),
          ),
        ],
      ),
    );
  }
}

@pragma('vm:entry-point')
Future<Map<String, dynamic>> runBenchmark(Map<String, dynamic> input) async {
  final rootPath = input['rootPath'] as String;
  final device = input['device'] as Map<String, dynamic>;
  try {
    final report = await _executeBenchmark(rootPath, device);
    await _writeReport(rootPath, report);
    return report;
  } catch (error, stackTrace) {
    final report = <String, dynamic>{
      'schema': 1,
      'completed_at': DateTime.now().toUtc().toIso8601String(),
      'passed': false,
      'device': device,
      'error': error.toString(),
      'stack_trace': stackTrace.toString(),
    };
    await _writeReport(rootPath, report);
    return report;
  }
}

Future<Map<String, dynamic>> _executeBenchmark(
  String rootPath,
  Map<String, dynamic> device,
) async {
  final root = Directory(rootPath);
  final manifestFile = File('${root.path}/$_manifestName');
  if (!await manifestFile.exists()) {
    throw StateError('Missing ${manifestFile.path}');
  }
  final manifest = jsonDecode(await manifestFile.readAsString());
  if (manifest is! Map<String, dynamic> || manifest['schema'] != 1) {
    throw const FormatException('Model manifest schema must equal 1');
  }

  final modelId = _requiredString(manifest, 'modelId');
  final modelFile = _privateChild(root, _requiredString(manifest, 'modelFile'));
  final tokensFile = _privateChild(
    root,
    _requiredString(manifest, 'tokensFile'),
  );
  final modelSha256 = _requiredString(manifest, 'modelSha256');
  final tokensSha256 = _requiredString(manifest, 'tokensSha256');
  await _verifyFile(modelFile, modelSha256);
  await _verifyFile(tokensFile, tokensSha256);
  final licenseName = _requiredString(manifest, 'licenseName');
  final licenseUrl = _requiredString(manifest, 'licenseUrl');
  final attribution = _requiredString(manifest, 'attribution');
  final language = _requiredString(manifest, 'language');
  final useItn = manifest['useInverseTextNormalization'];
  final threads = manifest['threads'];
  if (useItn is! bool) {
    throw const FormatException('useInverseTextNormalization must be a bool');
  }
  if (threads is! int || threads < 1 || threads > 8) {
    throw const FormatException('threads must be an integer from 1 through 8');
  }

  final corpus = Directory('${root.path}/corpus');
  if (!await corpus.exists()) throw StateError('Missing ${corpus.path}');
  final corpusMetadata = await _readCorpusMetadata(root);
  final wavFiles =
      await corpus
          .list(followLinks: false)
          .where(
            (entry) =>
                entry is File && entry.path.toLowerCase().endsWith('.wav'),
          )
          .cast<File>()
          .toList();
  wavFiles.sort((left, right) => left.path.compareTo(right.path));
  if (wavFiles.isEmpty) throw StateError('Corpus contains no WAV files');

  sherpa_onnx.initBindings();
  var peakRssKiB = _readRssKiB();
  final loadWatch = Stopwatch()..start();
  final recognizer = sherpa_onnx.OfflineRecognizer(
    sherpa_onnx.OfflineRecognizerConfig(
      model: sherpa_onnx.OfflineModelConfig(
        senseVoice: sherpa_onnx.OfflineSenseVoiceModelConfig(
          model: modelFile.path,
          language: language,
          useInverseTextNormalization: useItn,
        ),
        tokens: tokensFile.path,
        numThreads: threads,
        debug: false,
        provider: 'cpu',
      ),
    ),
  );
  loadWatch.stop();
  peakRssKiB = _max(peakRssKiB, _readRssKiB());

  final cases = <Map<String, dynamic>>[];
  var maxRtf = 0.0;
  var maxInferenceMs = 0;
  var longRuns = 0;
  var cerTotal = 0.0;
  var cerCount = 0;
  try {
    for (final wavFile in wavFiles) {
      final wave = sherpa_onnx.readWave(wavFile.path);
      if (wave.sampleRate != _sampleRate) {
        throw FormatException('${wavFile.path} is not 16 kHz');
      }
      if (wave.samples.isEmpty || wave.samples.length > _maxSamples) {
        throw FormatException('${wavFile.path} must contain 1 to 60 seconds');
      }
      final durationSeconds = wave.samples.length / wave.sampleRate;
      if (durationSeconds >= longRunMinimumSeconds) longRuns += 1;
      final stream = recognizer.createStream();
      late final sherpa_onnx.OfflineRecognizerResult result;
      final inferenceWatch = Stopwatch()..start();
      try {
        stream.acceptWaveform(
          samples: wave.samples,
          sampleRate: wave.sampleRate,
        );
        recognizer.decode(stream);
        result = recognizer.getResult(stream);
      } finally {
        inferenceWatch.stop();
        stream.free();
      }
      final inferenceMs = inferenceWatch.elapsedMilliseconds;
      final rtf =
          inferenceWatch.elapsedMicroseconds /
          Duration.microsecondsPerSecond /
          durationSeconds;
      maxRtf = rtf > maxRtf ? rtf : maxRtf;
      maxInferenceMs =
          inferenceMs > maxInferenceMs ? inferenceMs : maxInferenceMs;
      peakRssKiB = _max(peakRssKiB, _readRssKiB());

      final referenceFile = File('${_withoutExtension(wavFile.path)}.txt');
      String? reference;
      double? cer;
      if (await referenceFile.exists()) {
        reference = (await referenceFile.readAsString()).trim();
        cer = characterErrorRate(reference, result.text);
        cerTotal += cer;
        cerCount += 1;
      }
      cases.add(<String, dynamic>{
        'file': _fileName(wavFile.path),
        'duration_seconds': durationSeconds,
        'inference_ms': inferenceMs,
        'rtf': rtf,
        'text': result.text,
        'language': result.lang,
        'emotion': result.emotion,
        'event': result.event,
        if (reference != null) 'reference': reference,
        if (cer != null) 'cer': cer,
      });
    }
  } finally {
    recognizer.free();
  }

  final modelBytes = await modelFile.length() + await tokensFile.length();
  final gates = evaluateGates(
    modelBytes: modelBytes,
    peakRssKiB: peakRssKiB,
    maxInferenceMilliseconds: maxInferenceMs,
    realTimeFactor: maxRtf,
    corpusItems: cases.length,
    longRuns: longRuns,
  );
  return <String, dynamic>{
    'schema': 1,
    'completed_at': DateTime.now().toUtc().toIso8601String(),
    'passed': gates['passed'],
    'device': device,
    'model': <String, dynamic>{
      'id': modelId,
      'bytes': modelBytes,
      'sha256': modelSha256.toLowerCase(),
      'tokens_sha256': tokensSha256.toLowerCase(),
      'license': <String, String>{'name': licenseName, 'url': licenseUrl},
      'attribution': attribution,
      'language': language,
      'inverse_text_normalization': useItn,
      'threads': threads,
    },
    'runtime': <String, dynamic>{
      'sherpa_onnx_version': sherpa_onnx.getVersion(),
      'sherpa_onnx_git_sha': sherpa_onnx.getGitSha1(),
      'sherpa_onnx_git_date': sherpa_onnx.getGitDate(),
      'os': Platform.operatingSystemVersion,
      'processors': Platform.numberOfProcessors,
    },
    if (corpusMetadata != null) 'corpus': corpusMetadata,
    'limits': <String, dynamic>{
      'model_bytes': maxModelBytes,
      'peak_rss_kib': maxPeakRssKiB,
      'inference_ms': maxInferenceDurationMilliseconds,
      'max_rtf': maxRealTimeFactor,
      'minimum_corpus_items': minimumCorpusItems,
      'required_60_second_runs': requiredLongRuns,
    },
    'metrics': <String, dynamic>{
      'model_load_ms': loadWatch.elapsedMilliseconds,
      'peak_rss_kib': peakRssKiB,
      'max_inference_ms': maxInferenceMs,
      'max_rtf': maxRtf,
      'mean_cer': cerCount == 0 ? null : cerTotal / cerCount,
      'corpus_items': cases.length,
      'long_runs': longRuns,
    },
    'gates': gates,
    'cases': cases,
  };
}

Future<Map<String, dynamic>?> _readCorpusMetadata(Directory root) async {
  final file = File('${root.path}/$_corpusManifestName');
  if (!await file.exists()) return null;
  final decoded = jsonDecode(await file.readAsString());
  if (decoded is! Map<String, dynamic> || decoded['schema'] != 1) {
    throw const FormatException('Corpus manifest schema must equal 1');
  }
  _requiredString(decoded, 'id');
  _requiredString(decoded, 'kind');
  return decoded;
}

String _requiredString(Map<String, dynamic> json, String key) {
  final value = json[key];
  if (value is! String || value.trim().isEmpty) {
    throw FormatException('$key must be a non-empty string');
  }
  return value;
}

File _privateChild(Directory root, String name) {
  if (name == '.' ||
      name == '..' ||
      !RegExp(r'^[A-Za-z0-9._-]+$').hasMatch(name)) {
    throw FormatException('Unsafe private file name: $name');
  }
  return File('${root.path}/$name');
}

Future<void> _verifyFile(File file, String expectedSha256) async {
  if (!RegExp(r'^[0-9a-fA-F]{64}$').hasMatch(expectedSha256)) {
    throw FormatException('Invalid SHA-256 for ${file.path}');
  }
  if (!await file.exists()) throw StateError('Missing ${file.path}');
  final actual = (await sha256.bind(file.openRead()).first).toString();
  if (actual != expectedSha256.toLowerCase()) {
    throw StateError('SHA-256 mismatch for ${file.path}');
  }
}

Future<void> _writeReport(String rootPath, Map<String, dynamic> report) async {
  final root = Directory(rootPath);
  await root.create(recursive: true);
  final destination = File('${root.path}/$_reportName');
  final temporary = File('${destination.path}.tmp');
  await temporary.writeAsString(
    '${const JsonEncoder.withIndent('  ').convert(report)}\n',
  );
  if (await destination.exists()) await destination.delete();
  await temporary.rename(destination.path);
  debugPrint('ASR benchmark report: ${destination.path}');
}

int _readRssKiB() {
  try {
    final lines = File('/proc/self/status').readAsLinesSync();
    var result = 0;
    for (final line in lines) {
      if (line.startsWith('VmRSS:') || line.startsWith('VmHWM:')) {
        final match = RegExp(r'(\d+)').firstMatch(line);
        if (match != null) result = _max(result, int.parse(match.group(1)!));
      }
    }
    return result;
  } on FileSystemException {
    return 0;
  }
}

int _max(int left, int right) => left > right ? left : right;

String _fileName(String path) => path.split(Platform.pathSeparator).last;

String _withoutExtension(String path) {
  final dot = path.lastIndexOf('.');
  return dot == -1 ? path : path.substring(0, dot);
}
