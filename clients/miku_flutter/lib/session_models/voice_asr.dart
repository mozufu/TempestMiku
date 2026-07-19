part of '../session_models.dart';

const String localVoiceAsrEngineId = 'local';
const String selfHostedVoiceAsrEngineId = 'self_hosted';
const int voiceAsrSampleRate = 16000;
const int voiceAsrChannels = 1;
const int voiceAsrMaxPcm16Bytes = 1920000;

enum VoiceAsrEngineKind { local, remote }

final class VoiceAsrEngine {
  const VoiceAsrEngine({
    required this.id,
    required this.kind,
    required this.label,
    required this.available,
    this.modelId,
    this.maxDurationSeconds = 60,
  });

  factory VoiceAsrEngine.fromJson(Map<String, Object?> json) {
    final id = json['id'];
    final rawKind = json['kind'];
    final label = json['label'];
    final available = json['available'];
    final modelId = json['modelId'] ?? json['model_id'];
    final maxDuration =
        json['maxDurationSeconds'] ?? json['max_duration_seconds'] ?? 60;
    if (id is! String || id.isEmpty || id.length > 64) {
      throw const FormatException('voice ASR engine has an invalid id');
    }
    final kind = switch (rawKind) {
      'local' => VoiceAsrEngineKind.local,
      'remote' => VoiceAsrEngineKind.remote,
      _ => throw const FormatException('voice ASR engine has an invalid kind'),
    };
    if (label is! String || label.isEmpty || label.length > 120) {
      throw const FormatException('voice ASR engine has an invalid label');
    }
    if (available is! bool) {
      throw const FormatException(
        'voice ASR engine has an invalid availability',
      );
    }
    if (modelId != null &&
        (modelId is! String || modelId.isEmpty || modelId.length > 160)) {
      throw const FormatException('voice ASR engine has an invalid model id');
    }
    if (maxDuration is! num ||
        maxDuration.toInt() != maxDuration ||
        maxDuration < 1 ||
        maxDuration > 60) {
      throw const FormatException(
        'voice ASR engine has an invalid duration bound',
      );
    }
    if ((kind == VoiceAsrEngineKind.local && id != localVoiceAsrEngineId) ||
        (kind == VoiceAsrEngineKind.remote &&
            id != selfHostedVoiceAsrEngineId)) {
      throw const FormatException('voice ASR engine id and kind disagree');
    }
    return VoiceAsrEngine(
      id: id,
      kind: kind,
      label: label,
      available: available,
      modelId: modelId as String?,
      maxDurationSeconds: maxDuration.toInt(),
    );
  }

  final String id;
  final VoiceAsrEngineKind kind;
  final String label;
  final bool available;
  final String? modelId;
  final int maxDurationSeconds;
}

final class VoiceAsrEngineCatalog {
  const VoiceAsrEngineCatalog(this.engines);

  factory VoiceAsrEngineCatalog.fromJson(Map<String, Object?> json) {
    final rawEngines = json['engines'];
    if (rawEngines is! List) {
      throw const FormatException('voice ASR catalog is missing engines');
    }
    final engines = <VoiceAsrEngine>[];
    final ids = <String>{};
    for (final rawEngine in rawEngines) {
      if (rawEngine is! Map) {
        throw const FormatException(
          'voice ASR catalog contains an invalid engine',
        );
      }
      final engine = VoiceAsrEngine.fromJson(rawEngine.cast<String, Object?>());
      if (!ids.add(engine.id)) {
        throw const FormatException('voice ASR catalog contains duplicate ids');
      }
      engines.add(engine);
    }
    return VoiceAsrEngineCatalog(List.unmodifiable(engines));
  }

  factory VoiceAsrEngineCatalog.localOnly() => const VoiceAsrEngineCatalog([
    VoiceAsrEngine(
      id: localVoiceAsrEngineId,
      kind: VoiceAsrEngineKind.local,
      label: 'On-device',
      available: true,
    ),
  ]);

  final List<VoiceAsrEngine> engines;

  VoiceAsrEngine? byId(String id) {
    for (final engine in engines) {
      if (engine.id == id) return engine;
    }
    return null;
  }

  VoiceAsrEngine? get selfHosted => byId(selfHostedVoiceAsrEngineId);
}

final class VoiceAsrTranscript {
  const VoiceAsrTranscript({
    required this.text,
    required this.engineId,
    required this.modelId,
  });

  factory VoiceAsrTranscript.fromJson(Map<String, Object?> json) {
    final text = json['text'];
    final engineId = json['engineId'] ?? json['engine_id'];
    final modelId = json['modelId'] ?? json['model_id'];
    if (text is! String || text.trim().isEmpty || text.length > 16384) {
      throw const FormatException('voice ASR returned an invalid transcript');
    }
    if (engineId != selfHostedVoiceAsrEngineId) {
      throw const FormatException('voice ASR returned an unexpected engine');
    }
    if (modelId is! String || modelId.isEmpty || modelId.length > 160) {
      throw const FormatException('voice ASR returned an invalid model id');
    }
    return VoiceAsrTranscript(
      text: text.trim(),
      engineId: engineId as String,
      modelId: modelId,
    );
  }

  final String text;
  final String engineId;
  final String modelId;
}

void validateVoiceAsrPcm16Request({
  required String engineId,
  required String captureId,
  required int sampleRate,
  required Uint8List pcm16,
}) {
  if (engineId != selfHostedVoiceAsrEngineId) {
    throw ArgumentError.value(engineId, 'engineId', 'must be self_hosted');
  }
  if (!RegExp(
    r'^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$',
  ).hasMatch(captureId)) {
    throw ArgumentError.value(captureId, 'captureId', 'must be a UUID');
  }
  if (sampleRate != voiceAsrSampleRate) {
    throw ArgumentError.value(sampleRate, 'sampleRate', 'must be 16 kHz');
  }
  if (pcm16.isEmpty || pcm16.lengthInBytes.isOdd) {
    throw ArgumentError.value(
      pcm16.lengthInBytes,
      'pcm16',
      'must contain complete non-empty PCM16 samples',
    );
  }
  if (pcm16.lengthInBytes > voiceAsrMaxPcm16Bytes) {
    throw ArgumentError.value(
      pcm16.lengthInBytes,
      'pcm16',
      'exceeds the 60-second bound',
    );
  }
}
