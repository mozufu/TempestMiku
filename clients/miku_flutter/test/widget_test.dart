import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:miku_flutter/main.dart';
import 'package:miku_flutter/session_client_stub.dart';

void main() {
  testWidgets('shows remote control stream, final, mode, and project state',
      (WidgetTester tester) async {
    await tester.pumpWidget(MikuApp(client: ScriptedMikuClient()));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    expect(find.text('TempestMiku'), findsWidgets);
    expect(find.textContaining('個人助理'), findsWidgets);

    await tester.enterText(
        find.byType(EditableText), 'please fix code artifact://0');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.textContaining('認真工程師'), findsWidgets);
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
}
