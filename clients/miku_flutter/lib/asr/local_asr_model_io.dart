import 'dart:async';
import 'dart:io';
import 'dart:isolate';
import 'dart:typed_data';

import 'package:flutter/services.dart';
import 'package:sherpa_onnx/sherpa_onnx.dart' as sherpa;

import 'local_asr_engine.dart';
import 'local_asr_model_platform.dart';
import 'secure_buffer.dart';

LocalAsrModelManager createLocalAsrModelManager() =>
    const AndroidLocalAsrModelManager();

final class AndroidLocalAsrModelManager implements LocalAsrModelManager {
  const AndroidLocalAsrModelManager();

  static const MethodChannel _channel = MethodChannel(
    'org.mozufu.tempestmiku/voice-model',
  );

  static const EventChannel _installProgressChannel = EventChannel(
    'org.mozufu.tempestmiku/voice-model-progress',
  );

  @override
  bool get isSupported => Platform.isAndroid;

  @override
  Future<LocalAsrModelStatus> inspect() => _status('inspect');

  @override
  Future<LocalAsrModelStatus> install({
    void Function(LocalAsrModelInstallProgress)? onProgress,
    LocalAsrCancellationToken? cancellation,
  }) async {
    cancellation?.throwIfCancelled();
    final progressEvents = _installProgressChannel
        .receiveBroadcastStream()
        .listen(
          (event) {
            if (onProgress == null || event is! Map) return;
            final received = event['receivedBytes'];
            final total = event['totalBytes'];
            if (received is! int || total is! int) return;
            onProgress(
              LocalAsrModelInstallProgress(
                receivedBytes: received,
                totalBytes: total,
              ),
            );
          },
          onError: (Object _) {
            // Progress is best-effort; the install result stays authoritative.
          },
        );
    var cancelForwarded = false;
    Future<void>? cancelForwarder;
    cancelForwarder = cancellation?.whenCancelled.then((_) async {
      cancelForwarded = true;
      try {
        await _channel.invokeMethod<void>('cancelInstall');
      } on PlatformException {
        // Best-effort: the native installer may already have finished.
      }
    });
    try {
      return await _status('install');
    } catch (error) {
      if (cancellation?.isCancelled ?? false) {
        throw const LocalAsrCancelledException();
      }
      rethrow;
    } finally {
      await progressEvents.cancel();
      if (cancelForwarded) await cancelForwarder;
    }
  }

  @override
  Future<LocalAsrModelStatus> delete() => _status('delete');

  Future<LocalAsrModelStatus> _status(String method) async {
    if (!isSupported) {
      return const LocalAsrModelStatus(
        state: LocalAsrModelState.unsupported,
        reason: 'on-device voice models require Android',
      );
    }
    return LocalAsrModelStatus.fromChannel(
      await _channel.invokeMethod<Object?>(method),
    );
  }

  @override
  Future<LocalAsrWorker> spawn({
    LocalAsrCancellationToken? cancellation,
  }) async {
    cancellation?.throwIfCancelled();
    final status = await inspect();
    cancellation?.throwIfCancelled();
    if (!status.ready) {
      throw StateError('voice model is not verified: ${status.reason}');
    }
    return _SherpaParaformerWorker.spawn(
      status,
      cancellation: cancellation,
      toTraditional: (text) async {
        final converted = await _channel.invokeMethod<String>('toTraditional', {
          'text': text,
        });
        if (converted == null) {
          throw StateError(
            'local Traditional Chinese conversion returned no text',
          );
        }
        return converted;
      },
    );
  }
}

typedef _TraditionalConverter = Future<String> Function(String text);

/// One native sherpa-onnx recognizer per killable Dart isolate.
final class _SherpaParaformerWorker implements LocalAsrWorker {
  _SherpaParaformerWorker._({
    required Isolate isolate,
    required SendPort commands,
    required ReceivePort responses,
    required ReceivePort errors,
    required ReceivePort exits,
    required Completer<void> exitSignal,
    required _TraditionalConverter toTraditional,
  }) : _isolate = isolate,
       _commands = commands,
       _responses = responses,
       _errors = errors,
       _exits = exits,
       _exitSignal = exitSignal,
       _toTraditional = toTraditional;

  final Isolate _isolate;
  final SendPort _commands;
  final ReceivePort _responses;
  final ReceivePort _errors;
  final ReceivePort _exits;
  final Completer<void> _exitSignal;
  final _TraditionalConverter _toTraditional;
  final Map<int, Completer<Object?>> _pending = {};
  int _nextId = 1;
  bool _closed = false;
  bool _shuttingDown = false;
  Future<void>? _shutdownFuture;

  @override
  String get modelId => productionVoiceModelId;

  static Future<_SherpaParaformerWorker> spawn(
    LocalAsrModelStatus status, {
    LocalAsrCancellationToken? cancellation,
    required _TraditionalConverter toTraditional,
  }) async {
    cancellation?.throwIfCancelled();
    if (!status.ready) throw StateError('voice model status is not ready');
    final responses = ReceivePort('miku-asr-responses');
    final errors = ReceivePort('miku-asr-errors');
    final exits = ReceivePort('miku-asr-exits');
    final handshake = Completer<SendPort>();
    final exitSignal = Completer<void>();
    _SherpaParaformerWorker? worker;
    final isolate = await Isolate.spawn<SendPort>(
      _sherpaWorkerMain,
      responses.sendPort,
      debugName: 'miku-sherpa-paraformer',
      errorsAreFatal: true,
      onError: errors.sendPort,
      onExit: exits.sendPort,
    );
    if (cancellation?.isCancelled ?? false) {
      isolate.kill(priority: Isolate.immediate);
      responses.close();
      errors.close();
      exits.close();
      throw const LocalAsrCancelledException();
    }
    responses.listen((message) {
      if (message is SendPort && !handshake.isCompleted) {
        handshake.complete(message);
        return;
      }
      if (message is Map<Object?, Object?>) worker?._handleResponse(message);
    });
    errors.listen((error) {
      final message =
          error is List<Object?> && error.isNotEmpty ? error.first : error;
      final failure = StateError('local ASR worker failed: $message');
      if (worker case final active?) {
        active._failAll(failure);
      } else if (!handshake.isCompleted) {
        handshake.completeError(failure);
      }
    });
    exits.listen((_) {
      if (!exitSignal.isCompleted) exitSignal.complete();
      if (worker case final active?) {
        active._failAll(const LocalAsrCancelledException());
      } else if (!handshake.isCompleted) {
        handshake.completeError(const LocalAsrCancelledException());
      }
    });
    try {
      final commands = await handshake.future.timeout(
        const Duration(seconds: 5),
      );
      cancellation?.throwIfCancelled();
      final active = _SherpaParaformerWorker._(
        isolate: isolate,
        commands: commands,
        responses: responses,
        errors: errors,
        exits: exits,
        exitSignal: exitSignal,
        toTraditional: toTraditional,
      );
      worker = active;
      await active._command('configure', {
        'encoder': status.encoder,
        'decoder': status.decoder,
        'tokens': status.tokens,
      });
      cancellation?.throwIfCancelled();
      return active;
    } catch (_) {
      isolate.kill(priority: Isolate.immediate);
      responses.close();
      errors.close();
      exits.close();
      rethrow;
    }
  }

  @override
  Future<Duration> load() async {
    final micros = await _command('load', const {});
    if (micros is! int || micros < 0) {
      throw const FormatException(
        'local ASR worker returned an invalid load duration',
      );
    }
    return Duration(microseconds: micros);
  }

  @override
  Future<LocalAsrTranscript> transcribe(LocalAsrAudio audio) async {
    if (audio.sampleRate != localAsrSampleRate || audio.samples.isEmpty) {
      throw ArgumentError('local ASR audio was not normalized');
    }
    final started = Stopwatch()..start();
    final bytes = Uint8List.view(
      audio.samples.buffer,
      audio.samples.offsetInBytes,
      audio.samples.lengthInBytes,
    );
    final raw = await _command('transcribe', {
      'sampleRate': audio.sampleRate,
      'samples': TransferableTypedData.fromList([bytes]),
    });
    if (raw is! String) {
      throw const FormatException(
        'local ASR worker returned an invalid transcript',
      );
    }
    final converted = (await _toTraditional(raw)).trim();
    if (converted.isEmpty) {
      throw const FormatException('local ASR returned an empty transcript');
    }
    return LocalAsrTranscript(
      text: converted,
      inferenceDuration: started.elapsed,
      language: 'zh-Hant',
    );
  }

  Future<Object?> _command(String command, Map<String, Object?> payload) {
    if (_closed || _shuttingDown) {
      return Future.error(const LocalAsrCancelledException());
    }
    return _sendCommand(command, payload);
  }

  Future<Object?> _sendCommand(String command, Map<String, Object?> payload) {
    if (_closed) return Future.error(const LocalAsrCancelledException());
    final id = _nextId++;
    final completer = Completer<Object?>();
    _pending[id] = completer;
    _commands.send({'id': id, 'command': command, ...payload});
    return completer.future;
  }

  void _handleResponse(Map<Object?, Object?> response) {
    final id = response['id'];
    if (id is! int) return;
    final completer = _pending.remove(id);
    if (completer == null) return;
    if (response['ok'] == true) {
      completer.complete(response['value']);
    } else {
      completer.completeError(
        StateError('${response['error'] ?? 'local ASR failed'}'),
      );
    }
  }

  void _failAll(Object error) {
    if (_closed && _pending.isEmpty) return;
    final pending = _pending.values.toList();
    _pending.clear();
    for (final completer in pending) {
      if (!completer.isCompleted) completer.completeError(error);
    }
  }

  @override
  Future<void> kill() => _shutdown();

  @override
  Future<void> close() => _shutdown();

  Future<void> _shutdown() {
    final existing = _shutdownFuture;
    if (existing != null) return existing;
    if (_closed) return Future.value();
    _shuttingDown = true;
    final operation = () async {
      var exitedCleanly = false;
      try {
        await _sendCommand(
          'shutdown',
          const {},
        ).timeout(const Duration(seconds: 5));
        await _exitSignal.future.timeout(const Duration(milliseconds: 500));
        exitedCleanly = true;
      } catch (_) {
        // A wedged native call cannot cooperate. The hard kill below is the
        // bounded last resort; normal cancellation frees stream + recognizer.
      } finally {
        _closed = true;
        if (!exitedCleanly) _isolate.kill(priority: Isolate.immediate);
        _failAll(const LocalAsrCancelledException());
        _responses.close();
        _errors.close();
        _exits.close();
      }
    }();
    _shutdownFuture = operation;
    return operation;
  }
}

@pragma('vm:entry-point')
void _sherpaWorkerMain(SendPort parent) {
  final commands = ReceivePort('miku-asr-commands');
  parent.send(commands.sendPort);
  final runtime = _SherpaWorkerRuntime(parent: parent, commands: commands);
  commands.listen(runtime.handle);
}

final class _SherpaWorkerRuntime {
  _SherpaWorkerRuntime({required this.parent, required this.commands});

  final SendPort parent;
  final ReceivePort commands;
  sherpa.OnlineRecognizer? recognizer;
  Map<String, String>? paths;
  Future<String>? activeTranscription;
  bool cancelRequested = false;
  bool shuttingDown = false;

  void handle(Object? message) {
    unawaited(_handle(message));
  }

  Future<void> _handle(Object? message) async {
    if (message is! Map<Object?, Object?> || message['id'] is! int) return;
    final id = message['id']! as int;
    final command = message['command'];
    try {
      switch (command) {
        case 'configure':
          if (shuttingDown || activeTranscription != null) {
            throw StateError('local ASR worker was busy');
          }
          final configured = <String, String>{};
          for (final field in const ['encoder', 'decoder', 'tokens']) {
            final value = message[field];
            if (value is! String ||
                !File(value).isAbsolute ||
                !File(value).existsSync()) {
              throw StateError(
                'verified voice model path $field was unavailable',
              );
            }
            configured[field] = value;
          }
          paths = configured;
          parent.send({'id': id, 'ok': true});
        case 'load':
          if (shuttingDown || activeTranscription != null) {
            throw StateError('local ASR worker was busy');
          }
          if (recognizer != null) {
            parent.send({'id': id, 'ok': true, 'value': 0});
            return;
          }
          final modelPaths =
              paths ?? (throw StateError('voice model was not configured'));
          final stopwatch = Stopwatch()..start();
          sherpa.initBindings();
          recognizer = sherpa.OnlineRecognizer(
            sherpa.OnlineRecognizerConfig(
              feat: const sherpa.FeatureConfig(
                sampleRate: localAsrSampleRate,
                featureDim: 80,
              ),
              model: sherpa.OnlineModelConfig(
                paraformer: sherpa.OnlineParaformerModelConfig(
                  encoder: modelPaths['encoder']!,
                  decoder: modelPaths['decoder']!,
                ),
                tokens: modelPaths['tokens']!,
                numThreads: 2,
                provider: 'cpu',
                debug: false,
                modelType: 'paraformer',
              ),
              decodingMethod: 'greedy_search',
              enableEndpoint: false,
            ),
          );
          parent.send({
            'id': id,
            'ok': true,
            'value': stopwatch.elapsedMicroseconds,
          });
        case 'transcribe':
          if (shuttingDown || activeTranscription != null) {
            throw StateError('local ASR worker was busy');
          }
          cancelRequested = false;
          final job = _transcribe(message);
          activeTranscription = job;
          try {
            parent.send({'id': id, 'ok': true, 'value': await job});
          } finally {
            if (identical(activeTranscription, job)) activeTranscription = null;
          }
        case 'shutdown':
          shuttingDown = true;
          cancelRequested = true;
          final active = activeTranscription;
          if (active != null) {
            try {
              await active;
            } catch (_) {
              // Cancellation is expected; _transcribe already freed its stream.
            }
          }
          recognizer?.free();
          recognizer = null;
          parent.send({'id': id, 'ok': true});
          commands.close();
        default:
          throw StateError('unknown local ASR worker command');
      }
    } catch (error) {
      parent.send({'id': id, 'ok': false, 'error': error.toString()});
    }
  }

  Future<String> _transcribe(Map<Object?, Object?> message) async {
    final active =
        recognizer ?? (throw StateError('voice model was not loaded'));
    final transfer = message['samples'];
    final sampleRate = message['sampleRate'];
    if (transfer is! TransferableTypedData ||
        sampleRate != localAsrSampleRate) {
      throw StateError('local ASR audio payload was invalid');
    }
    // TransferableTypedData can materialize as an unmodifiable view on the
    // Android release runtime. Take explicit mutable ownership before bounds
    // checks and inference so deterministic erasure cannot mask a successful
    // transcription with an UnsupportedError.
    final data = cloneSensitiveBytes(transfer.materialize().asUint8List());
    if (data.isEmpty ||
        data.lengthInBytes % 4 != 0 ||
        data.lengthInBytes > localAsrMaxSamples * 4) {
      data.fillRange(0, data.length, 0);
      throw StateError('local ASR audio exceeded its bound');
    }
    final samples = Float32List.view(
      data.buffer,
      data.offsetInBytes,
      data.lengthInBytes ~/ 4,
    );
    final stream = active.createStream();
    Float32List? flushPadding;
    try {
      await Future<void>.delayed(Duration.zero);
      _checkCancelled();
      var steps = 0;
      Future<void> drainReady() async {
        while (active.isReady(stream)) {
          active.decode(stream);
          steps += 1;
          if (steps > localAsrMaxDecodeSteps) {
            throw StateError('local ASR decode step bound exceeded');
          }
          await Future<void>.delayed(Duration.zero);
          _checkCancelled();
        }
      }

      for (
        var offset = 0;
        offset < samples.length;
        offset += localAsrChunkSamples
      ) {
        final end = (offset + localAsrChunkSamples).clamp(0, samples.length);
        stream.acceptWaveform(
          samples: Float32List.sublistView(samples, offset, end),
          sampleRate: localAsrSampleRate,
        );
        await Future<void>.delayed(Duration.zero);
        _checkCancelled();
        await drainReady();
      }
      // Online Paraformer has a bounded look-ahead delay. One second of local
      // zero padding flushes final tokens without extending the 60s user-audio
      // allowance or introducing any microphone-derived samples.
      flushPadding = Float32List(localAsrFlushPaddingSamples);
      for (
        var offset = 0;
        offset < flushPadding.length;
        offset += localAsrChunkSamples
      ) {
        final end = (offset + localAsrChunkSamples).clamp(
          0,
          flushPadding.length,
        );
        stream.acceptWaveform(
          samples: Float32List.sublistView(flushPadding, offset, end),
          sampleRate: localAsrSampleRate,
        );
        await Future<void>.delayed(Duration.zero);
        _checkCancelled();
        await drainReady();
      }
      stream.inputFinished();
      await drainReady();
      final text = active.getResult(stream).text.trim();
      if (text.isEmpty) {
        throw const FormatException('local ASR returned an empty transcript');
      }
      return text;
    } finally {
      flushPadding?.fillRange(0, flushPadding.length, 0);
      data.fillRange(0, data.length, 0);
      stream.free();
    }
  }

  void _checkCancelled() {
    if (cancelRequested || shuttingDown) {
      throw const LocalAsrCancelledException();
    }
  }
}
