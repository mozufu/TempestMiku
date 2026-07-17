part of '../session_client_io.dart';

extension _NativeAuthClient on NativeMikuSessionClient {
  String _pairingDeviceNameImpl() => 'TempestMiku ${Platform.operatingSystem}';

  Future<String> _serverBaseUrlImpl() async {
    final prefs = await SharedPreferences.getInstance();
    final stored = prefs.getString(NativeMikuSessionClient._serverBaseUrlKey);
    if (stored != null && stored.trim().isNotEmpty) {
      return _normalizeServerBaseUrl(stored);
    }
    if (NativeMikuSessionClient._configuredServerBaseUrl.trim().isNotEmpty) {
      return _normalizeServerBaseUrl(
        NativeMikuSessionClient._configuredServerBaseUrl,
      );
    }
    if (kReleaseMode) {
      throw StateError('this device is not securely paired');
    }
    return NativeMikuSessionClient._defaultServerBaseUrl;
  }

  Future<void> _setServerBaseUrlImpl(String baseUrl) async {
    final normalized = _normalizeServerBaseUrl(baseUrl);
    final prefs = await SharedPreferences.getInstance();
    final previous = prefs.getString(NativeMikuSessionClient._serverBaseUrlKey);
    final previousNormalized =
        previous == null ? null : _tryNormalizeServerBaseUrl(previous);
    if (previousNormalized != normalized) {
      // Publish the new target only after all authority and state for the previous origin is gone.
      // A crash at any await before setString therefore leaves the old target unauthenticated.
      await _clearDeviceToken();
      await prefs.remove(NativeMikuSessionClient._sessionIdKey);
      await prefs.remove(NativeMikuSessionClient._lastEventIdKey);
      await prefs.setString(
        NativeMikuSessionClient._serverBaseUrlKey,
        normalized,
      );
    }
  }

  Future<void> _pairWithCodeImpl(MikuPairingTarget target) async {
    final normalized = _normalizeServerBaseUrl(target.serverBaseUrl);
    final json = await _pairRequest(normalized, <String, Object?>{
      'code': target.code,
      'deviceName': pairingDeviceName(),
      'platform': Platform.operatingSystem,
    });
    final token = json['token']?.toString().trim();
    if (token == null || !token.startsWith('tmk_dev_')) {
      throw const FormatException(
        'pairing response did not include a device token',
      );
    }
    final prefs = await SharedPreferences.getInstance();
    final previous = prefs.getString(NativeMikuSessionClient._serverBaseUrlKey);
    final previousNormalized =
        previous == null ? null : _tryNormalizeServerBaseUrl(previous);
    if (previousNormalized != null) {
      // The connector endpoint is stable across app registrations. Retire the old device's
      // server-side route before publishing the replacement credential so an old pairing cannot
      // continue targeting this installation.
      await _unregisterPushBestEffort();
    }
    if (previousNormalized != normalized) {
      await _clearDeviceToken();
    }
    await prefs.remove(NativeMikuSessionClient._sessionIdKey);
    await prefs.remove(NativeMikuSessionClient._lastEventIdKey);
    final credential = DeviceCredential(
      serverBaseUrl: normalized,
      token: token,
    );
    await _tokenStore.write(credential);
    _cachedCredential = credential;
    _tokenLoaded = true;
    // The origin-bound credential is safe if a crash occurs before this final publication: it
    // cannot authenticate requests to the still-selected old origin.
    await prefs.setString(
      NativeMikuSessionClient._serverBaseUrlKey,
      normalized,
    );
  }

  Future<void> _logoutImpl() async {
    try {
      await _request('POST', '/auth/logout');
    } finally {
      await _clearDeviceToken();
      final prefs = await SharedPreferences.getInstance();
      await prefs.remove(NativeMikuSessionClient._sessionIdKey);
      await prefs.remove(NativeMikuSessionClient._lastEventIdKey);
    }
  }

  Future<bool> _hasDeviceCredentialImpl() async {
    try {
      return (await _deviceToken()) != null;
    } catch (_) {
      return false;
    }
  }

  Future<NotificationReplyAuthority?> _notificationReplyAuthorityImpl() async {
    final baseUrl = await serverBaseUrl();
    final token = await _deviceToken(requestBaseUrl: baseUrl);
    if (token == null) return null;
    return NotificationReplyAuthority(
      serverBaseUrl: baseUrl,
      deviceToken: token,
    );
  }

  Future<void> _registerPushImpl({
    required String endpoint,
    required String p256dh,
    required String auth,
  }) async {
    await _request(
      'PUT',
      '/auth/push-registration',
      body: {
        'provider': 'unifiedpush',
        'registration': jsonEncode({
          'endpoint': endpoint,
          'p256dh': p256dh,
          'auth': auth,
        }),
      },
    );
  }

  Future<void> _unregisterPushImpl() async {
    await _request('DELETE', '/auth/push-registration');
  }

  Future<String?> _deviceToken({String? requestBaseUrl}) async {
    if (!_tokenLoaded) {
      _cachedCredential = await _tokenStore.read();
      _tokenLoaded = true;
    }
    final selectedServer = _normalizeServerBaseUrl(await serverBaseUrl());
    if (requestBaseUrl != null &&
        _normalizeServerBaseUrl(requestBaseUrl) != selectedServer) {
      return null;
    }
    final credential = _cachedCredential;
    if (credential == null || credential.serverBaseUrl != selectedServer) {
      return null;
    }
    final token = credential.token.trim();
    return token.isEmpty ? null : token;
  }

  Future<void> _clearDeviceToken() async {
    await _tokenStore.delete();
    _cachedCredential = null;
    _tokenLoaded = true;
  }

  Future<void> _unregisterPushBestEffort() async {
    try {
      await unregisterPush();
    } catch (_) {
      // Re-pairing must still work when the previous server is offline or already revoked.
    }
  }

  String _normalizeServerBaseUrl(String value) {
    return normalizeMikuServerBaseUrl(value);
  }

  String? _tryNormalizeServerBaseUrl(String value) {
    try {
      return _normalizeServerBaseUrl(value);
    } catch (_) {
      return null;
    }
  }
}
