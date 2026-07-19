import 'dart:typed_data';

/// Returns a writable, Dart-owned copy and erases the source when its backing
/// storage permits it.
///
/// Platform codecs and [TransferableTypedData] may expose an unmodifiable
/// typed-data view even though reading it is valid. Wiping that view directly
/// must never turn an otherwise valid transcription into a user-visible
/// failure. A fresh copy gives the ASR path deterministic writable ownership;
/// the source wipe remains best effort because some platform-owned buffers do
/// not expose writable backing storage.
Uint8List cloneSensitiveBytes(Uint8List source) {
  final copy = Uint8List.fromList(source);
  wipeSensitiveBytes(source);
  return copy;
}

/// Erases [bytes] without throwing when the view is platform-unmodifiable.
void wipeSensitiveBytes(Uint8List bytes) {
  try {
    bytes.fillRange(0, bytes.length, 0);
    return;
  } on UnsupportedError {
    // Some unmodifiable typed-data views still expose their writable backing
    // buffer. Use it only for erasure; if the platform protects that too, the
    // transient source remains eligible for collection while the owned copy is
    // still erased deterministically by its caller.
  }
  try {
    Uint8List.view(
      bytes.buffer,
      bytes.offsetInBytes,
      bytes.lengthInBytes,
    ).fillRange(0, bytes.lengthInBytes, 0);
  } on UnsupportedError {
    // Best effort by contract: never replace a successful ASR result with a
    // cleanup error from platform-owned immutable memory.
  }
}
