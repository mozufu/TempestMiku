part of 'conversation_screen.dart';

/// Voice ASR engine catalog, local model lifecycle, and engine selection for the
/// conversation screen.
extension _ConversationVoiceEngines on _ConversationScreenState {
  VoiceAsrEngine? get _selfHostedVoiceAsr => _voiceAsrCatalog.selfHosted;

  bool get _selfHostedVoiceAsrAvailable =>
      _selfHostedVoiceAsr?.available == true;

  bool get _selectedVoiceAsrReady => switch (_voiceAsrSelection) {
    VoiceAsrEngineKind.local => _voiceTranscriber != null,
    VoiceAsrEngineKind.remote => _selfHostedVoiceAsrAvailable,
  };

  String get _selectedVoiceAsrSummary => switch (_voiceAsrSelection) {
    VoiceAsrEngineKind.local => '本機辨識，音訊留在這台裝置',
    VoiceAsrEngineKind.remote when _voiceAsrCatalogLoading => '正在檢查家用自架服務',
    VoiceAsrEngineKind.remote when !_selfHostedVoiceAsrAvailable =>
      '家用自架服務目前無法使用',
    VoiceAsrEngineKind.remote =>
      '家用自架 · ${_selfHostedVoiceAsr?.modelId ?? _selfHostedVoiceAsr?.label ?? '已設定'}',
  };

  bool get _voiceOperationActive =>
      _voiceRecording ||
      _voiceProcessing ||
      _voicePermissionPending ||
      _voiceCaptureId != null ||
      (_voiceTranscriber?.isActive ?? false);

  Future<VoiceAppBuildFingerprint?> _inspectVoiceBuild() async {
    if (!_voiceCapture.isSupported) return null;
    try {
      return await _voiceCapture.inspectBuild().timeout(
        const Duration(seconds: 5),
      );
    } catch (_) {
      // Build identity is diagnostic metadata. Missing it must not block an
      // otherwise valid capture, review, or explicit send.
      return null;
    }
  }

  Future<VoiceAsrEngineCatalog> _refreshVoiceAsrEngines({
    bool allowFallback = true,
  }) async {
    final authorityEpoch = _serverAuthorityEpoch;
    if (mounted) _voiceSetState(() => _voiceAsrCatalogLoading = true);
    try {
      final catalog = await widget.client.voiceAsrEngines();
      if (mounted && authorityEpoch == _serverAuthorityEpoch) {
        _voiceSetState(() {
          _voiceAsrCatalog = catalog;
          _voiceAsrCatalogLoading = false;
        });
      }
      return catalog;
    } catch (_) {
      if (mounted && authorityEpoch == _serverAuthorityEpoch) {
        _voiceSetState(() {
          _voiceAsrCatalogLoading = false;
          if (allowFallback && _voiceAsrSelection == VoiceAsrEngineKind.local) {
            _voiceAsrCatalog = VoiceAsrEngineCatalog.localOnly();
          }
        });
      }
      if (!allowFallback) rethrow;
      return _voiceAsrCatalog;
    }
  }

  Future<LocalAsrModelStatus?> _refreshVoiceModel() async {
    if (widget.localAsrWorkers != null) return _voiceModelStatus;
    if (!_localAsrModels.isSupported) return null;
    final status = await _localAsrModels.inspect();
    if (!mounted) return status;
    await _applyVoiceModelStatus(status);
    return status;
  }

  Future<LocalAsrModelStatus> _installVoiceModel({
    void Function(LocalAsrModelInstallProgress)? onProgress,
    LocalAsrCancellationToken? cancellation,
  }) async {
    if (!_localAsrModels.isSupported || widget.localAsrWorkers != null) {
      throw UnsupportedError('本機語音模型無法在這台裝置上管理。');
    }
    final status = await _localAsrModels.install(
      onProgress: onProgress,
      cancellation: cancellation,
    );
    await _applyVoiceModelStatus(status);
    return status;
  }

  Future<LocalAsrModelStatus> _deleteVoiceModel() async {
    if (!_localAsrModels.isSupported || widget.localAsrWorkers != null) {
      throw UnsupportedError('本機語音模型無法在這台裝置上管理。');
    }
    if (_voiceOperationActive) {
      final cleaned = await _cancelVoiceCapture();
      if (!cleaned) {
        throw StateError('語音錄音尚未安全清除，沒有刪除模型。');
      }
    } else {
      await _voiceTranscriber?.cancel();
    }
    final status = await _localAsrModels.delete();
    await _applyVoiceModelStatus(status);
    return status;
  }

  Future<void> _applyVoiceModelStatus(LocalAsrModelStatus status) async {
    final activeSelection = _activeVoiceAsrSelection ?? _voiceAsrSelection;
    if (!status.ready &&
        widget.localAsrWorkers == null &&
        activeSelection == VoiceAsrEngineKind.local &&
        _voiceOperationActive) {
      final cleaned = await _cancelVoiceCapture();
      if (!cleaned) {
        throw StateError('語音錄音尚未安全清除，沒有切換模型狀態。');
      }
    }
    final previous = _voiceTranscriber;
    if (previous != null && widget.localAsrWorkers == null) {
      await previous.cancel();
    }
    if (!mounted) return;
    _voiceSetState(() {
      _voiceModelStatus = status;
      _voiceTranscriber =
          status.ready
              ? LocalAsrTranscriber(
                workers: _localAsrModels,
                timeout: widget.voiceInferenceTimeout,
              )
              : null;
    });
  }

  Future<bool> _selectVoiceAsrEngine(VoiceAsrEngineKind selection) async {
    if (selection == VoiceAsrEngineKind.remote &&
        !_selfHostedVoiceAsrAvailable) {
      return false;
    }
    if (_voiceOperationActive) return false;
    if (!mounted) return false;
    _voiceSetState(() {
      _voiceAsrSelection = selection;
      _voiceError = null;
    });
    return true;
  }
}
