import 'local_asr_engine.dart';

const String productionVoiceModelId =
    'csukuangfj/sherpa-onnx-streaming-paraformer-zh@'
    '2a7f71bb58885c1b522ed4e683abd397355d9fc4';
const int productionVoiceModelBytes = 237202501;

enum LocalAsrModelState { unsupported, missing, corrupt, ready }

final class LocalAsrModelStatus {
  const LocalAsrModelStatus({
    required this.state,
    required this.reason,
    this.modelId = productionVoiceModelId,
    this.encoder = '',
    this.decoder = '',
    this.tokens = '',
  });

  final LocalAsrModelState state;
  final String reason;
  final String modelId;
  final String encoder;
  final String decoder;
  final String tokens;

  bool get ready =>
      state == LocalAsrModelState.ready &&
      modelId == productionVoiceModelId &&
      encoder.isNotEmpty &&
      decoder.isNotEmpty &&
      tokens.isNotEmpty;

  factory LocalAsrModelStatus.fromChannel(Object? value) {
    if (value is! Map<Object?, Object?>) {
      throw const FormatException('voice model status was not an object');
    }
    final state = switch (value['state']) {
      'unsupported' => LocalAsrModelState.unsupported,
      'missing' => LocalAsrModelState.missing,
      'corrupt' => LocalAsrModelState.corrupt,
      'ready' => LocalAsrModelState.ready,
      _ => throw const FormatException('voice model status was unknown'),
    };
    String field(String name) =>
        value[name] is String ? value[name]! as String : '';
    final status = LocalAsrModelStatus(
      state: state,
      reason: field('reason'),
      modelId: field('modelId'),
      encoder: field('encoder'),
      decoder: field('decoder'),
      tokens: field('tokens'),
    );
    if (state == LocalAsrModelState.ready && !status.ready) {
      throw const FormatException('ready voice model status was incomplete');
    }
    return status;
  }
}

/// Byte-level progress for an in-flight voice model download (H7).
final class LocalAsrModelInstallProgress {
  const LocalAsrModelInstallProgress({
    required this.receivedBytes,
    required this.totalBytes,
  });

  final int receivedBytes;

  /// [productionVoiceModelBytes] when known, else 0.
  final int totalBytes;

  double? get fraction =>
      totalBytes > 0 ? (receivedBytes / totalBytes).clamp(0.0, 1.0) : null;
}

abstract interface class LocalAsrModelManager implements LocalAsrWorkerFactory {
  bool get isSupported;

  Future<LocalAsrModelStatus> inspect();

  /// Downloads only after an owner confirms the in-app installation dialog.
  ///
  /// [onProgress] receives byte-level download progress when the platform can
  /// report it. Completing [cancellation] aborts the download; the install
  /// future then fails with [LocalAsrCancelledException].
  Future<LocalAsrModelStatus> install({
    void Function(LocalAsrModelInstallProgress)? onProgress,
    LocalAsrCancellationToken? cancellation,
  });

  Future<LocalAsrModelStatus> delete();
}
