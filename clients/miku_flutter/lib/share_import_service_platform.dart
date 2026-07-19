import 'voice_capture_service_platform.dart';

const maxSharedTextLength = 16384;
const maxSharedSubjectLength = 240;

enum SharedContentSource { share, selection, quickCapture, voice }

enum VoiceTranscriptProvenance { local, selfHosted }

class SharedContent {
  const SharedContent({
    required this.text,
    this.subject,
    this.truncated = false,
    this.source = SharedContentSource.share,
    this.eventId,
    this.voiceQualityIssue,
    this.voiceDiagnostics,
    this.voiceBuildFingerprint,
    this.voiceTranscriptProvenance,
  });

  factory SharedContent.fromEvent(Map<Object?, Object?> event) {
    final source = switch (event['source']) {
      null || 'share' => SharedContentSource.share,
      'selection' => SharedContentSource.selection,
      'quick_capture' => SharedContentSource.quickCapture,
      'voice' => SharedContentSource.voice,
      _ => throw const FormatException('shared content has an invalid source'),
    };
    final rawText = event['text'];
    if (rawText is! String) {
      throw const FormatException('shared content is missing text');
    }
    final sanitizedBody = _sanitizeSharedText(rawText);
    final body = _truncateWithoutSplittingSurrogate(
      sanitizedBody,
      maxSharedTextLength,
    );
    if (body.isEmpty && source != SharedContentSource.quickCapture) {
      throw const FormatException('shared content is empty');
    }
    final eventId =
        source == SharedContentSource.quickCapture ||
                source == SharedContentSource.voice
            ? event['eventId']
            : null;
    if ((source == SharedContentSource.quickCapture ||
            source == SharedContentSource.voice) &&
        (eventId is! String || !_isQuickCaptureId(eventId))) {
      throw const FormatException('capture is missing its event id');
    }
    final voiceQualityIssue = switch ((
      source: source,
      value: event['voiceQualityIssue'],
    )) {
      (source: SharedContentSource.voice, value: null) => null,
      (source: SharedContentSource.voice, value: 'tooShort') =>
        VoiceCaptureQualityIssue.tooShort,
      (source: SharedContentSource.voice, value: 'tooQuiet') =>
        VoiceCaptureQualityIssue.tooQuiet,
      (source: SharedContentSource.voice, value: 'clipped') =>
        VoiceCaptureQualityIssue.clipped,
      (source: _, value: null) => null,
      _ =>
        throw const FormatException(
          'shared content has an invalid voice quality issue',
        ),
    };
    final voiceDiagnostics = switch ((
      source: source,
      value: event['voiceDiagnostics'],
    )) {
      (
        source: SharedContentSource.voice,
        value: VoiceCaptureDiagnostics diagnostics,
      ) =>
        diagnostics,
      (source: SharedContentSource.voice, value: null) => null,
      (source: _, value: null) => null,
      _ =>
        throw const FormatException(
          'shared content has invalid voice diagnostics',
        ),
    };
    final voiceBuildFingerprint = switch ((
      source: source,
      value: event['voiceBuildFingerprint'],
    )) {
      (
        source: SharedContentSource.voice,
        value: VoiceAppBuildFingerprint fingerprint,
      ) =>
        fingerprint,
      (source: SharedContentSource.voice, value: null) => null,
      (source: _, value: null) => null,
      _ =>
        throw const FormatException(
          'shared content has invalid voice build fingerprint',
        ),
    };
    final voiceTranscriptProvenance = switch ((
      source: source,
      value: event['voiceTranscriptProvenance'],
    )) {
      (source: SharedContentSource.voice, value: null || 'local') =>
        VoiceTranscriptProvenance.local,
      (source: SharedContentSource.voice, value: 'self_hosted') =>
        VoiceTranscriptProvenance.selfHosted,
      (source: _, value: null) => null,
      _ =>
        throw const FormatException(
          'shared content has invalid voice transcript provenance',
        ),
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
      eventId: eventId as String?,
      voiceQualityIssue: voiceQualityIssue,
      voiceDiagnostics: voiceDiagnostics,
      voiceBuildFingerprint: voiceBuildFingerprint,
      voiceTranscriptProvenance: voiceTranscriptProvenance,
    );
  }

  final String text;
  final String? subject;
  final bool truncated;
  final SharedContentSource source;
  final String? eventId;
  final VoiceCaptureQualityIssue? voiceQualityIssue;
  final VoiceCaptureDiagnostics? voiceDiagnostics;
  final VoiceAppBuildFingerprint? voiceBuildFingerprint;
  final VoiceTranscriptProvenance? voiceTranscriptProvenance;
}

bool _isQuickCaptureId(String value) => RegExp(
  r'^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$',
).hasMatch(value);

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
