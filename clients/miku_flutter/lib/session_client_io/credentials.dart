part of '../session_client_io.dart';

class DeviceCredential {
  const DeviceCredential({
    required this.serverBaseUrl,
    required this.token,
    this.deviceId,
  });

  final String serverBaseUrl;
  final String token;
  final String? deviceId;

  String encode() => jsonEncode({
    'version': deviceId == null ? 1 : 2,
    'serverBaseUrl': serverBaseUrl,
    'token': token,
    if (deviceId != null) 'deviceId': deviceId,
  });

  static DeviceCredential? decode(String? value) {
    if (value == null || value.isEmpty) return null;
    try {
      final json = jsonDecode(value);
      if (json is! Map || (json['version'] != 1 && json['version'] != 2)) {
        return null;
      }
      final serverBaseUrl = json['serverBaseUrl'];
      final token = json['token'];
      final deviceId = json['version'] == 2 ? json['deviceId'] : null;
      if (serverBaseUrl is! String ||
          serverBaseUrl.isEmpty ||
          token is! String ||
          !token.startsWith('tmk_dev_') ||
          (json['version'] == 2 &&
              (deviceId is! String ||
                  PairedAuthDeviceIdentity.fromStored(
                        serverBaseUrl: serverBaseUrl,
                        deviceId: deviceId,
                      ) ==
                      null))) {
        return null;
      }
      return DeviceCredential(
        serverBaseUrl: serverBaseUrl,
        token: token,
        deviceId: deviceId as String?,
      );
    } catch (_) {
      return null;
    }
  }
}

abstract class DeviceTokenStore {
  Future<DeviceCredential?> read();

  Future<void> write(DeviceCredential credential);

  Future<void> delete();
}

class SecureDeviceTokenStore implements DeviceTokenStore {
  SecureDeviceTokenStore({FlutterSecureStorage? storage})
    : _storage = storage ?? const FlutterSecureStorage();

  static const _key = 'tempestmiku.deviceCredential.v1';
  static const _legacyUnboundKey = 'tempestmiku.deviceToken';
  final FlutterSecureStorage _storage;

  @override
  Future<DeviceCredential?> read() async =>
      DeviceCredential.decode(await _storage.read(key: _key));

  @override
  Future<void> write(DeviceCredential credential) async {
    await _storage.delete(key: _legacyUnboundKey);
    await _storage.write(key: _key, value: credential.encode());
  }

  @override
  Future<void> delete() async {
    await _storage.delete(key: _key);
    await _storage.delete(key: _legacyUnboundKey);
  }
}

class MemoryDeviceTokenStore implements DeviceTokenStore {
  DeviceCredential? credential;

  @override
  Future<void> delete() async => credential = null;

  @override
  Future<DeviceCredential?> read() async => credential;

  @override
  Future<void> write(DeviceCredential credential) async =>
      this.credential = credential;
}
