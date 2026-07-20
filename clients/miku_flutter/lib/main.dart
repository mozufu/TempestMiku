import 'package:flutter/widgets.dart';

import 'asr/local_asr_model.dart';
import 'conversation_app.dart';
import 'notification_service.dart';
import 'session_client.dart';
import 'share_import_service.dart';
import 'voice_capture_service.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  runApp(
    TempestMikuApp(
      client: createDefaultClient(),
      shareImports: createShareImportService(),
      voiceCapture: createVoiceCaptureService(),
      localAsrModels: createLocalAsrModelManager(),
      notifications: createNotificationService(),
    ),
  );
}
