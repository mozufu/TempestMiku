import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/asr/local_asr_engine.dart';
import 'package:miku_flutter/voice_capture_service.dart';

void main() {
  const captureId = '10000000-0000-4000-8000-000000000001';
  const testApkSha256 =
      '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef';

  test('parses only an exact privacy-bounded app build fingerprint', () {
    final fingerprint = VoiceAppBuildFingerprint.fromPlatform({
      'applicationId': 'org.mozufu.tempestmiku',
      'versionName': '1.0.1',
      'versionCode': 2,
      'buildType': 'release',
      'apkSha256': testApkSha256,
    });

    expect(fingerprint.applicationId, 'org.mozufu.tempestmiku');
    expect(fingerprint.versionName, '1.0.1');
    expect(fingerprint.versionCode, 2);
    expect(fingerprint.buildType, 'release');
    expect(fingerprint.apkSha256, testApkSha256);
  });

  test('rejects malformed or identifying build fingerprint fields', () {
    final valid = <String, Object>{
      'applicationId': 'org.mozufu.tempestmiku',
      'versionName': '1.0.1',
      'versionCode': 2,
      'buildType': 'release',
      'apkSha256': testApkSha256,
    };
    for (final invalid in <Object?>[
      null,
      {
        'applicationId': 'org.mozufu.tempestmiku',
        'versionName': '1.0.1',
        'versionCode': 2,
        'apkSha256': testApkSha256,
      },
      {...valid, 'sourcePath': '/private/base.apk'},
      {...valid, 'captureId': captureId},
      {...valid, 'applicationId': '../app'},
      {...valid, 'versionName': ''},
      {...valid, 'versionCode': 0},
      {...valid, 'versionCode': 2100000001},
      {...valid, 'versionCode': '2'},
      {...valid, 'buildType': 'release/path'},
      {
        ...valid,
        'apkSha256':
            '0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF',
      },
      {...valid, 'apkSha256': '0b17'},
    ]) {
      expect(
        () => VoiceAppBuildFingerprint.fromPlatform(invalid),
        throwsFormatException,
      );
    }
  });

  test('build inspection delegates only to an explicit inspector', () async {
    final expected = VoiceAppBuildFingerprint(
      applicationId: 'org.mozufu.tempestmiku',
      versionName: '1.0.1',
      versionCode: 2,
      buildType: 'release',
      apkSha256: testApkSha256,
    );
    final MikuVoiceCaptureService inspectable = _InspectableCapture(expected);
    final MikuVoiceCaptureService captureOnly = _CaptureOnly();

    expect(await inspectable.inspectBuild(), same(expected));
    await expectLater(
      captureOnly.inspectBuild(),
      throwsA(isA<UnsupportedError>()),
    );
  });

  test('parses a bounded Android PCM16 result defensively', () {
    final source = Uint8List.fromList([0, 0, 1, 0]);
    final result = CapturedVoicePcm.fromPlatform({
      'captureId': captureId,
      'sampleRate': localAsrSampleRate,
      'pcm16': source,
    });

    expect(result.captureId, captureId);
    expect(result.sampleRate, localAsrSampleRate);
    expect(result.pcm16, [0, 0, 1, 0]);
    expect(result.diagnostics.duration, const Duration(microseconds: 125));
    expect(result.diagnostics.qualityIssue, VoiceCaptureQualityIssue.tooShort);
    expect(source, everyElement(0));
  });

  test(
    'accepts platform-unmodifiable PCM and keeps an owned writable clone',
    () {
      final backing = Uint8List.fromList([0, 0, 1, 0]);
      final source = backing.asUnmodifiableView();

      final result = CapturedVoicePcm.fromPlatform({
        'captureId': captureId,
        'sampleRate': localAsrSampleRate,
        'pcm16': source,
      });

      expect(result.pcm16, [0, 0, 1, 0]);
      expect(() => result.pcm16[0] = 7, returnsNormally);
      // The platform intentionally owns this immutable source. The production
      // path must not fail merely because it cannot erase that transient view;
      // its separately owned clone remains writable and is erased after use.
      expect(backing, [0, 0, 1, 0]);
    },
  );

  test('wipes decoded Android PCM when metadata or audio is rejected', () {
    final incompleteSource = Uint8List.fromList([1, 2]);
    expect(
      () => CapturedVoicePcm.fromPlatform({'pcm16': incompleteSource}),
      throwsFormatException,
    );
    expect(incompleteSource, everyElement(0));

    final malformedSource = Uint8List.fromList([1, 2, 3]);
    expect(
      () => CapturedVoicePcm.fromPlatform({
        'captureId': captureId,
        'sampleRate': localAsrSampleRate,
        'pcm16': malformedSource,
      }),
      throwsFormatException,
    );
    expect(malformedSource, everyElement(0));
  });

  test('rejects malformed IDs, rates, odd audio, and oversized audio', () {
    for (final value in [
      {'captureId': 'bad', 'sampleRate': 16000, 'pcm16': Uint8List(2)},
      {'captureId': captureId, 'sampleRate': 48000, 'pcm16': Uint8List(2)},
      {'captureId': captureId, 'sampleRate': 16000, 'pcm16': Uint8List(1)},
      {
        'captureId': captureId,
        'sampleRate': 16000,
        'pcm16': Uint8List(localAsrMaxPcm16Bytes + 2),
      },
    ]) {
      expect(() => CapturedVoicePcm.fromPlatform(value), throwsFormatException);
    }
    expect(
      () => CapturedVoicePcm.fromPlatform({'captureId': captureId}),
      throwsFormatException,
    );
  });

  test('reports only aggregate PCM quality diagnostics', () {
    Uint8List pcmFromSamples(Iterable<int> samples) {
      final values = samples.toList();
      final bytes = Uint8List(values.length * 2);
      final data = ByteData.sublistView(bytes);
      for (var index = 0; index < values.length; index += 1) {
        data.setInt16(index * 2, values[index], Endian.little);
      }
      return bytes;
    }

    final silent = VoiceCaptureDiagnostics.fromPcm16(
      Uint8List(localAsrSampleRate * 2),
      sampleRate: localAsrSampleRate,
    );
    expect(silent.duration, const Duration(seconds: 1));
    expect(silent.rmsDbfs, -120);
    expect(silent.nearZeroFraction, 1);
    expect(silent.activeFrameFraction, 0);
    expect(silent.leadingSilence, const Duration(seconds: 1));
    expect(silent.trailingSilence, const Duration(seconds: 1));
    expect(silent.qualityIssue, VoiceCaptureQualityIssue.tooQuiet);

    final clipped = VoiceCaptureDiagnostics.fromPcm16(
      pcmFromSamples(List<int>.filled(localAsrSampleRate, 32767)),
      sampleRate: localAsrSampleRate,
    );
    expect(clipped.duration, const Duration(seconds: 1));
    expect(clipped.peakDbfs, closeTo(0, 0.01));
    expect(clipped.clippedFraction, 1);
    expect(clipped.qualityIssue, VoiceCaptureQualityIssue.clipped);

    final healthy = VoiceCaptureDiagnostics.fromPcm16(
      pcmFromSamples(
        List<int>.generate(
          localAsrSampleRate,
          (index) => index.isEven ? 3000 : -3000,
        ),
      ),
      sampleRate: localAsrSampleRate,
    );
    expect(healthy.rmsDbfs, closeTo(-20.77, 0.02));
    expect(healthy.activeFrameFraction, 1);
    expect(healthy.qualityIssue, isNull);

    expect(
      () => VoiceCaptureDiagnostics.fromPcm16(
        Uint8List(0),
        sampleRate: localAsrSampleRate,
      ),
      throwsArgumentError,
    );
  });
}

class _CaptureOnly implements MikuVoiceCaptureService {
  @override
  bool get isSupported => true;

  @override
  Future<bool> cancel(String? captureId) async => false;

  @override
  Future<int> recoverOrphans() async => 0;

  @override
  Future<bool> requestPermission() async => false;

  @override
  Future<void> start(String captureId) async {}

  @override
  Future<CapturedVoicePcm> stop(String captureId) =>
      Future.error(UnsupportedError('not used by this test'));
}

final class _InspectableCapture extends _CaptureOnly
    implements MikuVoiceBuildInspector {
  _InspectableCapture(this.fingerprint);

  final VoiceAppBuildFingerprint fingerprint;

  @override
  Future<VoiceAppBuildFingerprint> inspectBuild() async => fingerprint;
}
