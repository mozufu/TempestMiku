import 'package:flutter/widgets.dart';

enum PairingScannerProblem { permissionDenied, unsupported, cameraError }

enum PairingScannerEventType { ready, payload, problem }

class PairingScannerEvent {
  const PairingScannerEvent._({
    required this.type,
    this.rawValue,
    this.problem,
  });

  const PairingScannerEvent.ready()
    : this._(type: PairingScannerEventType.ready);

  const PairingScannerEvent.payload(String rawValue)
    : this._(type: PairingScannerEventType.payload, rawValue: rawValue);

  const PairingScannerEvent.problem(PairingScannerProblem problem)
    : this._(type: PairingScannerEventType.problem, problem: problem);

  final PairingScannerEventType type;
  final String? rawValue;
  final PairingScannerProblem? problem;
}

/// Owns the short-lived camera authority for one pairing scan route.
///
/// Implementations must only emit QR payloads and must never persist, exchange,
/// log, or otherwise transform the one-time pairing value.
abstract class PairingScannerService {
  bool get isSupported;

  Stream<PairingScannerEvent> get events;

  Widget buildPreview();

  Future<void> start();

  Future<void> stop();

  Future<void> dispose();
}
