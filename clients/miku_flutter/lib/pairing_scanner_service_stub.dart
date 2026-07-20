import 'dart:async';

import 'package:flutter/widgets.dart';

import 'pairing_scanner_service_platform.dart';

PairingScannerService createPlatformPairingScannerService() =>
    _UnsupportedPairingScannerService();

class _UnsupportedPairingScannerService implements PairingScannerService {
  final StreamController<PairingScannerEvent> _events =
      StreamController<PairingScannerEvent>.broadcast();
  bool _disposed = false;

  @override
  bool get isSupported => false;

  @override
  Stream<PairingScannerEvent> get events => _events.stream;

  @override
  Widget buildPreview() => const ColoredBox(color: Color(0xff000000));

  @override
  Future<void> start() async {
    if (!_disposed) {
      _events.add(
        const PairingScannerEvent.problem(PairingScannerProblem.unsupported),
      );
    }
  }

  @override
  Future<void> stop() async {}

  @override
  Future<void> dispose() async {
    if (_disposed) return;
    _disposed = true;
    await _events.close();
  }
}
