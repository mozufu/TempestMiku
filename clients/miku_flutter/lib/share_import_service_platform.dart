const maxSharedTextLength = 16384;
const maxSharedSubjectLength = 240;

enum SharedContentSource { share, selection }

class SharedContent {
  const SharedContent({
    required this.text,
    this.subject,
    this.truncated = false,
    this.source = SharedContentSource.share,
  });

  factory SharedContent.fromEvent(Map<Object?, Object?> event) {
    final rawText = event['text'];
    if (rawText is! String) {
      throw const FormatException('shared content is missing text');
    }
    final sanitizedBody = _sanitizeSharedText(rawText);
    final body = _truncateWithoutSplittingSurrogate(
      sanitizedBody,
      maxSharedTextLength,
    );
    if (body.isEmpty) {
      throw const FormatException('shared content is empty');
    }
    final source = switch (event['source']) {
      null || 'share' => SharedContentSource.share,
      'selection' => SharedContentSource.selection,
      _ => throw const FormatException('shared content has an invalid source'),
    };
    final rawSubject =
        source == SharedContentSource.share ? event['subject'] : null;
    final sanitizedSubject =
        rawSubject is String ? _sanitizeSharedText(rawSubject) : '';
    final subject = _truncateWithoutSplittingSurrogate(
      sanitizedSubject,
      maxSharedSubjectLength,
    );
    final combined =
        subject.isNotEmpty && subject != body ? '$subject\n\n$body' : body;
    final wasTruncated =
        event['truncated'] == true ||
        sanitizedBody.length > maxSharedTextLength ||
        sanitizedSubject.length > maxSharedSubjectLength ||
        combined.length > maxSharedTextLength;
    return SharedContent(
      text: _truncateWithoutSplittingSurrogate(combined, maxSharedTextLength),
      subject: subject.isEmpty ? null : subject,
      truncated: wasTruncated,
      source: source,
    );
  }

  final String text;
  final String? subject;
  final bool truncated;
  final SharedContentSource source;
}

bool _isHighSurrogate(int codeUnit) => codeUnit >= 0xd800 && codeUnit <= 0xdbff;

bool _isLowSurrogate(int codeUnit) => codeUnit >= 0xdc00 && codeUnit <= 0xdfff;

String _truncateWithoutSplittingSurrogate(String value, int maxLength) {
  if (value.length <= maxLength) return value;
  var end = maxLength;
  if (end > 0 &&
      _isHighSurrogate(value.codeUnitAt(end - 1)) &&
      _isLowSurrogate(value.codeUnitAt(end))) {
    end -= 1;
  }
  return value.substring(0, end);
}

abstract class MikuShareImportService {
  bool get isSupported;

  Stream<SharedContent> get imports;
}

String _sanitizeSharedText(String value) =>
    value
        .replaceAll('\r\n', '\n')
        .replaceAll('\r', '\n')
        .runes
        .where(
          (rune) =>
              rune == 0x0a ||
              rune == 0x09 ||
              (rune >= 0x20 && !(rune >= 0x7f && rune <= 0x9f)),
        )
        .map((rune) => String.fromCharCodes([rune]))
        .join()
        .trim();
