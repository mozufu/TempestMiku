import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';
import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:path_provider/path_provider.dart';
import 'package:sherpa_onnx/sherpa_onnx.dart' as sherpa_onnx;
import 'package:tm_asr_benchmark/benchmark_contract.dart';
import 'package:tm_asr_benchmark/benchmark_metrics.dart';

const int _maxSamples = productionSampleRate * 60;
const String _manifestName = 'model-manifest.json';
const String _corpusManifestName = 'corpus-manifest.json';
const String _reportName = 'last-result.json';
const Duration _benchmarkSetupWatchdog = Duration(minutes: 5);
const Duration _benchmarkHousekeepingWatchdog = Duration(seconds: 30);
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
  String? _engineSelector;
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
      final manifestFile = File('${root.path}/$_manifestName');
      Map<String, dynamic>? report;
      String? engineSelector;
      if (await reportFile.exists()) {
        final decoded = jsonDecode(await reportFile.readAsString());
        if (decoded is Map<String, dynamic> &&
            decoded['schema'] == benchmarkReportSchema) {
          report = decoded;
        } else {
          await reportFile.delete();
        }
      }
      if (await manifestFile.exists()) {
        final decoded = jsonDecode(await manifestFile.readAsString());
        if (decoded is! Map<String, dynamic>) {
          throw const FormatException('Model manifest must be an object');
        }
        engineSelector =
            BenchmarkModelManifest.fromJson(decoded).contract.engineSelector;
      }
      if (report != null && report['engine'] != engineSelector) {
        await reportFile.delete();
        report = null;
      }
      if (!mounted) return;
      setState(() {
        _rootPath = root.path;
        _engineSelector = engineSelector;
        _report = report;
        _status =
            engineSelector == null
                ? 'Push a selected model contract and WAV corpus, then run.'
                : report == null
                ? 'Ready to run $engineSelector.'
                : 'Loaded the most recent report.';
      });
    } catch (error) {
      if (!mounted) return;
      setState(() => _status = 'Setup failed: $error');
    }
  }

  Future<void> _run() async {
    final rootPath = _rootPath;
    final rootToken = RootIsolateToken.instance;
    if (rootPath == null || _running || rootToken == null) return;
    setState(() {
      _running = true;
      _status =
          'Running ${_engineSelector ?? 'selected'} inference on a worker '
          'isolate…';
    });
    try {
      final device =
          await _deviceChannel.invokeMapMethod<String, dynamic>(
            'getDeviceInfo',
          ) ??
          <String, dynamic>{};
      final engine = _engineSelector;
      if (engine == null) {
        throw StateError('No verified benchmark engine is selected');
      }
      final report = await _runBenchmarkWithWatchdog(
        rootPath: rootPath,
        device: device,
        rootToken: rootToken,
        engine: engine,
      );
      if (!mounted) return;
      setState(() {
        _report = report;
        final passed = report['passed'] == true;
        _status =
            passed
                ? '${report['engine']} Android-device benchmark gates passed.'
                : 'Benchmark finished; one or more gates remain open.';
      });
    } catch (error) {
      if (!mounted) return;
      setState(() => _status = 'Benchmark could not start: $error');
    } finally {
      if (mounted) setState(() => _running = false);
    }
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
          SelectableText('Selected engine: ${_engineSelector ?? 'none'}'),
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
            Text(
              'Mean converted CER: '
              '${metrics['mean_converted_cer'] ?? 'n/a'}',
            ),
            Text(
              'Completed / failed / empty: '
              '${metrics['completed_items']} / ${metrics['failed_items']} / '
              '${metrics['empty_converted_items']}',
            ),
            Text(
              'Long runs completed / non-truncated: '
              '${metrics['completed_long_runs']} / '
              '${metrics['non_truncated_long_runs']}',
            ),
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

Future<Map<String, dynamic>> _runBenchmarkWithWatchdog({
  required String rootPath,
  required Map<String, dynamic> device,
  required RootIsolateToken rootToken,
  required String engine,
}) async {
  final contract = benchmarkContractForSelector(engine);
  final startedAt = DateTime.now().toUtc();
  final pendingReport = _benchmarkFailureReport(
    contract: contract,
    device: device,
    startedAt: startedAt,
    failureKind: 'benchmark_interrupted',
    phase: 'worker_launch',
    limit: _benchmarkSetupWatchdog,
    error:
        'Benchmark started but did not produce a terminal report. A process '
        'exit, crash, or watchdog termination must not leave stale passing '
        'evidence.',
  );
  await _writeReport(rootPath, pendingReport);

  final receivePort = ReceivePort();
  final worker = await Isolate.spawn<Map<String, dynamic>>(
    _benchmarkWorker,
    <String, dynamic>{
      'responsePort': receivePort.sendPort,
      'rootPath': rootPath,
      'device': device,
      'rootToken': rootToken,
    },
    onError: receivePort.sendPort,
    onExit: receivePort.sendPort,
    errorsAreFatal: true,
  );
  final completer = Completer<Map<String, dynamic>>();
  Timer? watchdog;
  var terminal = false;
  var phase = 'benchmark_setup';
  var limit = _benchmarkSetupWatchdog;
  Map<String, dynamic> context = const <String, dynamic>{};

  Future<void> terminateWithFailure({
    required String failureKind,
    required String error,
  }) async {
    if (terminal) return;
    terminal = true;
    watchdog?.cancel();
    // A synchronous native FFI call cannot be cancelled by Future.timeout.
    // Mark this worker for immediate termination before writing the terminal
    // report so it cannot later replace the failure with a stale success.
    worker.kill(priority: Isolate.immediate);
    final report = _benchmarkFailureReport(
      contract: contract,
      device: device,
      startedAt: startedAt,
      failureKind: failureKind,
      phase: phase,
      limit: limit,
      error: error,
      context: context,
    );
    Object? writeError;
    StackTrace? writeStackTrace;
    try {
      await _writeReport(rootPath, report);
    } catch (error, stackTrace) {
      writeError = error;
      writeStackTrace = stackTrace;
    } finally {
      receivePort.close();
      if (Platform.isAndroid &&
          (failureKind == 'native_decode_watchdog' ||
              failureKind == 'benchmark_phase_watchdog')) {
        // Isolate.kill is cooperative while native code is executing. Killing
        // this standalone benchmark package after the failure JSON is flushed
        // is the process boundary that guarantees a hung native call stops.
        if (!Process.killPid(pid, ProcessSignal.sigkill)) exit(124);
      }
    }
    if (!completer.isCompleted) {
      if (writeError != null) {
        completer.completeError(writeError, writeStackTrace);
      } else {
        completer.complete(report);
      }
    }
  }

  void armWatchdog(
    String nextPhase,
    Duration nextLimit, [
    Map<String, dynamic> nextContext = const <String, dynamic>{},
    Duration? timerDelay,
  ]) {
    if (terminal) return;
    phase = nextPhase;
    limit = nextLimit;
    context = Map<String, dynamic>.unmodifiable(nextContext);
    watchdog?.cancel();
    watchdog = Timer(timerDelay ?? nextLimit, () {
      final nativeDecode = nextPhase == 'native_decode';
      unawaited(
        terminateWithFailure(
          failureKind:
              nativeDecode
                  ? 'native_decode_watchdog'
                  : 'benchmark_phase_watchdog',
          error:
              'Benchmark watchdog exceeded ${nextLimit.inMilliseconds} ms '
              'during $nextPhase.',
        ),
      );
    });
  }

  receivePort.listen((message) {
    if (terminal) return;
    if (message is Map<Object?, Object?>) {
      final event = message.map(
        (key, value) => MapEntry(key.toString(), value),
      );
      switch (event['type']) {
        case 'result':
          final encodedReport = event['report'];
          if (encodedReport is! Map<Object?, Object?>) {
            unawaited(
              terminateWithFailure(
                failureKind: 'worker_protocol_error',
                error: 'Benchmark worker returned a non-object report.',
              ),
            );
            return;
          }
          terminal = true;
          watchdog?.cancel();
          receivePort.close();
          final report = encodedReport.map(
            (key, value) => MapEntry(key.toString(), value),
          );
          if (!completer.isCompleted) completer.complete(report);
          return;
        case 'model_load_started':
          armWatchdog('model_load', _benchmarkSetupWatchdog);
          return;
        case 'model_load_completed':
          armWatchdog('pre_case', _benchmarkHousekeepingWatchdog);
          return;
        case 'native_decode_started':
          final startedAtEpochMicroseconds =
              event['started_at_epoch_microseconds'];
          if (startedAtEpochMicroseconds is! int) {
            unawaited(
              terminateWithFailure(
                failureKind: 'worker_protocol_error',
                error: 'Native decode event omitted its wall-clock start.',
              ),
            );
            return;
          }
          const decodeLimit = Duration(
            milliseconds: maxInferenceDurationMilliseconds,
          );
          final elapsedMicroseconds =
              DateTime.now().toUtc().microsecondsSinceEpoch -
              startedAtEpochMicroseconds;
          final remainingMicroseconds =
              decodeLimit.inMicroseconds -
              (elapsedMicroseconds < 0 ? 0 : elapsedMicroseconds);
          armWatchdog(
            'native_decode',
            decodeLimit,
            <String, dynamic>{
              'file': event['file'],
              'category': event['category'],
              'decode_started_at_epoch_microseconds':
                  startedAtEpochMicroseconds,
            },
            Duration(
              microseconds:
                  remainingMicroseconds < 0 ? 0 : remainingMicroseconds,
            ),
          );
          return;
        case 'native_decode_completed':
          armWatchdog(
            'case_postprocessing',
            _benchmarkHousekeepingWatchdog,
            <String, dynamic>{
              'file': event['file'],
              'category': event['category'],
            },
          );
          return;
        case 'case_completed' || 'case_failed':
          armWatchdog('between_cases', _benchmarkHousekeepingWatchdog);
          return;
        default:
          unawaited(
            terminateWithFailure(
              failureKind: 'worker_protocol_error',
              error: 'Benchmark worker emitted an unknown event.',
            ),
          );
          return;
      }
    }
    if (message is List<Object?> && message.isNotEmpty) {
      unawaited(
        terminateWithFailure(
          failureKind: 'worker_isolate_error',
          error: 'Benchmark worker isolate failed: ${message.first}',
        ),
      );
      return;
    }
    unawaited(
      terminateWithFailure(
        failureKind: 'worker_exit',
        error: 'Benchmark worker exited before returning a report.',
      ),
    );
  });
  armWatchdog('benchmark_setup', _benchmarkSetupWatchdog);
  return completer.future;
}

Map<String, dynamic> _benchmarkFailureReport({
  required BenchmarkContract contract,
  required Map<String, dynamic> device,
  required DateTime startedAt,
  required String failureKind,
  required String phase,
  required Duration limit,
  required String error,
  Map<String, dynamic> context = const <String, dynamic>{},
}) {
  return <String, dynamic>{
    'schema': benchmarkReportSchema,
    'started_at': startedAt.toIso8601String(),
    'completed_at': DateTime.now().toUtc().toIso8601String(),
    'passed': false,
    'engine': contract.engineSelector,
    'evidence_scope': 'android_device',
    'host_results_qualify_as_device_evidence': false,
    'device': device,
    'model': _modelReport(contract),
    'error': error,
    'failure': <String, dynamic>{
      'kind': failureKind,
      'phase': phase,
      'wall_limit_ms': limit.inMilliseconds,
      if (context.isNotEmpty) 'context': context,
    },
    'gates': const <String, dynamic>{
      'passed': false,
      'failures': <String>['benchmark_watchdog_or_interruption'],
    },
  };
}

@pragma('vm:entry-point')
Future<void> _benchmarkWorker(Map<String, dynamic> input) async {
  final responsePort = input.remove('responsePort');
  if (responsePort is! SendPort) return;
  input['progressPort'] = responsePort;
  final report = await runBenchmark(input);
  responsePort.send(<String, dynamic>{'type': 'result', 'report': report});
}

@pragma('vm:entry-point')
Future<Map<String, dynamic>> runBenchmark(Map<String, dynamic> input) async {
  final rootPath = input['rootPath'] as String;
  final device = input['device'] as Map<String, dynamic>;
  final rootToken = input['rootToken'];
  final progressPort = input['progressPort'];
  BenchmarkModelManifest? manifest;
  try {
    if (rootToken is! RootIsolateToken) {
      throw StateError('missing Flutter root isolate token');
    }
    BackgroundIsolateBinaryMessenger.ensureInitialized(rootToken);
    manifest = await _readModelManifest(rootPath);
    final report = await _executeBenchmark(
      rootPath,
      device,
      manifest,
      onProgress:
          progressPort is SendPort ? (event) => progressPort.send(event) : null,
    );
    await _writeReport(rootPath, report);
    return report;
  } catch (error, stackTrace) {
    final report = <String, dynamic>{
      'schema': benchmarkReportSchema,
      'completed_at': DateTime.now().toUtc().toIso8601String(),
      'passed': false,
      if (manifest != null) ...<String, dynamic>{
        'engine': manifest.contract.engineSelector,
        'evidence_scope': 'android_device',
        'host_results_qualify_as_device_evidence': false,
        'model': _modelReport(manifest.contract),
      },
      'device': device,
      'error': error.toString(),
      'stack_trace': stackTrace.toString(),
    };
    await _writeReport(rootPath, report);
    return report;
  }
}

Future<BenchmarkModelManifest> _readModelManifest(String rootPath) async {
  final manifestFile = File('$rootPath/$_manifestName');
  if (!await manifestFile.exists()) {
    throw StateError('Missing ${manifestFile.path}');
  }
  final decoded = jsonDecode(await manifestFile.readAsString());
  if (decoded is! Map<String, dynamic>) {
    throw const FormatException('Model manifest must be an object');
  }
  return BenchmarkModelManifest.fromJson(decoded);
}

Map<String, dynamic> _modelReport(BenchmarkContract contract) =>
    <String, dynamic>{
      'id': contract.modelId,
      'repository': contract.repository,
      'commit': contract.commit,
      'source_revision_url': contract.sourceRevisionUrl,
      'bytes': contract.totalBytes,
      'files': [
        for (final file in contract.files)
          <String, dynamic>{
            'role': file.role,
            'path': file.path,
            'bytes': file.bytes,
            'sha256': file.sha256,
          },
      ],
      'license': const <String, String>{
        'name': 'Apache-2.0',
        'url': 'https://www.apache.org/licenses/LICENSE-2.0',
      },
      'attribution': contract.attribution,
    };

Future<Map<String, dynamic>> _executeBenchmark(
  String rootPath,
  Map<String, dynamic> device,
  BenchmarkModelManifest manifest, {
  void Function(Map<String, dynamic> event)? onProgress,
}) async {
  final root = Directory(rootPath);
  final contract = manifest.contract;
  for (final entry in manifest.files.values) {
    await _verifyFile(
      _privateChild(root, entry.path),
      expectedBytes: entry.bytes,
      expectedSha256: entry.sha256,
    );
  }

  final corpus = Directory('${root.path}/corpus');
  if (!await corpus.exists()) throw StateError('Missing ${corpus.path}');
  final corpusPlan = await _readCorpusPlan(root, corpus);
  if (corpusPlan.cases.isEmpty) {
    throw StateError('Corpus contains no WAV files');
  }

  sherpa_onnx.initBindings();
  final runtimeVersion = sherpa_onnx.getVersion();
  if (runtimeVersion != productionRuntimeVersion) {
    throw StateError(
      'sherpa_onnx runtime was $runtimeVersion, expected '
      '$productionRuntimeVersion',
    );
  }
  var peakRssKiB = _readRssKiB();
  int? maximumThermalStatus;
  var thermalSnapshotsComplete = true;
  final loadWatch = Stopwatch()..start();
  onProgress?.call(const <String, dynamic>{'type': 'model_load_started'});
  final recognizer = _createBenchmarkRecognizer(root, manifest);
  loadWatch.stop();
  onProgress?.call(const <String, dynamic>{'type': 'model_load_completed'});
  peakRssKiB = _max(peakRssKiB, _readRssKiB());

  final cases = <Map<String, dynamic>>[];
  var maxRtf = 0.0;
  var maxInferenceMs = 0;
  var longRuns = 0;
  var completedItems = 0;
  var failedItems = 0;
  var emptyRawItems = 0;
  var emptyConvertedItems = 0;
  var completedLongRuns = 0;
  var nonTruncatedLongRuns = 0;
  var rawCerTotal = 0.0;
  var rawCerCount = 0;
  var convertedCerTotal = 0.0;
  var convertedCerCount = 0;
  var codeSwitchCerTotal = 0.0;
  var codeSwitchCerCount = 0;
  double? minimumLongRunCoverage;
  double? minimumLongRunTailCoverage;
  try {
    for (final corpusCase in corpusPlan.cases) {
      double? durationSeconds;
      var isLong = corpusCase.expectedLong ?? false;
      final thermalBefore = await _thermalSnapshot();
      if (_thermalStatusValue(thermalBefore) == null) {
        thermalSnapshotsComplete = false;
      }
      maximumThermalStatus = _maxNullable(
        maximumThermalStatus,
        _thermalStatusValue(thermalBefore),
      );
      try {
        final wave = sherpa_onnx.readWave(corpusCase.audio.path);
        if (wave.sampleRate != productionSampleRate) {
          throw FormatException('${corpusCase.audio.path} is not 16 kHz');
        }
        if (wave.samples.isEmpty || wave.samples.length > _maxSamples) {
          throw FormatException(
            '${corpusCase.audio.path} must contain 1 to 60 seconds',
          );
        }
        durationSeconds = wave.samples.length / wave.sampleRate;
        isLong = durationSeconds >= longRunMinimumSeconds;
        if (corpusCase.expectedLong != null &&
            corpusCase.expectedLong != isLong) {
          throw FormatException(
            '${corpusCase.audio.path} long-run metadata did not match audio',
          );
        }
        if (isLong) longRuns += 1;

        onProgress?.call(<String, dynamic>{
          'type': 'native_decode_started',
          'file': _fileName(corpusCase.audio.path),
          'category': corpusCase.category,
          'started_at_epoch_microseconds':
              DateTime.now().toUtc().microsecondsSinceEpoch,
        });
        final inferenceWatch = Stopwatch()..start();
        final decodeWatch = Stopwatch()..start();
        final rawText = recognizer.decode(wave.samples);
        decodeWatch.stop();
        onProgress?.call(<String, dynamic>{
          'type': 'native_decode_completed',
          'file': _fileName(corpusCase.audio.path),
          'category': corpusCase.category,
        });
        final conversionWatch = Stopwatch()..start();
        final convertedText = await _toTraditional(rawText);
        conversionWatch.stop();
        inferenceWatch.stop();
        final inferenceMs = inferenceWatch.elapsedMilliseconds;
        final rtf =
            inferenceWatch.elapsedMicroseconds /
            Duration.microsecondsPerSecond /
            durationSeconds;
        maxRtf = rtf > maxRtf ? rtf : maxRtf;
        maxInferenceMs =
            inferenceMs > maxInferenceMs ? inferenceMs : maxInferenceMs;
        peakRssKiB = _max(peakRssKiB, _readRssKiB());
        completedItems += 1;
        if (isLong) completedLongRuns += 1;
        if (rawText.isEmpty) emptyRawItems += 1;
        if (convertedText.isEmpty) emptyConvertedItems += 1;

        String? reference;
        double? rawCer;
        double? convertedCer;
        double? referenceCoverage;
        double? tailReferenceCoverage;
        if (corpusCase.reference != null) {
          reference = (await corpusCase.reference!.readAsString()).trim();
          rawCer = characterErrorRate(reference, rawText);
          convertedCer = characterErrorRate(reference, convertedText);
          referenceCoverage = referenceLcsCoverage(reference, convertedText);
          tailReferenceCoverage = referenceTailLcsCoverage(
            reference,
            convertedText,
          );
          rawCerTotal += rawCer;
          rawCerCount += 1;
          convertedCerTotal += convertedCer;
          convertedCerCount += 1;
          if (corpusCase.category == 'code-switch') {
            codeSwitchCerTotal += convertedCer;
            codeSwitchCerCount += 1;
          }
          if (isLong) {
            minimumLongRunCoverage =
                minimumLongRunCoverage == null ||
                        referenceCoverage < minimumLongRunCoverage
                    ? referenceCoverage
                    : minimumLongRunCoverage;
            minimumLongRunTailCoverage =
                minimumLongRunTailCoverage == null ||
                        tailReferenceCoverage < minimumLongRunTailCoverage
                    ? tailReferenceCoverage
                    : minimumLongRunTailCoverage;
            if (referenceCoverage >= minimumLongRunReferenceCoverage &&
                tailReferenceCoverage >= minimumLongRunTailReferenceCoverage) {
              nonTruncatedLongRuns += 1;
            }
          }
        }
        final thermalAfter = await _thermalSnapshot();
        if (_thermalStatusValue(thermalAfter) == null) {
          thermalSnapshotsComplete = false;
        }
        maximumThermalStatus = _maxNullable(
          maximumThermalStatus,
          _thermalStatusValue(thermalAfter),
        );
        cases.add(<String, dynamic>{
          'file': _fileName(corpusCase.audio.path),
          'category': corpusCase.category,
          'status': 'completed',
          'long_run': isLong,
          'duration_seconds': durationSeconds,
          'inference_ms': inferenceMs,
          'decode_ms': decodeWatch.elapsedMilliseconds,
          'conversion_ms': conversionWatch.elapsedMilliseconds,
          'rtf': rtf,
          'raw_text': rawText,
          'converted_text': convertedText,
          if (reference != null) 'reference': reference,
          if (rawCer != null) 'raw_cer': rawCer,
          if (convertedCer != null) 'converted_cer': convertedCer,
          if (referenceCoverage != null)
            'reference_lcs_coverage': referenceCoverage,
          if (tailReferenceCoverage != null)
            'tail_reference_lcs_coverage': tailReferenceCoverage,
          if (thermalBefore != null) 'thermal_before': thermalBefore,
          if (thermalAfter != null) 'thermal_after': thermalAfter,
        });
        onProgress?.call(<String, dynamic>{
          'type': 'case_completed',
          'file': _fileName(corpusCase.audio.path),
          'category': corpusCase.category,
        });
      } catch (error, stackTrace) {
        failedItems += 1;
        final thermalAfter = await _thermalSnapshot();
        if (_thermalStatusValue(thermalAfter) == null) {
          thermalSnapshotsComplete = false;
        }
        maximumThermalStatus = _maxNullable(
          maximumThermalStatus,
          _thermalStatusValue(thermalAfter),
        );
        cases.add(<String, dynamic>{
          'file': _fileName(corpusCase.audio.path),
          'category': corpusCase.category,
          'status': 'failed',
          'long_run': isLong,
          if (durationSeconds != null) 'duration_seconds': durationSeconds,
          'error': error.toString(),
          'stack_trace': stackTrace.toString(),
          if (thermalBefore != null) 'thermal_before': thermalBefore,
          if (thermalAfter != null) 'thermal_after': thermalAfter,
        });
        onProgress?.call(<String, dynamic>{
          'type': 'case_failed',
          'file': _fileName(corpusCase.audio.path),
          'category': corpusCase.category,
        });
      }
    }
  } finally {
    recognizer.free();
  }

  final meanConvertedCer =
      convertedCerCount == 0 ? null : convertedCerTotal / convertedCerCount;
  final codeSwitchConvertedMeanCer =
      codeSwitchCerCount == 0 ? null : codeSwitchCerTotal / codeSwitchCerCount;
  final gates = evaluateGates(
    ranOnAndroidDevice: _isQualifiedAndroidDevice(device),
    modelBytes: contract.totalBytes,
    peakRssKiB: peakRssKiB,
    maxInferenceMilliseconds: maxInferenceMs,
    realTimeFactor: maxRtf,
    thermalStatusAvailable: thermalSnapshotsComplete,
    corpusIntegrityBound: corpusPlan.integrityBound,
    corpusItems: corpusPlan.cases.length,
    longRuns: longRuns,
    completedItems: completedItems,
    failedItems: failedItems,
    emptyRawItems: emptyRawItems,
    emptyConvertedItems: emptyConvertedItems,
    completedLongRuns: completedLongRuns,
    nonTruncatedLongRuns: nonTruncatedLongRuns,
    convertedMeanCer: meanConvertedCer,
    codeSwitchConvertedMeanCer: codeSwitchConvertedMeanCer,
  );
  return <String, dynamic>{
    'schema': benchmarkReportSchema,
    'completed_at': DateTime.now().toUtc().toIso8601String(),
    'passed': gates['passed'],
    'engine': contract.engineSelector,
    'evidence_scope': 'android_device',
    'host_results_qualify_as_device_evidence': false,
    'device': device,
    'model': _modelReport(contract),
    'runtime': <String, dynamic>{
      'engine': contract.engineSelector,
      'inference_mode': contract.inferenceMode,
      'sherpa_onnx_version': runtimeVersion,
      'sherpa_onnx_git_sha': sherpa_onnx.getGitSha1(),
      'sherpa_onnx_git_date': sherpa_onnx.getGitDate(),
      'provider': productionProvider,
      'threads': productionThreads,
      'sample_rate': productionSampleRate,
      'feature_dimension': productionFeatureDimension,
      'decoding_method': productionDecodingMethod,
      if (contract.engineSelector ==
          productionEngineSelector) ...<String, dynamic>{
        'chunk_samples': productionChunkSamples,
        'tail_padding_samples': productionTailPaddingSamples,
        'max_decode_steps': productionMaxDecodeSteps,
        'endpoint_detection': false,
      } else ...<String, dynamic>{
        'input_mode': 'whole_audio',
        'decode_trigger': 'after_input_complete',
      },
      'traditional_conversion': 'Android ICU $productionTransliterator',
      'os': Platform.operatingSystemVersion,
      'processors': Platform.numberOfProcessors,
    },
    if (corpusPlan.metadata != null)
      'corpus': <String, dynamic>{
        ...corpusPlan.metadata!,
        if (corpusPlan.manifestSha256 != null)
          'manifestSha256': corpusPlan.manifestSha256,
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
    'metrics': <String, dynamic>{
      'model_load_ms': loadWatch.elapsedMilliseconds,
      'peak_rss_kib': peakRssKiB,
      'maximum_thermal_status': maximumThermalStatus,
      'thermal_snapshots_complete': thermalSnapshotsComplete,
      'max_inference_ms': maxInferenceMs,
      'max_rtf': maxRtf,
      'mean_raw_cer': rawCerCount == 0 ? null : rawCerTotal / rawCerCount,
      'mean_converted_cer': meanConvertedCer,
      'code_switch_mean_converted_cer': codeSwitchConvertedMeanCer,
      'corpus_items': corpusPlan.cases.length,
      'completed_items': completedItems,
      'failed_items': failedItems,
      'empty_raw_items': emptyRawItems,
      'empty_converted_items': emptyConvertedItems,
      'long_runs': longRuns,
      'completed_long_runs': completedLongRuns,
      'non_truncated_long_runs': nonTruncatedLongRuns,
      'corpus_integrity_bound': corpusPlan.integrityBound,
      'minimum_long_run_reference_lcs_coverage': minimumLongRunCoverage,
      'minimum_long_run_tail_reference_lcs_coverage':
          minimumLongRunTailCoverage,
    },
    'gates': gates,
    'cases': cases,
  };
}

_BenchmarkRecognizer _createBenchmarkRecognizer(
  Directory root,
  BenchmarkModelManifest manifest,
) {
  final tokens = _privateChild(root, manifest.file('tokens').path);
  return switch (manifest.contract.engineSelector) {
    productionEngineSelector => _StreamingParaformerRecognizer(
      encoder: _privateChild(root, manifest.file('encoder').path),
      decoder: _privateChild(root, manifest.file('decoder').path),
      tokens: tokens,
    ),
    offlineCandidateEngineSelector => _OfflineParaformerRecognizer(
      model: _privateChild(root, manifest.file('model').path),
      tokens: tokens,
    ),
    _ =>
      throw StateError(
        'unsupported benchmark engine: ${manifest.contract.engineSelector}',
      ),
  };
}

abstract interface class _BenchmarkRecognizer {
  String decode(Float32List samples);

  void free();
}

final class _StreamingParaformerRecognizer implements _BenchmarkRecognizer {
  _StreamingParaformerRecognizer({
    required File encoder,
    required File decoder,
    required File tokens,
  }) : _recognizer = sherpa_onnx.OnlineRecognizer(
         sherpa_onnx.OnlineRecognizerConfig(
           feat: const sherpa_onnx.FeatureConfig(
             sampleRate: productionSampleRate,
             featureDim: productionFeatureDimension,
           ),
           model: sherpa_onnx.OnlineModelConfig(
             paraformer: sherpa_onnx.OnlineParaformerModelConfig(
               encoder: encoder.path,
               decoder: decoder.path,
             ),
             tokens: tokens.path,
             numThreads: productionThreads,
             provider: productionProvider,
             debug: false,
             modelType: 'paraformer',
           ),
           decodingMethod: productionDecodingMethod,
           enableEndpoint: false,
         ),
       );

  final sherpa_onnx.OnlineRecognizer _recognizer;

  @override
  String decode(Float32List samples) {
    final stream = _recognizer.createStream();
    final tailPadding = Float32List(productionTailPaddingSamples);
    var decodeSteps = 0;
    void drainReady() {
      while (_recognizer.isReady(stream)) {
        _recognizer.decode(stream);
        decodeSteps += 1;
        if (decodeSteps > productionMaxDecodeSteps) {
          throw StateError('ASR decode step bound exceeded');
        }
      }
    }

    try {
      for (
        var offset = 0;
        offset < samples.length;
        offset += productionChunkSamples
      ) {
        final end = (offset + productionChunkSamples).clamp(0, samples.length);
        stream.acceptWaveform(
          samples: Float32List.sublistView(samples, offset, end),
          sampleRate: productionSampleRate,
        );
        drainReady();
      }
      for (
        var offset = 0;
        offset < tailPadding.length;
        offset += productionChunkSamples
      ) {
        final end = (offset + productionChunkSamples).clamp(
          0,
          tailPadding.length,
        );
        stream.acceptWaveform(
          samples: Float32List.sublistView(tailPadding, offset, end),
          sampleRate: productionSampleRate,
        );
        drainReady();
      }
      stream.inputFinished();
      drainReady();
      return _recognizer.getResult(stream).text.trim();
    } finally {
      tailPadding.fillRange(0, tailPadding.length, 0);
      stream.free();
    }
  }

  @override
  void free() => _recognizer.free();
}

final class _OfflineParaformerRecognizer implements _BenchmarkRecognizer {
  _OfflineParaformerRecognizer({required File model, required File tokens})
    : _recognizer = sherpa_onnx.OfflineRecognizer(
        sherpa_onnx.OfflineRecognizerConfig(
          feat: const sherpa_onnx.FeatureConfig(
            sampleRate: productionSampleRate,
            featureDim: productionFeatureDimension,
          ),
          model: sherpa_onnx.OfflineModelConfig(
            paraformer: sherpa_onnx.OfflineParaformerModelConfig(
              model: model.path,
            ),
            tokens: tokens.path,
            numThreads: productionThreads,
            provider: productionProvider,
            debug: false,
            modelType: 'paraformer',
          ),
          decodingMethod: productionDecodingMethod,
        ),
      );

  final sherpa_onnx.OfflineRecognizer _recognizer;

  @override
  String decode(Float32List samples) {
    final stream = _recognizer.createStream();
    try {
      // Offline Paraformer receives the whole utterance and decodes exactly
      // once after input is complete; it never feeds partial streaming chunks.
      stream.acceptWaveform(samples: samples, sampleRate: productionSampleRate);
      _recognizer.decode(stream);
      return _recognizer.getResult(stream).text.trim();
    } finally {
      stream.free();
    }
  }

  @override
  void free() => _recognizer.free();
}

Future<String> _toTraditional(String rawText) async {
  final converted = await _deviceChannel.invokeMethod<String>(
    'toTraditional',
    <String, String>{'text': rawText},
  );
  if (converted == null) {
    throw StateError('Android ICU conversion returned no text');
  }
  return converted.trim();
}

Future<Map<String, dynamic>?> _thermalSnapshot() async {
  try {
    return await _deviceChannel.invokeMapMethod<String, dynamic>(
      'getThermalStatus',
    );
  } on PlatformException {
    return null;
  } on MissingPluginException {
    return null;
  }
}

int? _thermalStatusValue(Map<String, dynamic>? snapshot) {
  final value = snapshot?['status'];
  return value is int ? value : null;
}

bool _isQualifiedAndroidDevice(Map<String, dynamic> device) {
  final supportedAbis = device['supportedAbis'];
  return Platform.isAndroid &&
      device['sdk'] is int &&
      (device['sdk'] as int) >= productionMinimumAndroidSdk &&
      device['physicalDevice'] == true &&
      device['fingerprint'] is String &&
      (device['fingerprint'] as String).isNotEmpty &&
      supportedAbis is List<Object?> &&
      supportedAbis.contains('arm64-v8a');
}

Future<_CorpusPlan> _readCorpusPlan(Directory root, Directory corpus) async {
  final manifestFile = File('${root.path}/$_corpusManifestName');
  if (!await manifestFile.exists()) {
    return _CorpusPlan(
      metadata: null,
      manifestSha256: null,
      integrityBound: false,
      cases: await _discoverLegacyCorpus(corpus),
    );
  }
  final manifestBytes = await manifestFile.readAsBytes();
  final manifestSha256 = sha256.convert(manifestBytes).toString();
  final decoded = jsonDecode(utf8.decode(manifestBytes));
  if (decoded is! Map<String, dynamic>) {
    throw const FormatException('Corpus manifest must be an object');
  }
  if (decoded['schema'] == 1) {
    return _CorpusPlan(
      metadata: decoded,
      manifestSha256: manifestSha256,
      integrityBound: false,
      cases: await _discoverLegacyCorpus(corpus),
    );
  }
  final contract = BenchmarkCorpusManifest.fromJson(decoded);
  final cases = <_CorpusCase>[];
  for (final entry in contract.cases) {
    final audio = _privateCorpusChild(corpus, entry.file);
    final reference = _privateCorpusChild(corpus, entry.reference);
    if (!await audio.exists()) throw StateError('Missing ${audio.path}');
    if (!await reference.exists()) {
      throw StateError('Missing ${reference.path}');
    }
    await _verifyFile(
      audio,
      expectedBytes: entry.audioBytes,
      expectedSha256: entry.audioSha256,
    );
    await _verifyFile(
      reference,
      expectedBytes: entry.referenceBytes,
      expectedSha256: entry.referenceSha256,
    );
    cases.add(
      _CorpusCase(
        audio: audio,
        reference: reference,
        category: entry.category,
        expectedLong: entry.expectedLong,
      ),
    );
  }
  return _CorpusPlan(
    metadata: contract.metadata,
    manifestSha256: manifestSha256,
    integrityBound: true,
    cases: cases,
  );
}

Future<List<_CorpusCase>> _discoverLegacyCorpus(Directory corpus) async {
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
  return [
    for (final wav in wavFiles)
      _CorpusCase(
        audio: wav,
        reference: await _existingReference(wav),
        category: _categoryFromFileName(_fileName(wav.path)),
        expectedLong: null,
      ),
  ];
}

Future<File?> _existingReference(File wav) async {
  final reference = File('${_withoutExtension(wav.path)}.txt');
  return await reference.exists() ? reference : null;
}

String _categoryFromFileName(String name) {
  const categories = [
    'code-switch',
    'taiwan-local',
    'conversation',
    'engineering',
    'companion',
    'numbers',
    'daily',
  ];
  return categories.where(name.contains).firstOrNull ?? 'unspecified';
}

File _privateChild(Directory root, String name) {
  if (!_safePrivateName(name)) {
    throw FormatException('Unsafe private file name: $name');
  }
  return File('${root.path}/$name');
}

File _privateCorpusChild(Directory corpus, String name) {
  if (!_safePrivateName(name)) {
    throw FormatException('Unsafe corpus file name: $name');
  }
  return File('${corpus.path}/$name');
}

bool _safePrivateName(String name) =>
    name != '.' && name != '..' && RegExp(r'^[A-Za-z0-9._-]+$').hasMatch(name);

Future<void> _verifyFile(
  File file, {
  required int expectedBytes,
  required String expectedSha256,
}) async {
  if (!await file.exists()) throw StateError('Missing ${file.path}');
  final actualBytes = await file.length();
  if (actualBytes != expectedBytes) {
    throw StateError(
      'Size mismatch for ${file.path}: $actualBytes != $expectedBytes',
    );
  }
  final actual = (await sha256.bind(file.openRead()).first).toString();
  if (actual != expectedSha256) {
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
    flush: true,
  );
  // The harness runs on Android's POSIX filesystem, where rename replaces the
  // destination atomically. Never delete the prior report first: a crash must
  // leave either the old terminal report or the fully flushed replacement.
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

int? _maxNullable(int? left, int? right) {
  if (left == null) return right;
  if (right == null) return left;
  return _max(left, right);
}

String _fileName(String path) => path.split(Platform.pathSeparator).last;

String _withoutExtension(String path) {
  final dot = path.lastIndexOf('.');
  return dot == -1 ? path : path.substring(0, dot);
}

final class _CorpusPlan {
  const _CorpusPlan({
    required this.metadata,
    required this.manifestSha256,
    required this.integrityBound,
    required this.cases,
  });

  final Map<String, dynamic>? metadata;
  final String? manifestSha256;
  final bool integrityBound;
  final List<_CorpusCase> cases;
}

final class _CorpusCase {
  const _CorpusCase({
    required this.audio,
    required this.reference,
    required this.category,
    required this.expectedLong,
  });

  final File audio;
  final File? reference;
  final String category;
  final bool? expectedLong;
}
