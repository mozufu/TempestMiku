import 'package:flutter/widgets.dart';

import 'conversation_app.dart';
import 'session_client.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  runApp(TempestMikuApp(client: createDefaultClient()));
}
