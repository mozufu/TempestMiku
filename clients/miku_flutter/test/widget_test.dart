import 'package:flutter_test/flutter_test.dart';

import 'package:miku_flutter/main.dart';

void main() {
  testWidgets('shows TempestMiku scaffold', (WidgetTester tester) async {
    await tester.pumpWidget(const MikuApp());

    expect(find.text('TempestMiku'), findsWidgets);
    expect(
      find.text('Flutter Web/PWA client scaffold for the server SSE stream.'),
      findsOneWidget,
    );
  });
}
