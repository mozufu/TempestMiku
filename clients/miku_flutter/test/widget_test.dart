import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:miku_flutter/main.dart';
import 'package:miku_flutter/session_client_stub.dart';
import 'package:miku_flutter/session_models.dart';

void main() {
  test('does not advance the persisted cursor past unresolved gates', () {
    expect(shouldRememberEventId('approval', const {}), isFalse);
    expect(
      shouldRememberEventId(
        'write_proposal',
        const {'kind': 'memory', 'status': 'pending'},
      ),
      isFalse,
    );
    expect(
      shouldRememberEventId(
        'write_proposal',
        const {'kind': 'memory', 'status': 'approved'},
      ),
      isTrue,
    );
  });

  testWidgets(
      'shows remote control stream, final, hidden mode state, and project state',
      (WidgetTester tester) async {
    await tester.pumpWidget(MikuApp(client: ScriptedMikuClient()));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.text('TempestMiku'), findsWidgets);
    expect(find.text('助理'), findsOneWidget);
    expect(find.text('個人助理'), findsNothing);
    expect(find.text('燒烤'), findsNothing);
    expect(find.text('著陸'), findsNothing);
    expect(find.text('工程'), findsNothing);
    expect(find.text('交棒'), findsNothing);

    await tester.enterText(
        find.byType(EditableText), 'please fix code artifact://0');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.textContaining('認真工程師'), findsNothing);
    expect(find.text('工程'), findsOneWidget);
    expect(find.text('燒烤'), findsNothing);
    expect(find.text('著陸'), findsNothing);
    expect(find.text('交棒'), findsNothing);
    expect(find.textContaining('Miku heard: please fix code artifact://0'),
        findsWidgets);
    expect(find.text('artifact://0'), findsOneWidget);

    await tester.ensureVisible(find.text('artifact://0'));
    await tester.pump();
    await tester.tap(
      find.widgetWithText(GestureDetector, 'artifact://0'),
      warnIfMissed: false,
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Scripted resource'), findsOneWidget);
    expect(find.text('Preview for artifact://0'), findsOneWidget);

    await tester.tap(find.byType(ModalBarrier).last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    await tester.tap(find.byIcon(Icons.more_horiz));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    await tester.tap(find.text('推廣 Session'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    await tester.tap(find.byIcon(Icons.more_horiz));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('project://tempestmiku · 2 promoted'), findsOneWidget);
    expect(find.text('Continue from latest session result'), findsOneWidget);
  });

  testWidgets('records active conversation rounds in the thread',
      (WidgetTester tester) async {
    await tester.pumpWidget(MikuApp(client: ScriptedMikuClient()));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'first status check');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    await tester.enterText(find.byType(EditableText), 'second status check');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('回合 1'), findsOneWidget);
    expect(find.text('回合 2'), findsOneWidget);
    expect(find.text('first status check'), findsOneWidget);
    expect(find.text('second status check'), findsOneWidget);
    expect(find.text('Miku heard: first status check'), findsOneWidget);
    expect(find.text('Miku heard: second status check'), findsOneWidget);
  });

  testWidgets('opens session history, creates a new session, and restores one',
      (WidgetTester tester) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'first history check');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    await tester.tap(find.byIcon(Icons.history));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Sessions'), findsOneWidget);
    expect(find.text('Miku heard: first history check'), findsWidgets);

    final newSessionButton = find.ancestor(
      of: find.byIcon(Icons.add).last,
      matching: find.byType(GestureDetector),
    );
    await tester.tap(newSessionButton);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    await tester.pump(const Duration(milliseconds: 100));
    expect(await client.listSessions(), hasLength(2));
    expect(find.text('Miku heard: first history check'), findsNothing);

    await tester.enterText(find.byType(EditableText), 'second history check');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('second history check'), findsOneWidget);

    await tester.tap(find.byIcon(Icons.history));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Miku heard: first history check'), findsWidgets);
    expect(find.text('Miku heard: second history check'), findsWidgets);

    await tester.tap(find.text('Miku heard: first history check').last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('first history check'), findsOneWidget);
    expect(find.text('Miku heard: first history check'), findsOneWidget);
    expect(find.text('second history check'), findsNothing);
  });

  testWidgets('shows selector from mode dropdown and exposes lock',
      (WidgetTester tester) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.text('個人助理'), findsNothing);
    expect(find.text('助理鎖定'), findsNothing);
    expect(find.text('助理'), findsOneWidget);

    await tester.tap(find.text('助理'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('模式 / 鎖定'), findsOneWidget);
    expect(find.text('個人助理'), findsOneWidget);
    expect(find.text('鎖定助理'), findsOneWidget);

    await tester.tap(find.text('認真工程師'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(client.overriddenModes, contains('serious_engineer'));
    expect(find.text('工程'), findsOneWidget);
    expect(find.text('認真工程師'), findsNothing);

    await tester.tap(find.text('工程'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('鎖定工程'), findsOneWidget);
    await tester.tap(find.text('鎖定工程'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(client.lockedModes, contains('serious_engineer'));
    expect(find.text('工程鎖定'), findsOneWidget);
  });

  testWidgets('renders and resolves memory write proposals',
      (WidgetTester tester) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'remember this for me');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('Memory proposal'), findsOneWidget);
    expect(find.text('Brian prefers approval-backed memory writes.'),
        findsOneWidget);
    expect(find.text('scope global'), findsOneWidget);
    expect(find.text('provenance scripted chat turn'), findsOneWidget);
    expect(find.textContaining('待核可 · memory.write'), findsNothing);

    await tester.tap(find.text('Save memory'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(client.resolvedApprovals, hasLength(1));
    expect(client.resolvedApprovals.single, endsWith(':approve'));
    expect(find.text('Memory proposal'), findsNothing);
  });
}
