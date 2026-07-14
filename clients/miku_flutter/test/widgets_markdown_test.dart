import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:miku_flutter/main.dart';
import 'package:miku_flutter/ratex_formula.dart';

void main() {
  Future<void> pumpMarkdown(
    WidgetTester tester,
    String text, {
    double width = 320,
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Align(
            alignment: Alignment.topLeft,
            child: SizedBox(width: width, child: MikuMarkdownBody(text: text)),
          ),
        ),
      ),
    );
    await tester.pump();
  }

  testWidgets('body and code are selectable with an accessible copy action', (
    tester,
  ) async {
    final clipboardWrites = <String>[];
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'Clipboard.setData') {
        final arguments = call.arguments as Map<Object?, Object?>;
        clipboardWrites.add(arguments['text']! as String);
      }
      return null;
    });
    addTearDown(
      () => messenger.setMockMethodCallHandler(SystemChannels.platform, null),
    );

    await pumpMarkdown(tester, '''A readable paragraph that can be selected.

```dart
final deliberatelyLongName = 'this line is wider than the message surface';
```''');

    expect(find.byType(SelectionArea), findsOneWidget);
    final horizontalScroll = find.byWidgetPredicate(
      (widget) =>
          widget is SingleChildScrollView &&
          widget.scrollDirection == Axis.horizontal,
    );
    expect(horizontalScroll, findsOneWidget);
    final copyButton = find.widgetWithIcon(
      IconButton,
      Icons.content_copy_outlined,
    );
    expect(copyButton, findsOneWidget);
    expect(tester.getSize(copyButton), const Size(48, 48));
    expect(find.bySemanticsLabel('Copy code'), findsOneWidget);

    await tester.tap(copyButton);
    await tester.pump();

    expect(clipboardWrites, [
      "final deliberatelyLongName = 'this line is wider than the message surface';",
    ]);
    expect(find.text('Code copied'), findsOneWidget);
  });

  testWidgets('http links are explicit and copy-only', (tester) async {
    final clipboardWrites = <String>[];
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'Clipboard.setData') {
        final arguments = call.arguments as Map<Object?, Object?>;
        clipboardWrites.add(arguments['text']! as String);
      }
      return null;
    });
    addTearDown(
      () => messenger.setMockMethodCallHandler(SystemChannels.platform, null),
    );

    await pumpMarkdown(
      tester,
      'Read [the docs](https://example.test/docs) or '
      'https://miku.example.test/status. Ignore [FTP](ftp://example.test).',
    );

    expect(
      find.bySemanticsLabel('Copy link https://example.test/docs'),
      findsOneWidget,
    );
    expect(
      find.bySemanticsLabel('Copy link https://miku.example.test/status'),
      findsOneWidget,
    );
    expect(find.bySemanticsLabel('Copy link ftp://example.test'), findsNothing);
    final linkButtons = find.widgetWithIcon(IconButton, Icons.link);
    expect(linkButtons, findsNWidgets(2));
    for (final element in linkButtons.evaluate()) {
      expect(
        tester.getSize(find.byElementPredicate((e) => e == element)),
        const Size(48, 48),
      );
    }

    await tester.tap(
      find.bySemanticsLabel('Copy link https://miku.example.test/status'),
    );
    await tester.pump();

    expect(clipboardWrites, ['https://miku.example.test/status']);
    expect(find.text('Link copied'), findsOneWidget);
  });

  testWidgets('tables scroll horizontally and preserve RaTeX rendering', (
    tester,
  ) async {
    await pumpMarkdown(tester, r'''| Name | Value |
| :--- | ---: |
| Euler | $e^{i\pi}+1=0$ |
| Access | `read \| write` |

$$\sin z = \frac{e^{iz}-e^{-iz}}{2i}$$''');

    expect(find.byType(Table), findsOneWidget);
    expect(find.text('Name', findRichText: true), findsOneWidget);
    expect(find.text('read | write', findRichText: true), findsOneWidget);
    expect(find.byType(RaTeXFormula), findsNWidgets(2));
    expect(
      find.byWidgetPredicate(
        (widget) => widget is Semantics && widget.properties.header == true,
      ),
      findsNWidgets(2),
    );
    expect(
      find.byWidgetPredicate(
        (widget) =>
            widget is SingleChildScrollView &&
            widget.scrollDirection == Axis.horizontal,
      ),
      findsOneWidget,
    );
  });
}
