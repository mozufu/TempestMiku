part of '../session_models.dart';

/// Server acknowledgement for one authenticated push-registration upsert.
///
/// This is deliberately a receipt from the successful PUT response, not a
/// claim that a later server-side registration is still active.
class PushRegistrationMetadata {
  const PushRegistrationMetadata({
    required this.deviceId,
    required this.provider,
    required this.createdAt,
    required this.updatedAt,
    this.disabledAt,
  });

  final String deviceId;
  final String provider;
  final String createdAt;
  final String updatedAt;
  final String? disabledAt;

  bool get acknowledgedActive => disabledAt == null;

  static PushRegistrationMetadata fromJson(Map<String, Object?> json) {
    final metadata = PushRegistrationMetadata(
      deviceId: _stringValue(json['deviceId']),
      provider: _stringValue(json['provider']),
      createdAt: _stringValue(json['createdAt']),
      updatedAt: _stringValue(json['updatedAt']),
      disabledAt: _nullableString(json['disabledAt']),
    );
    if (metadata.deviceId.isEmpty ||
        metadata.provider.isEmpty ||
        metadata.createdAt.isEmpty ||
        metadata.updatedAt.isEmpty) {
      throw const FormatException(
        'push registration response was missing required metadata',
      );
    }
    return metadata;
  }
}
