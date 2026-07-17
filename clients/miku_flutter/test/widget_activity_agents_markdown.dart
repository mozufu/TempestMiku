part of 'widget_test.dart';

void _registerActivityAgentAndMarkdownTests() {
  testWidgets('promotes actor completion resources from activity feed', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'handoff actor links');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    await _openActivitySheet(tester);

    final toolCall = find.text('呼叫工具 execute').last;
    final cellStart = find.text('執行程式').last;
    final actorCompleted = find.text('完成 Worker0').last;
    expect(toolCall, findsOneWidget);
    expect(cellStart, findsOneWidget);
    expect(actorCompleted, findsOneWidget);
    expect(find.textContaining('agents.spawn'), findsWidgets);
    expect(
      tester.getTopLeft(toolCall).dy,
      lessThan(tester.getTopLeft(cellStart).dy),
    );
    expect(
      tester.getTopLeft(cellStart).dy,
      lessThan(tester.getTopLeft(actorCompleted).dy),
    );

    final artifactLink = find.byKey(
      const ValueKey('activity-resource:artifact://0'),
    );
    final historyLink = find.byKey(
      const ValueKey('activity-resource:history://Worker0'),
    );
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

    await _popRoute(tester);

    await tester.ensureVisible(historyLink.last);
    await tester.pump();
    await tester.tap(historyLink.last);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Scripted resource'), findsOneWidget);
    expect(find.text('Preview for history://Worker0'), findsOneWidget);

    await _popRoute(tester);

    await _closeActivitySheet(tester);

    await _tapDrawerAction(tester, 'Promote Session');

    expect(
      client.promotedSummaries.single,
      'Actor Worker0 completed child resource artifact://0',
    );
    expect(client.promotedResources.single, [
      'artifact://0',
      'history://Worker0',
    ]);

    await _openContext(tester);
    await tester.ensureVisible(find.text('project://tempestmiku · 3 promoted'));

    expect(find.text('project://tempestmiku · 3 promoted'), findsOneWidget);
  });

  testWidgets('keeps activity trace visible after final', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient(pauseBeforeFinal: true);
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(
      find.byType(EditableText),
      'handoff actor live trace',
    );
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    final activityCard = find.byKey(const ValueKey('agent-activity:1'));
    await _scrollChatUntilVisible(tester, activityCard);
    expect(activityCard, findsOneWidget);
    expect(find.text('Agents · 0 running / 1 stopped'), findsOneWidget);
    expect(find.text('呼叫工具 execute'), findsNothing);

    client.completePausedTurn();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    await _scrollChatUntilVisible(tester, activityCard);
    expect(find.text('Agents · 0 running / 1 stopped'), findsOneWidget);
    expect(
      find.textContaining('Actor Worker0 completed', skipOffstage: false),
      findsOneWidget,
    );

    await _openActivitySheet(tester);

    expect(find.text('Prompt / Activity'), findsOneWidget);
    expect(find.text('呼叫工具 execute'), findsWidgets);
    expect(find.text('執行程式'), findsWidgets);
    expect(find.text('完成 Worker0'), findsWidgets);
  });

  testWidgets('renders markdown and keeps reasoning visible after final', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(
      find.byType(EditableText),
      'markdown with reasoning',
    );
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('P4 memo', findRichText: true), findsOneWidget);
    expect(find.text('•', findRichText: true), findsOneWidget);
    expect(find.text('☐', findRichText: true), findsOneWidget);
    expect(find.byType(RaTeXFormula), findsNWidgets(2));
    expect(find.text(r'\sin z = \frac{e^{iz}-e^{-iz}}{2i}'), findsOneWidget);
    expect(find.text(r'e^{i\pi}+1=0'), findsOneWidget);
    expect(find.textContaining(r'\\['), findsNothing);
    expect(find.text('Thinking'), findsOneWidget);
    final thinking = find.text('Thinking');
    await _scrollChatUntilVisible(tester, thinking);
    await tester.tap(
      find.ancestor(of: thinking, matching: find.byType(InkWell)).first,
    );
    await tester.pump();
    expect(
      find.textContaining(
        'Compare scheduler invariants',
        findRichText: true,
        skipOffstage: false,
      ),
      findsOneWidget,
    );
  });

  testWidgets('handles actor approval, child resource, and reconnect cursor', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'handoff actor approval');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('Handoff'), findsNothing);
    final activityCard = find.byKey(const ValueKey('agent-activity:1'));
    await _scrollChatUntilVisible(tester, activityCard);
    expect(find.text('Agents · 0 running / 1 stopped'), findsOneWidget);
    expect(find.text('worker agent · Worker0'), findsOneWidget);
    expect(find.text('stopped'), findsOneWidget);
    expect(find.text('呼叫工具 execute'), findsNothing);

    await _openActivitySheet(tester);

    expect(find.text('Agents · Round 1'), findsOneWidget);
    expect(find.text('Prompt / Activity'), findsOneWidget);
    expect(find.text('呼叫工具 execute'), findsWidgets);
    expect(find.text('執行程式'), findsWidgets);
    expect(find.text('啟動 worker · Worker0'), findsWidgets);
    expect(find.text('完成 Worker0'), findsWidgets);
    expect(find.text('程式結果'), findsWidgets);

    await _closeActivitySheet(tester);

    final actorReply = find.textContaining(
      'Actor Worker0 completed',
      skipOffstage: false,
    );
    expect(actorReply, findsOneWidget);
    await tester.ensureVisible(actorReply);
    await tester.pump();
    expect(find.textContaining('Actor Worker0 completed'), findsOneWidget);
    expect(find.text('artifact://0'), findsWidgets);

    final answerArtifactLink = find.byKey(
      const ValueKey('resource:artifact://0'),
    );
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

    final approvalCard = find.byKey(
      const ValueKey('approval:proc.run cargo clean'),
    );
    await _scrollChatUntilVisible(tester, approvalCard);
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
