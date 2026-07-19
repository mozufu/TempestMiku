import 'local_asr_engine.dart';
import 'local_asr_model_platform.dart';

LocalAsrModelManager createLocalAsrModelManager() =>
    const UnsupportedLocalAsrModelManager();

final class UnsupportedLocalAsrModelManager implements LocalAsrModelManager {
  const UnsupportedLocalAsrModelManager();

  @override
  bool get isSupported => false;

  @override
  Future<LocalAsrModelStatus> inspect() async => const LocalAsrModelStatus(
    state: LocalAsrModelState.unsupported,
    reason:
        'on-device voice models are available only on supported Android devices',
  );

  @override
  Future<LocalAsrModelStatus> install() => inspect();

  @override
  Future<LocalAsrModelStatus> delete() => inspect();

  @override
  Future<LocalAsrWorker> spawn({LocalAsrCancellationToken? cancellation}) =>
      Future.error(UnsupportedError('on-device voice model is unavailable'));
}
