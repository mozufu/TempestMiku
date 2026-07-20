import 'dart:async';

import 'package:flutter/material.dart';
import 'package:mobile_scanner/mobile_scanner.dart';

import 'pairing_scanner_service_platform.dart';

PairingScannerService createPlatformPairingScannerService() =>
    _MobilePairingScannerService();

class _MobilePairingScannerService implements PairingScannerService {
  _MobilePairingScannerService()
    : _controller = MobileScannerController(
        autoStart: false,
        formats: const [BarcodeFormat.qrCode],
      );

  final MobileScannerController _controller;
  final StreamController<PairingScannerEvent> _events =
      StreamController<PairingScannerEvent>.broadcast();
  bool _running = false;
  bool _disposed = false;
  PairingScannerProblem? _lastProblem;

  @override
  bool get isSupported => true;

  @override
  Stream<PairingScannerEvent> get events => _events.stream;

  @override
  Widget buildPreview() {
    return MobileScanner(
      controller: _controller,
      useAppLifecycleState: false,
      onDetect: (capture) {
        if (_disposed) return;
        for (final barcode in capture.barcodes) {
          final rawValue = barcode.rawValue;
          if (barcode.format == BarcodeFormat.qrCode &&
              rawValue != null &&
              rawValue.isNotEmpty) {
            _events.add(PairingScannerEvent.payload(rawValue));
          }
        }
      },
      onDetectError: (_, _) {
        _emitProblem(PairingScannerProblem.cameraError);
      },
      errorBuilder: (_, error) {
        _emitProblem(_problemFor(error));
        return const ColoredBox(color: Colors.black);
      },
      placeholderBuilder: (_) => const ColoredBox(color: Colors.black),
    );
  }

  @override
  Future<void> start() async {
    if (_disposed || _running) return;
    _lastProblem = null;
    try {
      await _controller.start();
      if (_disposed) return;
      _running = true;
      _events.add(const PairingScannerEvent.ready());
    } on MobileScannerException catch (error) {
      _emitProblem(_problemFor(error));
    } on Exception {
      _emitProblem(PairingScannerProblem.cameraError);
    }
  }

  @override
  Future<void> stop() async {
    if (_disposed || !_running) return;
    _running = false;
    try {
      await _controller.stop();
    } on MobileScannerException catch (error) {
      if (error.errorCode != MobileScannerErrorCode.controllerUninitialized &&
          error.errorCode != MobileScannerErrorCode.controllerDisposed) {
        _emitProblem(_problemFor(error));
      }
    }
  }

  @override
  Future<void> dispose() async {
    if (_disposed) return;
    await stop();
    _disposed = true;
    await _controller.dispose();
    await _events.close();
  }

  PairingScannerProblem _problemFor(MobileScannerException error) {
    return switch (error.errorCode) {
      MobileScannerErrorCode.permissionDenied =>
        PairingScannerProblem.permissionDenied,
      MobileScannerErrorCode.unsupported => PairingScannerProblem.unsupported,
      _ => PairingScannerProblem.cameraError,
    };
  }

  void _emitProblem(PairingScannerProblem problem) {
    if (_disposed || _lastProblem == problem) return;
    _running = false;
    _lastProblem = problem;
    _events.add(PairingScannerEvent.problem(problem));
  }
}
