#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
APK="${1:-$ROOT/clients/miku_flutter/build/app/outputs/flutter-apk/app-arm64-v8a-release.apk}"
PACKAGE="org.mozufu.tempestmiku"
EXPECTED_CERT_SHA256="503f865843464347cb2d2f90be3ab4dbf68bae690443bf8fd262e0dcc0b58ee1"
ARM64_SPLIT_VERSION_CODE_OFFSET=2000

for tool in unzip shasum; do
  command -v "$tool" >/dev/null || {
    echo "missing required APK inspection tool: $tool" >&2
    exit 1
  }
done

find_aapt2() {
  if [[ -n "${AAPT2:-}" ]]; then
    [[ -x "$AAPT2" ]] || {
      echo "AAPT2 is not executable: $AAPT2" >&2
      return 1
    }
    printf '%s\n' "$AAPT2"
    return
  fi
  if command -v aapt2 >/dev/null; then
    command -v aapt2
    return
  fi
  local sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
  local candidate
  candidate="$(
    find "$sdk_root/build-tools" -type f -name aapt2 2>/dev/null |
      sort -V |
      tail -n 1 || true
  )"
  [[ -n "$candidate" && -x "$candidate" ]] || {
    echo "could not locate aapt2; set AAPT2 or ANDROID_SDK_ROOT" >&2
    return 1
  }
  printf '%s\n' "$candidate"
}

find_apksigner() {
  if [[ -n "${APKSIGNER:-}" ]]; then
    [[ -x "$APKSIGNER" ]] || {
      echo "APKSIGNER is not executable: $APKSIGNER" >&2
      return 1
    }
    printf '%s\n' "$APKSIGNER"
    return
  fi
  if command -v apksigner >/dev/null; then
    command -v apksigner
    return
  fi
  local sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
  local candidate
  candidate="$(
    find "$sdk_root/build-tools" -type f -name apksigner 2>/dev/null |
      sort -V |
      tail -n 1 || true
  )"
  [[ -n "$candidate" && -x "$candidate" ]] || {
    echo "could not locate apksigner; set APKSIGNER or ANDROID_SDK_ROOT" >&2
    return 1
  }
  printf '%s\n' "$candidate"
}

[[ -f "$APK" ]] || {
  echo "missing signed arm64 release APK: $APK" >&2
  exit 1
}

aapt2="$(find_aapt2)"
badging="$("$aapt2" dump badging "$APK")"
manifest_xml="$("$aapt2" dump xmltree "$APK" --file AndroidManifest.xml)"
package_line="$(grep '^package:' <<<"$badging")"
application_id="$(sed -n "s/^package: name='\([^']*\)'.*/\1/p" <<<"$package_line")"
version_code="$(sed -n "s/.* versionCode='\([^']*\)'.*/\1/p" <<<"$package_line")"
version_name="$(sed -n "s/.* versionName='\([^']*\)'.*/\1/p" <<<"$package_line")"
declared_version="$(awk '$1 == "version:" {print $2; exit}' "$ROOT/clients/miku_flutter/pubspec.yaml")"
expected_version_name="${declared_version%+*}"
declared_build_number="${declared_version#*+}"
expected_version_code="$((declared_build_number + ARM64_SPLIT_VERSION_CODE_OFFSET))"

[[ "$application_id" == "$PACKAGE" ]] || {
  echo "release APK package id is not $PACKAGE" >&2
  exit 1
}
[[ "$version_name" == "$expected_version_name" && "$version_code" == "$expected_version_code" ]] || {
  echo "release APK version $version_name+$version_code does not match pubspec $declared_version with the arm64 split offset (+$ARM64_SPLIT_VERSION_CODE_OFFSET)" >&2
  exit 1
}
if grep -q '^application-debuggable' <<<"$badging" ||
  grep -Eq 'debuggable\([^)]*\)=true' <<<"$manifest_xml"; then
  echo "release APK is debuggable" >&2
  exit 1
fi

grep -Eq 'allowBackup\([^)]*\)=false' <<<"$manifest_xml"
grep -Eq 'usesCleartextTraffic\([^)]*\)=false' <<<"$manifest_xml"

expected_permissions="$(printf '%s\n' \
  android.permission.ACCESS_NETWORK_STATE \
  android.permission.CAMERA \
  android.permission.FOREGROUND_SERVICE \
  android.permission.INTERNET \
  android.permission.POST_NOTIFICATIONS \
  android.permission.RECEIVE_BOOT_COMPLETED \
  android.permission.RECORD_AUDIO \
  android.permission.WAKE_LOCK \
  org.mozufu.tempestmiku.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION |
  sort)"
actual_permissions="$(
  sed -n "s/^uses-permission: name='\([^']*\)'.*/\1/p" <<<"$badging" |
    sort -u
)"
[[ "$actual_permissions" == "$expected_permissions" ]] || {
  echo "release APK permission set drifted" >&2
  diff <(printf '%s\n' "$expected_permissions") <(printf '%s\n' "$actual_permissions") >&2 || true
  exit 1
}

abis="$(
  unzip -Z1 "$APK" |
    awk -F/ '$1 == "lib" && NF == 3 {print $2}' |
    sort -u
)"
[[ "$abis" == "arm64-v8a" ]] || {
  echo "release APK must contain only arm64-v8a native libraries; got: $abis" >&2
  exit 1
}
unzip -Z1 "$APK" | grep '^lib/arm64-v8a/libsherpa-onnx-c-api\.so$' >/dev/null || {
  echo "release APK is missing the selected local-ASR runtime" >&2
  exit 1
}

forbidden="$(
  unzip -Z1 "$APK" |
    grep -Ei '(^|/)(model-manifest\.json|tokens\.txt|[^/]+\.(onnx|wav|pcm|flac|mp3|m4a|aac))$' || true
)"
[[ -z "$forbidden" ]] || {
  echo "release APK bundled forbidden model/audio material:" >&2
  echo "$forbidden" >&2
  exit 1
}

apksigner="$(find_apksigner)"
signature="$("$apksigner" verify --verbose --print-certs "$APK" 2>&1)"
grep -q 'Verified using v2 scheme (APK Signature Scheme v2): true' <<<"$signature"
grep -q 'Number of signers: 1' <<<"$signature"
grep -qi "certificate SHA-256 digest: $EXPECTED_CERT_SHA256" <<<"$signature"
grep -q 'key algorithm: RSA' <<<"$signature"
grep -q 'key size (bits): 4096' <<<"$signature"

bytes="$(
  if stat -f %z "$APK" >/dev/null 2>&1; then
    stat -f %z "$APK"
  else
    stat -c %s "$APK"
  fi
)"
sha256="$(shasum -a 256 "$APK" | awk '{print $1}')"
printf 'verified P6.6 release APK\npath=%s\nversion=%s+%s\nbytes=%s\nsha256=%s\ncertificate_sha256=%s\n' \
  "$APK" "$version_name" "$version_code" "$bytes" "$sha256" "$EXPECTED_CERT_SHA256"
