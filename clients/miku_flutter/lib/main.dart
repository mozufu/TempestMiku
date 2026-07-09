import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'ratex_formula.dart';
import 'session_client.dart';
import 'session_models.dart';

part 'theme.dart';
part 'copy.dart';
part 'modes.dart';
part 'app.dart';
part 'home.dart';
part 'app_models.dart';
part 'home_event_parsing.dart';
part 'widgets.dart';
part 'widgets_chat.dart';
part 'widgets_markdown.dart';
part 'widgets_agent_status.dart';
part 'widgets_activity_cards.dart';
part 'sheets.dart';
part 'sheet_mode_activity.dart';
part 'sheet_history.dart';
part 'sheet_approval_overflow.dart';
part 'sheet_drive.dart';
part 'sheet_resource.dart';

void main() {
  runApp(MikuApp(client: createDefaultClient()));
}
