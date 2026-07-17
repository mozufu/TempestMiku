import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:miku_flutter/main.dart';
import 'package:miku_flutter/notification_service.dart';
import 'package:miku_flutter/ratex_formula.dart';
import 'package:miku_flutter/session_client_io.dart' as io_client;
import 'package:miku_flutter/session_client_stub.dart';
import 'package:miku_flutter/session_models.dart';
import 'package:miku_flutter/session_sse.dart';

part 'widget_test_support.dart';
part 'widget_pairing_transport.dart';
part 'widget_notifications.dart';
part 'widget_drive_resources.dart';
part 'widget_sessions_modes.dart';
part 'widget_memory_evolution.dart';
part 'widget_activity_agents_markdown.dart';

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  _registerNotificationPolicyTests();
  _registerPairingAndTransportTests();
  _registerEvolutionReviewTests();
  _registerSessionShellTests();
  _registerAsyncTransportTests();
  _registerNotificationActionTests();
  _registerDriveAndResourceTests();
  _registerSessionAndModeTests();
  _registerMemoryProposalTests();
  _registerActivityAgentAndMarkdownTests();
}
