import 'dart:convert';
import 'dart:io';

import 'package:tm_asr_benchmark/real_speaker_evaluation.dart';

Future<void> main(List<String> arguments) async {
  if (arguments.length < 2 || arguments.length > 3) {
    stderr.writeln(
      'usage: dart run bin/score_real_speaker.dart '
      'INPUT.json EXPECTED_FINAL_APK_SHA256 [OUTPUT.json]',
    );
    exitCode = 2;
    return;
  }
  try {
    final decoded = jsonDecode(await File(arguments.first).readAsString());
    if (decoded is! Map<String, dynamic>) {
      throw const FormatException('input must be a JSON object');
    }
    final report = evaluateRealSpeakerCorpus(
      RealSpeakerEvaluationInput.fromJson(
        decoded,
        expectedApkSha256: arguments[1],
      ),
    );
    final output = '${const JsonEncoder.withIndent('  ').convert(report)}\n';
    if (arguments.length == 3) {
      final input = File(arguments.first).absolute;
      final destination = File(arguments[2]).absolute;
      if (input.path == destination.path) {
        throw const FormatException(
          'output must not replace the private input',
        );
      }
      await _writeAtomically(destination, output);
    } else {
      stdout.write(output);
    }
    if (report['passed'] != true) exitCode = 1;
  } catch (error) {
    stderr.writeln('real-speaker evaluation rejected: $error');
    exitCode = 2;
  }
}

Future<void> _writeAtomically(File destination, String output) async {
  await destination.parent.create(recursive: true);
  final temporary = File('${destination.path}.tmp');
  await temporary.writeAsString(output, flush: true);
  await temporary.rename(destination.path);
}
