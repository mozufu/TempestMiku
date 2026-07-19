const int maxModelBytes = 350 * 1024 * 1024;
const int maxPeakRssKiB = 1024 * 1024;
const int maxInferenceDurationMilliseconds = 45000;
const double maxRealTimeFactor = 0.5;
const int minimumCorpusItems = 50;
const int requiredLongRuns = 3;
const double longRunMinimumSeconds = 59;
const double maxConvertedMeanCer = 0.12;
const double maxCodeSwitchConvertedMeanCer = 0.25;
const double minimumLongRunReferenceCoverage = 0.75;
const double minimumLongRunTailReferenceCoverage = 0.60;

Map<String, Object> evaluateGates({
  required bool ranOnAndroidDevice,
  required int modelBytes,
  required int peakRssKiB,
  required int maxInferenceMilliseconds,
  required double realTimeFactor,
  required bool thermalStatusAvailable,
  required bool corpusIntegrityBound,
  required int corpusItems,
  required int longRuns,
  required int completedItems,
  required int failedItems,
  required int emptyRawItems,
  required int emptyConvertedItems,
  required int completedLongRuns,
  required int nonTruncatedLongRuns,
  required double? convertedMeanCer,
  required double? codeSwitchConvertedMeanCer,
}) {
  final failures = <String>[];
  if (!ranOnAndroidDevice) failures.add('android_device_execution');
  if (modelBytes > maxModelBytes) failures.add('model_bytes');
  if (peakRssKiB <= 0) {
    failures.add('peak_rss_unavailable');
  } else if (peakRssKiB > maxPeakRssKiB) {
    failures.add('peak_rss_kib');
  }
  if (maxInferenceMilliseconds > maxInferenceDurationMilliseconds) {
    failures.add('inference_timeout');
  }
  if (realTimeFactor > maxRealTimeFactor) failures.add('max_rtf');
  if (!thermalStatusAvailable) failures.add('thermal_status_unavailable');
  if (!corpusIntegrityBound) failures.add('corpus_integrity');
  if (corpusItems < minimumCorpusItems) failures.add('corpus_items');
  if (longRuns < requiredLongRuns) failures.add('long_runs');
  if (completedItems != corpusItems || failedItems != 0) {
    failures.add('completion');
  }
  if (emptyRawItems != 0) failures.add('empty_raw_output');
  if (emptyConvertedItems != 0) failures.add('empty_converted_output');
  if (completedLongRuns < requiredLongRuns) {
    failures.add('long_run_completion');
  }
  if (nonTruncatedLongRuns < requiredLongRuns) {
    failures.add('long_run_reference_coverage');
  }
  if (convertedMeanCer == null || convertedMeanCer > maxConvertedMeanCer) {
    failures.add('converted_mean_cer');
  }
  if (codeSwitchConvertedMeanCer == null ||
      codeSwitchConvertedMeanCer > maxCodeSwitchConvertedMeanCer) {
    failures.add('code_switch_converted_mean_cer');
  }
  return <String, Object>{
    'passed': failures.isEmpty,
    'failures': List<String>.unmodifiable(failures),
  };
}

/// Fraction of normalized reference characters preserved in order.
///
/// Unlike transcript length alone, longest-common-subsequence coverage cannot
/// be satisfied by padding a truncated result with unrelated text. The long
/// run gate uses this as a deterministic wholesale-truncation detector; CER
/// remains the separate accuracy metric.
double referenceLcsCoverage(String expected, String actual) {
  final reference = _normalizedRunes(expected);
  final hypothesis = _normalizedRunes(actual);
  if (reference.isEmpty) return hypothesis.isEmpty ? 1 : 0;
  return _lcsLength(reference, hypothesis) / reference.length;
}

/// Coverage attributable specifically to the final third of the reference.
///
/// This subtracts the best LCS match available to the first two thirds from
/// the full-reference match. A result containing only a long utterance's
/// prefix therefore scores zero for its missing tail even when the overall LCS
/// coverage is 0.75 or unrelated padding makes the output look long enough.
double referenceTailLcsCoverage(String expected, String actual) {
  final reference = _normalizedRunes(expected);
  final hypothesis = _normalizedRunes(actual);
  if (reference.isEmpty) return hypothesis.isEmpty ? 1 : 0;
  final tailStart = (reference.length * 2) ~/ 3;
  final tailLength = reference.length - tailStart;
  final fullMatch = _lcsLength(reference, hypothesis);
  final prefixMatch = _lcsLength(reference.sublist(0, tailStart), hypothesis);
  final tailMatch = _max2(0, fullMatch - prefixMatch);
  final coverage = tailMatch / tailLength;
  return coverage > 1 ? 1 : coverage;
}

int _lcsLength(List<int> reference, List<int> hypothesis) {
  var previous = List<int>.filled(hypothesis.length + 1, 0);
  for (final expectedRune in reference) {
    final current = List<int>.filled(hypothesis.length + 1, 0);
    for (var index = 0; index < hypothesis.length; index += 1) {
      current[index + 1] =
          expectedRune == hypothesis[index]
              ? previous[index] + 1
              : _max2(current[index], previous[index + 1]);
    }
    previous = current;
  }
  return previous.last;
}

double characterErrorRate(String expected, String actual) {
  final reference = _normalizedRunes(expected);
  final hypothesis = _normalizedRunes(actual);
  if (reference.isEmpty) return hypothesis.isEmpty ? 0 : 1;

  var previous = List<int>.generate(hypothesis.length + 1, (index) => index);
  for (
    var referenceIndex = 0;
    referenceIndex < reference.length;
    referenceIndex++
  ) {
    final current = List<int>.filled(hypothesis.length + 1, 0);
    current[0] = referenceIndex + 1;
    for (
      var hypothesisIndex = 0;
      hypothesisIndex < hypothesis.length;
      hypothesisIndex++
    ) {
      final substitutionCost =
          reference[referenceIndex] == hypothesis[hypothesisIndex] ? 0 : 1;
      current[hypothesisIndex + 1] = _minimum3(
        current[hypothesisIndex] + 1,
        previous[hypothesisIndex + 1] + 1,
        previous[hypothesisIndex] + substitutionCost,
      );
    }
    previous = current;
  }
  return previous.last / reference.length;
}

/// Returns the exact text representation used by CER and LCS metrics.
///
/// Callers that validate corpus identity or minimum lexical content must use
/// this function too. Keeping one normalization path prevents a reference
/// from looking unique or meaningful to the schema while collapsing to an
/// empty or duplicate value when CER is calculated.
String normalizeForCharacterErrorRate(String value) {
  return value.toLowerCase().replaceAll(
    RegExp(
      r'''[\s，。！？、,.!?;；:："'“”‘’「」『』（）()【】\[\]{}<>《》〈〉…‥—–\-_~`@#$%\^&\*\+=|\\/·]+''',
    ),
    '',
  );
}

List<int> _normalizedRunes(String value) {
  return normalizeForCharacterErrorRate(value).runes.toList(growable: false);
}

int _minimum3(int first, int second, int third) {
  var result = first < second ? first : second;
  if (third < result) result = third;
  return result;
}

int _max2(int first, int second) => first > second ? first : second;
