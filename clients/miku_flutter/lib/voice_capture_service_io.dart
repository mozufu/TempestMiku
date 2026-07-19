import 'dart:io';

import 'package:flutter/services.dart';

import 'voice_capture_service_platform.dart';

const _voiceCapture = MethodChannel('org.mozufu.tempestmiku/voice-capture');

MikuVoiceCaptureService createVoiceCaptureService() =>
    const AndroidVoiceCaptureService();

class AndroidVoiceCaptureService
    implements MikuVoiceCaptureService, MikuVoiceBuildInspector {
  const AndroidVoiceCaptureService();

  @override
  bool get isSupported => Platform.isAndroid;

  @override
  Future<VoiceAppBuildFingerprint> inspectBuild() async {
    if (!isSupported) {
      throw UnsupportedError('app build fingerprint is unavailable');
    }
    return VoiceAppBuildFingerprint.fromPlatform(
      await _voiceCapture.invokeMethod<Object?>('inspectBuild'),
    );
  }

  @override
  Future<int> recoverOrphans() async {
    if (!isSupported) return 0;
    return await _voiceCapture.invokeMethod<int>('recover') ?? 0;
  }

  @override
  Future<bool> requestPermission() async {
    if (!isSupported) return false;
    return await _voiceCapture.invokeMethod<bool>('requestPermission') ?? false;
  }

  @override
  Future<void> start(String captureId) async {
    if (!isSupported) throw UnsupportedError('voice capture is unavailable');
    await _voiceCapture.invokeMethod<void>('start', {'captureId': captureId});
  }

  @override
  Future<CapturedVoicePcm> stop(String captureId) async {
    if (!isSupported) throw UnsupportedError('voice capture is unavailable');
    final result = await _voiceCapture.invokeMethod<Object?>('stop', {
      'captureId': captureId,
    });
    return CapturedVoicePcm.fromPlatform(result);
  }

  @override
  Future<bool> cancel(String? captureId) async {
    if (!isSupported) return false;
    return await _voiceCapture.invokeMethod<bool>('cancel', {
          if (captureId != null) 'captureId': captureId,
        }) ??
        false;
  }
}
