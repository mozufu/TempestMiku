import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/asr/local_asr_model.dart';

void main() {
  test(
    'accepts only complete ready status for the pinned production model',
    () {
      final status = LocalAsrModelStatus.fromChannel({
        'state': 'ready',
        'reason': 'verified',
        'modelId': productionVoiceModelId,
        'encoder': '/private/encoder.int8.onnx',
        'decoder': '/private/decoder.int8.onnx',
        'tokens': '/private/tokens.txt',
      });
      expect(status.ready, isTrue);
      expect(productionVoiceModelBytes, 237202501);

      expect(
        () => LocalAsrModelStatus.fromChannel({
          'state': 'ready',
          'reason': 'claimed without paths',
          'modelId': productionVoiceModelId,
        }),
        throwsFormatException,
      );
      expect(
        () => LocalAsrModelStatus.fromChannel({
          'state': 'ready',
          'reason': 'wrong model',
          'modelId': 'ambient-model',
          'encoder': '/e',
          'decoder': '/d',
          'tokens': '/t',
        }),
        throwsFormatException,
      );
    },
  );

  test('missing corrupt and unsupported states always fail closed', () {
    for (final state in const ['missing', 'corrupt', 'unsupported']) {
      final status = LocalAsrModelStatus.fromChannel({
        'state': state,
        'reason': state,
        'modelId': productionVoiceModelId,
      });
      expect(status.ready, isFalse);
    }
  });
}
