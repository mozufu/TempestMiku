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
    expect(shouldRememberEventId('drive_put', const {}), isTrue);
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

  testWidgets('shows and resolves pending drive filing approval',
      (WidgetTester tester) async {
    final client = ScriptedMikuClient();
    final session = await client.createSession();
    client.seedPendingApproval(
      session.id,
      approvalId: 'approval-drive',
      action: 'drive.put inbox/approval-drop.md',
      scope: const {
        'capability': 'drive.put',
        'sourceUri': 'drop://browser/approval-drop.md',
      },
    );
    client.seedPendingApproval(
      session.id,
      approvalId: 'approval-drive-deny',
      action: 'drive.put inbox/blocked-drop.md',
      scope: const {
        'capability': 'drive.put',
        'sourceUri': 'drop://browser/blocked-drop.md',
      },
    );

    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    final approvalCard =
        find.byKey(const ValueKey('approval:drive.put inbox/approval-drop.md'));
    expect(approvalCard, findsOneWidget);
    expect(find.text('Pending approval · drive.put inbox/approval-drop.md'),
        findsOneWidget);

    await tester.tap(find.bySemanticsLabel('Open drive feed'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Pending drive approvals'), findsOneWidget);
    expect(find.text('drive.put inbox/approval-drop.md'), findsOneWidget);
    expect(find.text('drive.put inbox/blocked-drop.md'), findsOneWidget);
    await tester.tap(find.bySemanticsLabel('Close drive feed'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    await tester.ensureVisible(approvalCard);
    await tester.tap(approvalCard);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Approval needed'), findsOneWidget);
    await tester.tap(find.text('Approve once'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(client.resolvedApprovals, contains('approval-drive:approve'));
    expect(approvalCard, findsNothing);

    final denyCard =
        find.byKey(const ValueKey('approval:drive.put inbox/blocked-drop.md'));
    expect(denyCard, findsOneWidget);
    await tester.ensureVisible(denyCard);
    await tester.tap(denyCard);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    await tester.tap(find.text('Deny'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(client.resolvedApprovals, contains('approval-drive-deny:deny'));
    expect(denyCard, findsNothing);
  });

  testWidgets('dogfoods drive research feed from remote control UI',
      (WidgetTester tester) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(
      find.byType(EditableText),
      'research drive workspace for p5',
    );
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 150));

    expect(find.text('Drive organizer completed'), findsWidgets);
    expect(find.textContaining('drive://projects/tempestmiku/research'),
        findsWidgets);

    final activityResource = find.byKey(
      const ValueKey(
        'activity-resource:drive://projects/tempestmiku/research/p5-drive-workspace.md',
      ),
    );
    expect(activityResource, findsWidgets);
    await tester.ensureVisible(activityResource.first);
    await tester.pump();
    await tester.tap(activityResource.first);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Scripted drive note'), findsOneWidget);
    expect(find.textContaining('# Scripted drive note'), findsOneWidget);

    await tester.tap(find.byType(ModalBarrier).last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    await tester.tap(find.bySemanticsLabel('Open drive feed'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(client.driveFeedRequests, greaterThan(0));
    expect(find.text('Drive'), findsWidgets);
    expect(find.text('Recent documents'), findsWidgets);
    expect(find.text('P5 drive research notes'), findsOneWidget);
    expect(find.text('Organizer proposals'), findsOneWidget);
    expect(
      find.textContaining(
        'inbox/raw-research.md -> projects/tempestmiku/research',
      ),
      findsOneWidget,
    );
    expect(find.text('Virtual folders'), findsOneWidget);

    final row = find.byKey(
      const ValueKey(
        'drive-feed:drive://projects/tempestmiku/research/p5-drive-workspace.md',
      ),
    );
    await tester.ensureVisible(row);
    await tester.pump();
    await tester.tap(row);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Scripted drive note'), findsOneWidget);
    expect(
      find.textContaining('Local citation corpus is ready.'),
      findsOneWidget,
    );
  });

  testWidgets('opens drive uri surfaced by a runtime cell result',
      (WidgetTester tester) async {
    final client = ScriptedMikuClient(pauseBeforeFinal: true);
    final session = await client.createSession();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'start runtime');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    client.emitEvent(
      session.id,
      const MikuEvent(
        type: 'cell_result',
        id: 'cell-result-drive-uri',
        data: {
          'shaped':
              'stdout:\ndisplay: {"filedUri":"drive://projects/tempestmiku/research/p5-drive-workspace.md"}\n\nresult:\nnull',
        },
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    final resultResource = find.byKey(
      const ValueKey(
        'activity-resource:drive://projects/tempestmiku/research/p5-drive-workspace.md',
      ),
    );
    expect(resultResource, findsOneWidget);
    await tester.tap(resultResource);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Scripted drive note'), findsOneWidget);
    expect(find.textContaining('# Scripted drive note'), findsOneWidget);
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

  testWidgets('phone view resolves a dream-origin memory proposal',
      (WidgetTester tester) async {
    tester.view.physicalSize = const Size(390, 844);
    tester.view.devicePixelRatio = 1;
    addTearDown(() {
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
    });

    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'dream captured this');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('Memory proposal'), findsOneWidget);
    expect(find.text('provenance post-session-dream'), findsOneWidget);
    await tester.tap(find.text('Save memory'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(client.resolvedApprovals, hasLength(1));
    expect(client.resolvedApprovals.single, endsWith(':approve'));
    expect(find.text('Memory proposal'), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('promotes actor completion resources from activity feed',
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

    final activityCard = find.byKey(const ValueKey('agent-activity:1'));
    await tester.ensureVisible(activityCard);
    await tester.pump();
    await tester.tap(activityCard);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    final toolCall = find.text('呼叫工具 execute').last;
    final cellStart = find.text('執行程式').last;
    final actorCompleted = find.text('完成 Worker0').last;
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
    expect(artifactLink, findsWidgets);
    expect(historyLink, findsWidgets);
    expect(find.text('artifact://0'), findsWidgets);
    expect(find.text('history://Worker0'), findsWidgets);

    await tester.ensureVisible(artifactLink.last);
    await tester.pump();
    await tester.tap(artifactLink.last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Scripted resource'), findsOneWidget);
    expect(find.text('Preview for artifact://0'), findsOneWidget);

    await tester.tap(find.byType(ModalBarrier).last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    await tester.ensureVisible(historyLink.last);
    await tester.pump();
    await tester.tap(historyLink.last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Scripted resource'), findsOneWidget);
    expect(find.text('Preview for history://Worker0'), findsOneWidget);

    await tester.tap(find.byType(ModalBarrier).last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    await tester.tapAt(const Offset(20, 20));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    await tester.tap(find.byIcon(Icons.more_horiz));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    await tester.tap(find.text('Promote Session'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(client.promotedSummaries.single,
        'Actor Worker0 completed child resource artifact://0');
    expect(client.promotedResources.single, [
      'artifact://0',
      'history://Worker0',
    ]);

    await tester.tap(find.byIcon(Icons.more_horiz));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('project://tempestmiku · 3 promoted'), findsOneWidget);
  });

  testWidgets('keeps activity trace visible after final',
      (WidgetTester tester) async {
    final client = ScriptedMikuClient(pauseBeforeFinal: true);
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(
        find.byType(EditableText), 'handoff actor live trace');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('呼叫工具 execute'), findsWidgets);
    expect(find.text('執行程式'), findsWidgets);
    expect(find.text('完成 Worker0'), findsWidgets);

    client.completePausedTurn();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('Agents · 0 running / 1 stopped'), findsOneWidget);
    expect(find.text('呼叫工具 execute'), findsWidgets);
    expect(find.text('執行程式'), findsWidgets);
    expect(find.text('完成 Worker0'), findsOneWidget);
    expect(find.textContaining('Actor Worker0 completed', skipOffstage: false),
        findsOneWidget);

    final activityCard = find.byKey(const ValueKey('agent-activity:1'));
    await tester.ensureVisible(activityCard);
    await tester.pump();
    await tester.tap(activityCard);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Prompt / Activity'), findsOneWidget);
    expect(find.text('呼叫工具 execute'), findsWidgets);
    expect(find.text('執行程式'), findsWidgets);
    expect(find.text('完成 Worker0'), findsWidgets);
  });

  testWidgets('renders markdown and keeps reasoning visible after final',
      (WidgetTester tester) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(
        find.byType(EditableText), 'markdown with reasoning');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('P4 memo', findRichText: true), findsOneWidget);
    expect(find.text('•', findRichText: true), findsOneWidget);
    expect(find.text('☐', findRichText: true), findsOneWidget);
    expect(find.text('Thinking'), findsOneWidget);
    expect(
      find.textContaining('Compare scheduler invariants',
          findRichText: true, skipOffstage: false),
      findsOneWidget,
    );
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
    expect(find.text('呼叫工具 execute'), findsOneWidget);

    final activityCard = find.byKey(const ValueKey('agent-activity:1'));
    await tester.ensureVisible(activityCard);
    await tester.pump();
    await tester.tap(activityCard);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Agents · Round 1'), findsOneWidget);
    expect(find.text('Prompt / Activity'), findsOneWidget);
    expect(find.text('呼叫工具 execute'), findsWidgets);
    expect(find.text('執行程式'), findsWidgets);
    expect(find.text('啟動 worker · Worker0'), findsWidgets);
    expect(find.text('完成 Worker0'), findsWidgets);
    expect(find.text('程式結果'), findsWidgets);

    await tester.tap(find.byIcon(Icons.close).last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    final actorReply =
        find.textContaining('Actor Worker0 completed', skipOffstage: false);
    expect(actorReply, findsOneWidget);
    await tester.ensureVisible(actorReply);
    await tester.pump();
    expect(find.textContaining('Actor Worker0 completed'), findsOneWidget);
    expect(find.text('artifact://0'), findsWidgets);

    final answerArtifactLink =
        find.byKey(const ValueKey('resource:artifact://0'));
    expect(answerArtifactLink, findsOneWidget);
    await tester.ensureVisible(answerArtifactLink);
    await tester.pump();
    await tester.tap(answerArtifactLink);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Scripted resource'), findsOneWidget);
    expect(find.text('Preview for artifact://0'), findsOneWidget);

    await tester.tap(find.byType(ModalBarrier).last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    final approvalCard =
        find.byKey(const ValueKey('approval:proc.run cargo clean'));
    await tester.scrollUntilVisible(
      approvalCard,
      220,
      scrollable: find.byType(Scrollable).first,
    );
    await tester.pump();
    expect(approvalCard, findsOneWidget);
    await tester.ensureVisible(approvalCard);
    await tester.pump();
    await tester.tap(approvalCard);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('actorId: Worker0'), findsOneWidget);
    await tester.tap(find.text('Approve once'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(client.resolvedApprovals, hasLength(1));
    expect(client.resolvedApprovals.single, endsWith(':approve'));
    expect(approvalCard, findsNothing);

    final remembered = client.rememberedLastEventIds.values.single;
    await tester.pumpWidget(MikuApp(key: UniqueKey(), client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(client.eventResumeIds.last, remembered);
  });
}
