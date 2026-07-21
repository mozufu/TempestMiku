part of '../session_client_stub.dart';

const ModeCatalog _scriptedModeCatalog = ModeCatalog(
  defaultMode: 'personal_assistant',
  modes: [
    ModeProfile(
      id: 'personal_assistant',
      label: 'Personal Assistant',
      defaultScope: 'global',
      capabilityClass: 'conversation',
      activeSkills: ['miku-voice', 'personal-assistant-state-capture'],
      capabilities: ['drive.*', 'project.*', 'http.request', 'resources.read:artifact', 'resources.read:drive', 'resources.read:skill'],
      description: 'Planning, reminders, writing, and open loops.',
    ),
    ModeProfile(
      id: 'ambiguity_grill',
      label: 'Ambiguity Grill',
      defaultScope: 'global',
      capabilityClass: 'conversation',
      activeSkills: ['miku-voice', 'ambiguity-grill'],
      capabilities: [],
      description: 'Sharp clarification before planning.',
    ),
    ModeProfile(
      id: 'negative_state_grounding',
      label: 'Negative-State Grounding',
      defaultScope: 'global',
      capabilityClass: 'conversation',
      activeSkills: ['miku-voice', 'negative-state-grounding'],
      capabilities: [],
      description: 'Stabilize overwhelm before action.',
    ),
    ModeProfile(
      id: 'serious_engineer',
      label: 'Serious Engineer',
      defaultScope: 'project:tempestmiku',
      capabilityClass: 'engineering',
      activeSkills: [],
      capabilities: ['fs.*', 'code.*', 'proc.*', 'backend.coding'],
      description: 'Code, production, irreversible, or external work.',
    ),
    ModeProfile(
      id: 'handoff',
      label: 'Handoff',
      defaultScope: 'project:tempestmiku',
      capabilityClass: 'handoff',
      activeSkills: ['oh-my-pi-handoff'],
      capabilities: ['agents.*', 'backend.coding'],
      description: 'Delegate implementation-heavy coding work.',
    ),
  ],
);
