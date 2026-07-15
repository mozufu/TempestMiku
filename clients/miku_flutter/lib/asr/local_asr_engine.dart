import 'dart:typed_data';

const int localAsrSampleRate = 16000;
const int localAsrMaxDurationSeconds = 60;
const int localAsrMaxSamples = localAsrSampleRate * localAsrMaxDurationSeconds;
const int localAsrMaxPcm16Bytes = localAsrMaxSamples * 2;

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
