part of '../session_models.dart';

Map<String, Object?>? _mapValue(Object? value) {
  if (value is Map<String, Object?>) return value;
  if (value is Map) return value.cast<String, Object?>();
  return null;
}

List<Map<String, Object?>> _mapList(Object? value) {
  return ((value as List?) ?? const [])
      .whereType<Map>()
      .map((item) => item.cast<String, Object?>())
      .toList();
}

List<String> _stringList(Object? value) {
  return ((value as List?) ?? const [])
      .map((item) => item.toString())
      .where((item) => item.isNotEmpty)
      .toList();
}

String _stringValue(Object? value) => value?.toString() ?? '';

String? _nullableString(Object? value) {
  final text = _stringValue(value);
  return text.isEmpty ? null : text;
}

int? _intValue(Object? value) {
  if (value is num) return value.toInt();
  return int.tryParse(_stringValue(value));
}

double? _doubleValue(Object? value) {
  if (value is num) return value.toDouble();
  return double.tryParse(_stringValue(value));
}
