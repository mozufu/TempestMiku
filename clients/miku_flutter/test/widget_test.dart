import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:miku_flutter/main.dart';
import 'package:miku_flutter/session_client_stub.dart';

void main() {
  testWidgets('shows remote control stream, final, mode, and project state',
      (WidgetTester tester) async {
    await tester.pumpWidget(MikuApp(client: ScriptedMikuClient()));

    expect(find.text('TempestMiku'), findsWidgets);
    expect(find.text('Personal Assistant'), findsWidgets);

    await tester.enterText(find.byType(EditableText), 'please fix code artifact://0');
    await tester.tap(find.text('Send'));
    await tester.pump();
    await tester.pump();

    expect(find.text('Serious Engineer'), findsWidgets);
    expect(find.textContaining('Miku heard: please fix code artifact://0'), findsWidgets);
    expect(find.text('artifact://0'), findsOneWidget);
    expect(find.text('Continue from latest session result'), findsOneWidget);
  });
}
