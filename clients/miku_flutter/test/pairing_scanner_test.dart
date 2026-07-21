import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/pairing_scanner.dart';

const _pairingCode =
    '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef';
const _pairingLink =
    'tempestmiku://pair?v=1&server=https%3A%2F%2Fmiku.example.test%3A9443&code=$_pairingCode';

void main() {
  testWidgets('returns the exact reviewed v1 payload without pairing', (
    tester,
  ) async {
    final service = _FakePairingScannerService();
    final result = await _openScanner(tester, service);

    service.emitPayload(_pairingLink);
    await tester.pumpAndSettle();

    expect(await result, _pairingLink);
    expect(service.startCalls, 1);
    expect(service.stopCalls, greaterThanOrEqualTo(1));
  });

  testWidgets('ignores unversioned, wrong-version, and malformed QR values', (
    tester,
  ) async {
    final service = _FakePairingScannerService();
    final result = await _openScanner(tester, service);

    for (final value in [
      'https://miku.example.test/pair',
      'tempestmiku://pair?server=https%3A%2F%2Fmiku.example.test&code=$_pairingCode',
      'tempestmiku://pair?v=2&server=https%3A%2F%2Fmiku.example.test&code=$_pairingCode',
      'tempestmiku://pair?v=1&server=https%3A%2F%2Fmiku.example.test&code=short',
      '$_pairingLink&unexpected=value',
    ]) {
      service.emitPayload(value);
      await tester.pump();
      expect(find.byType(PairingScannerPage), findsOneWidget);
    }

    expect(find.textContaining('TempestMiku v1'), findsOneWidget);
    await tester.tap(find.byKey(const Key('pairing-scanner-cancel')));
    await tester.pumpAndSettle();
    expect(await result, isNull);
  });

  testWidgets('shows permission denial and retries only on explicit action', (
    tester,
  ) async {
    final service = _FakePairingScannerService(
      starts: const [PairingScannerProblem.permissionDenied, null],
    );
    final result = await _openScanner(tester, service);

    expect(find.textContaining('系統設定允許'), findsOneWidget);
    expect(service.startCalls, 1);

    final retry = find.byKey(const Key('pairing-scanner-retry'));
    expect(retry, findsOneWidget);
    expect(tester.getSize(retry).height, greaterThanOrEqualTo(44));
    await tester.tap(retry);
    await tester.pump();

    expect(service.startCalls, 2);
    expect(find.textContaining('一次性 QR'), findsOneWidget);

    await tester.tap(find.byKey(const Key('pairing-scanner-cancel')));
    await tester.pumpAndSettle();
    expect(await result, isNull);
  });

  testWidgets('permission denial offers a system-settings deep link', (
    tester,
  ) async {
    final service = _FakePairingScannerService(
      starts: const [PairingScannerProblem.permissionDenied],
    );
    final result = await _openScanner(tester, service);

    final openSettings = find.byKey(const Key('pairing-scanner-open-settings'));
    expect(openSettings, findsOneWidget);
    expect(tester.getSize(openSettings).height, greaterThanOrEqualTo(48));
    await tester.tap(openSettings);
    await tester.pump();
    expect(service.openAppSettingsCalls, 1);

    await tester.tap(find.byKey(const Key('pairing-scanner-cancel')));
    await tester.pumpAndSettle();
    expect(await result, isNull);
  });

  testWidgets('camera errors never offer the system-settings deep link', (
    tester,
  ) async {
    final service = _FakePairingScannerService(
      starts: const [PairingScannerProblem.cameraError],
    );
    final result = await _openScanner(tester, service);

    expect(
      find.byKey(const Key('pairing-scanner-open-settings')),
      findsNothing,
    );
    expect(find.byKey(const Key('pairing-scanner-retry')), findsOneWidget);

    await tester.tap(find.byKey(const Key('pairing-scanner-close')));
    await tester.pumpAndSettle();
    expect(await result, isNull);
  });

  testWidgets('shows camera errors with a retry and preserves cancel/back', (
    tester,
  ) async {
    final service = _FakePairingScannerService(
      starts: const [PairingScannerProblem.cameraError],
    );
    final result = await _openScanner(tester, service);

    expect(find.textContaining('其他程式占用相機'), findsOneWidget);
    expect(find.byKey(const Key('pairing-scanner-retry')), findsOneWidget);

    await tester.tap(find.byKey(const Key('pairing-scanner-close')));
    await tester.pumpAndSettle();
    expect(await result, isNull);
    expect(service.stopCalls, greaterThanOrEqualTo(1));
  });

  testWidgets('unsupported cameras offer a paste-flow return without retry', (
    tester,
  ) async {
    final service = _FakePairingScannerService(
      supported: false,
      starts: const [PairingScannerProblem.unsupported],
    );
    final result = await _openScanner(tester, service);

    expect(find.textContaining('貼上一次性'), findsOneWidget);
    expect(find.byKey(const Key('pairing-scanner-retry')), findsNothing);
    expect(find.byKey(const Key('pairing-scanner-close')), findsOneWidget);

    await tester.binding.handlePopRoute();
    await tester.pumpAndSettle();
    expect(await result, isNull);
  });

  testWidgets('pauses and resumes camera authority with app lifecycle', (
    tester,
  ) async {
    final service = _FakePairingScannerService();
    final result = await _openScanner(tester, service);

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    await tester.pump();
    expect(service.stopCalls, 1);

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
    await tester.pump();
    expect(service.stopCalls, 1);

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pump();
    expect(service.startCalls, 2);

    await tester.tap(find.byKey(const Key('pairing-scanner-cancel')));
    await tester.pumpAndSettle();
    expect(await result, isNull);
  });

  testWidgets('a pause during camera startup is fenced before resume', (
    tester,
  ) async {
    final startGate = Completer<void>();
    final service = _FakePairingScannerService(firstStartGate: startGate);
    final result = await _openScanner(tester, service);

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    await tester.pump();
    startGate.complete();
    await tester.pump();
    await tester.pump();

    expect(service.stopCalls, 1);

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pump();
    expect(service.startCalls, 2);

    await tester.tap(find.byKey(const Key('pairing-scanner-cancel')));
    await tester.pumpAndSettle();
    expect(await result, isNull);
  });

  testWidgets('scanner guidance and cancel control expose semantics', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    final service = _FakePairingScannerService();
    final result = await _openScanner(tester, service);

    expect(find.bySemanticsLabel('將配對 QR 對準掃描框'), findsOneWidget);
    expect(find.byTooltip('取消掃描'), findsOneWidget);
    expect(
      tester.getSize(find.byKey(const Key('pairing-scanner-cancel'))).height,
      greaterThanOrEqualTo(44),
    );

    await tester.tap(find.byKey(const Key('pairing-scanner-cancel')));
    await tester.pumpAndSettle();
    expect(await result, isNull);
    semantics.dispose();
  });
}

Future<Future<String?>> _openScanner(
  WidgetTester tester,
  PairingScannerService service,
) async {
  final result = Completer<String?>();
  await tester.pumpWidget(
    MaterialApp(
      home: Builder(
        builder: (context) {
          return Scaffold(
            body: Center(
              child: FilledButton(
                key: const Key('open-scanner'),
                onPressed: () async {
                  final value = await Navigator.of(context).push<String>(
                    MaterialPageRoute(
                      builder: (_) => PairingScannerPage(service: service),
                    ),
                  );
                  if (!result.isCompleted) result.complete(value);
                },
                child: const Text('Open'),
              ),
            ),
          );
        },
      ),
    ),
  );
  await tester.tap(find.byKey(const Key('open-scanner')));
  await tester.pumpAndSettle();
  return result.future;
}

class _FakePairingScannerService implements PairingScannerService {
  _FakePairingScannerService({
    this.supported = true,
    this.starts = const [],
    this.firstStartGate,
  });

  final bool supported;
  final List<PairingScannerProblem?> starts;
  final Completer<void>? firstStartGate;
  final StreamController<PairingScannerEvent> _events =
      StreamController<PairingScannerEvent>.broadcast(sync: true);

  int startCalls = 0;
  int stopCalls = 0;
  int openAppSettingsCalls = 0;

  @override
  bool get isSupported => supported;

  @override
  Stream<PairingScannerEvent> get events => _events.stream;

  @override
  Widget buildPreview() => const ColoredBox(color: Colors.black);

  void emitPayload(String value) {
    _events.add(PairingScannerEvent.payload(value));
  }

  @override
  Future<void> start() async {
    final index = startCalls++;
    if (index == 0) await firstStartGate?.future;
    final problem = index < starts.length ? starts[index] : null;
    if (problem == null) {
      _events.add(const PairingScannerEvent.ready());
    } else {
      _events.add(PairingScannerEvent.problem(problem));
    }
  }

  @override
  Future<void> stop() async {
    stopCalls += 1;
  }

  @override
  Future<void> openAppSettings() async {
    openAppSettingsCalls += 1;
  }

  @override
  Future<void> dispose() async {
    await _events.close();
  }
}
