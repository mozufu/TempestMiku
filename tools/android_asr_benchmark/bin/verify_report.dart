import 'dart:convert';
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:tm_asr_benchmark/report_verification.dart';

Future<void> main(List<String> arguments) async {
  if (arguments.length < 3 || arguments.length > 4) {
    stderr.writeln(
      'usage: dart run bin/verify_report.dart '
      'ENGINE CORPUS_MANIFEST BENCHMARK_APK [REPORT.json]',
    );
    exitCode = 2;
    return;
  }
  try {
    final sourceBytes =
        arguments.length == 4
            ? await File(arguments[3]).readAsBytes()
            : await stdin.expand((chunk) => chunk).toList();
    final decoded = jsonDecode(utf8.decode(sourceBytes));
    if (decoded is! Map<String, dynamic>) {
      throw const FormatException('report must be a JSON object');
    }
    final corpus = await BenchmarkCorpusEvidence.load(File(arguments[1]));
    final apkSha256 = await sha256File(File(arguments[2]));
    verifyBenchmarkReport(
      report: decoded,
      expectedEngine: arguments[0],
      corpusEvidence: corpus,
      expectedBenchmarkApkSha256: apkSha256,
      sourceSha256: sha256.convert(sourceBytes).toString(),
    );
    stdout.writeln(const JsonEncoder.withIndent('  ').convert(decoded));
    stderr.writeln(
      'verified independently recomputed physical-Android report for '
      '${arguments[0]}',
    );
  } catch (error) {
    stderr.writeln('benchmark report rejected: $error');
    exitCode = 1;
  }
}
