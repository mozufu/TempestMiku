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

  testWidgets('compact mobile chrome stays readable at 390px',
      (WidgetTester tester) async {
    tester.view.physicalSize = const Size(390, 844);
    tester.view.devicePixelRatio = 1;
    addTearDown(() {
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
    });

    await tester.pumpWidget(MikuApp(client: ScriptedMikuClient()));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.text('Miku'), findsOneWidget);
    expect(find.text('TempestMiku'), findsNothing);
    expect(find.text('Miku is here'), findsOneWidget);
    expect(find.text('Miku 在這裡'), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('primary chat controls expose selected-language semantics',
      (WidgetTester tester) async {
    await tester.pumpWidget(MikuApp(client: ScriptedMikuClient()));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.bySemanticsLabel('Open sessions'), findsOneWidget);
    expect(find.bySemanticsLabel('Open more actions'), findsOneWidget);
    expect(find.bySemanticsLabel('Send message'), findsOneWidget);

    await tester.enterText(find.byType(EditableText), 'code artifact://0');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.byKey(const ValueKey('resource:artifact://0')), findsOneWidget);
  });

  testWidgets('language switch toggles chrome without changing chat content',
      (WidgetTester tester) async {
    await tester.pumpWidget(MikuApp(client: ScriptedMikuClient()));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.text('Miku is here'), findsOneWidget);
    expect(find.text('Miku 在這裡'), findsNothing);

    await tester.tap(find.text('EN'));
    await tester.pump();

    expect(find.text('Miku 在這裡'), findsOneWidget);
    expect(find.text('Miku is here'), findsNothing);
    expect(find.bySemanticsLabel('送出訊息'), findsOneWidget);

    await tester.enterText(find.byType(EditableText), 'hello in any language');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('hello in any language'), findsOneWidget);
    expect(find.textContaining('Miku heard: hello in any language'),
        findsOneWidget);
  });

  testWidgets(
      'shows remote control stream, final, hidden mode state, and project state',
      (WidgetTester tester) async {
    await tester.pumpWidget(MikuApp(client: ScriptedMikuClient()));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.text('TempestMiku'), findsWidgets);
    expect(find.text('Personal'), findsOneWidget);
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
    expect(find.text('Serious'), findsOneWidget);
    expect(find.text('燒烤'), findsNothing);
    expect(find.text('著陸'), findsNothing);
    expect(find.text('交棒'), findsNothing);
    expect(find.textContaining('Miku heard: please fix code artifact://0'),
        findsWidgets);
    expect(find.text('artifact://0'), findsOneWidget);

    await tester.ensureVisible(find.text('artifact://0'));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('resource:artifact://0')));
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
    await tester.tap(find.text('Promote Session'));
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

    expect(find.text('Round 1'), findsOneWidget);
    expect(find.text('Round 2'), findsOneWidget);
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

    await tester.tap(find.byIcon(Icons.add).last);
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
    expect(find.text('Personal locked'), findsNothing);
    expect(find.text('Personal'), findsOneWidget);

    await tester.tap(find.text('Personal'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Mode / Lock'), findsOneWidget);
    expect(find.text('Personal Assistant'), findsOneWidget);
    expect(find.text('Lock Personal'), findsOneWidget);

    await tester.tap(find.text('Serious Engineer'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(client.overriddenModes, contains('serious_engineer'));
    expect(find.text('Serious'), findsOneWidget);
    expect(find.text('認真工程師'), findsNothing);

    await tester.tap(find.text('Serious'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Lock Serious'), findsOneWidget);
    await tester.tap(find.text('Lock Serious'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(client.lockedModes, contains('serious_engineer'));
    expect(find.text('Serious locked'), findsOneWidget);
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
    expect(
        find.textContaining('Pending approval · memory.write'), findsNothing);

    await tester.tap(find.text('Save memory'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(client.resolvedApprovals, hasLength(1));
    expect(client.resolvedApprovals.single, endsWith(':approve'));
    expect(find.text('Memory proposal'), findsNothing);
  });

  testWidgets('opens actor completion resources from activity feed',
      (WidgetTester tester) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'handoff actor links');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    await tester.tap(find.text('Agents · 0 running / 1 stopped'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    final toolCall = find.text('呼叫工具 execute');
    final cellStart = find.text('執行程式');
    final actorCompleted = find.text('完成 Worker0');
    expect(toolCall, findsOneWidget);
    expect(cellStart, findsOneWidget);
    expect(actorCompleted, findsOneWidget);
    expect(tester.getTopLeft(toolCall).dy,
        lessThan(tester.getTopLeft(cellStart).dy));
    expect(
      tester.getTopLeft(cellStart).dy,
      lessThan(tester.getTopLeft(actorCompleted).dy),
    );

    final artifactLink =
        find.byKey(const ValueKey('activity-resource:artifact://0'));
    final historyLink =
        find.byKey(const ValueKey('activity-resource:history://Worker0'));
    expect(artifactLink, findsOneWidget);
    expect(historyLink, findsOneWidget);
    expect(find.text('artifact://0'), findsWidgets);
    expect(find.text('history://Worker0'), findsOneWidget);

    await tester.tap(artifactLink);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Scripted resource'), findsOneWidget);
    expect(find.text('Preview for artifact://0'), findsOneWidget);

    await tester.tap(find.byType(ModalBarrier).last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    await tester.tap(historyLink);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Scripted resource'), findsOneWidget);
    expect(find.text('Preview for history://Worker0'), findsOneWidget);
  });

  testWidgets('handles actor approval, child resource, and reconnect cursor',
      (WidgetTester tester) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'handoff actor approval');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('Handoff'), findsOneWidget);
    expect(find.text('Agents · 0 running / 1 stopped'), findsOneWidget);
    expect(find.text('worker agent · Worker0'), findsOneWidget);
    expect(find.text('stopped'), findsOneWidget);
    expect(find.text('呼叫工具 execute'), findsNothing);

    await tester.tap(find.text('Agents · 0 running / 1 stopped'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Agents · Round 1'), findsOneWidget);
    expect(find.text('Prompt / Activity'), findsOneWidget);
    expect(find.text('呼叫工具 execute'), findsOneWidget);
    expect(find.text('執行程式'), findsOneWidget);
    expect(find.text('啟動 worker · Worker0'), findsOneWidget);
    expect(find.text('完成 Worker0'), findsOneWidget);
    expect(find.text('程式結果'), findsOneWidget);

    await tester.tap(find.byIcon(Icons.close).last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    final actorReply =
        find.textContaining('Actor Worker0 completed', skipOffstage: false);
    expect(actorReply, findsOneWidget);
    await tester.ensureVisible(actorReply);
    await tester.pump();
    expect(find.textContaining('Actor Worker0 completed'), findsOneWidget);
    expect(find.text('artifact://0'), findsOneWidget);
    expect(find.textContaining('Pending approval · proc.run cargo clean'),
        findsOneWidget);

    await tester.ensureVisible(find.text('artifact://0'));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey('resource:artifact://0')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Scripted resource'), findsOneWidget);
    expect(find.text('Preview for artifact://0'), findsOneWidget);

    await tester.tap(find.byType(ModalBarrier).last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    await tester.tap(
      find.textContaining('Pending approval · proc.run cargo clean'),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('actorId: Worker0'), findsOneWidget);
    await tester.tap(find.text('Approve once'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(client.resolvedApprovals, hasLength(1));
    expect(client.resolvedApprovals.single, endsWith(':approve'));
    expect(find.textContaining('Pending approval · proc.run cargo clean'),
        findsNothing);

    final remembered = client.rememberedLastEventIds.values.single;
    await tester.pumpWidget(MikuApp(key: UniqueKey(), client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(client.eventResumeIds.last, remembered);
    expect(find.textContaining('event #$remembered'), findsOneWidget);
  });
}
