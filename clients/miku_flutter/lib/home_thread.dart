part of 'main.dart';

class _MikuChatSurface extends StatelessWidget {
  const _MikuChatSurface({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.status,
    required this.scrollController,
    required this.dotAnimation,
    required this.rounds,
    required this.memoryProposals,
    required this.approvals,
    required this.showJumpToLatest,
    required this.approvalForProposal,
    required this.isRenderedAsMemoryProposal,
    required this.onJumpToLatest,
    required this.onShowActivity,
    required this.onOpenResource,
    required this.onResolve,
    required this.onShowApproval,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final String status;
  final ScrollController scrollController;
  final AnimationController dotAnimation;
  final List<_ConversationRound> rounds;
  final List<MemoryWriteProposal> memoryProposals;
  final List<ApprovalPrompt> approvals;
  final bool showJumpToLatest;
  final ApprovalPrompt? Function(MemoryWriteProposal) approvalForProposal;
  final bool Function(ApprovalPrompt) isRenderedAsMemoryProposal;
  final VoidCallback onJumpToLatest;
  final ValueChanged<_ConversationRound> onShowActivity;
  final ValueChanged<String> onOpenResource;
  final void Function(ApprovalPrompt, String) onResolve;
  final ValueChanged<ApprovalPrompt> onShowApproval;

  @override
  Widget build(BuildContext context) {
    return Stack(
      children: [
        Positioned.fill(
          child: _MikuThread(
            tok: tok,
            copy: copy,
            accent: accent,
            status: status,
            scrollController: scrollController,
            dotAnimation: dotAnimation,
            rounds: rounds,
            memoryProposals: memoryProposals,
            approvals: approvals,
            approvalForProposal: approvalForProposal,
            isRenderedAsMemoryProposal: isRenderedAsMemoryProposal,
            onShowActivity: onShowActivity,
            onOpenResource: onOpenResource,
            onResolve: onResolve,
            onShowApproval: onShowApproval,
          ),
        ),
        if (showJumpToLatest)
          Positioned(
            right: 18,
            bottom: 12,
            child: Semantics(
              button: true,
              label: copy.pick('Jump to latest message', '跳到最新訊息'),
              child: FloatingActionButton.small(
                heroTag: 'jump-to-latest',
                onPressed: onJumpToLatest,
                child: const Icon(Icons.arrow_downward_rounded),
              ),
            ),
          ),
      ],
    );
  }
}

class _MikuThread extends StatelessWidget {
  const _MikuThread({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.status,
    required this.scrollController,
    required this.dotAnimation,
    required this.rounds,
    required this.memoryProposals,
    required this.approvals,
    required this.approvalForProposal,
    required this.isRenderedAsMemoryProposal,
    required this.onShowActivity,
    required this.onOpenResource,
    required this.onResolve,
    required this.onShowApproval,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final String status;
  final ScrollController scrollController;
  final AnimationController dotAnimation;
  final List<_ConversationRound> rounds;
  final List<MemoryWriteProposal> memoryProposals;
  final List<ApprovalPrompt> approvals;
  final ApprovalPrompt? Function(MemoryWriteProposal) approvalForProposal;
  final bool Function(ApprovalPrompt) isRenderedAsMemoryProposal;
  final ValueChanged<_ConversationRound> onShowActivity;
  final ValueChanged<String> onOpenResource;
  final void Function(ApprovalPrompt, String) onResolve;
  final ValueChanged<ApprovalPrompt> onShowApproval;

  @override
  Widget build(BuildContext context) {
    final items = <Widget>[];

    if (rounds.isEmpty && memoryProposals.isEmpty && approvals.isEmpty) {
      items.add(_EmptyState(tok: tok, status: status, copy: copy));
      items.add(const SizedBox(height: 14));
    }

    for (final round in rounds) {
      if (round.userText.isNotEmpty) {
        items.add(
          _UserBubble(tok: tok, text: round.userText, accent: tok.accentSoft),
        );
        items.add(const SizedBox(height: 10));
      }

      final assistantText = round.assistantText;
      void addActivityTrace() {
        if (round.activities.isEmpty) return;
        items.add(
          _AgentStatusBar(
            tok: tok,
            copy: copy,
            accent: accent,
            anim: dotAnimation,
            roundIndex: round.index,
            agents: _agentStatuses(round.activities),
            activities: round.activities,
            expanded: round.activityExpanded,
            onTap: () => onShowActivity(round),
            onOpenResource: onOpenResource,
          ),
        );
        items.add(const SizedBox(height: 10));
      }

      void addReasoningTrace() {
        if (!round.hasReasoning) return;
        items.add(
          _ThinkingTrace(
            tok: tok,
            copy: copy,
            accent: accent,
            text: round.reasoningText,
            expanded: round.reasoningExpanded,
            isStreaming:
                round.assistantFinalText.isEmpty &&
                round.isStreaming &&
                round.assistantStreamedText.isEmpty,
          ),
        );
        items.add(const SizedBox(height: 10));
      }

      void addAssistantAnswer() {
        if (assistantText.isEmpty) return;
        items.add(
          _MikuBubble(
            tok: tok,
            copy: copy,
            text: assistantText,
            accent: accent,
            resources: _extractResources(assistantText),
            onOpenResource: onOpenResource,
            isStreaming: round.assistantFinalText.isEmpty && round.isStreaming,
          ),
        );
      }

      if (assistantText.isNotEmpty) {
        addAssistantAnswer();
        items.add(const SizedBox(height: 10));
      } else if (round.isStreaming) {
        items.add(
          _TypingIndicator(tok: tok, accent: accent, anim: dotAnimation),
        );
        items.add(const SizedBox(height: 10));
      }
      addActivityTrace();
      addReasoningTrace();
      items.add(const SizedBox(height: 14));
    }

    for (final proposal in memoryProposals) {
      final approval = approvalForProposal(proposal);
      items.add(
        _MemoryProposalCard(
          tok: tok,
          copy: copy,
          proposal: proposal,
          approval: approval,
          accent: accent,
          onApprove:
              approval == null ? null : () => onResolve(approval, 'approve'),
          onDeny: approval == null ? null : () => onResolve(approval, 'deny'),
        ),
      );
      items.add(const SizedBox(height: 10));
    }

    for (final approval in approvals.where(
      (item) => !isRenderedAsMemoryProposal(item),
    )) {
      items.add(
        _ApprovalCard(
          tok: tok,
          copy: copy,
          approval: approval,
          accent: accent,
          onTap: () => onShowApproval(approval),
        ),
      );
      items.add(const SizedBox(height: 10));
    }

    return LayoutBuilder(
      builder: (context, constraints) {
        return Center(
          child: SizedBox(
            width: math.min(constraints.maxWidth, 720),
            height: constraints.maxHeight,
            child: ListView.builder(
              controller: scrollController,
              padding: const EdgeInsets.fromLTRB(14, 10, 14, 16),
              itemCount: items.length,
              itemBuilder: (context, index) => items[index],
            ),
          ),
        );
      },
    );
  }
}
