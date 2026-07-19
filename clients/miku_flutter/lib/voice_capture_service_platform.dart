import 'dart:math' as math;
import 'dart:typed_data';

import 'asr/local_asr_engine.dart';
import 'asr/secure_buffer.dart';

final _voiceCaptureIdPattern = RegExp(
  r'^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-4[0-9a-fA-F]{3}-[89aAbB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$',
);
final _applicationIdPattern = RegExp(
  r'^[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)+$',
);
final _versionNamePattern = RegExp(r'^[A-Za-z0-9][A-Za-z0-9._+\-]{0,127}$');
final _buildTypePattern = RegExp(r'^[a-z][a-z0-9_-]{0,31}$');
final _sha256Pattern = RegExp(r'^[0-9a-f]{64}$');

final class VoiceAppBuildFingerprint {
  factory VoiceAppBuildFingerprint({
    required String applicationId,
    required String versionName,
    required int versionCode,
    required String buildType,
    required String apkSha256,
  }) {
    if (applicationId.length > 255 ||
        !_applicationIdPattern.hasMatch(applicationId) ||
        !_versionNamePattern.hasMatch(versionName) ||
        versionCode < 1 ||
        versionCode > 2100000000 ||
        !_buildTypePattern.hasMatch(buildType) ||
        !_sha256Pattern.hasMatch(apkSha256)) {
      throw const FormatException('invalid app build fingerprint');
    }
    return VoiceAppBuildFingerprint._(
      applicationId: applicationId,
      versionName: versionName,
      versionCode: versionCode,
      buildType: buildType,
      apkSha256: apkSha256,
    );
  }

  factory VoiceAppBuildFingerprint.fromPlatform(Object? value) {
    const keys = {
      'applicationId',
      'versionName',
      'versionCode',
      'buildType',
      'apkSha256',
    };
    if (value is! Map<Object?, Object?> ||
        value.length != keys.length ||
        value.keys.any((key) => key is! String || !keys.contains(key))) {
      throw const FormatException('invalid Android app build fingerprint');
    }
    final applicationId = value['applicationId'];
    final versionName = value['versionName'];
    final versionCode = value['versionCode'];
    final buildType = value['buildType'];
    final apkSha256 = value['apkSha256'];
    if (applicationId is! String ||
        versionName is! String ||
        versionCode is! int ||
        buildType is! String ||
        apkSha256 is! String) {
      throw const FormatException('incomplete Android app build fingerprint');
    }
    return VoiceAppBuildFingerprint(
      applicationId: applicationId,
      versionName: versionName,
      versionCode: versionCode,
      buildType: buildType,
      apkSha256: apkSha256,
    );
  }

  const VoiceAppBuildFingerprint._({
    required this.applicationId,
    required this.versionName,
    required this.versionCode,
    required this.buildType,
    required this.apkSha256,
  });

  final String applicationId;
  final String versionName;
  final int versionCode;
  final String buildType;
  final String apkSha256;
}

enum VoiceCaptureQualityIssue { tooShort, tooQuiet, clipped }

final class VoiceCaptureDiagnostics {
  const VoiceCaptureDiagnostics({
    required this.duration,
    required this.rmsDbfs,
    required this.peakDbfs,
    required this.clippedFraction,
    required this.nearZeroFraction,
    required this.activeFrameFraction,
    required this.leadingSilence,
    required this.trailingSilence,
  });

  factory VoiceCaptureDiagnostics.fromPcm16(
    Uint8List pcm16, {
    required int sampleRate,
  }) {
    if (sampleRate <= 0 || pcm16.isEmpty || pcm16.lengthInBytes.isOdd) {
      throw ArgumentError(
        'diagnostics require non-empty PCM16 and a positive sample rate',
      );
    }
    final samples = pcm16.lengthInBytes ~/ 2;
    final frameSamples = math.max(1, sampleRate ~/ 50);
    final data = ByteData.sublistView(pcm16);
    var sumSquares = 0.0;
    var peak = 0;
    var clipped = 0;
    var nearZero = 0;
    var frameSumSquares = 0.0;
    var frameSampleCount = 0;
    var frameCount = 0;
    var activeFrames = 0;
    int? firstActiveFrame;
    var lastActiveFrame = -1;

    void finishFrame() {
      if (frameSampleCount == 0) return;
      final frameRms = math.sqrt(frameSumSquares / frameSampleCount);
      if (frameRms >= 32768.0 * math.pow(10.0, -50.0 / 20.0)) {
        firstActiveFrame ??= frameCount;
        lastActiveFrame = frameCount;
        activeFrames += 1;
      }
      frameCount += 1;
      frameSumSquares = 0;
      frameSampleCount = 0;
    }

    for (var index = 0; index < samples; index += 1) {
      final sample = data.getInt16(index * 2, Endian.little);
      final magnitude = sample.abs();
      final square = sample.toDouble() * sample.toDouble();
      sumSquares += square;
      frameSumSquares += square;
      frameSampleCount += 1;
      if (magnitude > peak) peak = magnitude;
      if (magnitude >= 32760) clipped += 1;
      if (magnitude <= 1) nearZero += 1;
      if (frameSampleCount == frameSamples) finishFrame();
    }
    finishFrame();

    double dbfs(double amplitude) {
      if (amplitude <= 0) return -120;
      return math.max(-120, 20 * math.log(amplitude / 32768.0) / math.ln10);
    }

    final durationMicros =
        samples * Duration.microsecondsPerSecond ~/ sampleRate;
    final duration = Duration(microseconds: durationMicros);
    final frameMicros =
        Duration.microsecondsPerSecond * frameSamples ~/ sampleRate;
    final firstActive = firstActiveFrame;
    final leadingMicros =
        firstActive == null
            ? durationMicros
            : math.min(durationMicros, firstActive * frameMicros).toInt();
    final activeEndMicros =
        lastActiveFrame < 0
            ? 0
            : math
                .min(durationMicros, (lastActiveFrame + 1) * frameMicros)
                .toInt();

    return VoiceCaptureDiagnostics(
      duration: duration,
      rmsDbfs: dbfs(math.sqrt(sumSquares / samples)),
      peakDbfs: dbfs(peak.toDouble()),
      clippedFraction: clipped / samples,
      nearZeroFraction: nearZero / samples,
      activeFrameFraction: frameCount == 0 ? 0 : activeFrames / frameCount,
      leadingSilence: Duration(microseconds: leadingMicros),
      trailingSilence: Duration(
        microseconds: math.max(0, durationMicros - activeEndMicros),
      ),
    );
  }

  final Duration duration;
  final double rmsDbfs;
  final double peakDbfs;
  final double clippedFraction;
  final double nearZeroFraction;
  final double activeFrameFraction;
  final Duration leadingSilence;
  final Duration trailingSilence;

  VoiceCaptureQualityIssue? get qualityIssue {
    if (duration < const Duration(milliseconds: 350)) {
      return VoiceCaptureQualityIssue.tooShort;
    }
    if (nearZeroFraction >= 0.98 || rmsDbfs <= -55) {
      return VoiceCaptureQualityIssue.tooQuiet;
    }
    if (clippedFraction >= 0.02) {
      return VoiceCaptureQualityIssue.clipped;
    }
    return null;
  }

  @override
  String toString() =>
      'durationMs=${duration.inMilliseconds}, '
      'rmsDbfs=${rmsDbfs.toStringAsFixed(1)}, '
      'peakDbfs=${peakDbfs.toStringAsFixed(1)}, '
      'clipped=${clippedFraction.toStringAsFixed(4)}, '
      'nearZero=${nearZeroFraction.toStringAsFixed(4)}, '
      'activeFrames=${activeFrameFraction.toStringAsFixed(4)}, '
      'leadingSilenceMs=${leadingSilence.inMilliseconds}, '
      'trailingSilenceMs=${trailingSilence.inMilliseconds}';
}

final class CapturedVoicePcm {
  factory CapturedVoicePcm({
    required String captureId,
    required int sampleRate,
    required Uint8List pcm16,
  }) {
    final owned = _validateAndCopy(captureId, sampleRate, pcm16);
    return CapturedVoicePcm._(
      captureId: captureId,
      sampleRate: sampleRate,
      pcm16: owned,
      diagnostics: VoiceCaptureDiagnostics.fromPcm16(
        owned,
        sampleRate: sampleRate,
      ),
    );
  }

  CapturedVoicePcm._({
    required this.captureId,
    required this.sampleRate,
    required this.pcm16,
    required this.diagnostics,
  });

  static Uint8List _validateAndCopy(
    String captureId,
    int sampleRate,
    Uint8List pcm16,
  ) {
    if (!_voiceCaptureIdPattern.hasMatch(captureId)) {
      throw ArgumentError.value(captureId, 'captureId', 'invalid capture id');
    }
    if (sampleRate != localAsrSampleRate) {
      throw ArgumentError.value(sampleRate, 'sampleRate', 'must be 16 kHz');
    }
    if (pcm16.isEmpty || pcm16.lengthInBytes.isOdd) {
      throw ArgumentError.value(
        pcm16.lengthInBytes,
        'pcm16',
        'must contain complete non-empty PCM16 samples',
      );
    }
    if (pcm16.lengthInBytes > localAsrMaxPcm16Bytes) {
      throw ArgumentError.value(
        pcm16.lengthInBytes,
        'pcm16',
        'exceeds the 60-second bound',
      );
    }
    return cloneSensitiveBytes(pcm16);
  }

  factory CapturedVoicePcm.fromPlatform(Object? value) {
    if (value is! Map) {
      throw const FormatException('invalid Android voice capture result');
    }
    final captureId = value['captureId'];
    final sampleRate = value['sampleRate'];
    final pcm16 = value['pcm16'];
    try {
      if (captureId is! String || sampleRate is! int || pcm16 is! Uint8List) {
        throw const FormatException(
          'Android voice capture result is incomplete',
        );
      }
      return CapturedVoicePcm(
        captureId: captureId,
        sampleRate: sampleRate,
        pcm16: pcm16,
      );
    } on ArgumentError catch (error) {
      throw FormatException(
        'Android voice capture result was rejected: $error',
      );
    } finally {
      // Invalid platform values never reach _validateAndCopy, so erase a
      // writable source here as well. A valid source is already handled while
      // cloning; duplicate erasure is harmless.
      if (pcm16 is Uint8List) wipeSensitiveBytes(pcm16);
    }
  }

  final String captureId;
  final int sampleRate;
  final Uint8List pcm16;
  final VoiceCaptureDiagnostics diagnostics;
}

abstract interface class MikuVoiceCaptureService {
  bool get isSupported;

  Future<int> recoverOrphans();

  Future<bool> requestPermission();

  Future<void> start(String captureId);

  Future<CapturedVoicePcm> stop(String captureId);

  Future<bool> cancel(String? captureId);
}

abstract interface class MikuVoiceBuildInspector {
  Future<VoiceAppBuildFingerprint> inspectBuild();
}

extension MikuVoiceBuildInspection on MikuVoiceCaptureService {
  Future<VoiceAppBuildFingerprint> inspectBuild() {
    final service = this;
    if (service is MikuVoiceBuildInspector) {
      final inspector = service as MikuVoiceBuildInspector;
      return inspector.inspectBuild();
    }
    return Future.error(
      UnsupportedError('app build fingerprint is unavailable'),
    );
  }
}
