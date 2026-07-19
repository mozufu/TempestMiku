import 'dart:convert';

import 'package:crypto/crypto.dart';
import 'package:tm_asr_benchmark/benchmark_contract.dart';
import 'package:tm_asr_benchmark/benchmark_metrics.dart';

const int realSpeakerInputSchema = 1;
const String realSpeakerReportSchema = 'tempestmiku.p6-6.real-speaker-eval.v2';
const int minimumRealSpeakerItems = 10;
const int maximumRealSpeakerItems = 100;
const int minimumRealSpeakerNormalizedReferenceCharacters = 6;
const double maximumRealSpeakerMeanCer = 0.20;
const double maximumRealSpeakerP90Cer = 0.35;
const double maximumRealSpeakerCodeSwitchMeanCer = 0.30;
const String productionReleaseCertificateSha256 =
    '503f865843464347cb2d2f90be3ab4dbf68bae690443bf8fd262e0dcc0b58ee1';

final _captureUuidV4Pattern = RegExp(
  r'^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$',
  caseSensitive: false,
);

final _latinFillerPattern = RegExp(
  r'^(?:(?:u+h*|u+m+|h+m+|e+r+m*|a+h+|m+))+$',
  caseSensitive: false,
);
final _cjkFillerPattern = RegExp(r'^[嗯呃啊喔哦唔欸诶哎誒額]+$');

const Set<String> requiredRealSpeakerTags = <String>{
  'quiet',
  'noisy',
  'normal-pace',
  'fast',
  'disfluent-corrected',
  'taiwan-local',
  'numeric',
  'proper-name',
  'code-switch',
};

final class RealSpeakerEvaluationInput {
  const RealSpeakerEvaluationInput({
    required this.deviceFingerprint,
    required this.apkSha256,
    required this.certificateSha256,
    required this.localInputSha256,
    required this.items,
  });

  final String deviceFingerprint;
  final String apkSha256;
  final String certificateSha256;
  final String localInputSha256;
  final List<RealSpeakerEvaluationItem> items;

  factory RealSpeakerEvaluationInput.fromJson(
    Map<String, dynamic> json, {
    required String expectedApkSha256,
  }) {
    final localInputSha256 = _canonicalJsonSha256(json);
    _expectExactKeys(json, const <String>{
      'schema',
      'consent',
      'device',
      'engine',
      'modelId',
      'conversion',
      'items',
    });
    _expect(json, 'schema', realSpeakerInputSchema);
    _expect(json, 'engine', productionEngineSelector);
    _expect(json, 'modelId', productionModelId);
    _expect(json, 'conversion', 'Android ICU Simplified-Traditional');

    final consent = _object(json, 'consent');
    _expectExactKeys(consent, const <String>{
      'recordingExplicitlyConsented',
      'exactReferencesCaptured',
      'audioUploaded',
      'audioRetained',
    });
    _expect(consent, 'recordingExplicitlyConsented', true);
    _expect(consent, 'exactReferencesCaptured', true);
    _expect(consent, 'audioUploaded', false);
    _expect(consent, 'audioRetained', false);

    final device = _object(json, 'device');
    _expectExactKeys(device, const <String>{
      'fingerprint',
      'apkSha256',
      'certificateSha256',
    });
    final deviceFingerprint = _nonEmptyString(device, 'fingerprint');
    final apkSha256 = _sha256(device, 'apkSha256');
    if (!RegExp(r'^[0-9a-f]{64}$').hasMatch(expectedApkSha256)) {
      throw const FormatException(
        'expected APK SHA-256 must be a lowercase digest',
      );
    }
    if (apkSha256 != expectedApkSha256) {
      throw const FormatException(
        'real-speaker input APK did not match the caller-provided final APK',
      );
    }
    final certificateSha256 = _sha256(device, 'certificateSha256');
    if (certificateSha256 != productionReleaseCertificateSha256) {
      throw const FormatException('release certificate fingerprint drifted');
    }

    final encodedItems = json['items'];
    if (encodedItems is! List<Object?> ||
        encodedItems.length < minimumRealSpeakerItems ||
        encodedItems.length > maximumRealSpeakerItems) {
      throw const FormatException(
        'real-speaker corpus must contain 10 to 100 items',
      );
    }
    final ids = <String>{};
    final captureIds = <String>{};
    final references = <String>{};
    final coveredTags = <String>{};
    final items = <RealSpeakerEvaluationItem>[];
    for (final encoded in encodedItems) {
      if (encoded is! Map<Object?, Object?>) {
        throw const FormatException('real-speaker item must be an object');
      }
      final item = RealSpeakerEvaluationItem.fromJson(
        encoded.map((key, value) => MapEntry(key.toString(), value)),
      );
      if (!ids.add(item.id)) {
        throw FormatException('duplicate real-speaker item id: ${item.id}');
      }
      if (!captureIds.add(item.captureId.toLowerCase())) {
        throw FormatException(
          'duplicate real-speaker captureId: ${item.captureId}',
        );
      }
      final normalizedReference = normalizeForCharacterErrorRate(
        item.reference,
      );
      if (!references.add(normalizedReference)) {
        throw const FormatException(
          'real-speaker references must be unique after CER normalization',
        );
      }
      coveredTags.addAll(item.tags);
      items.add(item);
    }
    final missingTags = requiredRealSpeakerTags.difference(coveredTags);
    if (missingTags.isNotEmpty) {
      throw FormatException(
        'real-speaker corpus is missing tags: ${missingTags.join(', ')}',
      );
    }
    return RealSpeakerEvaluationInput(
      deviceFingerprint: deviceFingerprint,
      apkSha256: apkSha256,
      certificateSha256: certificateSha256,
      localInputSha256: localInputSha256,
      items: List<RealSpeakerEvaluationItem>.unmodifiable(items),
    );
  }
}

final class RealSpeakerEvaluationItem {
  const RealSpeakerEvaluationItem({
    required this.id,
    required this.captureId,
    required this.reference,
    required this.hypothesis,
    required this.tags,
    required this.truncated,
    required this.qualityIssue,
  });

  final String id;
  final String captureId;
  final String reference;
  final String hypothesis;
  final Set<String> tags;
  final bool truncated;
  final String? qualityIssue;

  factory RealSpeakerEvaluationItem.fromJson(Map<String, dynamic> json) {
    _expectExactKeys(json, const <String>{
      'id',
      'captureId',
      'reference',
      'hypothesis',
      'tags',
      'truncated',
      'qualityIssue',
    });
    final id = _nonEmptyString(json, 'id');
    if (!RegExp(r'^[a-z0-9][a-z0-9_-]{0,63}$').hasMatch(id)) {
      throw const FormatException('real-speaker item id is invalid');
    }
    final captureId = _nonEmptyString(json, 'captureId');
    if (!_captureUuidV4Pattern.hasMatch(captureId)) {
      throw const FormatException('captureId must be a UUIDv4');
    }
    final reference = _nonEmptyString(json, 'reference').trim();
    final placeholderReference = reference.toLowerCase();
    if (placeholderReference.contains('replace_with') ||
        placeholderReference.contains('placeholder') ||
        RegExp(r'\b(?:todo|tbd)\b').hasMatch(placeholderReference)) {
      throw const FormatException(
        'reference must be an exact non-placeholder utterance',
      );
    }
    _validateMeaningfulReference(reference);
    final hypothesis = json['hypothesis'];
    if (hypothesis is! String) {
      throw const FormatException('hypothesis must be a string');
    }
    if (reference.length > 16_384 || hypothesis.length > 16_384) {
      throw const FormatException('real-speaker text exceeds 16,384 units');
    }
    final encodedTags = json['tags'];
    if (encodedTags is! List<Object?> || encodedTags.isEmpty) {
      throw const FormatException('real-speaker item tags are required');
    }
    final tags = <String>{};
    for (final tag in encodedTags) {
      if (tag is! String || !requiredRealSpeakerTags.contains(tag)) {
        throw FormatException('unknown real-speaker tag: $tag');
      }
      if (!tags.add(tag)) {
        throw FormatException('duplicate real-speaker tag: $tag');
      }
    }
    final truncated = json['truncated'];
    if (truncated is! bool) {
      throw const FormatException('truncated must be a boolean');
    }
    final qualityIssue = json['qualityIssue'];
    if (qualityIssue != null &&
        qualityIssue != 'tooShort' &&
        qualityIssue != 'tooQuiet' &&
        qualityIssue != 'clipped') {
      throw const FormatException('qualityIssue is invalid');
    }
    return RealSpeakerEvaluationItem(
      id: id,
      captureId: captureId,
      reference: reference,
      hypothesis: hypothesis.trim(),
      tags: Set<String>.unmodifiable(tags),
      truncated: truncated,
      qualityIssue: qualityIssue as String?,
    );
  }
}

Map<String, dynamic> evaluateRealSpeakerCorpus(
  RealSpeakerEvaluationInput input, {
  DateTime? evaluatedAt,
}) {
  final items = <Map<String, dynamic>>[];
  final allCer = <double>[];
  final codeSwitchCer = <double>[];
  var emptyItems = 0;
  var truncatedItems = 0;
  var signalQualityWarningItems = 0;
  for (final item in input.items) {
    final normalizedReference = normalizeForCharacterErrorRate(item.reference);
    final normalizedHypothesis = normalizeForCharacterErrorRate(
      item.hypothesis,
    );
    final cer = characterErrorRate(item.reference, item.hypothesis);
    allCer.add(cer);
    if (item.tags.contains('code-switch')) codeSwitchCer.add(cer);
    if (item.hypothesis.isEmpty) emptyItems += 1;
    if (item.truncated) truncatedItems += 1;
    if (item.qualityIssue != null) signalQualityWarningItems += 1;
    items.add(<String, dynamic>{
      'id': item.id,
      'tags': item.tags.toList()..sort(),
      'normalized_reference_characters': normalizedReference.runes.length,
      'normalized_hypothesis_characters': normalizedHypothesis.runes.length,
      'converted_cer': cer,
      'empty': item.hypothesis.isEmpty,
      'truncated': item.truncated,
      'quality_issue': item.qualityIssue,
    });
  }
  final meanCer = _mean(allCer);
  final p90Cer = _nearestRankPercentile(allCer, 0.90);
  final codeSwitchMeanCer = _mean(codeSwitchCer);
  final failures = <String>[
    if (meanCer > maximumRealSpeakerMeanCer) 'converted_mean_cer',
    if (p90Cer > maximumRealSpeakerP90Cer) 'converted_p90_cer',
    if (codeSwitchMeanCer > maximumRealSpeakerCodeSwitchMeanCer)
      'code_switch_converted_mean_cer',
    if (emptyItems != 0) 'empty_output',
    if (truncatedItems != 0) 'truncated_output',
    if (signalQualityWarningItems != 0) 'signal_quality_warning',
  ];
  return <String, dynamic>{
    'schema': realSpeakerReportSchema,
    'evaluated_at':
        (evaluatedAt ?? DateTime.now().toUtc()).toUtc().toIso8601String(),
    'passed': failures.isEmpty,
    'evidence_scope': 'consented_real_speaker_text_metrics',
    'audio_retained': false,
    'audio_uploaded': false,
    'engine': productionEngineSelector,
    'model_id': productionModelId,
    'conversion': 'Android ICU Simplified-Traditional',
    'release': <String, dynamic>{
      'apk_sha256': input.apkSha256,
      'certificate_sha256': input.certificateSha256,
    },
    'input_binding': <String, dynamic>{
      'algorithm': 'sha256',
      'canonicalization': 'recursive_sorted_json_keys',
      'sha256': input.localInputSha256,
      'includes_random_capture_ids': true,
    },
    'privacy': const <String, dynamic>{
      'audio_retained': false,
      'audio_uploaded': false,
      'raw_device_fingerprint_retained': false,
      'capture_ids_retained': false,
      'reference_text_retained': false,
      'hypothesis_text_retained': false,
      'per_item_text_hashes_retained': false,
    },
    'limits': <String, dynamic>{
      'minimum_items': minimumRealSpeakerItems,
      'minimum_normalized_reference_characters':
          minimumRealSpeakerNormalizedReferenceCharacters,
      'maximum_converted_mean_cer': maximumRealSpeakerMeanCer,
      'maximum_converted_p90_cer': maximumRealSpeakerP90Cer,
      'maximum_code_switch_converted_mean_cer':
          maximumRealSpeakerCodeSwitchMeanCer,
      'required_tags': requiredRealSpeakerTags.toList()..sort(),
      'empty_items': 0,
      'truncated_items': 0,
      'signal_quality_warning_items': 0,
    },
    'metrics': <String, dynamic>{
      'items': input.items.length,
      'converted_mean_cer': meanCer,
      'converted_p90_cer': p90Cer,
      'code_switch_converted_mean_cer': codeSwitchMeanCer,
      'empty_items': emptyItems,
      'truncated_items': truncatedItems,
      'signal_quality_warning_items': signalQualityWarningItems,
    },
    'gates': <String, dynamic>{
      'passed': failures.isEmpty,
      'failures': failures,
    },
    // Exact text, raw device fingerprint, capture IDs, and per-item text
    // hashes stay in the owner's ignored local input. Random capture IDs salt
    // the single whole-input binding without being disclosed here.
    'items': items,
  };
}

double _mean(List<double> values) =>
    values.fold<double>(0, (sum, value) => sum + value) / values.length;

double _nearestRankPercentile(List<double> values, double percentile) {
  final sorted = values.toList()..sort();
  final rank =
      (percentile * sorted.length).ceil().clamp(1, sorted.length).toInt();
  return sorted[rank - 1];
}

void _validateMeaningfulReference(String reference) {
  final normalized = normalizeForCharacterErrorRate(reference);
  final runes = normalized.runes.toList(growable: false);
  if (runes.length < minimumRealSpeakerNormalizedReferenceCharacters) {
    throw const FormatException(
      'reference must contain at least 6 CER-normalized characters',
    );
  }
  if (!runes.any(_isLexicalRune)) {
    throw const FormatException(
      'reference must contain lexical letters, numbers, or CJK text',
    );
  }
  if (_latinFillerPattern.hasMatch(normalized) ||
      _cjkFillerPattern.hasMatch(normalized)) {
    throw const FormatException(
      'reference must be a lexical utterance, not a filler vocalization',
    );
  }
}

bool _isLexicalRune(int rune) {
  return (rune >= 0x30 && rune <= 0x39) ||
      (rune >= 0x41 && rune <= 0x5a) ||
      (rune >= 0x61 && rune <= 0x7a) ||
      (rune >= 0xc0 && rune <= 0x2af) ||
      (rune >= 0x3100 && rune <= 0x312f) ||
      (rune >= 0x3400 && rune <= 0x4dbf) ||
      (rune >= 0x4e00 && rune <= 0x9fff) ||
      (rune >= 0xf900 && rune <= 0xfaff) ||
      (rune >= 0xff10 && rune <= 0xff19) ||
      (rune >= 0x20000 && rune <= 0x3134f);
}

String _canonicalJsonSha256(Map<String, dynamic> input) {
  final canonical = jsonEncode(_canonicalJsonValue(input));
  return sha256.convert(utf8.encode(canonical)).toString();
}

Object? _canonicalJsonValue(Object? value) {
  if (value is Map<Object?, Object?>) {
    final keys = value.keys.map((key) => key.toString()).toList()..sort();
    return <String, Object?>{
      for (final key in keys) key: _canonicalJsonValue(value[key]),
    };
  }
  if (value is List<Object?>) {
    return <Object?>[for (final item in value) _canonicalJsonValue(item)];
  }
  if (value == null || value is String || value is num || value is bool) {
    return value;
  }
  throw FormatException(
    'input contains a non-JSON value of type ${value.runtimeType}',
  );
}

Map<String, dynamic> _object(Map<String, dynamic> json, String key) {
  final value = json[key];
  if (value is! Map<Object?, Object?>) {
    throw FormatException('$key must be an object');
  }
  return value.map((key, value) => MapEntry(key.toString(), value));
}

void _expect(Map<Object?, Object?> json, String key, Object expected) {
  if (json[key] != expected) throw FormatException('$key must equal $expected');
}

void _expectExactKeys(
  Map<Object?, Object?> json,
  Set<String> required, {
  Set<String> optional = const <String>{},
}) {
  final keys = json.keys.map((key) => key.toString()).toSet();
  final missing = required.difference(keys);
  final unknown = keys.difference(required.union(optional));
  if (missing.isNotEmpty || unknown.isNotEmpty) {
    throw FormatException(
      'object keys did not match contract; missing=${missing.join(',')}; '
      'unknown=${unknown.join(',')}',
    );
  }
}

String _nonEmptyString(Map<Object?, Object?> json, String key) {
  final value = json[key];
  if (value is! String || value.trim().isEmpty) {
    throw FormatException('$key must be a non-empty string');
  }
  return value;
}

String _sha256(Map<Object?, Object?> json, String key) {
  final value = _nonEmptyString(json, key);
  if (!RegExp(r'^[0-9a-f]{64}$').hasMatch(value)) {
    throw FormatException('$key must be a lowercase SHA-256 digest');
  }
  return value;
}
