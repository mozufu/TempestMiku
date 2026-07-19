import 'dart:convert';
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:tm_asr_benchmark/report_verification.dart';

Future<void> main(List<String> arguments) async {
  if (arguments.length < 4 || arguments.length > 5) {
    stderr.writeln(
      'usage: dart run bin/verify_pair.dart '
      'PRODUCTION_REPORT.json CANDIDATE_REPORT.json '
      'CORPUS_MANIFEST BENCHMARK_APK [OUTPUT.json]',
    );
    exitCode = 2;
    return;
  }
  try {
    final productionBytes = await File(arguments[0]).readAsBytes();
    final candidateBytes = await File(arguments[1]).readAsBytes();
    final production = _decodeReport(productionBytes, 'production');
    final candidate = _decodeReport(candidateBytes, 'candidate');
    final corpus = await BenchmarkCorpusEvidence.load(File(arguments[2]));
    final apkSha256 = await sha256File(File(arguments[3]));
    final report = verifyBenchmarkPair(
      productionReport: production,
      productionReportSha256: sha256.convert(productionBytes).toString(),
      candidateReport: candidate,
      candidateReportSha256: sha256.convert(candidateBytes).toString(),
      corpusEvidence: corpus,
      expectedBenchmarkApkSha256: apkSha256,
    );
    final output = '${const JsonEncoder.withIndent('  ').convert(report)}\n';
    if (arguments.length == 5) {
      await File(arguments[4]).writeAsString(output, flush: true);
    } else {
      stdout.write(output);
    }
    stderr.writeln(
      'verified production/candidate reports from the same physical '
      'installation, Android build, APK, and corpus',
    );
  } catch (error) {
    stderr.writeln('benchmark A/B pair rejected: $error');
    exitCode = 1;
  }
}

Map<String, dynamic> _decodeReport(List<int> bytes, String label) {
  final decoded = jsonDecode(utf8.decode(bytes));
  if (decoded is! Map<String, dynamic>) {
    throw FormatException('$label report must be a JSON object');
  }
  return decoded;
}
