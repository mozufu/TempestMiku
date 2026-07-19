import 'voice_capture_service_platform.dart';

MikuVoiceCaptureService createVoiceCaptureService() =>
    const UnsupportedVoiceCaptureService();

class UnsupportedVoiceCaptureService
    implements MikuVoiceCaptureService, MikuVoiceBuildInspector {
  const UnsupportedVoiceCaptureService();

  @override
  bool get isSupported => false;

  @override
  Future<VoiceAppBuildFingerprint> inspectBuild() =>
      Future.error(UnsupportedError('app build fingerprint is unavailable'));

  @override
  Future<bool> cancel(String? captureId) async => false;

  @override
  Future<int> recoverOrphans() async => 0;

  @override
  Future<bool> requestPermission() async => false;

  @override
  Future<void> start(String captureId) =>
      Future.error(UnsupportedError('voice capture is unavailable'));

  @override
  Future<CapturedVoicePcm> stop(String captureId) =>
      Future.error(UnsupportedError('voice capture is unavailable'));
}
