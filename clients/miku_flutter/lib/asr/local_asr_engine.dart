import 'dart:async';
import 'dart:typed_data';

const int localAsrSampleRate = 16000;
const int localAsrMaxDurationSeconds = 60;
const int localAsrMaxSamples = localAsrSampleRate * localAsrMaxDurationSeconds;
const int localAsrMaxPcm16Bytes = localAsrMaxSamples * 2;
const int localAsrFlushPaddingSeconds = 1;
const int localAsrFlushPaddingSamples =
    localAsrSampleRate * localAsrFlushPaddingSeconds;
const int localAsrChunkSamples = localAsrSampleRate ~/ 10;
const int localAsrMaxDecodeSteps = 20000;

final class LocalAsrAudio {
  LocalAsrAudio({required this.samples, required this.sampleRate}) {
    if (sampleRate != localAsrSampleRate) {
      throw ArgumentError.value(
        sampleRate,
        'sampleRate',
        'on-device ASR accepts exactly 16 kHz audio',
      );
    }
    if (samples.isEmpty) {
      throw ArgumentError.value(samples.length, 'samples', 'audio is empty');
    }
    if (samples.length > localAsrMaxSamples) {
      throw ArgumentError.value(
        samples.length,
        'samples',
        'audio exceeds the 60-second bound',
      );
    }
  }

  final Float32List samples;
  final int sampleRate;

  double get durationSeconds => samples.length / sampleRate;

  factory LocalAsrAudio.fromPcm16(
    Uint8List pcm16, {
    int sampleRate = localAsrSampleRate,
  }) {
    if (pcm16.isEmpty || pcm16.lengthInBytes.isOdd) {
      throw ArgumentError.value(
        pcm16.lengthInBytes,
        'pcm16',
        'audio must contain complete non-empty PCM16 samples',
      );
    }
    if (pcm16.lengthInBytes > localAsrMaxPcm16Bytes) {
      throw ArgumentError.value(
        pcm16.lengthInBytes,
        'pcm16',
        'audio exceeds the 60-second bound',
      );
    }
    final bytes = ByteData.sublistView(pcm16);
    final samples = Float32List(pcm16.lengthInBytes ~/ 2);
    for (var index = 0; index < samples.length; index += 1) {
      samples[index] = bytes.getInt16(index * 2, Endian.little) / 32768.0;
    }
    return LocalAsrAudio(samples: samples, sampleRate: sampleRate);
  }
}

final class LocalAsrTranscript {
  const LocalAsrTranscript({
    required this.text,
    required this.inferenceDuration,
    this.language = '',
    this.emotion = '',
    this.event = '',
  });

  final String text;
  final Duration inferenceDuration;
  final String language;
  final String emotion;
  final String event;
}

/// Replaceable authority-free inference boundary for P6.6.
///
/// Implementations receive normalized in-memory PCM and return text only. Model
/// installation, microphone capture, review, and durable message sending remain
/// outside this interface.
abstract interface class LocalAsrEngine {
  String get modelId;

  Future<Duration> load();

  Future<LocalAsrTranscript> transcribe(LocalAsrAudio audio);

  Future<void> close();
}

/// A killable worker boundary around one engine instance.
///
/// Production implementations must isolate inference in a native worker or a
/// Dart isolate whose resources are actually terminated by [kill]. Keeping the
/// engine behind this boundary prevents a timed-out model call from continuing
/// to retain microphone-derived audio in the UI process.
abstract interface class LocalAsrWorker implements LocalAsrEngine {
  Future<void> kill();
}

abstract interface class LocalAsrWorkerFactory {
  Future<LocalAsrWorker> spawn({LocalAsrCancellationToken? cancellation});
}

/// Synchronous cancellation signal shared with a worker factory while it is
/// still verifying models or starting its isolate.
///
/// Factories should check [isCancelled] before every irreversible startup
/// step. [LocalAsrTranscriber] also guards factories that ignore the token, so
/// a late worker is retired without ever loading or receiving audio.
final class LocalAsrCancellationToken {
  final Completer<void> _signal = Completer<void>();

  bool get isCancelled => _signal.isCompleted;

  Future<void> get whenCancelled => _signal.future;

  void throwIfCancelled() {
    if (isCancelled) throw const LocalAsrCancelledException();
  }

  void _cancel() {
    if (!_signal.isCompleted) _signal.complete();
  }
}

final class _LocalAsrOperation {
  _LocalAsrOperation() : cancellation = LocalAsrCancellationToken();

  final LocalAsrCancellationToken cancellation;
  final Completer<void> settled = Completer<void>();
  LocalAsrWorker? worker;
}

final class _SpawnOutcome {
  const _SpawnOutcome.worker(this.worker) : error = null, cancelled = false;
  const _SpawnOutcome.error(this.error) : worker = null, cancelled = false;
  const _SpawnOutcome.cancelled()
    : worker = null,
      error = null,
      cancelled = true;

  final LocalAsrWorker? worker;
  final Object? error;
  final bool cancelled;
}

final class LocalAsrCancelledException implements Exception {
  const LocalAsrCancelledException();

  @override
  String toString() => 'Local ASR transcription was cancelled';
}

/// Owns at most one killable local inference worker.
final class LocalAsrTranscriber {
  LocalAsrTranscriber({
    required this.workers,
    this.timeout = const Duration(seconds: 45),
  });

  final LocalAsrWorkerFactory workers;
  final Duration timeout;
  _LocalAsrOperation? _activeOperation;

  bool get isActive => _activeOperation != null;

  Future<LocalAsrTranscript> transcribe(LocalAsrAudio audio) async {
    if (_activeOperation != null) {
      throw StateError('local ASR transcription is already active');
    }
    if (timeout <= Duration.zero) {
      throw ArgumentError.value(timeout, 'timeout', 'must be positive');
    }
    final operation = _LocalAsrOperation();
    _activeOperation = operation;
    try {
      return await (() async {
        final worker = await _spawnWorker(operation);
        operation.cancellation.throwIfCancelled();
        operation.worker = worker;
        await worker.load();
        operation.cancellation.throwIfCancelled();
        final transcript = await worker.transcribe(audio);
        operation.cancellation.throwIfCancelled();
        return transcript;
      })().timeout(
        timeout,
        onTimeout: () async {
          operation.cancellation._cancel();
          final worker = operation.worker;
          if (worker != null) await worker.kill();
          throw TimeoutException(
            'local ASR exceeded the ${timeout.inSeconds}-second bound',
            timeout,
          );
        },
      );
    } finally {
      operation.cancellation._cancel();
      final worker = operation.worker;
      if (worker != null) await worker.close();
      if (identical(_activeOperation, operation)) _activeOperation = null;
      if (!operation.settled.isCompleted) operation.settled.complete();
    }
  }

  Future<void> cancel() async {
    final operation = _activeOperation;
    if (operation == null) return;
    operation.cancellation._cancel();
    final worker = operation.worker;
    if (worker != null) await worker.kill();
    await operation.settled.future;
  }

  Future<LocalAsrWorker> _spawnWorker(_LocalAsrOperation operation) async {
    final pending = workers.spawn(cancellation: operation.cancellation);
    final outcome = await Future.any<_SpawnOutcome>([
      pending.then<_SpawnOutcome>(
        _SpawnOutcome.worker,
        onError: (Object error) => _SpawnOutcome.error(error),
      ),
      operation.cancellation.whenCancelled.then<_SpawnOutcome>(
        (_) => const _SpawnOutcome.cancelled(),
      ),
    ]);
    if (outcome.cancelled) {
      // A factory may not understand cancellation. Keep a listener on its
      // source future so any worker created later is retired before load or
      // audio transfer, without keeping the foreground cancellation waiting.
      unawaited(
        pending.then<void>((lateWorker) async {
          await lateWorker.kill();
          await lateWorker.close();
        }, onError: (_) {}),
      );
      throw const LocalAsrCancelledException();
    }
    final error = outcome.error;
    if (error != null) throw error;
    final worker = outcome.worker!;
    if (operation.cancellation.isCancelled) {
      await worker.kill();
      await worker.close();
      throw const LocalAsrCancelledException();
    }
    return worker;
  }
}
