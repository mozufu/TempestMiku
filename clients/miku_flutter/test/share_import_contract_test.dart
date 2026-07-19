import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/miku_api.dart';

void main() {
  test('shared content parser sanitizes, combines, and bounds input', () {
    final parsed = SharedContent.fromEvent({
      'text': ' https://example.test/\u0000path ',
      'subject': ' Example title\u0007 ',
      'truncated': false,
    });
    expect(parsed.text, 'Example title\n\nhttps://example.test/path');
    expect(parsed.subject, 'Example title');
    expect(parsed.truncated, isFalse);
    expect(parsed.source, SharedContentSource.share);

    final selection = SharedContent.fromEvent({
      'text': ' explain this ',
      'source': 'selection',
      'subject': 'ignored share subject',
    });
    expect(selection.text, 'explain this');
    expect(selection.subject, isNull);
    expect(selection.source, SharedContentSource.selection);

    final capture = SharedContent.fromEvent({
      'text': '',
      'source': 'quick_capture',
      'eventId': '12345678-1234-4abc-8def-1234567890ab',
    });
    expect(capture.text, isEmpty);
    expect(capture.source, SharedContentSource.quickCapture);

    final voice = SharedContent.fromEvent({
      'text': 'local draft',
      'source': 'voice',
      'eventId': '12345678-1234-4abc-8def-1234567890ab',
      'voiceQualityIssue': 'tooQuiet',
      'voiceTranscriptProvenance': 'self_hosted',
      'voiceDiagnostics': const VoiceCaptureDiagnostics(
        duration: Duration(seconds: 2),
        rmsDbfs: -24,
        peakDbfs: -3,
        clippedFraction: 0,
        nearZeroFraction: 0.1,
        activeFrameFraction: 0.8,
        leadingSilence: Duration(milliseconds: 100),
        trailingSilence: Duration(milliseconds: 200),
      ),
    });
    expect(voice.voiceQualityIssue, VoiceCaptureQualityIssue.tooQuiet);
    expect(
      voice.voiceTranscriptProvenance,
      VoiceTranscriptProvenance.selfHosted,
    );

    final bounded = SharedContent.fromEvent({
      'text': 'x' * maxSharedTextLength,
      'subject': 'title',
    });
    expect(bounded.text.length, maxSharedTextLength);
    expect(bounded.truncated, isTrue);

    final emojiBoundary = SharedContent.fromEvent({
      'text': '${'x' * (maxSharedTextLength - 1)}😀',
    });
    expect(emojiBoundary.text.length, maxSharedTextLength - 1);
    expect(emojiBoundary.truncated, isTrue);

    expect(
      () => SharedContent.fromEvent({'text': ' \u0000 '}),
      throwsFormatException,
    );
    expect(
      () => SharedContent.fromEvent({'text': 'x', 'source': 'unknown'}),
      throwsFormatException,
    );
    expect(
      () => SharedContent.fromEvent({
        'text': 'x',
        'voiceDiagnostics': VoiceCaptureDiagnostics.fromPcm16(
          Uint8List.fromList([0, 0]),
          sampleRate: localAsrSampleRate,
        ),
      }),
      throwsFormatException,
    );
  });
}
