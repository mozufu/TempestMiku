import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:miku_flutter/asr/local_asr_engine.dart';
import 'package:miku_flutter/main.dart';
import 'package:miku_flutter/session_client_stub.dart';
import 'package:miku_flutter/share_import_service.dart';
import 'package:miku_flutter/voice_capture_service.dart';

class RecordingShareImportService implements MikuShareImportService {
  final controller = StreamController<SharedContent>.broadcast(sync: true);

  @override
  bool get isSupported => true;

  @override
  Stream<SharedContent> get imports => controller.stream;

  void close() {
    unawaited(controller.close());
  }
}

class ColdStartShareImportService implements MikuShareImportService {
  const ColdStartShareImportService(this.contents);

  final List<SharedContent> contents;

  @override
  bool get isSupported => true;

  @override
  Stream<SharedContent> get imports => Stream.fromIterable(contents);
}

void main() {
  test('shared content parser sanitizes, combines, and bounds input', () {
    final parsed = SharedContent.fromEvent({
      'text': ' https://example.test/\u0000path ',
      'subject': ' Example title\u0007 ',
      'truncated': false,
    });
    expect(parsed.text, 'Example title\n\nhttps://example.test/path');
    expect(parsed.subject, 'Example title');
    expect(parsed.truncated, isFalse);
    expect(parsed.source, SharedContentSource.share);

    final selection = SharedContent.fromEvent({
      'text': ' explain this ',
      'source': 'selection',
      'subject': 'ignored share subject',
    });
    expect(selection.text, 'explain this');
    expect(selection.subject, isNull);
    expect(selection.source, SharedContentSource.selection);

    final capture = SharedContent.fromEvent({
      'text': '',
      'source': 'quick_capture',
      'eventId': '12345678-1234-4abc-8def-1234567890ab',
    });
    expect(capture.text, isEmpty);
    expect(capture.subject, isNull);
    expect(capture.source, SharedContentSource.quickCapture);
    expect(capture.eventId, '12345678-1234-4abc-8def-1234567890ab');

    final voice = SharedContent.fromEvent({
      'text': 'local draft',
      'source': 'voice',
      'eventId': '12345678-1234-4abc-8def-1234567890ab',
      'voiceQualityIssue': 'tooQuiet',
      'voiceTranscriptProvenance': 'self_hosted',
      'voiceDiagnostics': const VoiceCaptureDiagnostics(
        duration: Duration(seconds: 2),
        rmsDbfs: -24,
        peakDbfs: -3,
        clippedFraction: 0,
        nearZeroFraction: 0.1,
        activeFrameFraction: 0.8,
        leadingSilence: Duration(milliseconds: 100),
        trailingSilence: Duration(milliseconds: 200),
      ),
      'voiceBuildFingerprint': VoiceAppBuildFingerprint(
        applicationId: 'org.mozufu.tempestmiku',
        versionName: '1.0.2',
        versionCode: 3,
        buildType: 'release',
        apkSha256:
            '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef',
      ),
    });
    expect(voice.voiceQualityIssue, VoiceCaptureQualityIssue.tooQuiet);
    expect(voice.voiceDiagnostics?.duration, const Duration(seconds: 2));
    expect(voice.voiceBuildFingerprint?.versionCode, 3);
    expect(
      voice.voiceTranscriptProvenance,
      VoiceTranscriptProvenance.selfHosted,
    );

    final bounded = SharedContent.fromEvent({
      'text': 'x' * maxSharedTextLength,
      'subject': 'title',
      'truncated': false,
    });
    expect(bounded.text.length, maxSharedTextLength);
    expect(bounded.truncated, isTrue);

    final emojiBoundary = SharedContent.fromEvent({
      'text': '${'x' * (maxSharedTextLength - 1)}😀',
    });
    expect(emojiBoundary.text.length, maxSharedTextLength - 1);
    expect(emojiBoundary.truncated, isTrue);

    expect(
      () => SharedContent.fromEvent({'text': ' \u0000 '}),
      throwsFormatException,
    );
    expect(
      () => SharedContent.fromEvent({'text': 'x', 'source': 'unknown'}),
      throwsFormatException,
    );
    expect(
      () => SharedContent.fromEvent({
        'text': 'x',
        'voiceQualityIssue': 'clipped',
      }),
      throwsFormatException,
    );
    expect(
      () => SharedContent.fromEvent({
        'text': 'x',
        'source': 'voice',
        'eventId': '12345678-1234-4abc-8def-1234567890ab',
        'voiceTranscriptProvenance': 'cloud',
      }),
      throwsFormatException,
    );
    expect(
      () => SharedContent.fromEvent({
        'text': 'x',
        'voiceBuildFingerprint': VoiceAppBuildFingerprint(
          applicationId: 'org.mozufu.tempestmiku',
          versionName: '1.0.2',
          versionCode: 3,
          buildType: 'release',
          apkSha256:
              '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef',
        ),
      }),
      throwsFormatException,
    );
    expect(
      () => SharedContent.fromEvent({
        'text': 'x',
        'voiceDiagnostics': VoiceCaptureDiagnostics.fromPcm16(
          Uint8List.fromList([0, 0]),
          sampleRate: localAsrSampleRate,
        ),
      }),
      throwsFormatException,
    );
    expect(
      () => SharedContent.fromEvent({'text': '', 'source': 'quick_capture'}),
      throwsFormatException,
    );
  });

  testWidgets(
    'empty quick capture requires review and cancellation sends nothing',
    (tester) async {
      final client = ScriptedMikuClient();
      final shares = RecordingShareImportService();
      addTearDown(shares.close);
      await tester.pumpWidget(MikuApp(client: client, shareImports: shares));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));

      shares.controller.add(
        const SharedContent(
          text: '',
          source: SharedContentSource.quickCapture,
          eventId: '10000000-0000-4000-8000-000000000001',
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 350));

      expect(find.text('Quick capture'), findsOneWidget);
      expect(find.text('Capture draft'), findsOneWidget);
      expect((await client.listSessions()).single.messageCount, 0);
      expect(
        tester
            .widget<FilledButton>(find.byKey(const ValueKey('shareImportSend')))
            .onPressed,
        isNull,
      );

      await tester.tap(find.text('Cancel'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 350));
      expect(find.text('Quick capture'), findsNothing);
      expect((await client.listSessions()).single.messageCount, 0);
    },
  );

  testWidgets('warm quick capture replaces the draft and deduplicates its id', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final shares = RecordingShareImportService();
    addTearDown(shares.close);
    await tester.pumpWidget(MikuApp(client: client, shareImports: shares));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    shares.controller.add(
      const SharedContent(
        text: 'first draft',
        source: SharedContentSource.quickCapture,
        eventId: '20000000-0000-4000-8000-000000000001',
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    await tester.enterText(
      find.byKey(const ValueKey('shareImportEditor')),
      'locally edited draft',
    );
    await tester.tap(find.text('Current chat'));
    await tester.pump();

    shares.controller.add(
      const SharedContent(
        text: 'newest draft',
        source: SharedContentSource.quickCapture,
        eventId: '20000000-0000-4000-8000-000000000002',
      ),
    );
    await tester.pump();
    expect(
      tester
          .widget<TextField>(find.byKey(const ValueKey('shareImportEditor')))
          .controller
          ?.text,
      'newest draft',
    );
    expect(
      tester
          .widget<FilledButton>(find.byKey(const ValueKey('shareImportSend')))
          .onPressed,
      isNull,
    );

    shares.controller.add(
      const SharedContent(
        text: 'duplicate must be ignored',
        source: SharedContentSource.quickCapture,
        eventId: '20000000-0000-4000-8000-000000000002',
      ),
    );
    await tester.pump();
    expect(find.text('duplicate must be ignored'), findsNothing);

    await tester.tap(find.text('Cancel'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Quick capture'), findsNothing);
    expect((await client.listSessions()).single.messageCount, 0);
  });

  testWidgets('cold-start duplicate capture recovers into one review only', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    const capture = SharedContent(
      text: 'cold capture',
      source: SharedContentSource.quickCapture,
      eventId: '30000000-0000-4000-8000-000000000001',
    );
    await tester.pumpWidget(
      MikuApp(
        client: client,
        shareImports: const ColdStartShareImportService([capture, capture]),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 450));

    expect(find.text('Quick capture'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 450));
    expect(find.text('Quick capture'), findsNothing);
    expect((await client.listSessions()).single.messageCount, 0);
  });

  testWidgets('sends an edited quick capture to current and new sessions', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final shares = RecordingShareImportService();
    addTearDown(shares.close);
    await tester.pumpWidget(MikuApp(client: client, shareImports: shares));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    shares.controller.add(
      const SharedContent(
        text: 'current capture',
        source: SharedContentSource.quickCapture,
        eventId: '40000000-0000-4000-8000-000000000001',
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    await tester.enterText(
      find.byKey(const ValueKey('shareImportEditor')),
      'edited current capture',
    );
    await tester.tap(find.text('Current chat'));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('shareImportSend')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 650));

    shares.controller.add(
      const SharedContent(
        text: 'new capture',
        source: SharedContentSource.quickCapture,
        eventId: '40000000-0000-4000-8000-000000000002',
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    await tester.tap(find.text('New chat'));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('shareImportSend')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 650));

    final sessions = await client.listSessions();
    expect(sessions, hasLength(2));
    expect(
      sessions.any(
        (session) =>
            session.messageCount == 2 &&
            session.preview == 'Miku heard: edited current capture',
      ),
      isTrue,
    );
    expect(
      sessions.any(
        (session) =>
            session.messageCount == 2 &&
            session.preview == 'Miku heard: new capture',
      ),
      isTrue,
    );
  });

  testWidgets('reviews selected Android text and cancellation never sends', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final shares = RecordingShareImportService();
    addTearDown(shares.close);
    await tester.pumpWidget(MikuApp(client: client, shareImports: shares));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    shares.controller.add(
      const SharedContent(
        text: 'Explain this selected text',
        source: SharedContentSource.selection,
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Ask Miku about this'), findsOneWidget);
    expect(find.text('Selected text'), findsOneWidget);
    expect(find.text('Selected in another Android app'), findsOneWidget);
    expect((await client.listSessions()).single.messageCount, 0);
    expect(
      tester
          .widget<FilledButton>(find.byKey(const ValueKey('shareImportSend')))
          .onPressed,
      isNull,
    );

    await tester.tap(find.text('Cancel'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Ask Miku about this'), findsNothing);
    expect((await client.listSessions()).single.messageCount, 0);
  });

  testWidgets('reviews and edits Android shares before sending', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final shares = RecordingShareImportService();
    addTearDown(shares.close);
    await tester.pumpWidget(MikuApp(client: client, shareImports: shares));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    shares.controller.add(
      const SharedContent(
        text: 'Example title\n\nhttps://example.test',
        subject: 'Example title',
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Share with Miku'), findsOneWidget);
    expect(
      tester
          .widget<TextField>(find.byKey(const ValueKey('shareImportEditor')))
          .controller
          ?.text,
      'Example title\n\nhttps://example.test',
    );
    expect((await client.listSessions()).single.messageCount, 0);
    expect(
      tester
          .widget<FilledButton>(find.byKey(const ValueKey('shareImportSend')))
          .onPressed,
      isNull,
    );

    await tester.enterText(
      find.byKey(const ValueKey('shareImportEditor')),
      'Please summarize https://example.test',
    );
    await tester.tap(find.text('Current chat'));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('shareImportSend')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 450));
    for (var attempt = 0; attempt < 10; attempt++) {
      await tester.pump(const Duration(milliseconds: 100));
      final sessions = await client.listSessions();
      if (sessions.any((session) => session.messageCount == 2)) break;
    }

    expect(find.text('Please summarize https://example.test'), findsOneWidget);
    expect(
      find.textContaining(
        'Miku heard: Please summarize https://example.test',
        findRichText: true,
      ),
      findsOneWidget,
    );
    expect((await client.listSessions()).single.messageCount, 2);
  });

  testWidgets('can route a reviewed share into a new session', (tester) async {
    final client = ScriptedMikuClient();
    final shares = RecordingShareImportService();
    addTearDown(shares.close);
    await tester.pumpWidget(MikuApp(client: client, shareImports: shares));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    shares.controller.add(const SharedContent(text: 'new session share'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect((await client.listSessions()), hasLength(1));

    await tester.tap(find.text('New chat'));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('shareImportSend')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 450));
    for (var attempt = 0; attempt < 10; attempt++) {
      await tester.pump(const Duration(milliseconds: 100));
      final sessions = await client.listSessions();
      if (sessions.any((session) => session.messageCount == 2)) break;
    }

    final sessions = await client.listSessions();
    expect(sessions, hasLength(2));
    expect(
      sessions.any(
        (session) =>
            session.messageCount == 2 &&
            session.preview == 'Miku heard: new session share',
      ),
      isTrue,
    );
  });
}
