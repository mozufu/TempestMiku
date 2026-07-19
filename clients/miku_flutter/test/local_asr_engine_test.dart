import 'dart:async';
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
    expect(localAsrFlushPaddingSamples, 16000);
    expect(localAsrChunkSamples, 1600);
    expect(localAsrFlushPaddingSamples ~/ localAsrChunkSamples, 10);
    expect(
      localAsrMaxDecodeSteps,
      greaterThan(
        (localAsrMaxSamples + localAsrFlushPaddingSamples) ~/
            localAsrChunkSamples,
      ),
    );
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

  test('decodes little-endian PCM16 without exceeding normalized bounds', () {
    final audio = LocalAsrAudio.fromPcm16(
      Uint8List.fromList([0x00, 0x80, 0x00, 0x00, 0xff, 0x7f]),
    );
    expect(audio.samples, hasLength(3));
    expect(audio.samples[0], -1.0);
    expect(audio.samples[1], 0.0);
    expect(audio.samples[2], closeTo(32767 / 32768, 0.000001));

    expect(() => LocalAsrAudio.fromPcm16(Uint8List(1)), throwsArgumentError);
    expect(
      () => LocalAsrAudio.fromPcm16(Uint8List(localAsrMaxPcm16Bytes + 2)),
      throwsArgumentError,
    );
  });

  test(
    '45-second worker boundary is configurable, killable, and closes',
    () async {
      final worker = _FakeWorker();
      final transcriber = LocalAsrTranscriber(
        workers: _FakeWorkerFactory(worker),
        timeout: const Duration(milliseconds: 1),
      );
      final audio = LocalAsrAudio.fromPcm16(Uint8List.fromList([0, 0]));

      await expectLater(
        transcriber.transcribe(audio),
        throwsA(isA<TimeoutException>()),
      );
      expect(worker.loaded, isTrue);
      expect(worker.killed, isTrue);
      expect(worker.closed, isTrue);
    },
  );

  test(
    'explicit cancellation kills the active worker and drops its result',
    () async {
      final worker = _FakeWorker();
      final transcriber = LocalAsrTranscriber(
        workers: _FakeWorkerFactory(worker),
      );
      final audio = LocalAsrAudio.fromPcm16(Uint8List.fromList([0, 0]));
      final pending = transcriber.transcribe(audio);
      final cancelled = expectLater(
        pending,
        throwsA(isA<LocalAsrCancelledException>()),
      );
      await Future<void>.delayed(Duration.zero);

      await transcriber.cancel();

      await cancelled;
      expect(worker.killed, isTrue);
      expect(worker.closed, isTrue);
    },
  );

  test('repeated cancellation and timeout retire every worker', () async {
    final workers = List.generate(4, (_) => _FakeWorker());
    final factory = _QueueWorkerFactory(workers);
    final audio = LocalAsrAudio.fromPcm16(Uint8List.fromList([0, 0]));

    for (var index = 0; index < 2; index += 1) {
      final transcriber = LocalAsrTranscriber(workers: factory);
      final pending = transcriber.transcribe(audio);
      final cancelled = expectLater(
        pending,
        throwsA(isA<LocalAsrCancelledException>()),
      );
      await Future<void>.delayed(Duration.zero);
      await transcriber.cancel();
      await cancelled;
    }
    for (var index = 0; index < 2; index += 1) {
      final transcriber = LocalAsrTranscriber(
        workers: factory,
        timeout: const Duration(milliseconds: 1),
      );
      await expectLater(
        transcriber.transcribe(audio),
        throwsA(isA<TimeoutException>()),
      );
    }

    expect(factory.spawned, 4);
    expect(workers.every((worker) => worker.killed && worker.closed), isTrue);
  });

  test(
    'cancel is a barrier while spawn is pending and retires a late worker',
    () async {
      final worker = _FakeWorker();
      final factory = _PendingWorkerFactory();
      final transcriber = LocalAsrTranscriber(workers: factory);
      final audio = LocalAsrAudio.fromPcm16(Uint8List.fromList([0, 0]));
      final pending = transcriber.transcribe(audio);
      final cancelled = expectLater(
        pending,
        throwsA(isA<LocalAsrCancelledException>()),
      );
      await Future<void>.delayed(Duration.zero);

      await transcriber.cancel();

      await cancelled;
      expect(transcriber.isActive, isFalse);
      expect(factory.cancellation?.isCancelled, isTrue);
      expect(worker.loaded, isFalse);

      factory.complete(worker);
      await Future<void>.delayed(Duration.zero);
      await Future<void>.delayed(Duration.zero);
      expect(worker.killed, isTrue);
      expect(worker.closed, isTrue);
      expect(worker.loaded, isFalse);
    },
  );

  test('the transcription timeout includes a pending worker spawn', () async {
    final worker = _FakeWorker();
    final factory = _PendingWorkerFactory();
    final transcriber = LocalAsrTranscriber(
      workers: factory,
      timeout: const Duration(milliseconds: 5),
    );
    final audio = LocalAsrAudio.fromPcm16(Uint8List.fromList([0, 0]));

    await expectLater(
      transcriber.transcribe(audio),
      throwsA(isA<TimeoutException>()),
    );
    expect(transcriber.isActive, isFalse);
    expect(factory.cancellation?.isCancelled, isTrue);

    factory.complete(worker);
    await Future<void>.delayed(Duration.zero);
    await Future<void>.delayed(Duration.zero);
    expect(worker.killed, isTrue);
    expect(worker.closed, isTrue);
    expect(worker.loaded, isFalse);
  });
}

final class _FakeWorkerFactory implements LocalAsrWorkerFactory {
  const _FakeWorkerFactory(this.worker);

  final _FakeWorker worker;

  @override
  Future<LocalAsrWorker> spawn({
    LocalAsrCancellationToken? cancellation,
  }) async => worker;
}

final class _QueueWorkerFactory implements LocalAsrWorkerFactory {
  _QueueWorkerFactory(this.workers);

  final List<_FakeWorker> workers;
  int spawned = 0;

  @override
  Future<LocalAsrWorker> spawn({
    LocalAsrCancellationToken? cancellation,
  }) async => workers[spawned++];
}

final class _PendingWorkerFactory implements LocalAsrWorkerFactory {
  final Completer<LocalAsrWorker> _worker = Completer<LocalAsrWorker>();
  LocalAsrCancellationToken? cancellation;

  @override
  Future<LocalAsrWorker> spawn({LocalAsrCancellationToken? cancellation}) {
    this.cancellation = cancellation;
    return _worker.future;
  }

  void complete(LocalAsrWorker worker) => _worker.complete(worker);
}

final class _FakeWorker implements LocalAsrWorker {
  final Completer<LocalAsrTranscript> _result = Completer();
  bool loaded = false;
  bool killed = false;
  bool closed = false;

  @override
  String get modelId => 'fake-local-only';

  @override
  Future<Duration> load() async {
    loaded = true;
    return Duration.zero;
  }

  @override
  Future<LocalAsrTranscript> transcribe(LocalAsrAudio audio) => _result.future;

  @override
  Future<void> kill() async {
    killed = true;
    if (!_result.isCompleted) {
      _result.complete(
        const LocalAsrTranscript(
          text: 'discarded',
          inferenceDuration: Duration.zero,
        ),
      );
    }
  }

  @override
  Future<void> close() async {
    closed = true;
  }
}
