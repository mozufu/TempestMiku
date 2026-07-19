import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:tm_asr_benchmark/benchmark_contract.dart';
import 'package:tm_asr_benchmark/real_speaker_evaluation.dart';

void main() {
  test(
    'passing corpus covers consent, exact references, and every category',
    () {
      final input = RealSpeakerEvaluationInput.fromJson(
        _input(),
        expectedApkSha256: _digest('a'),
      );
      final report = evaluateRealSpeakerCorpus(
        input,
        evaluatedAt: DateTime.utc(2026, 7, 18),
      );

      expect(report['passed'], isTrue);
      expect(report['schema'], realSpeakerReportSchema);
      expect(report['audio_retained'], isFalse);
      expect(report['audio_uploaded'], isFalse);
      final metrics = report['metrics']! as Map<String, dynamic>;
      expect(metrics['items'], minimumRealSpeakerItems);
      expect(metrics['converted_mean_cer'], 0);
      expect(metrics['converted_p90_cer'], 0);
      final items = report['items']! as List<Map<String, dynamic>>;
      expect(items.first, isNot(contains('reference')));
      expect(items.first, isNot(contains('hypothesis')));
      expect(items.first, isNot(contains('reference_sha256')));
      expect(items.first, isNot(contains('hypothesis_sha256')));
      expect(items.first, isNot(contains('capture_id')));
      expect(items.first, containsPair('quality_issue', null));
      final encodedReport = jsonEncode(report);
      expect(encodedReport, isNot(contains(input.deviceFingerprint)));
      expect(encodedReport, isNot(contains(input.items.first.captureId)));
      expect(encodedReport, isNot(contains(input.items.first.reference)));
      final binding = report['input_binding']! as Map<String, dynamic>;
      expect(binding['sha256'], input.localInputSha256);
      expect(binding['sha256'], hasLength(64));
      expect(
        report['privacy'],
        containsPair('per_item_text_hashes_retained', false),
      );
    },
  );

  test('quality, empty, and truncation failures remain explicit', () {
    final encoded = _input();
    final items = encoded['items']! as List<Map<String, dynamic>>;
    for (final item in items) {
      item['hypothesis'] = '完全不同的輸出';
    }
    items.first['hypothesis'] = '';
    items.first['qualityIssue'] = 'tooQuiet';
    items[1]['truncated'] = true;

    final report = evaluateRealSpeakerCorpus(
      RealSpeakerEvaluationInput.fromJson(
        encoded,
        expectedApkSha256: _digest('a'),
      ),
    );
    expect(report['passed'], isFalse);
    expect(
      (report['gates']! as Map<String, dynamic>)['failures'],
      containsAll(<String>[
        'converted_mean_cer',
        'converted_p90_cer',
        'code_switch_converted_mean_cer',
        'empty_output',
        'truncated_output',
        'signal_quality_warning',
      ]),
    );
  });

  test('missing consent, categories, and unknown fields fail closed', () {
    final noConsent = _input();
    (noConsent['consent']!
            as Map<String, dynamic>)['recordingExplicitlyConsented'] =
        false;
    expect(
      () => RealSpeakerEvaluationInput.fromJson(
        noConsent,
        expectedApkSha256: _digest('a'),
      ),
      throwsFormatException,
    );

    final missingCategory = _input();
    for (final item
        in missingCategory['items']! as List<Map<String, dynamic>>) {
      (item['tags']! as List<String>).remove('code-switch');
      if ((item['tags']! as List<String>).isEmpty) {
        item['tags'] = <String>['normal-pace'];
      }
    }
    expect(
      () => RealSpeakerEvaluationInput.fromJson(
        missingCategory,
        expectedApkSha256: _digest('a'),
      ),
      throwsFormatException,
    );

    final unknown = _input();
    (unknown['items']! as List<Map<String, dynamic>>).first['audioPath'] =
        '/private/recording.wav';
    expect(
      () => RealSpeakerEvaluationInput.fromJson(
        unknown,
        expectedApkSha256: _digest('a'),
      ),
      throwsFormatException,
    );
  });

  test('APK, capture, quality, and exact-reference bindings fail closed', () {
    expect(
      () => RealSpeakerEvaluationInput.fromJson(
        _input(),
        expectedApkSha256: _digest('b'),
      ),
      throwsFormatException,
    );

    final missingQuality = _input();
    (missingQuality['items']! as List<Map<String, dynamic>>).first.remove(
      'qualityIssue',
    );
    expect(
      () => RealSpeakerEvaluationInput.fromJson(
        missingQuality,
        expectedApkSha256: _digest('a'),
      ),
      throwsFormatException,
    );

    final badCapture = _input();
    (badCapture['items']! as List<Map<String, dynamic>>).first['captureId'] =
        'not-a-uuid';
    expect(
      () => RealSpeakerEvaluationInput.fromJson(
        badCapture,
        expectedApkSha256: _digest('a'),
      ),
      throwsFormatException,
    );

    final duplicateCapture = _input();
    final captureItems =
        duplicateCapture['items']! as List<Map<String, dynamic>>;
    captureItems[1]['captureId'] = captureItems.first['captureId'];
    expect(
      () => RealSpeakerEvaluationInput.fromJson(
        duplicateCapture,
        expectedApkSha256: _digest('a'),
      ),
      throwsFormatException,
    );

    final duplicateReference = _input();
    final duplicateItems =
        duplicateReference['items']! as List<Map<String, dynamic>>;
    duplicateItems[1]['reference'] = duplicateItems.first['reference'];
    expect(
      () => RealSpeakerEvaluationInput.fromJson(
        duplicateReference,
        expectedApkSha256: _digest('a'),
      ),
      throwsFormatException,
    );

    final placeholder = _input();
    (placeholder['items']! as List<Map<String, dynamic>>).first['reference'] =
        'REPLACE_WITH_EXACT_CONSENTED_SENTENCE';
    expect(
      () => RealSpeakerEvaluationInput.fromJson(
        placeholder,
        expectedApkSha256: _digest('a'),
      ),
      throwsFormatException,
    );
  });

  test('references use CER normalization and reject nonlexical audio', () {
    for (final invalid in <String>[
      '，。！？……',
      'ummmmm',
      'Uh-h-h-h',
      '嗯、呃、啊、嗯、呃、啊',
      '短句',
      '😶😶😶😶😶😶',
    ]) {
      final encoded = _input();
      (encoded['items']! as List<Map<String, dynamic>>).first['reference'] =
          invalid;
      expect(
        () => RealSpeakerEvaluationInput.fromJson(
          encoded,
          expectedApkSha256: _digest('a'),
        ),
        throwsFormatException,
        reason: 'reference should be rejected: $invalid',
      );
    }

    final normalizedDuplicate = _input();
    final duplicateItems =
        normalizedDuplicate['items']! as List<Map<String, dynamic>>;
    duplicateItems[1]['reference'] =
        '  ${duplicateItems.first['reference']}，！？  ';
    expect(
      () => RealSpeakerEvaluationInput.fromJson(
        normalizedDuplicate,
        expectedApkSha256: _digest('a'),
      ),
      throwsFormatException,
    );
  });

  test('whole-input binding changes with an undisclosed capture UUID', () {
    final first = RealSpeakerEvaluationInput.fromJson(
      _input(),
      expectedApkSha256: _digest('a'),
    );
    final changed = _input();
    (changed['items']! as List<Map<String, dynamic>>).first['captureId'] =
        'd44b2760-49d8-4e90-bec7-390c95f11a7b';
    final second = RealSpeakerEvaluationInput.fromJson(
      changed,
      expectedApkSha256: _digest('a'),
    );

    expect(second.localInputSha256, isNot(first.localInputSha256));
    final report = evaluateRealSpeakerCorpus(second);
    final encodedReport = jsonEncode(report);
    expect(encodedReport, isNot(contains(second.items.first.captureId)));
    expect(encodedReport, isNot(contains(second.deviceFingerprint)));
  });
}

const _captureIds = <String>[
  '2f8c3d4e-5a61-4b72-8c83-94d5e6f70819',
  '31a4b5c6-d7e8-49f0-a1b2-c3d4e5f60718',
  '4c5d6e7f-8091-4a2b-b3c4-d5e6f708192a',
  '5d6e7f80-91a2-4b3c-84d5-e6f708192a3b',
  '6e7f8091-a2b3-4c4d-95e6-f708192a3b4c',
  '7f8091a2-b3c4-4d5e-a6f7-08192a3b4c5d',
  '8091a2b3-c4d5-4e6f-b708-192a3b4c5d6e',
  '91a2b3c4-d5e6-4f70-8819-2a3b4c5d6e7f',
  'a2b3c4d5-e6f7-4081-992a-3b4c5d6e7f80',
  'b3c4d5e6-f708-4192-aa3b-4c5d6e7f8091',
];

Map<String, dynamic> _input() => <String, dynamic>{
  'schema': realSpeakerInputSchema,
  'consent': <String, dynamic>{
    'recordingExplicitlyConsented': true,
    'exactReferencesCaptured': true,
    'audioUploaded': false,
    'audioRetained': false,
  },
  'device': <String, dynamic>{
    'fingerprint': 'vendor/device/device:15/build/id:user/release-keys',
    'apkSha256': _digest('a'),
    'certificateSha256': productionReleaseCertificateSha256,
  },
  'engine': productionEngineSelector,
  'modelId': productionModelId,
  'conversion': 'Android ICU Simplified-Traditional',
  'items': <Map<String, dynamic>>[
    for (var index = 0; index < minimumRealSpeakerItems; index += 1)
      <String, dynamic>{
        'id': 'speaker-${index + 1}',
        'captureId': _captureIds[index],
        'reference': '這是第${index + 1}句精確參考文字',
        'hypothesis': '這是第${index + 1}句精確參考文字',
        'tags': <String>[
          requiredRealSpeakerTags.elementAt(
            index % requiredRealSpeakerTags.length,
          ),
        ],
        'truncated': false,
        'qualityIssue': null,
      },
  ],
};

String _digest(String character) => List<String>.filled(64, character).join();
