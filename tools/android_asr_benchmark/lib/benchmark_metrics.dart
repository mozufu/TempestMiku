const int maxModelBytes = 350 * 1024 * 1024;
const int maxPeakRssKiB = 1024 * 1024;
const int maxInferenceDurationMilliseconds = 45000;
const double maxRealTimeFactor = 0.5;
const int minimumCorpusItems = 50;
const int requiredLongRuns = 3;
const double longRunMinimumSeconds = 59;

Map<String, Object> evaluateGates({
  required int modelBytes,
  required int peakRssKiB,
  required int maxInferenceMilliseconds,
  required double realTimeFactor,
  required int corpusItems,
  required int longRuns,
}) {
  final failures = <String>[];
  if (modelBytes > maxModelBytes) failures.add('model_bytes');
  if (peakRssKiB > maxPeakRssKiB) failures.add('peak_rss_kib');
  if (maxInferenceMilliseconds > maxInferenceDurationMilliseconds) {
    failures.add('inference_timeout');
  }
  if (realTimeFactor > maxRealTimeFactor) failures.add('max_rtf');
  if (corpusItems < minimumCorpusItems) failures.add('corpus_items');
  if (longRuns < requiredLongRuns) failures.add('long_runs');
  return <String, Object>{
    'passed': failures.isEmpty,
    'failures': List<String>.unmodifiable(failures),
  };
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

List<int> _normalizedRunes(String value) {
  final normalized = value.toLowerCase().replaceAll(
    RegExp(r'''[\s，。！？、,.!?;；:："'“”‘’「」『』（）()【】\[\]]+'''),
    '',
  );
  return normalized.runes.toList(growable: false);
}

int _minimum3(int first, int second, int third) {
  var result = first < second ? first : second;
  if (third < result) result = third;
  return result;
}
