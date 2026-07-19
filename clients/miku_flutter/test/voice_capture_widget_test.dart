import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/asr/local_asr_engine.dart';
import 'package:miku_flutter/asr/local_asr_model.dart';
import 'package:miku_flutter/main.dart';
import 'package:miku_flutter/notification_service.dart';
import 'package:miku_flutter/session_client_stub.dart';
import 'package:miku_flutter/session_models.dart';
import 'package:miku_flutter/voice_capture_service.dart';

void main() {
  testWidgets('immutable platform PCM reaches editable explicit-send review', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final capture = _FakeVoiceCaptureService();
    final workers = _FakeWorkerFactory(text: 'draft from local voice');
    await tester.pumpWidget(
      MikuApp(client: client, voiceCapture: capture, localAsrWorkers: workers),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 120));

    expect(capture.recoverCalls, 1);
    await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
    await tester.pump();
    expect(capture.permissionCalls, 1);
    expect(capture.startCalls, 1);
    expect(capture.activeId, isNotNull);

    await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(capture.stopCalls, 1);
    expect(workers.spawnCalls, 1);
    expect(capture.lastCapturedPcm, everyElement(0));
    expect(find.text('Voice capture'), findsOneWidget);
    expect(find.text('Transcript draft'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('voiceCaptureQualityWarning')),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('voiceCaptureQualityWarning')),
        matching: find.text(
          'The recording was very short. Check the draft or record it again.',
        ),
      ),
      findsOneWidget,
    );
    expect(
      tester
          .widget<TextField>(find.byKey(const ValueKey('shareImportEditor')))
          .controller
          ?.text,
      'draft from local voice',
    );
    expect(
      find.byKey(const ValueKey('voiceCaptureDiagnostics')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('voiceCaptureDiagnosticsSummary')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('voiceCaptureId')), findsOneWidget);
    expect(
      find.textContaining('Capture ID ${capture.lastStoppedId}'),
      findsOneWidget,
    );
    expect(
      find.textContaining('Audio uses app-private temporary storage'),
      findsOneWidget,
    );
    expect(capture.inspectBuildCalls, 1);
    expect(find.byKey(const ValueKey('voiceBuildFingerprint')), findsOneWidget);
    expect(find.textContaining('1.0.2+3 · release'), findsOneWidget);
    expect(find.textContaining('APK SHA-256 0123456789abcdef'), findsOneWidget);
    expect((await client.listSessions()).single.messageCount, 0);
    expect(
      tester
          .widget<FilledButton>(find.byKey(const ValueKey('shareImportSend')))
          .onPressed,
      isNull,
    );

    await tester.enterText(
      find.byKey(const ValueKey('shareImportEditor')),
      'edited local transcript',
    );
    await tester.ensureVisible(find.text('Current chat'));
    await tester.pump();
    await tester.tap(find.text('Current chat'));
    await tester.pump();
    await tester.ensureVisible(find.byKey(const ValueKey('shareImportSend')));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('shareImportSend')));
    await tester.pump(const Duration(milliseconds: 650));

    expect((await client.listSessions()).single.messageCount, 2);
    expect(
      (await client.listSessions()).single.preview,
      'Miku heard: edited local transcript',
    );
  });

  testWidgets('build inspection failure does not block editable review', (
    tester,
  ) async {
    final capture = _FakeVoiceCaptureService(buildInspectionFails: true);
    final workers = _FakeWorkerFactory(text: 'still reviewable');
    await tester.pumpWidget(
      MikuApp(
        client: ScriptedMikuClient(),
        voiceCapture: capture,
        localAsrWorkers: workers,
      ),
    );
    await tester.pump(const Duration(milliseconds: 120));

    await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(capture.inspectBuildCalls, 1);
    expect(workers.spawnCalls, 1);
    expect(find.text('Transcript draft'), findsOneWidget);
    expect(
      find.text(
        'Build fingerprint unavailable. Transcription was not blocked.',
      ),
      findsOneWidget,
    );
    expect(
      tester
          .widget<TextField>(find.byKey(const ValueKey('shareImportEditor')))
          .controller
          ?.text,
      'still reviewable',
    );
  });

  testWidgets('reviewed voice draft can explicitly start a new chat', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final capture = _FakeVoiceCaptureService();
    final workers = _FakeWorkerFactory(text: 'voice draft for new chat');
    await tester.pumpWidget(
      MikuApp(client: client, voiceCapture: capture, localAsrWorkers: workers),
    );
    await tester.pump(const Duration(milliseconds: 120));

    await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    await tester.ensureVisible(find.text('New chat'));
    await tester.pump();
    await tester.tap(find.text('New chat'));
    await tester.pump();
    await tester.ensureVisible(find.byKey(const ValueKey('shareImportSend')));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('shareImportSend')));
    await tester.pump(const Duration(milliseconds: 650));

    final sessions = await client.listSessions();
    expect(sessions, hasLength(2));
    expect(
      sessions.any(
        (session) =>
            session.messageCount == 2 &&
            session.preview == 'Miku heard: voice draft for new chat',
      ),
      isTrue,
    );
  });

  testWidgets(
    'record cancellation and lifecycle exit never transcribe or send',
    (tester) async {
      final client = ScriptedMikuClient();
      final capture = _FakeVoiceCaptureService();
      final workers = _FakeWorkerFactory(text: 'must not appear');
      await tester.pumpWidget(
        MikuApp(
          client: client,
          voiceCapture: capture,
          localAsrWorkers: workers,
        ),
      );
      await tester.pump(const Duration(milliseconds: 120));

      await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('voiceCaptureCancel')));
      await tester.pump();
      expect(capture.cancelCalls, 1);
      expect(capture.activeId, isNull);
      expect(workers.spawnCalls, 0);
      expect(find.text('Voice capture'), findsNothing);

      await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
      await tester.pump();
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
      await tester.pump();
      expect(capture.cancelCalls, 2);
      expect(capture.activeId, isNull);
      expect(workers.spawnCalls, 0);
      expect((await client.listSessions()).single.messageCount, 0);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    },
  );

  testWidgets(
    'failed native cancellation stays retiring until an explicit retry succeeds',
    (tester) async {
      final client = ScriptedMikuClient();
      final capture = _FakeVoiceCaptureService(cancelSucceeds: false);
      final workers = _FakeWorkerFactory(text: 'must never be reviewed');
      await tester.pumpWidget(
        MikuApp(
          client: client,
          voiceCapture: capture,
          localAsrWorkers: workers,
        ),
      );
      await tester.pump(const Duration(milliseconds: 120));

      await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
      await tester.pump();
      final captureId = capture.activeId;
      expect(captureId, isNotNull);

      await tester.tap(find.byKey(const ValueKey('voiceCaptureCancel')));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 120));

      expect(capture.cancelCalls, 1);
      expect(capture.activeId, captureId);
      expect(find.byKey(const ValueKey('voiceCaptureCancel')), findsOneWidget);
      expect(
        tester
            .widget<IconButton>(
              find.byKey(const ValueKey('voiceCaptureAction')),
            )
            .onPressed,
        isNull,
      );
      expect(workers.spawnCalls, 0);
      expect((await client.listSessions()).single.messageCount, 0);

      capture.cancelSucceeds = true;
      await tester.tap(find.byKey(const ValueKey('voiceCaptureCancel')));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 120));

      expect(capture.cancelCalls, 2);
      expect(capture.activeId, isNull);
      expect(find.byKey(const ValueKey('voiceCaptureCancel')), findsNothing);
      expect(
        tester
            .widget<IconButton>(
              find.byKey(const ValueKey('voiceCaptureAction')),
            )
            .onPressed,
        isNotNull,
      );
      expect((await client.listSessions()).single.messageCount, 0);
    },
  );

  testWidgets('duplicate start taps are gated while permission is pending', (
    tester,
  ) async {
    final permission = Completer<bool>();
    final capture = _FakeVoiceCaptureService(permission: permission.future);
    final client = ScriptedMikuClient();
    await tester.pumpWidget(
      MikuApp(
        client: client,
        voiceCapture: capture,
        localAsrWorkers: _FakeWorkerFactory(text: 'not yet'),
      ),
    );
    await tester.pump(const Duration(milliseconds: 120));

    await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    final pendingButton = tester.widget<IconButton>(
      find.byKey(const ValueKey('voiceCaptureAction')),
    );
    expect(pendingButton.onPressed, isNull);
    await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
    expect(capture.permissionCalls, 1);
    expect(capture.startCalls, 0);

    permission.complete(true);
    await tester.pump();
    expect(capture.startCalls, 1);
    expect(capture.recoverCalls, 1);

    await tester.tap(find.byKey(const ValueKey('voiceCaptureCancel')));
    await tester.pump();
  });

  testWidgets('permission denial never starts, transcribes, or sends', (
    tester,
  ) async {
    final capture = _FakeVoiceCaptureService(permission: Future.value(false));
    final client = ScriptedMikuClient();
    final workers = _FakeWorkerFactory(text: 'must not appear');
    await tester.pumpWidget(
      MikuApp(client: client, voiceCapture: capture, localAsrWorkers: workers),
    );
    await tester.pump(const Duration(milliseconds: 120));

    await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));

    expect(capture.permissionCalls, 1);
    expect(capture.startCalls, 0);
    expect(capture.stopCalls, 0);
    expect(workers.spawnCalls, 0);
    expect(find.text('Microphone permission was not granted.'), findsOneWidget);
    expect((await client.listSessions()).single.messageCount, 0);
  });

  testWidgets(
    'temporary inactive state during permission prompt preserves the request',
    (tester) async {
      final permission = Completer<bool>();
      final capture = _FakeVoiceCaptureService(permission: permission.future);
      await tester.pumpWidget(
        MikuApp(
          client: ScriptedMikuClient(),
          voiceCapture: capture,
          localAsrWorkers: _FakeWorkerFactory(text: 'not transcribed yet'),
        ),
      );
      await tester.pump(const Duration(milliseconds: 120));

      await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
      await tester.pump();
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      await tester.pump();
      expect(capture.cancelCalls, 0);
      expect(capture.startCalls, 0);

      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      permission.complete(true);
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 120));
      expect(capture.startCalls, 1);
      expect(capture.activeId, isNotNull);

      await tester.tap(find.byKey(const ValueKey('voiceCaptureCancel')));
      await tester.pump();
      expect(capture.cancelCalls, 1);
    },
  );

  testWidgets('backgrounded permission result never starts recording', (
    tester,
  ) async {
    final permission = Completer<bool>();
    final capture = _FakeVoiceCaptureService(permission: permission.future);
    final client = ScriptedMikuClient();
    await tester.pumpWidget(
      MikuApp(
        client: client,
        voiceCapture: capture,
        localAsrWorkers: _FakeWorkerFactory(text: 'must not appear'),
      ),
    );
    await tester.pump(const Duration(milliseconds: 120));

    await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
    await tester.pump();
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
    permission.complete(true);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 120));

    expect(capture.startCalls, 0);
    expect(capture.stopCalls, 0);
    expect(capture.activeId, isNull);
    expect((await client.listSessions()).single.messageCount, 0);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
  });

  testWidgets('missing local model keeps microphone action disabled', (
    tester,
  ) async {
    final capture = _FakeVoiceCaptureService();
    await tester.pumpWidget(
      MikuApp(client: ScriptedMikuClient(), voiceCapture: capture),
    );
    await tester.pump(const Duration(milliseconds: 120));

    final button = tester.widget<IconButton>(
      find.byKey(const ValueKey('voiceCaptureAction')),
    );
    expect(button.onPressed, isNull);
    expect(capture.permissionCalls, 0);
    expect(capture.recoverCalls, 1);
  });

  testWidgets('unavailable home ASR cannot be selected', (tester) async {
    tester.view.physicalSize = const Size(800, 1200);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final client = _RemoteVoiceAsrClient(available: false);
    final capture = _FakeVoiceCaptureService();
    await tester.pumpWidget(MikuApp(client: client, voiceCapture: capture));
    await tester.pump(const Duration(milliseconds: 220));

    expect(
      tester
          .widget<IconButton>(find.byKey(const ValueKey('voiceCaptureAction')))
          .onPressed,
      isNull,
    );
    await _openVoiceAsrSettings(tester);

    final remoteTile = tester.widget<ListTile>(
      find.byKey(const ValueKey('selectSelfHostedVoiceAsr')),
    );
    expect(remoteTile.enabled, isFalse);
    expect(remoteTile.onTap, isNull);
    expect(find.text('Unavailable on the paired server'), findsOneWidget);
    expect(client.catalogCalls, greaterThanOrEqualTo(2));
    expect(client.transcribeCalls, 0);
  });

  testWidgets(
    'home ASR requires confirmation, needs no local model, and opens a sourced review',
    (tester) async {
      tester.view.physicalSize = const Size(800, 1200);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final client = _RemoteVoiceAsrClient(available: true);
      final capture = _FakeVoiceCaptureService();
      await tester.pumpWidget(MikuApp(client: client, voiceCapture: capture));
      await tester.pump(const Duration(milliseconds: 220));

      expect(
        tester
            .widget<IconButton>(
              find.byKey(const ValueKey('voiceCaptureAction')),
            )
            .onPressed,
        isNull,
      );
      await _openVoiceAsrSettings(tester);
      expect(find.text('Configured · tea-asr-1.1-mini'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('selectSelfHostedVoiceAsr')));
      await tester.pump();
      expect(
        find.byKey(const ValueKey('confirmSelfHostedVoiceAsr')),
        findsOneWidget,
      );
      expect(
        find.textContaining('There is no cloud or local fallback'),
        findsOneWidget,
      );
      await tester.tap(find.text('Cancel').last);
      await tester.pump();
      expect(
        tester
            .widget<IconButton>(
              find.byKey(const ValueKey('voiceCaptureAction')),
            )
            .onPressed,
        isNull,
      );

      await tester.tap(find.byKey(const ValueKey('selectSelfHostedVoiceAsr')));
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('confirmSelfHostedVoiceAsr')));
      await tester.pump(const Duration(milliseconds: 350));
      expect(
        tester
            .widget<IconButton>(
              find.byKey(const ValueKey('voiceCaptureAction')),
            )
            .onPressed,
        isNotNull,
      );

      await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 350));

      expect(client.transcribeCalls, 1);
      expect(client.lastEngineId, selfHostedVoiceAsrEngineId);
      expect(client.lastSampleRate, voiceAsrSampleRate);
      expect(client.receivedSnapshot, [0, 0, 1, 0]);
      expect(client.receivedBuffer, everyElement(0));
      expect(capture.lastCapturedPcm, everyElement(0));
      expect(find.text('Voice capture'), findsOneWidget);
      expect(
        find.text('Transcribed by your fixed self-hosted home service'),
        findsOneWidget,
      );
      expect(
        find.textContaining('sent through your paired TempestMiku server'),
        findsOneWidget,
      );
      expect((await client.listSessions()).single.messageCount, 0);
      expect(
        tester
            .widget<FilledButton>(find.byKey(const ValueKey('shareImportSend')))
            .onPressed,
        isNull,
      );
    },
  );

  testWidgets('home ASR failure has no local fallback and creates no review', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(800, 1200);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final client = _RemoteVoiceAsrClient(available: true, fail: true);
    final capture = _FakeVoiceCaptureService();
    await tester.pumpWidget(MikuApp(client: client, voiceCapture: capture));
    await tester.pump(const Duration(milliseconds: 220));
    await _selectRemoteVoiceAsr(tester);

    await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(client.transcribeCalls, 1);
    expect(find.text('Voice capture'), findsNothing);
    expect(find.textContaining('home ASR unavailable'), findsOneWidget);
    expect(capture.lastCapturedPcm, everyElement(0));
    expect((await client.listSessions()).single.messageCount, 0);
  });

  testWidgets('explicit cancel aborts active home ASR and wipes PCM', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(800, 1200);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final client = _PendingRemoteVoiceAsrClient();
    final capture = _FakeVoiceCaptureService();
    await tester.pumpWidget(MikuApp(client: client, voiceCapture: capture));
    await tester.pump(const Duration(milliseconds: 220));
    await _selectRemoteVoiceAsr(tester);

    await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
    await tester.pump();
    expect(client.transcribeCalls, 1);
    expect(find.byKey(const ValueKey('voiceCaptureCancel')), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('voiceCaptureCancel')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 120));

    expect(client.cancelRequests, 1);
    expect(client.receivedBuffer, everyElement(0));
    expect(capture.lastCapturedPcm, everyElement(0));
    expect(find.text('Voice capture'), findsNothing);
    expect((await client.listSessions()).single.messageCount, 0);
  });

  testWidgets('background and dispose abort active home ASR and wipe PCM', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(800, 1200);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    for (final dispose in [false, true]) {
      final client = _PendingRemoteVoiceAsrClient();
      final capture = _FakeVoiceCaptureService();
      await tester.pumpWidget(MikuApp(client: client, voiceCapture: capture));
      await tester.pump(const Duration(milliseconds: 220));
      await _selectRemoteVoiceAsr(tester);
      await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
      await tester.pump();
      expect(client.transcribeCalls, 1);

      if (dispose) {
        await tester.pumpWidget(const SizedBox.shrink());
      } else {
        tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
      }
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 120));

      expect(client.cancelRequests, 1);
      expect(client.receivedBuffer, everyElement(0));
      expect(capture.lastCapturedPcm, everyElement(0));
      expect((await client.listSessions()).single.messageCount, 0);
      if (!dispose) {
        tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
        tester.binding.handleAppLifecycleStateChanged(
          AppLifecycleState.inactive,
        );
        tester.binding.handleAppLifecycleStateChanged(
          AppLifecycleState.resumed,
        );
        await tester.pumpWidget(const SizedBox.shrink());
        await tester.pump();
      }
    }
  });

  testWidgets(
    'recording re-pair cancels before swapping authority and blocks a new mic start',
    (tester) async {
      tester.view.physicalSize = const Size(800, 1600);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final events = <String>[];
      final pairGate = Completer<void>();
      final client = _AuthorityRemoteVoiceAsrClient(
        eventLog: events,
        pairGate: pairGate,
      );
      final capture = _FakeVoiceCaptureService(events: events);
      final notifications = _FailingPermissionNotificationService();
      await tester.pumpWidget(
        MikuApp(
          client: client,
          notifications: notifications,
          voiceCapture: capture,
          localAsrWorkers: _FakeWorkerFactory(text: 'unused local result'),
        ),
      );
      await tester.pump(const Duration(milliseconds: 220));
      await _selectRemoteVoiceAsr(tester);

      await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
      await tester.pump();
      expect(capture.activeId, isNotNull);

      await _openConnectionAction(tester, 'Server target');
      await tester.tap(find.text('Scan QR'));
      await tester.pump(const Duration(milliseconds: 350));
      await tester.pump(const Duration(milliseconds: 350));
      expect(find.byType(PairingScannerPage), findsOneWidget);
      Navigator.of(
        tester.element(find.byType(PairingScannerPage)),
      ).pop(_replacementPairingLink);
      await tester.pump(const Duration(milliseconds: 350));
      expect(find.text('Pair with this server?'), findsOneWidget);
      await tester.tap(find.text('Pair securely'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 120));

      expect(capture.cancelCalls, 1);
      expect(capture.activeId, isNull);
      expect(client.transcribeCalls, 0);
      expect(client.pairCalls, 1);
      expect(
        events.indexOf('capture.cancel'),
        lessThan(events.indexOf('client.pair')),
      );
      final pendingMic = tester.widget<IconButton>(
        find.byKey(const ValueKey('voiceCaptureAction')),
      );
      expect(pendingMic.onPressed, isNull);
      await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
      expect(capture.permissionCalls, 1);
      expect(find.text('Voice capture'), findsNothing);
      await _openVoiceAsrSettings(tester);
      expect(
        find.byKey(const ValueKey('selectSelfHostedVoiceAsr')),
        findsNothing,
      );
      expect(
        find.byKey(const ValueKey('confirmSelfHostedVoiceAsr')),
        findsNothing,
      );

      pairGate.complete();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 650));

      expect(client.server, 'https://new-home.example');
      expect(notifications.permissionCalls, 1);
      expect(find.textContaining('Pairing link failed'), findsNothing);
      expect(
        tester
            .widget<IconButton>(
              find.byKey(const ValueKey('voiceCaptureAction')),
            )
            .onPressed,
        isNotNull,
      );
      expect((await client.listSessions()).single.messageCount, 0);
    },
  );

  testWidgets(
    'in-flight home ASR disconnect cancels before logout and drops a stale transcript',
    (tester) async {
      tester.view.physicalSize = const Size(800, 1600);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final events = <String>[];
      final logoutGate = Completer<void>();
      final client = _AuthorityRemoteVoiceAsrClient(
        eventLog: events,
        holdTranscription: true,
        completeTranscriptOnCancel: true,
        logoutGate: logoutGate,
      );
      final capture = _FakeVoiceCaptureService(events: events);
      await tester.pumpWidget(
        MikuApp(
          client: client,
          voiceCapture: capture,
          localAsrWorkers: _FakeWorkerFactory(text: 'unused local result'),
        ),
      );
      await tester.pump(const Duration(milliseconds: 220));
      await _selectRemoteVoiceAsr(tester);

      await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
      await tester.pump();
      expect(client.transcribeCalls, 1);

      await _openConnectionAction(tester, 'Disconnect');
      await tester.tap(find.widgetWithText(FilledButton, 'Disconnect'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 120));

      expect(client.cancelRequests, 1);
      expect(client.logoutCalls, 1);
      expect(
        events.indexOf('client.cancel-transcription'),
        lessThan(events.indexOf('client.logout')),
      );
      expect(client.receivedBuffer, everyElement(0));
      expect(capture.lastCapturedPcm, everyElement(0));
      expect(find.text('Voice capture'), findsNothing);
      expect(
        tester
            .widget<IconButton>(
              find.byKey(const ValueKey('voiceCaptureAction')),
            )
            .onPressed,
        isNull,
      );
      await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
      expect(capture.permissionCalls, 1);
      expect((await client.listSessions()).single.messageCount, 0);

      logoutGate.complete();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 350));
      expect(find.text('Voice capture'), findsNothing);
      expect(find.byKey(const ValueKey('voiceCaptureAction')), findsNothing);
      expect((await client.listSessions()).single.messageCount, 0);
    },
  );

  testWidgets('failed recorder cleanup blocks the server authority change', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(800, 1600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final client = _AuthorityRemoteVoiceAsrClient(eventLog: <String>[]);
    final capture = _FakeVoiceCaptureService(cancelSucceeds: false);
    await tester.pumpWidget(
      MikuApp(
        client: client,
        voiceCapture: capture,
        localAsrWorkers: _FakeWorkerFactory(text: 'unused local result'),
      ),
    );
    await tester.pump(const Duration(milliseconds: 220));
    await _selectRemoteVoiceAsr(tester);

    await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
    await tester.pump();
    final captureId = capture.activeId;
    expect(captureId, isNotNull);

    await _openConnectionAction(tester, 'Disconnect');
    await tester.tap(find.widgetWithText(FilledButton, 'Disconnect'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 120));

    expect(capture.cancelCalls, 1);
    expect(capture.activeId, captureId);
    expect(client.logoutCalls, 0);
    expect(client.server, 'https://old-home.example');
    expect(client.transcribeCalls, 0);
    expect(find.byKey(const ValueKey('voiceCaptureCancel')), findsOneWidget);
    expect(find.text('Voice capture'), findsNothing);
    expect((await client.listSessions()).single.messageCount, 0);

    capture.cancelSucceeds = true;
    await tester.tap(find.byKey(const ValueKey('voiceCaptureCancel')));
    await tester.pump(const Duration(milliseconds: 120));
    expect(capture.activeId, isNull);
  });

  testWidgets('model download requires explicit owner confirmation', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(800, 1200);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final models = _FakeModelManager();
    await tester.pumpWidget(
      MikuApp(
        client: ScriptedMikuClient(),
        voiceCapture: _FakeVoiceCaptureService(),
        localAsrModels: models,
      ),
    );
    await tester.pump(const Duration(milliseconds: 180));
    expect(models.inspectCalls, 1);
    expect(models.installCalls, 0);

    await tester.tap(find.byTooltip('Open menu'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    DefaultTabController.of(tester.element(find.byType(TabBar))).index = 2;
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 180));
    final voiceModelRow = find.text('On-device voice model');
    await tester.ensureVisible(voiceModelRow);
    await tester.pump();
    await tester.tap(
      find.ancestor(of: voiceModelRow, matching: find.byType(InkWell)).first,
    );
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.byKey(const ValueKey('installVoiceModel')), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('installVoiceModel')));
    await tester.pump();
    expect(models.installCalls, 0);
    expect(
      find.byKey(const ValueKey('confirmVoiceModelInstall')),
      findsOneWidget,
    );
    await tester.tap(find.text('Cancel').last);
    await tester.pump();
    expect(models.installCalls, 0);

    await tester.tap(find.byKey(const ValueKey('installVoiceModel')));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('confirmVoiceModelInstall')));
    await tester.pump(const Duration(milliseconds: 300));
    expect(models.installCalls, 1);
    expect(find.text('Installed and verified'), findsOneWidget);
    await tester.tap(find.text('Close'));
    await tester.pump();

    final button = tester.widget<IconButton>(
      find.byKey(const ValueKey('voiceCaptureAction')),
    );
    expect(button.onPressed, isNotNull);
  });

  testWidgets('deleting the model cancels an active native recording', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(800, 1200);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final capture = _FakeVoiceCaptureService();
    final models = _FakeModelManager(installed: true);
    await tester.pumpWidget(
      MikuApp(
        client: ScriptedMikuClient(),
        voiceCapture: capture,
        localAsrModels: models,
      ),
    );
    await tester.pump(const Duration(milliseconds: 180));

    await tester.tap(find.byKey(const ValueKey('voiceCaptureAction')));
    await tester.pump();
    expect(capture.activeId, isNotNull);

    await tester.tap(find.byTooltip('Open menu'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    DefaultTabController.of(tester.element(find.byType(TabBar))).index = 2;
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 180));
    final voiceModelRow = find.text('On-device voice model');
    await tester.ensureVisible(voiceModelRow);
    await tester.pump();
    await tester.tap(
      find.ancestor(of: voiceModelRow, matching: find.byType(InkWell)).first,
    );
    await tester.pump(const Duration(milliseconds: 400));
    await tester.tap(find.byKey(const ValueKey('deleteVoiceModel')));
    await tester.pump(const Duration(milliseconds: 300));

    expect(models.deleteCalls, 1);
    expect(capture.cancelCalls, 1);
    expect(capture.stopCalls, 0);
    expect(capture.activeId, isNull);
    expect(
      tester
          .widget<IconButton>(find.byKey(const ValueKey('voiceCaptureAction')))
          .onPressed,
      isNull,
    );
  });
}

final class _FakeVoiceCaptureService
    implements MikuVoiceCaptureService, MikuVoiceBuildInspector {
  _FakeVoiceCaptureService({
    this.permission,
    this.cancelSucceeds = true,
    this.buildInspectionFails = false,
    this.events,
  }) : fingerprint = VoiceAppBuildFingerprint(
         applicationId: 'org.mozufu.tempestmiku',
         versionName: '1.0.2',
         versionCode: 3,
         buildType: 'release',
         apkSha256:
             '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef',
       );

  final Future<bool>? permission;
  final bool buildInspectionFails;
  final List<String>? events;
  final VoiceAppBuildFingerprint fingerprint;
  bool cancelSucceeds;
  int recoverCalls = 0;
  int permissionCalls = 0;
  int startCalls = 0;
  int stopCalls = 0;
  int cancelCalls = 0;
  int inspectBuildCalls = 0;
  String? activeId;
  String? lastStoppedId;
  Uint8List? lastCapturedPcm;

  @override
  Future<VoiceAppBuildFingerprint> inspectBuild() async {
    inspectBuildCalls += 1;
    if (buildInspectionFails) throw StateError('inspection failed');
    return fingerprint;
  }

  @override
  bool get isSupported => true;

  @override
  Future<int> recoverOrphans() async {
    recoverCalls += 1;
    activeId = null;
    return 1;
  }

  @override
  Future<bool> requestPermission() async {
    permissionCalls += 1;
    return await permission ?? true;
  }

  @override
  Future<void> start(String captureId) async {
    if (activeId != null) throw StateError('duplicate capture');
    events?.add('capture.start');
    activeId = captureId;
    startCalls += 1;
  }

  @override
  Future<CapturedVoicePcm> stop(String captureId) async {
    if (activeId != captureId) throw StateError('capture mismatch');
    events?.add('capture.stop');
    activeId = null;
    lastStoppedId = captureId;
    stopCalls += 1;
    final captured = CapturedVoicePcm.fromPlatform({
      'captureId': captureId,
      'sampleRate': localAsrSampleRate,
      'pcm16': Uint8List.fromList([0, 0, 1, 0]).asUnmodifiableView(),
    });
    lastCapturedPcm = captured.pcm16;
    return captured;
  }

  @override
  Future<bool> cancel(String? captureId) async {
    events?.add('capture.cancel');
    cancelCalls += 1;
    if (activeId == null) return false;
    if (captureId != null && activeId != captureId) return false;
    if (!cancelSucceeds) return false;
    activeId = null;
    return true;
  }
}

Future<void> _openVoiceAsrSettings(WidgetTester tester) async {
  await tester.tap(find.byTooltip('Open menu'));
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 350));
  DefaultTabController.of(tester.element(find.byType(TabBar))).index = 2;
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 180));
  final voiceAsrRow = find.text('Voice recognition');
  await tester.ensureVisible(voiceAsrRow);
  await tester.pump();
  await tester.tap(
    find.ancestor(of: voiceAsrRow, matching: find.byType(InkWell)).first,
  );
  await tester.pump(const Duration(milliseconds: 400));
}

Future<void> _selectRemoteVoiceAsr(WidgetTester tester) async {
  await _openVoiceAsrSettings(tester);
  await tester.tap(find.byKey(const ValueKey('selectSelfHostedVoiceAsr')));
  await tester.pump();
  await tester.tap(find.byKey(const ValueKey('confirmSelfHostedVoiceAsr')));
  await tester.pump(const Duration(milliseconds: 350));
}

Future<void> _openConnectionAction(WidgetTester tester, String label) async {
  await tester.tap(find.byTooltip('Open menu'));
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 350));
  DefaultTabController.of(tester.element(find.byType(TabBar))).index = 2;
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 180));
  final action = find.text(label);
  final drawerScrollables = find.descendant(
    of: find.byType(Drawer),
    matching: find.byType(Scrollable),
  );
  await tester.scrollUntilVisible(
    action,
    220,
    scrollable: drawerScrollables.last,
    maxScrolls: 12,
  );
  await tester.tap(
    find.ancestor(of: action, matching: find.byType(InkWell)).first,
  );
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 400));
}

const _replacementPairingLink =
    'tempestmiku://pair?v=1&server=https%3A%2F%2Fnew-home.example&code='
    '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef';

class _RemoteVoiceAsrClient extends ScriptedMikuClient {
  _RemoteVoiceAsrClient({required this.available, this.fail = false});

  final bool available;
  final bool fail;
  int catalogCalls = 0;
  int transcribeCalls = 0;
  String? lastEngineId;
  int? lastSampleRate;
  Uint8List? receivedBuffer;
  List<int>? receivedSnapshot;

  @override
  Future<VoiceAsrEngineCatalog> voiceAsrEngines() async {
    catalogCalls += 1;
    return VoiceAsrEngineCatalog.fromJson({
      'engines': [
        const {
          'id': 'local',
          'kind': 'local',
          'label': 'On-device',
          'available': true,
          'maxDurationSeconds': 60,
        },
        {
          'id': 'self_hosted',
          'kind': 'remote',
          'label': 'Home remote',
          'available': available,
          'modelId': 'tea-asr-1.1-mini',
          'maxDurationSeconds': 60,
        },
      ],
    });
  }

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
    lastSampleRate = sampleRate;
    receivedBuffer = pcm16;
    receivedSnapshot = List<int>.from(pcm16);
    if (fail) throw StateError('home ASR unavailable');
    return const VoiceAsrTranscript(
      text: '幫我記得今天晚上九點要倒垃圾',
      engineId: selfHostedVoiceAsrEngineId,
      modelId: 'tea-asr-1.1-mini',
    );
  }
}

final class _PendingRemoteVoiceAsrClient extends _RemoteVoiceAsrClient {
  _PendingRemoteVoiceAsrClient() : super(available: true);

  Completer<VoiceAsrTranscript>? _pending;
  int cancelRequests = 0;

  @override
  Future<VoiceAsrTranscript> transcribeVoicePcm16({
    required String engineId,
    required String captureId,
    required int sampleRate,
    required Uint8List pcm16,
  }) {
    validateVoiceAsrPcm16Request(
      engineId: engineId,
      captureId: captureId,
      sampleRate: sampleRate,
      pcm16: pcm16,
    );
    transcribeCalls += 1;
    lastEngineId = engineId;
    lastSampleRate = sampleRate;
    receivedBuffer = pcm16;
    receivedSnapshot = List<int>.from(pcm16);
    final pending = Completer<VoiceAsrTranscript>();
    _pending = pending;
    return pending.future;
  }

  @override
  Future<void> cancelVoiceAsrTranscription() async {
    cancelRequests += 1;
    final pending = _pending;
    if (pending != null && !pending.isCompleted) {
      pending.completeError(StateError('home ASR request aborted'));
    }
  }
}

final class _AuthorityRemoteVoiceAsrClient extends _RemoteVoiceAsrClient
    implements ServerTargetClient {
  _AuthorityRemoteVoiceAsrClient({
    required this.eventLog,
    this.pairGate,
    this.logoutGate,
    this.holdTranscription = false,
    this.completeTranscriptOnCancel = false,
  }) : super(available: true);

  final List<String> eventLog;
  final Completer<void>? pairGate;
  final Completer<void>? logoutGate;
  final bool holdTranscription;
  final bool completeTranscriptOnCancel;
  Completer<VoiceAsrTranscript>? _pendingTranscript;
  String server = 'https://old-home.example';
  int pairCalls = 0;
  int logoutCalls = 0;
  int cancelRequests = 0;

  @override
  Future<VoiceAsrTranscript> transcribeVoicePcm16({
    required String engineId,
    required String captureId,
    required int sampleRate,
    required Uint8List pcm16,
  }) {
    if (!holdTranscription) {
      return super.transcribeVoicePcm16(
        engineId: engineId,
        captureId: captureId,
        sampleRate: sampleRate,
        pcm16: pcm16,
      );
    }
    validateVoiceAsrPcm16Request(
      engineId: engineId,
      captureId: captureId,
      sampleRate: sampleRate,
      pcm16: pcm16,
    );
    transcribeCalls += 1;
    lastEngineId = engineId;
    lastSampleRate = sampleRate;
    receivedBuffer = pcm16;
    receivedSnapshot = List<int>.from(pcm16);
    final pending = Completer<VoiceAsrTranscript>();
    _pendingTranscript = pending;
    return pending.future;
  }

  @override
  Future<void> cancelVoiceAsrTranscription() async {
    eventLog.add('client.cancel-transcription');
    cancelRequests += 1;
    final pending = _pendingTranscript;
    if (pending == null || pending.isCompleted) return;
    if (completeTranscriptOnCancel) {
      pending.complete(
        const VoiceAsrTranscript(
          text: 'stale transcript that must not open review',
          engineId: selfHostedVoiceAsrEngineId,
          modelId: 'tea-asr-1.1-mini',
        ),
      );
    } else {
      pending.completeError(StateError('home ASR request aborted'));
    }
  }

  @override
  String pairingDeviceName() => 'TempestMiku widget test';

  @override
  Future<String> serverBaseUrl() async => server;

  @override
  Future<void> setServerBaseUrl(String baseUrl) async {
    server = baseUrl;
  }

  @override
  Future<void> pairWithCode(MikuPairingTarget target) async {
    pairCalls += 1;
    eventLog.add('client.pair');
    await pairGate?.future;
    server = target.serverBaseUrl;
  }

  @override
  Future<void> logout() async {
    logoutCalls += 1;
    eventLog.add('client.logout');
    await logoutGate?.future;
  }
}

final class _FailingPermissionNotificationService
    implements MikuNotificationService {
  final _actions = StreamController<ApprovalNotificationAction>.broadcast(
    sync: true,
  );
  int permissionCalls = 0;

  @override
  Stream<ApprovalNotificationAction> get actions => _actions.stream;

  @override
  bool get isSupported => true;

  @override
  Future<void> initialize() async {}

  @override
  Future<bool> requestPermission() async {
    permissionCalls += 1;
    throw StateError('notification permission unavailable');
  }

  @override
  Future<void> showApproval({
    required String sessionId,
    required String approvalId,
    required String action,
    String? expiresAt,
  }) async {}

  @override
  Future<void> cancelApproval(String approvalId) async {}
}

final class _FakeWorkerFactory implements LocalAsrWorkerFactory {
  _FakeWorkerFactory({required this.text});

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
  String get modelId => 'deterministic-fake';

  @override
  Future<Duration> load() async => Duration.zero;

  @override
  Future<LocalAsrTranscript> transcribe(LocalAsrAudio audio) async =>
      LocalAsrTranscript(text: text, inferenceDuration: Duration.zero);

  @override
  Future<void> kill() async {}

  @override
  Future<void> close() async {}
}

final class _FakeModelManager implements LocalAsrModelManager {
  _FakeModelManager({this.installed = false});

  int inspectCalls = 0;
  int installCalls = 0;
  int deleteCalls = 0;
  bool installed;

  @override
  bool get isSupported => true;

  LocalAsrModelStatus get _status =>
      installed
          ? const LocalAsrModelStatus(
            state: LocalAsrModelState.ready,
            reason: 'verified',
            encoder: '/private/encoder.int8.onnx',
            decoder: '/private/decoder.int8.onnx',
            tokens: '/private/tokens.txt',
          )
          : const LocalAsrModelStatus(
            state: LocalAsrModelState.missing,
            reason: 'not installed',
          );

  @override
  Future<LocalAsrModelStatus> inspect() async {
    inspectCalls += 1;
    return _status;
  }

  @override
  Future<LocalAsrModelStatus> install() async {
    installCalls += 1;
    installed = true;
    return _status;
  }

  @override
  Future<LocalAsrModelStatus> delete() async {
    deleteCalls += 1;
    installed = false;
    return _status;
  }

  @override
  Future<LocalAsrWorker> spawn({
    LocalAsrCancellationToken? cancellation,
  }) async {
    cancellation?.throwIfCancelled();
    return _FakeWorker('local model');
  }
}
