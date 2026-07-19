import 'voice_capture_service_stub.dart'
    if (dart.library.io) 'voice_capture_service_io.dart'
    as impl;
import 'voice_capture_service_platform.dart';

export 'voice_capture_service_platform.dart';

MikuVoiceCaptureService createVoiceCaptureService() =>
    impl.createVoiceCaptureService();
