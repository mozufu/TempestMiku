part of 'conversation_screen.dart';

extension _ConversationVoice on _ConversationScreenState {
  Future<void> _startVoiceCapture() async {
    final selection = _voiceAsrSelection;
    if (!_voiceCapture.isSupported || !_selectedVoiceAsrReady) {
      _showVoiceError('語音辨識尚未就緒，請先到設定檢查辨識方式與模型。');
      return;
    }
    if (_voiceRecording || _voiceProcessing || _sending || !_canCompose) {
      return;
    }
    if (_appLifecycle != AppLifecycleState.resumed) {
      _showVoiceError('App 不在前景，暫時無法開始錄音；請回到 App 後再試一次。');
      return;
    }
    final epoch = ++_voiceOperationEpoch;
    _voiceSetState(() {
      _voiceProcessing = true;
      _voicePermissionPending = true;
      _voiceError = null;
    });
    try {
      final permitted = await _voiceCapture.requestPermission();
      if (!mounted || epoch != _voiceOperationEpoch) return;
      _voicePermissionPending = false;
      if (!permitted) {
        _showVoiceError('沒有取得麥克風權限；未開始錄音，也不會送出任何內容。');
        return;
      }
      if (_appLifecycle != AppLifecycleState.resumed) return;
      final captureId = _newVoiceCaptureId();
      await _voiceCapture.start(captureId);
      if (!mounted || epoch != _voiceOperationEpoch) {
        await _voiceCapture.cancel(captureId);
        return;
      }
      _voiceSetState(() {
        _voiceCaptureId = captureId;
        _activeVoiceAsrSelection = selection;
        _voiceRecording = true;
      });
      final durationSeconds =
          selection == VoiceAsrEngineKind.remote
              ? _selfHostedVoiceAsr?.maxDurationSeconds ??
                  localAsrMaxDurationSeconds
              : localAsrMaxDurationSeconds;
      _voiceLimitTimer?.cancel();
      _voiceLimitTimer = Timer(Duration(seconds: durationSeconds), () {
        if (mounted && _voiceRecording && _voiceCaptureId == captureId) {
          unawaited(_stopVoiceCapture());
        }
      });
    } catch (error) {
      if (mounted && epoch == _voiceOperationEpoch) {
        _showVoiceError(_friendlyVoiceError(error));
      }
    } finally {
      if (mounted && epoch == _voiceOperationEpoch) {
        _voicePermissionPending = false;
        _voiceSetState(() => _voiceProcessing = false);
      }
    }
  }

  Future<void> _stopVoiceCapture() async {
    final captureId = _voiceCaptureId;
    final selection = _activeVoiceAsrSelection ?? _voiceAsrSelection;
    if (!_voiceRecording || captureId == null) return;
    final epoch = ++_voiceOperationEpoch;
    _voiceLimitTimer?.cancel();
    _voiceLimitTimer = null;
    _voiceSetState(() {
      _voiceRecording = false;
      _voiceProcessing = true;
      _voiceCaptureId = null;
      _voiceError = null;
    });

    CapturedVoicePcm? captured;
    LocalAsrAudio? audio;
    try {
      captured = await _voiceCapture.stop(captureId);
      if (!mounted || epoch != _voiceOperationEpoch) return;
      if (captured.captureId != captureId) {
        throw const FormatException('錄音識別碼在停止時發生變化。');
      }
      final qualityIssue = captured.diagnostics.qualityIssue;
      late final String transcriptText;
      late final VoiceTranscriptProvenance provenance;
      switch (selection) {
        case VoiceAsrEngineKind.local:
          final transcriber = _voiceTranscriber;
          if (transcriber == null) {
            throw StateError('已驗證的本機語音模型目前無法使用。');
          }
          audio = LocalAsrAudio.fromPcm16(
            captured.pcm16,
            sampleRate: captured.sampleRate,
          );
          final transcript = await transcriber.transcribe(audio);
          transcriptText = transcript.text.trim();
          provenance = VoiceTranscriptProvenance.local;
        case VoiceAsrEngineKind.remote:
          final transcript = await widget.client.transcribeVoicePcm16(
            engineId: selfHostedVoiceAsrEngineId,
            captureId: captureId,
            sampleRate: captured.sampleRate,
            pcm16: captured.pcm16,
          );
          transcriptText = transcript.text.trim();
          provenance = VoiceTranscriptProvenance.selfHosted;
      }
      if (transcriptText.isEmpty) {
        throw const FormatException('語音辨識沒有產生可檢查的文字。');
      }
      if (!mounted || epoch != _voiceOperationEpoch) return;
      final buildFingerprint = await _voiceBuildFingerprint;
      if (!mounted || epoch != _voiceOperationEpoch) return;
      _enqueueImport(
        SharedContent.fromEvent({
          'source': 'voice',
          'eventId': captureId,
          'text': transcriptText,
          'voiceTranscriptProvenance':
              provenance == VoiceTranscriptProvenance.selfHosted
                  ? 'self_hosted'
                  : 'local',
          if (qualityIssue != null) 'voiceQualityIssue': qualityIssue.name,
          'voiceDiagnostics': captured.diagnostics,
          if (buildFingerprint != null)
            'voiceBuildFingerprint': buildFingerprint,
        }),
      );
    } on LocalAsrCancelledException {
      // Explicit cancellation never creates a review or sends a message.
    } catch (error) {
      if (mounted && epoch == _voiceOperationEpoch) {
        _showVoiceError(_friendlyVoiceError(error));
      }
    } finally {
      captured?.pcm16.fillRange(0, captured.pcm16.length, 0);
      audio?.samples.fillRange(0, audio.samples.length, 0);
      if (mounted && epoch == _voiceOperationEpoch) {
        _voiceSetState(() {
          _voiceProcessing = false;
          _activeVoiceAsrSelection = null;
        });
      }
    }
  }

  Future<bool> _cancelVoiceCapture() async {
    final captureId = _voiceCaptureId;
    final activeSelection = _activeVoiceAsrSelection;
    final hadNativeCapture = captureId != null;
    final transcriber = _voiceTranscriber;
    final needsLocalCancel = transcriber?.isActive ?? false;
    final needsRemoteCancel =
        activeSelection == VoiceAsrEngineKind.remote && _voiceProcessing;
    final hadOperation =
        _voiceOperationActive || needsLocalCancel || needsRemoteCancel;
    final epoch = ++_voiceOperationEpoch;
    _voicePermissionPending = false;
    _voiceLimitTimer?.cancel();
    _voiceLimitTimer = null;

    if (!hadOperation) return true;
    if (mounted) {
      _voiceSetState(() {
        _voiceRecording = false;
        _voiceProcessing = true;
        _voiceError = null;
      });
    }

    var nativeCleaned = !hadNativeCapture;
    Object? cleanupError;
    try {
      final result = await _voiceCapture.cancel(captureId);
      nativeCleaned = !hadNativeCapture || result;
      if (!nativeCleaned) {
        cleanupError = StateError('原生錄音器尚未確認清除。');
      }
    } catch (error) {
      cleanupError = error;
    }
    if (needsRemoteCancel || activeSelection == VoiceAsrEngineKind.remote) {
      try {
        await widget.client.cancelVoiceAsrTranscription();
      } catch (error) {
        cleanupError ??= error;
      }
    }
    if (needsLocalCancel && transcriber != null) {
      try {
        await transcriber.cancel();
      } catch (error) {
        cleanupError ??= error;
      }
    }

    if (cleanupError == null) {
      if (mounted && epoch == _voiceOperationEpoch) {
        _voiceSetState(() {
          _voiceRecording = false;
          _voiceProcessing = false;
          _voiceCaptureId = null;
          _activeVoiceAsrSelection = null;
          _voiceError = null;
        });
      }
      return true;
    }

    if (mounted && epoch == _voiceOperationEpoch) {
      _voiceSetState(() {
        _voiceRecording = false;
        _voiceProcessing = true;
        _voiceCaptureId = nativeCleaned ? null : captureId;
        _voiceError = '語音清除尚未完成；請再按一次取消。伺服器或模型不會在此狀態切換。';
      });
      _showVoiceSnack('語音清除尚未完成，請再試一次。');
    }
    return false;
  }

  Future<bool> _prepareForAuthorityMutation() async {
    _serverAuthorityEpoch += 1;
    return _cancelVoiceCapture();
  }

  void _resetVoiceAuthorityState() {
    _serverAuthorityEpoch += 1;
    _voiceOperationEpoch += 1;
    _voiceLimitTimer?.cancel();
    _voiceLimitTimer = null;
    if (!mounted) return;
    _voiceSetState(() {
      _voiceAsrCatalog = VoiceAsrEngineCatalog.localOnly();
      _voiceAsrSelection = VoiceAsrEngineKind.local;
      _activeVoiceAsrSelection = null;
      _voiceAsrCatalogLoading = false;
      _voiceRecording = false;
      _voiceProcessing = false;
      _voicePermissionPending = false;
      _voiceCaptureId = null;
      _voiceError = null;
    });
  }

  void _showVoiceError(String message) {
    if (!mounted) return;
    _voiceSetState(() => _voiceError = message);
    _showVoiceSnack(message);
  }

  void _showVoiceSnack(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
      ..hideCurrentSnackBar()
      ..showSnackBar(SnackBar(content: Text(message)));
  }

  String _newVoiceCaptureId() {
    final random = math.Random.secure();
    final bytes = List<int>.generate(16, (_) => random.nextInt(256));
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    String hex(int value) => value.toRadixString(16).padLeft(2, '0');
    final value = bytes.map(hex).join();
    return '${value.substring(0, 8)}-'
        '${value.substring(8, 12)}-'
        '${value.substring(12, 16)}-'
        '${value.substring(16, 20)}-'
        '${value.substring(20)}';
  }
}

extension _VoiceSettingsActions on _SettingsSheetState {
  Future<void> _loadVoiceSettings() async {
    if (_voiceSettingsLoading || _voiceModelOperation) return;
    _voiceSetState(() {
      _voiceSettingsLoading = true;
      _voiceSettingsError = null;
    });
    LocalAsrModelStatus? modelStatus;
    VoiceAsrEngineCatalog? catalog;
    Object? modelError;
    Object? catalogError;
    await Future.wait<void>([
      () async {
        try {
          modelStatus = await widget.onRefreshVoiceModel();
        } catch (error) {
          modelError = error;
        }
      }(),
      () async {
        try {
          catalog = await widget.onRefreshVoiceCatalog();
        } catch (error) {
          catalogError = error;
        }
      }(),
    ]);
    if (!mounted) return;
    _voiceSetState(() {
      if (modelStatus != null) _voiceModelStatus = modelStatus;
      if (catalog != null) _voiceCatalog = catalog!;
      _voiceSettingsLoading = false;
      if (modelError != null && catalogError != null) {
        _voiceSettingsError = '本機模型與家用自架服務狀態目前都讀不到。';
      } else if (modelError != null) {
        _voiceSettingsError = '本機語音模型狀態目前讀不到。';
      } else if (catalogError != null) {
        _voiceSettingsError = '家用自架語音服務狀態目前讀不到。';
      }
    });
  }

  Future<void> _selectLocalVoiceEngine() async {
    if (_voiceModelOperation || _voiceSettingsLoading) return;
    final changed = await widget.onSelectVoiceEngine(VoiceAsrEngineKind.local);
    if (!mounted) return;
    _voiceSetState(() {
      if (changed) {
        _voiceSelection = VoiceAsrEngineKind.local;
        _voiceSettingsError = null;
      } else {
        _voiceSettingsError = '請先完成目前的語音輸入，再切換辨識方式。';
      }
    });
  }

  Future<void> _selectRemoteVoiceEngine() async {
    final remote = _voiceCatalog.selfHosted;
    if (_voiceModelOperation ||
        _voiceSettingsLoading ||
        remote?.available != true) {
      return;
    }
    if (_voiceSelection == VoiceAsrEngineKind.remote) return;
    final confirmed = await _showDisclosureConfirm(
      context,
      title: '使用家裡的自架 ASR？',
      summary: '錄音會傳到你設定好的家用語音服務，而不是留在這台裝置上處理。',
      details:
          '只會送到你已配對、固定設定的家用 TempestMiku 服務，不會改送第三方雲端，'
          '失敗時也不會自動退回本機辨識。轉寫一定先開啟讓你編輯確認，不會自動送出給 Miku。',
      confirmLabel: '使用家用服務',
      confirmKey: const Key('confirm-self-hosted-voice-asr'),
    );
    if (confirmed != true || !mounted) return;
    final changed = await widget.onSelectVoiceEngine(VoiceAsrEngineKind.remote);
    if (!mounted) return;
    _voiceSetState(() {
      if (changed) {
        _voiceSelection = VoiceAsrEngineKind.remote;
        _voiceSettingsError = null;
      } else {
        _voiceSettingsError = '家用自架服務目前無法使用，辨識方式沒有改變。';
      }
    });
  }

  Future<void> _confirmInstallVoiceModel() async {
    if (_voiceModelOperation) return;
    final confirmed = await _showDisclosureConfirm(
      context,
      title: '安裝已驗證的本機語音模型？',
      summary: '下載約 226 MB 的離線語音模型，之後辨識完全在裝置上進行。',
      details:
          '模型來自 Hugging Face 上固定版本的 csukuangfj 模型（Apache-2.0 授權），'
          '安裝後存放在應用程式的私有空間，不會納入系統備份。只有識別碼與檔案都驗證通過'
          '才會啟用；轉寫仍須你確認後才會送出。',
      confirmLabel: '下載並驗證',
      confirmKey: const Key('confirm-install-voice-model'),
    );
    if (confirmed != true || !mounted) return;
    final cancellation = LocalAsrCancellationToken();
    _voiceSetState(() {
      _voiceModelOperation = true;
      _voiceSettingsError = null;
      _installProgress = null;
      _installCancellation = cancellation;
    });
    try {
      final status = await widget.onInstallVoiceModel(
        onProgress: (progress) {
          if (!mounted) return;
          _voiceSetState(() => _installProgress = progress);
        },
        cancellation: cancellation,
      );
      if (!mounted) return;
      _voiceSetState(() => _voiceModelStatus = status);
    } on LocalAsrCancelledException {
      if (!mounted) return;
      _voiceSetState(() => _voiceSettingsError = '已取消下載，模型仍保持停用。');
    } catch (_) {
      if (!mounted) return;
      _voiceSetState(() => _voiceSettingsError = '模型沒有安裝完成或驗證失敗，仍保持停用。');
    } finally {
      if (mounted) {
        _voiceSetState(() {
          _voiceModelOperation = false;
          _installProgress = null;
          _installCancellation = null;
        });
      }
    }
  }

  Future<void> _confirmDeleteVoiceModel() async {
    if (_voiceModelOperation) return;
    final confirmed = await _showDisclosureConfirm(
      context,
      title: '刪除本機語音模型？',
      summary: '停止目前的語音作業，並移除裝置上的離線語音模型。',
      details: '之後仍可以重新下載並驗證；這不會刪除任何對話內容或轉寫草稿。',
      cancelLabel: '保留模型',
      confirmLabel: '刪除模型',
      confirmKey: const Key('confirm-delete-voice-model'),
      destructive: true,
    );
    if (confirmed != true || !mounted) return;
    _voiceSetState(() {
      _voiceModelOperation = true;
      _voiceSettingsError = null;
    });
    try {
      final status = await widget.onDeleteVoiceModel();
      if (!mounted) return;
      _voiceSetState(() => _voiceModelStatus = status);
    } catch (_) {
      if (!mounted) return;
      _voiceSetState(() => _voiceSettingsError = '模型沒有刪除；請先確認語音錄音已安全清除。');
    } finally {
      if (mounted) _voiceSetState(() => _voiceModelOperation = false);
    }
  }
}

class _VoiceSettingsPanel extends StatelessWidget {
  const _VoiceSettingsPanel({
    required this.modelStatus,
    required this.catalog,
    required this.selection,
    required this.loading,
    required this.modelOperation,
    required this.installing,
    required this.installProgress,
    required this.error,
    required this.onSelectLocal,
    required this.onSelectRemote,
    required this.onInstallModel,
    required this.onDeleteModel,
    required this.onCancelInstall,
  });

  final LocalAsrModelStatus? modelStatus;
  final VoiceAsrEngineCatalog catalog;
  final VoiceAsrEngineKind selection;
  final bool loading;
  final bool modelOperation;
  final bool installing;
  final LocalAsrModelInstallProgress? installProgress;
  final String? error;
  final VoidCallback onSelectLocal;
  final VoidCallback onSelectRemote;
  final VoidCallback onInstallModel;
  final VoidCallback onDeleteModel;
  final VoidCallback? onCancelInstall;

  @override
  Widget build(BuildContext context) {
    final palette = TmTokens.of(context);
    final remote = catalog.selfHosted;
    final status = modelStatus;
    final modelLabel = switch (status?.state) {
      LocalAsrModelState.ready => '已安裝並驗證',
      LocalAsrModelState.missing => '尚未安裝',
      LocalAsrModelState.corrupt => '檔案毀損，已停用',
      LocalAsrModelState.unsupported => '這台裝置不支援',
      null => '正在檢查',
    };
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (error != null) ...[
          Semantics(
            liveRegion: true,
            child: Text(
              error!,
              key: const Key('voice-settings-error'),
              style: Theme.of(context).textTheme.bodySmall?.copyWith(
                color: Theme.of(context).colorScheme.error,
              ),
            ),
          ),
          const SizedBox(height: 8),
        ],
        Container(
          key: const Key('voice-model-settings'),
          padding: const EdgeInsets.fromLTRB(14, 12, 10, 12),
          decoration: BoxDecoration(
            color: Theme.of(context).colorScheme.surface,
            borderRadius: BorderRadius.circular(14),
            border: Border.all(color: palette.outline),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  const Icon(Icons.memory_rounded, size: 20),
                  const SizedBox(width: 10),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          '裝置端語音模型',
                          style: Theme.of(context).textTheme.labelLarge,
                        ),
                        Text(
                          modelLabel,
                          key: const Key('voice-model-status'),
                          style: Theme.of(
                            context,
                          ).textTheme.bodySmall?.copyWith(color: palette.muted),
                        ),
                      ],
                    ),
                  ),
                  if (modelOperation && installing)
                    TextButton(
                      key: const Key('cancel-voice-model-install'),
                      onPressed: onCancelInstall,
                      child: const Text('取消'),
                    )
                  else if (modelOperation)
                    const Padding(
                      padding: EdgeInsets.all(12),
                      child: SizedBox.square(
                        dimension: 18,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      ),
                    )
                  else if (status?.ready == true ||
                      status?.state == LocalAsrModelState.corrupt)
                    TextButton(
                      key: const Key('delete-voice-model'),
                      onPressed: onDeleteModel,
                      child: const Text('刪除'),
                    )
                  else if (status != null &&
                      status.state != LocalAsrModelState.unsupported)
                    FilledButton.tonal(
                      key: const Key('install-voice-model'),
                      onPressed: onInstallModel,
                      child: const Text('安裝'),
                    ),
                ],
              ),
              if (modelOperation && installing) ...[
                const SizedBox(height: 8),
                LinearProgressIndicator(
                  key: const Key('voice-model-install-progress'),
                  value: installProgress?.fraction,
                ),
                if (installProgress != null &&
                    installProgress!.totalBytes > 0) ...[
                  const SizedBox(height: 4),
                  Text(
                    '下載中 ${(installProgress!.receivedBytes / (1024 * 1024)).round()}'
                    ' / ${(installProgress!.totalBytes / (1024 * 1024)).round()} MiB',
                    key: const Key('voice-model-install-progress-label'),
                    style: Theme.of(
                      context,
                    ).textTheme.bodySmall?.copyWith(color: palette.muted),
                  ),
                ],
              ],
              if (status?.reason.trim().isNotEmpty == true) ...[
                const SizedBox(height: 6),
                Text(
                  status!.reason,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(
                    context,
                  ).textTheme.bodySmall?.copyWith(color: palette.muted),
                ),
              ],
              const SizedBox(height: 6),
              Text(
                '本機辨識只在模型完整且識別碼驗證通過時啟用；音訊不離開裝置。',
                style: Theme.of(
                  context,
                ).textTheme.bodySmall?.copyWith(color: palette.muted),
              ),
            ],
          ),
        ),
        const SizedBox(height: 10),
        Container(
          decoration: BoxDecoration(
            color: Theme.of(context).colorScheme.surface,
            borderRadius: BorderRadius.circular(14),
            border: Border.all(color: palette.outline),
          ),
          clipBehavior: Clip.antiAlias,
          child: Column(
            children: [
              ListTile(
                key: const Key('select-local-voice-asr'),
                minTileHeight: 56,
                selected: selection == VoiceAsrEngineKind.local,
                leading: const Icon(Icons.phone_android_rounded),
                title: const Text('本機辨識'),
                subtitle: Text(
                  status?.ready == true ? '音訊留在裝置上' : '需要已驗證的本機模型',
                ),
                trailing:
                    selection == VoiceAsrEngineKind.local
                        ? const Icon(Icons.check_circle_rounded)
                        : null,
                onTap: loading || modelOperation ? null : onSelectLocal,
              ),
              Divider(height: 1, color: palette.outline),
              ListTile(
                key: const Key('select-self-hosted-voice-asr'),
                minTileHeight: 56,
                enabled: remote?.available == true,
                selected: selection == VoiceAsrEngineKind.remote,
                leading: const Icon(Icons.home_work_outlined),
                title: const Text('家用遠端（自架）'),
                subtitle: Text(
                  remote?.available == true
                      ? '已設定 · ${remote?.modelId ?? remote?.label}'
                      : '配對的伺服器目前未提供',
                ),
                trailing:
                    selection == VoiceAsrEngineKind.remote
                        ? const Icon(Icons.check_circle_rounded)
                        : null,
                onTap:
                    loading || modelOperation || remote?.available != true
                        ? null
                        : onSelectRemote,
              ),
            ],
          ),
        ),
        if (selection == VoiceAsrEngineKind.remote) ...[
          const SizedBox(height: 8),
          Text(
            '錄音會經配對的伺服器傳到固定的家用服務；失敗時不會改送雲端或退回本機。',
            key: const Key('self-hosted-voice-disclosure'),
            style: Theme.of(
              context,
            ).textTheme.bodySmall?.copyWith(color: palette.muted),
          ),
        ],
      ],
    );
  }
}

String _friendlyVoiceError(Object error) {
  if (error is TimeoutException) {
    return '語音辨識已超過時間上限；錄音已清除，沒有建立草稿。';
  }
  if (error is FormatException && error.message.toString().contains('沒有產生')) {
    return '沒有辨識到可檢查的文字；錄音已清除，沒有自動送出。';
  }
  final detail = error.toString().replaceFirst(RegExp(r'^\w+Exception: '), '');
  return detail.trim().isEmpty ? '語音輸入沒有完成；錄音已清除，沒有自動送出。' : '語音輸入沒有完成：$detail';
}
