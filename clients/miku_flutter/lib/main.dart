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
part 'widgets.dart';
part 'sheets.dart';

void main() {
  runApp(MikuApp(client: createDefaultClient()));
}
