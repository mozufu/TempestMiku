import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/main.dart';
import 'package:mobile_scanner/mobile_scanner.dart';

void main() {
  testWidgets('pairing scanner pauses and resumes its camera with the app', (
    tester,
  ) async {
    final controller = _LifecycleScannerController(hasPermission: true);
    await tester.pumpWidget(
      MaterialApp(
        home: PairingScannerPage(
          controller: controller,
          preview: const ColoredBox(color: Colors.black),
        ),
      ),
    );

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    await tester.pump();
    expect(controller.stopCalls, 1);

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    await tester.pump();
    expect(controller.stopCalls, 1);

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pump();
    expect(controller.startCalls, 1);
  });

  testWidgets('pairing scanner ignores lifecycle before camera permission', (
    tester,
  ) async {
    final controller = _LifecycleScannerController(hasPermission: false);
    await tester.pumpWidget(
      MaterialApp(
        home: PairingScannerPage(
          controller: controller,
          preview: const ColoredBox(color: Colors.black),
        ),
      ),
    );

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pump();

    expect(controller.stopCalls, 0);
    expect(controller.startCalls, 0);
  });

  testWidgets('pairing scanner removes its lifecycle observer on dispose', (
    tester,
  ) async {
    final controller = _LifecycleScannerController(hasPermission: true);
    await tester.pumpWidget(
      MaterialApp(
        home: PairingScannerPage(
          controller: controller,
          preview: const ColoredBox(color: Colors.black),
        ),
      ),
    );
    await tester.pumpWidget(const MaterialApp(home: SizedBox()));

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    await tester.pump();

    expect(controller.stopCalls, 0);
  });
}

class _LifecycleScannerController extends MobileScannerController {
  _LifecycleScannerController({required bool hasPermission})
    : super(autoStart: false) {
    if (hasPermission) {
      value = value.copyWith(
        availableCameras: 1,
        cameraDirection: CameraFacing.back,
        isInitialized: true,
        isRunning: true,
      );
    }
  }

  int startCalls = 0;
  int stopCalls = 0;

  @override
  Future<void> start({
    CameraFacing? cameraDirection,
    CameraLensType? cameraLensType,
  }) async {
    startCalls += 1;
    value = value.copyWith(isRunning: true);
  }

  @override
  Future<void> stop() async {
    stopCalls += 1;
    value = value.copyWith(isRunning: false);
  }
}
