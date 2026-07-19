part of '../session_client_io.dart';

extension _NativeVoiceAsr on NativeMikuSessionClient {
  Future<VoiceAsrEngineCatalog> _voiceAsrEnginesImpl() async {
    final json = await _request('GET', '/voice/asr/engines');
    return VoiceAsrEngineCatalog.fromJson(json);
  }

  Future<VoiceAsrTranscript> _transcribeVoicePcm16Impl({
    required String engineId,
    required String captureId,
    required int sampleRate,
    required Uint8List pcm16,
  }) async {
    validateVoiceAsrPcm16Request(
      engineId: engineId,
      captureId: captureId,
      sampleRate: sampleRate,
      pcm16: pcm16,
    );
    final json = await _binaryRequest(
      'POST',
      '/voice/asr/transcriptions',
      body: pcm16,
      headers: {
        'x-tm-asr-engine-id': engineId,
        'x-tm-capture-id': captureId,
        'x-tm-sample-rate': '$sampleRate',
        'x-tm-channels': '$voiceAsrChannels',
      },
    );
    return VoiceAsrTranscript.fromJson(json);
  }

  Future<void> _cancelVoiceAsrTranscriptionImpl() async {
    _voiceAsrRequestEpoch += 1;
    final error = StateError('voice ASR transcription cancelled');
    _activeVoiceAsrRequest?.abort(error);
    final cancellation = _activeVoiceAsrCancellation;
    if (cancellation != null && !cancellation.isCompleted) {
      cancellation.completeError(error);
    }
    final done = _activeVoiceAsrDone;
    if (done == null) return;
    try {
      await done.future.timeout(const Duration(seconds: 2));
    } on TimeoutException {
      // Defensive only: the cancellation race normally completes immediately.
      // The revoked epoch still guarantees that a late open cannot upload.
    }
  }
}
