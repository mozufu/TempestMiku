import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/asr/local_asr_engine.dart';
import 'package:miku_flutter/asr/local_asr_model.dart';
import 'package:miku_flutter/conversation_app.dart';
import 'package:miku_flutter/session_client_stub.dart';
import 'package:miku_flutter/session_models.dart';
import 'package:miku_flutter/voice_capture_service.dart';

void main() {
  Future<void> loadApp(
    WidgetTester tester, {
    required ScriptedMikuClient client,
    required _FakeVoiceCapture capture,
    LocalAsrWorkerFactory? workers,
    LocalAsrModelManager? models,
  }) async {
    await tester.pumpWidget(
      TempestMikuApp(
        client: client,
        themeMode: ThemeMode.light,
        voiceCapture: capture,
        localAsrWorkers: workers,
        localAsrModels: models,
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 150));
  }

  testWidgets(
    'local capture is wiped and enters editable explicit-send review without replacing the composer draft',
    (tester) async {
      tester.view.physicalSize = const Size(800, 1100);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final client = ScriptedMikuClient();
      final capture = _FakeVoiceCapture();
      final workers = _FakeWorkerFactory('本機語音草稿');
      await loadApp(tester, client: client, capture: capture, workers: workers);

      await tester.enterText(
        find.byKey(const Key('conversation-composer')),
        '原本輸入框草稿',
      );
      await tester.tap(find.byKey(const Key('voice-capture-action')));
      await tester.pump();
      expect(capture.permissionCalls, 1);
      expect(capture.activeId, isNotNull);
      expect(find.textContaining('錄音中'), findsOneWidget);

      await tester.tap(find.byKey(const Key('voice-capture-action')));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));

      expect(capture.stopCalls, 1);
      expect(workers.spawnCalls, 1);
      expect(capture.lastCapturedPcm, everyElement(0));
      expect(find.byKey(const Key('import-review-sheet')), findsOneWidget);
      expect(find.text('語音轉寫草稿'), findsOneWidget);
      expect(find.byKey(const Key('voice-quality-warning')), findsOneWidget);
      expect(find.byKey(const Key('voice-diagnostics')), findsOneWidget);
      expect(find.text('本機裝置端辨識 · 原始音訊已清除'), findsOneWidget);
      expect(
        tester
            .widget<TextField>(find.byKey(const Key('conversation-composer')))
            .controller
            ?.text,
        '原本輸入框草稿',
      );
      expect((await client.listSessions()).single.messageCount, 0);
      expect(
        tester
            .widget<FilledButton>(find.byKey(const Key('send-import')))
            .onPressed,
        isNull,
      );

      await tester.enterText(
        find.byKey(const Key('import-review-editor')),
        '編輯後的本機語音草稿',
      );
      await tester.tap(find.text('目前對話'));
      await tester.pump();
      await tester.tap(find.byKey(const Key('send-import')));
      await tester.pump(const Duration(milliseconds: 650));

      expect((await client.listSessions()).single.messageCount, 2);
      expect(
        tester
            .widget<TextField>(find.byKey(const Key('conversation-composer')))
            .controller
            ?.text,
        '原本輸入框草稿',
      );
    },
  );

  testWidgets('inactive lifecycle explains why voice capture cannot start', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final capture = _FakeVoiceCapture();
    final workers = _FakeWorkerFactory('不應出現');
    await loadApp(tester, client: client, capture: capture, workers: workers);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    await tester.pump();

    tester
        .widget<IconButton>(find.byKey(const Key('voice-capture-action')))
        .onPressed!();
    await tester.pump();

    expect(find.byKey(const Key('voice-composer-status')), findsOneWidget);
    expect(capture.permissionCalls, 0);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
  });

  testWidgets('lifecycle exit cancels capture and never transcribes or sends', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final capture = _FakeVoiceCapture();
    final workers = _FakeWorkerFactory('不應出現');
    await loadApp(tester, client: client, capture: capture, workers: workers);

    await tester.tap(find.byKey(const Key('voice-capture-action')));
    await tester.pump();
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
    await tester.pump(const Duration(milliseconds: 100));

    expect(capture.cancelCalls, 1);
    expect(capture.activeId, isNull);
    expect(workers.spawnCalls, 0);
    expect(find.byKey(const Key('import-review-sheet')), findsNothing);
    expect((await client.listSessions()).single.messageCount, 0);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
  });

  testWidgets(
    'self-hosted selection requires disclosure and a failed request never falls back locally',
    (tester) async {
      tester.view.physicalSize = const Size(800, 1200);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final client = _RemoteVoiceClient(fail: true);
      final capture = _FakeVoiceCapture();
      await loadApp(tester, client: client, capture: capture);

      final initialMic = tester.widget<IconButton>(
        find.byKey(const Key('voice-capture-action')),
      );
      expect(initialMic.onPressed, isNull);
      await _openSettings(tester);
      await _scrollSettingsTo(
        tester,
        find.byKey(const Key('select-self-hosted-voice-asr')),
      );
      await tester.tap(find.byKey(const Key('select-self-hosted-voice-asr')));
      await tester.pump();
      expect(find.textContaining('不會在失敗時自動退回本機'), findsOneWidget);
      await tester.tap(find.byKey(const Key('confirm-self-hosted-voice-asr')));
      await tester.pump();
      expect(
        find.byKey(const Key('self-hosted-voice-disclosure')),
        findsOneWidget,
      );
      Navigator.of(
        tester.element(find.byKey(const Key('settings-sheet'))),
      ).pop();
      await tester.pumpAndSettle();

      expect(
        tester
            .widget<IconButton>(find.byKey(const Key('voice-capture-action')))
            .onPressed,
        isNotNull,
      );
      await tester.tap(find.byKey(const Key('voice-capture-action')));
      await tester.pump();
      await tester.tap(find.byKey(const Key('voice-capture-action')));
      await tester.pump(const Duration(milliseconds: 300));

      expect(client.transcribeCalls, 1);
      expect(client.lastEngineId, selfHostedVoiceAsrEngineId);
      expect(capture.lastCapturedPcm, everyElement(0));
      expect(find.byKey(const Key('import-review-sheet')), findsNothing);
      expect(find.textContaining('home ASR unavailable'), findsWidgets);
      expect((await client.listSessions()).single.messageCount, 0);
    },
  );

  testWidgets(
    'model install is explicit and enables the local microphone only after verification',
    (tester) async {
      tester.view.physicalSize = const Size(800, 1200);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final models = _FakeModelManager();
      await loadApp(
        tester,
        client: ScriptedMikuClient(),
        capture: _FakeVoiceCapture(),
        models: models,
      );
      await _openSettings(tester);
      await _scrollSettingsTo(
        tester,
        find.byKey(const Key('install-voice-model')),
      );

      await tester.tap(find.byKey(const Key('install-voice-model')));
      await tester.pump();
      expect(models.installCalls, 0);
      expect(find.textContaining('固定 commit'), findsOneWidget);
      await tester.tap(find.widgetWithText(TextButton, '取消').last);
      await tester.pump();
      expect(models.installCalls, 0);

      await tester.tap(find.byKey(const Key('install-voice-model')));
      await tester.pump();
      await tester.tap(find.byKey(const Key('confirm-install-voice-model')));
      await tester.pump(const Duration(milliseconds: 250));
      expect(models.installCalls, 1);
      expect(find.text('已安裝並驗證'), findsOneWidget);
      Navigator.of(
        tester.element(find.byKey(const Key('settings-sheet'))),
      ).pop();
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<IconButton>(find.byKey(const Key('voice-capture-action')))
            .onPressed,
        isNotNull,
      );
    },
  );

  testWidgets('deleting the verified model first cancels active recording', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(800, 1200);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final capture = _FakeVoiceCapture();
    final models = _FakeModelManager(installed: true);
    await loadApp(
      tester,
      client: ScriptedMikuClient(),
      capture: capture,
      models: models,
    );
    await tester.tap(find.byKey(const Key('voice-capture-action')));
    await tester.pump();
    expect(capture.activeId, isNotNull);

    await _openSettings(tester);
    await _scrollSettingsTo(
      tester,
      find.byKey(const Key('delete-voice-model')),
    );
    await tester.tap(find.byKey(const Key('delete-voice-model')));
    await tester.pump();
    await tester.tap(find.byKey(const Key('confirm-delete-voice-model')));
    await tester.pump(const Duration(milliseconds: 250));

    expect(capture.cancelCalls, 1);
    expect(capture.activeId, isNull);
    expect(models.deleteCalls, 1);
    expect(find.text('尚未安裝'), findsOneWidget);
  });

  testWidgets('failed recorder cleanup blocks logout authority mutation', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(800, 1200);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final client = ScriptedMikuClient();
    final capture = _FakeVoiceCapture(cancelSucceeds: false);
    await loadApp(
      tester,
      client: client,
      capture: capture,
      workers: _FakeWorkerFactory('不應出現'),
    );
    await tester.tap(find.byKey(const Key('voice-capture-action')));
    await tester.pump();
    expect(capture.activeId, isNotNull);

    await _openSettings(tester);
    await _scrollSettingsTo(tester, find.byKey(const Key('logout-device')));
    await tester.tap(find.byKey(const Key('logout-device')));
    await tester.pump();
    await tester.tap(find.byKey(const Key('confirm-logout')));
    await tester.pump(const Duration(milliseconds: 150));

    expect(capture.cancelCalls, 1);
    expect(client.logoutCount, 0);
    expect(find.byKey(const Key('settings-sheet')), findsOneWidget);
    expect(find.textContaining('因此沒有登出'), findsOneWidget);

    // Let widget disposal retire the recorder after the assertion that the
    // authority mutation was blocked.
    capture.cancelSucceeds = true;
  });
}

Future<void> _openSettings(WidgetTester tester) async {
  await tester.tap(find.byKey(const Key('open-left-drawer')));
  await tester.pumpAndSettle();
  await tester.tap(find.byKey(const Key('drawer-settings')));
  await tester.pumpAndSettle();
}

Future<void> _scrollSettingsTo(WidgetTester tester, Finder target) async {
  await tester.scrollUntilVisible(
    target,
    180,
    scrollable:
        find
            .descendant(
              of: find.byKey(const Key('settings-sheet')),
              matching: find.byType(Scrollable),
            )
            .first,
  );
  await tester.pump();
}

final class _FakeVoiceCapture
    implements MikuVoiceCaptureService, MikuVoiceBuildInspector {
  _FakeVoiceCapture({this.cancelSucceeds = true});

  bool cancelSucceeds;
  int permissionCalls = 0;
  int startCalls = 0;
  int stopCalls = 0;
  int cancelCalls = 0;
  String? activeId;
  Uint8List? lastCapturedPcm;

  @override
  bool get isSupported => true;

  @override
  Future<int> recoverOrphans() async => 0;

  @override
  Future<bool> requestPermission() async {
    permissionCalls += 1;
    return true;
  }

  @override
  Future<void> start(String captureId) async {
    activeId = captureId;
    startCalls += 1;
  }

  @override
  Future<CapturedVoicePcm> stop(String captureId) async {
    if (activeId != captureId) throw StateError('capture mismatch');
    activeId = null;
    stopCalls += 1;
    final captured = CapturedVoicePcm.fromPlatform({
      'captureId': captureId,
      'sampleRate': localAsrSampleRate,
      'pcm16': Uint8List.fromList([0, 0, 1, 0]),
    });
    lastCapturedPcm = captured.pcm16;
    return captured;
  }

  @override
  Future<bool> cancel(String? captureId) async {
    cancelCalls += 1;
    if (activeId == null) return false;
    if (captureId != null && captureId != activeId) return false;
    if (!cancelSucceeds) return false;
    activeId = null;
    return true;
  }

  @override
  Future<VoiceAppBuildFingerprint> inspectBuild() async =>
      VoiceAppBuildFingerprint(
        applicationId: 'org.mozufu.tempestmiku',
        versionName: '1.0.3',
        versionCode: 4,
        buildType: 'release',
        apkSha256:
            '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef',
      );
}

final class _FakeWorkerFactory implements LocalAsrWorkerFactory {
  _FakeWorkerFactory(this.text);

  final String text;
  int spawnCalls = 0;

  @override
  Future<LocalAsrWorker> spawn({
    LocalAsrCancellationToken? cancellation,
  }) async {
    cancellation?.throwIfCancelled();
    spawnCalls += 1;
    return _FakeWorker(text);
  }
}

final class _FakeWorker implements LocalAsrWorker {
  _FakeWorker(this.text);

  final String text;

  @override
  String get modelId => 'deterministic-test';

  @override
  Future<Duration> load() async => Duration.zero;

  @override
  Future<LocalAsrTranscript> transcribe(LocalAsrAudio audio) async =>
      LocalAsrTranscript(text: text, inferenceDuration: Duration.zero);

  @override
  Future<void> close() async {}

  @override
  Future<void> kill() async {}
}

final class _RemoteVoiceClient extends ScriptedMikuClient {
  _RemoteVoiceClient({required this.fail});

  final bool fail;
  int transcribeCalls = 0;
  String? lastEngineId;

  @override
  Future<VoiceAsrEngineCatalog> voiceAsrEngines() async =>
      VoiceAsrEngineCatalog.fromJson({
        'engines': const [
          {
            'id': localVoiceAsrEngineId,
            'kind': 'local',
            'label': 'On-device',
            'available': true,
            'maxDurationSeconds': 60,
          },
          {
            'id': selfHostedVoiceAsrEngineId,
            'kind': 'remote',
            'label': 'Home remote',
            'available': true,
            'modelId': 'tea-asr-1.1-mini',
            'maxDurationSeconds': 60,
          },
        ],
      });

  @override
  Future<VoiceAsrTranscript> transcribeVoicePcm16({
    required String engineId,
    required String captureId,
    required int sampleRate,
    required Uint8List pcm16,
  }) async {
    validateVoiceAsrPcm16Request(
      engineId: engineId,
      captureId: captureId,
      sampleRate: sampleRate,
      pcm16: pcm16,
    );
    transcribeCalls += 1;
    lastEngineId = engineId;
    if (fail) throw StateError('home ASR unavailable');
    return const VoiceAsrTranscript(
      text: '自架轉寫草稿',
      engineId: selfHostedVoiceAsrEngineId,
      modelId: 'tea-asr-1.1-mini',
    );
  }
}

final class _FakeModelManager implements LocalAsrModelManager {
  _FakeModelManager({this.installed = false});

  bool installed;
  int installCalls = 0;
  int deleteCalls = 0;

  @override
  bool get isSupported => true;

  LocalAsrModelStatus get status =>
      installed
          ? const LocalAsrModelStatus(
            state: LocalAsrModelState.ready,
            reason: 'verified',
            encoder: '/private/encoder.onnx',
            decoder: '/private/decoder.onnx',
            tokens: '/private/tokens.txt',
          )
          : const LocalAsrModelStatus(
            state: LocalAsrModelState.missing,
            reason: 'not installed',
          );

  @override
  Future<LocalAsrModelStatus> inspect() async => status;

  @override
  Future<LocalAsrModelStatus> install() async {
    installCalls += 1;
    installed = true;
    return status;
  }

  @override
  Future<LocalAsrModelStatus> delete() async {
    deleteCalls += 1;
    installed = false;
    return status;
  }

  @override
  Future<LocalAsrWorker> spawn({
    LocalAsrCancellationToken? cancellation,
  }) async {
    cancellation?.throwIfCancelled();
    return _FakeWorker('已驗證模型草稿');
  }
}
