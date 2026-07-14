import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:miku_flutter/main.dart';
import 'package:miku_flutter/session_client_stub.dart';
import 'package:miku_flutter/share_import_service.dart';

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

    await tester.enterText(
      find.byKey(const ValueKey('shareImportEditor')),
      'Please summarize https://example.test',
    );
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
      find.text('Miku heard: Please summarize https://example.test'),
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
