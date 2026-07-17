import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:mobile_scanner/mobile_scanner.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'ratex_formula.dart';
import 'notification_service.dart';
import 'share_import_service.dart';
import 'session_client.dart';
import 'session_models.dart';

part 'theme.dart';
part 'theme_preferences.dart';
part 'brand.dart';
part 'copy.dart';
part 'modes.dart';
part 'app.dart';
part 'adaptive_shell.dart';
part 'home.dart';
part 'home_chrome.dart';
part 'home_thread.dart';
part 'app_models.dart';
part 'home_event_parsing.dart';
part 'home_activity_mapping.dart';
part 'widgets.dart';
part 'widgets_chat.dart';
part 'widgets_markdown.dart';
part 'widgets_markdown_renderer.dart';
part 'widgets_markdown_support.dart';
part 'widgets_markdown_table.dart';
part 'widgets_markdown_parser.dart';
part 'widgets_markdown_inline.dart';
part 'widgets_agent_status.dart';
part 'widgets_activity_cards.dart';
part 'sheets.dart';
part 'sheet_mode_activity.dart';
part 'sheet_history.dart';
part 'sheet_approval_overflow.dart';
part 'sheet_drive.dart';
part 'sheet_drive/approvals.dart';
part 'sheet_drive/documents.dart';
part 'sheet_drive/proposals.dart';
part 'sheet_drive/widgets.dart';
part 'sheet_resource.dart';
part 'sheet_share_import.dart';
part 'pairing_scanner.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  runApp(
    MikuApp(
      client: createDefaultClient(),
      notifications: createNotificationService(),
      shareImports: createShareImportService(),
    ),
  );
}
