/* SayFlex — states prototype.
 *
 * The point of this file is the STATES table below. Every state named there is
 * one the application actually produces: a backend that failed its warm-up, a
 * download being verified, a provider whose free allowance ran out, a
 * recording nobody heard. A layout that only looks right in the happy state is
 * not a design, and the switcher in the footer exists so each one can be
 * looked at directly. */

const BAR_COUNT = 42;

/* ── Dictation ─────────────────────────────────────────── */

const DICTATION_STATES = {
  ready: {
    label: 'Ready',
    subtitle: 'Local speech · ready',
    signal: 'ready',
    value: null,
    caption: 'hold Ctrl + Alt + Space',
    status: 'SYSTEM READY',
  },
  listening: {
    label: 'Listening',
    subtitle: 'Local speech · listening',
    signal: 'listening',
    value: '0:04',
    caption: 'release to transcribe',
    status: 'LISTENING',
  },
  processing: {
    label: 'Processing',
    subtitle: 'Local speech · processing',
    signal: 'processing',
    value: '0:04',
    caption: 'transcribing 4.2 s of audio',
    status: 'PROCESSING',
  },
  inserted: {
    label: 'Inserted',
    subtitle: 'Local speech · inserted',
    signal: 'ready',
    value: '212',
    caption: 'ms from release to insertion',
    metrics: { latency: '212 ms', target: 'Code.exe', source: 'Local · CUDA' },
    status: 'INSERTED',
  },
  remote: {
    label: 'Via service',
    subtitle: 'Groq · transcribed remotely',
    signal: 'ready',
    value: '1.9',
    caption: 'seconds from release to insertion',
    metrics: { latency: '1 940 ms', target: 'Code.exe', source: 'Groq' },
    note: {
      tone: 'signal',
      text: 'This recording was sent to Groq. Audio left this device because a service is above the local engine in your order.',
      action: 'Reorder',
    },
    statusMode: 'AUDIO LEAVES DEVICE',
    status: 'INSERTED VIA GROQ',
  },
  'no-speech': {
    label: 'No speech',
    subtitle: 'Local speech · nothing recognized',
    signal: 'no-speech',
    value: null,
    caption: 'no speech was recognized',
    note: {
      text: 'Nothing was heard in 1.1 s of audio. The recording was not saved.',
    },
    status: 'NO SPEECH',
  },
  'target-lost': {
    label: 'Target lost',
    subtitle: 'Local speech · target window closed',
    signal: 'ready',
    value: '318',
    caption: 'ms — kept, not delivered',
    metrics: { latency: '318 ms', target: 'closed', source: 'Local · CUDA' },
    note: {
      tone: 'signal',
      text: 'The window that was recorded is gone. The transcript is in history and can be placed wherever the caret is now.',
      action: 'Insert last',
    },
    status: 'TARGET LOST',
  },
  'engine-fallback': {
    label: 'Fallback',
    subtitle: 'Vulkan · CUDA did not pass warm-up',
    signal: 'ready',
    value: null,
    caption: 'hold Ctrl + Alt + Space',
    note: {
      tone: 'signal',
      text: 'CUDA did not complete a warm-up pass on this machine, so Vulkan is active. Dictation works; the first result may be slower.',
      action: 'Details',
    },
    engineNote: 'Vulkan active · CUDA failed warm-up',
    status: 'VULKAN ACTIVE',
  },
};

/* ── Models ────────────────────────────────────────────── */

const MODEL_STATES = {
  'at-rest': [
    { name: 'Whisper Large v3 Turbo', tag: 'Recommended', size: '1.62 GB', detail: 'Default profile · detects the spoken language', state: 'selected', text: 'Selected' },
    { name: 'Whisper Large v3', size: '3.10 GB', detail: 'Maximum quality · can translate speech to English', state: 'installed', text: 'Installed', action: 'Use' },
    { name: 'Whisper Medium', size: '1.53 GB', detail: 'Balanced profile for systems below the recommended one', state: 'idle', text: 'Not installed', action: 'Install' },
    { name: 'Whisper Small', size: '0.49 GB', detail: 'Lightweight · lower download and resource use', state: 'idle', text: 'Not installed', action: 'Install' },
  ],
  installing: [
    { name: 'Whisper Large v3 Turbo', tag: 'Recommended', size: '1.62 GB', detail: 'Default profile · detects the spoken language', state: 'selected', text: 'Selected' },
    { name: 'Whisper Large v3', size: '3.10 GB', detail: '1.34 GB of 3.10 GB · resumes if interrupted', state: 'downloading', progress: 43, text: '43 %', action: 'Cancel' },
    { name: 'Whisper Medium', size: '1.53 GB', detail: 'Balanced profile for systems below the recommended one', state: 'idle', text: 'Not installed', action: 'Install' },
    { name: 'Whisper Small', size: '0.49 GB', detail: 'Lightweight · lower download and resource use', state: 'idle', text: 'Not installed', action: 'Install' },
  ],
  verifying: [
    { name: 'Whisper Large v3 Turbo', tag: 'Recommended', size: '1.62 GB', detail: 'Default profile · detects the spoken language', state: 'selected', text: 'Selected' },
    { name: 'Whisper Large v3', size: '3.10 GB', detail: 'Checking SHA-256 before the file is put in place', state: 'verifying', progress: 100, text: 'Verifying' },
    { name: 'Whisper Medium', size: '1.53 GB', detail: 'Balanced profile for systems below the recommended one', state: 'idle', text: 'Not installed', action: 'Install' },
    { name: 'Whisper Small', size: '0.49 GB', detail: 'Lightweight · lower download and resource use', state: 'idle', text: 'Not installed', action: 'Install' },
  ],
  failed: [
    { name: 'Whisper Large v3 Turbo', tag: 'Recommended', size: '1.62 GB', detail: 'Default profile · detects the spoken language', state: 'selected', text: 'Selected' },
    { name: 'Whisper Large v3', size: '3.10 GB', detail: 'Checksum did not match · the download was discarded', state: 'refused', text: 'Verification failed', action: 'Retry' },
    { name: 'Whisper Medium', size: '1.53 GB', detail: 'Balanced profile for systems below the recommended one', state: 'idle', text: 'Not installed', action: 'Install' },
    { name: 'Whisper Small', size: '0.49 GB', detail: 'Lightweight · lower download and resource use', state: 'idle', text: 'Not installed', action: 'Install' },
  ],
  adopted: [
    { name: 'Whisper Large v3 Turbo', tag: 'Recommended', size: '1.62 GB', detail: 'Reused from the previous installation without copying', state: 'selected', text: 'Selected' },
    { name: 'Whisper Large v3', size: '3.10 GB', detail: 'Maximum quality · can translate speech to English', state: 'idle', text: 'Not installed', action: 'Install' },
    { name: 'Whisper Medium', size: '1.53 GB', detail: 'Balanced profile for systems below the recommended one', state: 'idle', text: 'Not installed', action: 'Install' },
    { name: 'Whisper Small', size: '0.49 GB', detail: 'Lightweight · lower download and resource use', state: 'idle', text: 'Not installed', action: 'Install' },
  ],
};

const PACK_ROWS = {
  'at-rest': [
    { name: 'Qwen3.5 4B', tag: 'Fast', size: '2.80 GB', detail: 'Multilingual · runs locally through Vulkan', state: 'selected', text: 'Selected' },
    { name: 'Qwen3.5 9B', tag: 'Quality', size: '5.70 GB', detail: 'Higher quality · larger download and more memory', state: 'idle', text: 'Not installed', action: 'Install' },
  ],
  installing: [
    { name: 'Qwen3.5 4B', tag: 'Fast', size: '2.80 GB', detail: 'Multilingual · runs locally through Vulkan', state: 'selected', text: 'Selected' },
    { name: 'Qwen3.5 9B', tag: 'Quality', size: '5.70 GB', detail: 'Runtime 32 MB, then weights · 0.4 GB of 5.70 GB', state: 'downloading', progress: 7, text: '7 %', action: 'Cancel' },
  ],
};
PACK_ROWS.verifying = PACK_ROWS['at-rest'];
PACK_ROWS.failed = PACK_ROWS['at-rest'];
PACK_ROWS.adopted = PACK_ROWS['at-rest'];

/* ── Providers ─────────────────────────────────────────── */

const PROVIDER_STATES = {
  'local-only': {
    privacy: { text: 'Audio stays on this device', tone: 'ink' },
    statusMode: 'LOCAL · PRIVATE',
    rows: [
      { name: 'This computer', detail: 'Whisper Large v3 Turbo · CUDA', state: 'active', text: 'In use' },
      { name: 'Groq', detail: '2 000 requests a day · free', state: 'disabled', text: 'No key stored', action: 'Add key' },
      { name: 'Cloudflare Workers AI', detail: '10 000 neurons a day, shared across models', state: 'disabled', text: 'No key stored', action: 'Add key' },
      { name: 'Microsoft Azure Speech', detail: '5 hours a month · needs a region', state: 'disabled', text: 'No key stored', action: 'Add key' },
      { name: 'Deepgram', detail: '$200 of credit, no expiry', state: 'disabled', text: 'No key stored', action: 'Add key' },
      { name: 'Custom endpoint', detail: 'Any OpenAI-compatible address · HTTPS only', state: 'disabled', text: 'Not set', action: 'Set up' },
    ],
  },
  'service-first': {
    privacy: { text: 'Audio leaves this device', tone: 'signal' },
    statusMode: 'AUDIO LEAVES DEVICE',
    rows: [
      { name: 'Cloudflare Workers AI', detail: 'Key ••••4f21 · 6 400 of 10 000 neurons left today', state: 'active', text: 'In use' },
      { name: 'Groq', detail: 'Key ••••9c07 · 2 000 requests a day', state: 'installed', text: 'Standing by' },
      { name: 'This computer', detail: 'Whisper Large v3 Turbo · CUDA', state: 'installed', text: 'Last resort' },
      { name: 'Microsoft Azure Speech', detail: '5 hours a month · needs a region', state: 'disabled', text: 'No key stored', action: 'Add key' },
      { name: 'Deepgram', detail: '$200 of credit, no expiry', state: 'disabled', text: 'No key stored', action: 'Add key' },
      { name: 'Custom endpoint', detail: 'Any OpenAI-compatible address · HTTPS only', state: 'disabled', text: 'Not set', action: 'Set up' },
    ],
  },
  'quota-spent': {
    privacy: { text: 'Audio leaves this device', tone: 'signal' },
    statusMode: 'AUDIO LEAVES DEVICE',
    rows: [
      { name: 'Cloudflare Workers AI', detail: 'Daily neurons spent · resets at 00:00 UTC', state: 'exhausted', text: 'Out until 00:00' },
      { name: 'Groq', detail: 'Key ••••9c07 · 1 840 requests left today', state: 'active', text: 'In use' },
      { name: 'This computer', detail: 'Whisper Large v3 Turbo · CUDA', state: 'installed', text: 'Last resort' },
      { name: 'Microsoft Azure Speech', detail: 'Recording longer than 60 s · not offered for it', state: 'installed', text: 'Standing by' },
      { name: 'Deepgram', detail: '$200 of credit, no expiry', state: 'disabled', text: 'No key stored', action: 'Add key' },
      { name: 'Custom endpoint', detail: 'Any OpenAI-compatible address · HTTPS only', state: 'disabled', text: 'Not set', action: 'Set up' },
    ],
  },
  'key-refused': {
    privacy: { text: 'Audio leaves this device', tone: 'signal' },
    statusMode: 'AUDIO LEAVES DEVICE',
    rows: [
      { name: 'Cloudflare Workers AI', detail: 'The service refused this key · waiting will not fix it', state: 'refused', text: 'Key refused', action: 'Replace' },
      { name: 'Groq', detail: 'Key ••••9c07 · 2 000 requests a day', state: 'active', text: 'In use' },
      { name: 'This computer', detail: 'Whisper Large v3 Turbo · CUDA', state: 'installed', text: 'Last resort' },
      { name: 'Microsoft Azure Speech', detail: '5 hours a month · needs a region', state: 'disabled', text: 'No key stored', action: 'Add key' },
      { name: 'Deepgram', detail: '$200 of credit, no expiry', state: 'disabled', text: 'No key stored', action: 'Add key' },
      { name: 'Custom endpoint', detail: 'Any OpenAI-compatible address · HTTPS only', state: 'disabled', text: 'Not set', action: 'Set up' },
    ],
  },
  'all-out': {
    privacy: { text: 'Nothing can transcribe right now', tone: 'signal' },
    statusMode: 'NO PROVIDER AVAILABLE',
    rows: [
      { name: 'Cloudflare Workers AI', detail: 'Daily neurons spent · resets at 00:00 UTC', state: 'exhausted', text: 'Out until 00:00' },
      { name: 'Groq', detail: 'Daily requests spent · resets at 00:00 UTC', state: 'exhausted', text: 'Out until 00:00' },
      { name: 'This computer', detail: 'Switched off in this order', state: 'disabled', text: 'Off', action: 'Switch on' },
      { name: 'Microsoft Azure Speech', detail: '5 hours a month · needs a region', state: 'disabled', text: 'No key stored', action: 'Add key' },
      { name: 'Deepgram', detail: '$200 of credit, no expiry', state: 'disabled', text: 'No key stored', action: 'Add key' },
      { name: 'Custom endpoint', detail: 'Any OpenAI-compatible address · HTTPS only', state: 'disabled', text: 'Not set', action: 'Set up' },
    ],
  },
};

/* ── Overview ──────────────────────────────────────────── */

const OVERVIEW_STATES = {
  ready: {
    statement: 'Ready to dictate',
    sub: 'Hold Ctrl + Alt + Space anywhere in Windows.',
    privacy: { text: 'Audio stays on this device', tone: 'ink' },
    statusMode: 'LOCAL · PRIVATE',
    status: 'SYSTEM READY',
    facts: [
      { label: 'engine', value: 'CUDA', note: 'verified by warm-up · 5070 Ti' },
      { label: 'model', value: 'Whisper Large v3 Turbo', note: '1.62 GB · detects the language' },
      { label: 'translation', value: 'Qwen3.5 4B', note: 'warm · RU ⇄ EN' },
      { label: 'last dictation', value: '212 ms', note: 'release to insertion' },
    ],
  },
  'first-run': {
    statement: 'One download away',
    sub: 'Speech recognition needs a model on this machine. Nothing is downloaded without asking.',
    privacy: { text: 'Audio stays on this device', tone: 'ink' },
    statusMode: 'LOCAL · PRIVATE',
    status: 'NO MODEL',
    note: {
      text: 'Whisper Large v3 Turbo is the recommended profile: 1.62 GB, detects the spoken language, runs on the GPU.',
      action: 'Install',
    },
    facts: [
      { label: 'engine', value: 'CUDA', note: 'detected · not yet verified' },
      { label: 'model', value: 'None', note: 'nothing installed' },
      { label: 'translation', value: 'Not installed', note: 'optional' },
      { label: 'hotkey', value: 'Ctrl + Alt + Space', note: 'already registered' },
    ],
  },
  downloading: {
    statement: 'Getting the model',
    sub: 'You can keep working. The download resumes by itself if it is interrupted.',
    privacy: { text: 'Audio stays on this device', tone: 'ink' },
    statusMode: 'LOCAL · PRIVATE',
    status: 'DOWNLOADING',
    facts: [
      { label: 'engine', value: 'CUDA', note: 'detected · not yet verified' },
      { label: 'model', value: '43 %', note: '1.34 GB of 1.62 GB', tone: 'signal' },
      { label: 'translation', value: 'Not installed', note: 'optional' },
      { label: 'hotkey', value: 'Ctrl + Alt + Space', note: 'available after install' },
    ],
  },
  degraded: {
    statement: 'Ready, on the slower engine',
    sub: 'Hold Ctrl + Alt + Space anywhere in Windows.',
    privacy: { text: 'Audio stays on this device', tone: 'ink' },
    statusMode: 'LOCAL · PRIVATE',
    status: 'VULKAN ACTIVE',
    note: {
      tone: 'signal',
      text: 'CUDA did not complete a warm-up pass on this machine, so Vulkan is active. Dictation works; the first result may be slower.',
      action: 'Details',
    },
    facts: [
      { label: 'engine', value: 'Vulkan', note: 'CUDA failed warm-up', tone: 'signal' },
      { label: 'model', value: 'Whisper Large v3 Turbo', note: '1.62 GB · detects the language' },
      { label: 'translation', value: 'Qwen3.5 4B', note: 'unloaded · loads on demand' },
      { label: 'last dictation', value: '480 ms', note: 'release to insertion' },
    ],
  },
  remote: {
    statement: 'Ready — through Cloudflare',
    sub: 'A service is above the local engine in your transcription order.',
    privacy: { text: 'Audio leaves this device', tone: 'signal' },
    statusMode: 'AUDIO LEAVES DEVICE',
    status: 'SYSTEM READY',
    note: {
      tone: 'signal',
      text: 'Recordings are sent to Cloudflare Workers AI. When its daily allowance runs out, Groq is next, then this computer.',
      action: 'Reorder',
    },
    facts: [
      { label: 'transcription', value: 'Cloudflare', note: '6 400 of 10 000 neurons left today', tone: 'signal' },
      { label: 'fallback', value: 'Groq, then local', note: 'used when the one above runs out' },
      { label: 'translation', value: 'Qwen3.5 4B', note: 'still local' },
      { label: 'last dictation', value: '1 940 ms', note: 'release to insertion' },
    ],
  },
};

/* ── Translation ───────────────────────────────────────── */

const TRANSLATION_STATES = {
  ready: {
    summary: { text: 'Runs on this device', tone: 'ink' },
    status: 'SYSTEM READY',
    rows: [
      { label: 'Direction', hint: 'Decided by what the selection is written in', value: 'RU ⇄ EN' },
      { label: 'Model', hint: 'Qwen3.5 4B · 2.80 GB installed', value: 'Fast' },
      { label: 'Engine', hint: 'Kept in memory between requests', value: 'Warm' },
      { label: 'Result', hint: 'Where the translation goes', value: 'Preview' },
      { label: 'Preview blur', hint: 'Backdrop behind the floating preview', value: 'stepper', step: 3 },
      { label: 'Hotkey', hint: 'Translates the current selection', value: 'Ctrl + Alt + F' },
    ],
  },
  warming: {
    summary: { text: 'Loading the model', tone: 'signal' },
    status: 'LOADING MODEL',
    note: { tone: 'signal', text: 'Qwen3.5 4B is being loaded onto the GPU. The first translation after this waits for it; the rest do not.' },
    rows: [
      { label: 'Direction', hint: 'Decided by what the selection is written in', value: 'RU ⇄ EN' },
      { label: 'Model', hint: 'Loading · 2.80 GB', value: 'Fast', tone: 'signal' },
      { label: 'Engine', hint: 'Kept in memory between requests', value: 'Warming', tone: 'signal' },
      { label: 'Result', hint: 'Where the translation goes', value: 'Preview' },
      { label: 'Preview blur', hint: 'Backdrop behind the floating preview', value: 'stepper', step: 3 },
      { label: 'Hotkey', hint: 'Translates the current selection', value: 'Ctrl + Alt + F' },
    ],
  },
  'not-installed': {
    summary: { text: 'No pack installed', tone: 'signal' },
    status: 'NOT INSTALLED',
    note: {
      text: 'Translation needs a language pack and a local runtime. Nothing is downloaded until you ask for it.',
      action: 'Install',
    },
    rows: [
      { label: 'Direction', hint: 'Available once a pack is installed', value: 'RU ⇄ EN', muted: true },
      { label: 'Model', hint: 'Qwen3.5 4B · 2.80 GB, or 9B · 5.70 GB', value: 'None', tone: 'signal' },
      { label: 'Runtime', hint: 'llama.cpp Vulkan · verified download', value: '32 MB', muted: true },
      { label: 'Result', hint: 'Where the translation goes', value: 'Preview', muted: true },
      { label: 'Hotkey', hint: 'Does nothing until a pack is installed', value: 'Ctrl + Alt + F', muted: true },
    ],
  },
  replacing: {
    summary: { text: 'Replaces the selection', tone: 'ink' },
    status: 'SYSTEM READY',
    note: {
      text: 'The translation is written straight over the selected text. The original goes to history, so nothing is lost.',
    },
    rows: [
      { label: 'Direction', hint: 'Decided by what the selection is written in', value: 'RU ⇄ EN' },
      { label: 'Model', hint: 'Qwen3.5 9B · 5.70 GB installed', value: 'Quality' },
      { label: 'Engine', hint: 'Unloaded after each request to free memory', value: 'On demand' },
      { label: 'Result', hint: 'Written over the selection', value: 'Replace' },
      { label: 'Hotkey', hint: 'Translates the current selection', value: 'Ctrl + Alt + F' },
    ],
  },
  failed: {
    summary: { text: 'Engine did not start', tone: 'signal' },
    status: 'ENGINE FAILED',
    note: {
      tone: 'signal',
      text: 'The local translation server did not answer in ninety seconds. Dictation and layout correction are unaffected.',
      action: 'Retry',
    },
    rows: [
      { label: 'Direction', hint: 'Decided by what the selection is written in', value: 'RU ⇄ EN', muted: true },
      { label: 'Model', hint: 'Qwen3.5 4B · installed and verified', value: 'Fast' },
      { label: 'Engine', hint: 'Vulkan runtime present · server did not start', value: 'Failed', tone: 'signal' },
      { label: 'Result', hint: 'Where the translation goes', value: 'Preview', muted: true },
      { label: 'Hotkey', hint: 'Reports the failure instead of translating', value: 'Ctrl + Alt + F', muted: true },
    ],
  },
};

/* ── Visuals ───────────────────────────────────────────── */

const VISUAL_STYLES = [
  { id: 'orb', name: 'Voice<br>orb', mark: 'spot-mark', preview: 'orb' },
  { id: 'spectrum', name: 'Spectrum', mark: 'area-mark', preview: 'spectrum' },
  { id: 'ribbon', name: 'Fluid<br>ribbon', mark: 'target-mark', preview: 'ribbon' },
  { id: 'slate', name: 'Writing<br>slate', mark: 'sun-mark', preview: 'slate' },
  { id: 'companion', name: 'Companion<br>orb', mark: 'spot-mark', preview: 'companion' },
  { id: 'off', name: 'Off', mark: 'target-mark', preview: 'off' },
];

const VISUAL_STATES = {
  orb: {
    style: 'orb',
    subtitle: 'Voice orb · shown while recording',
    caption: 'Follows the level of your voice',
    sideNote: 'Drawn by the application itself, over whatever you are typing into.',
    status: 'SYSTEM READY',
  },
  spectrum: {
    style: 'spectrum',
    subtitle: 'Spectrum · shown while recording',
    caption: 'Thirty-two bands, thirty-one times a second',
    sideNote: 'Drawn by the application itself, over whatever you are typing into.',
    status: 'SYSTEM READY',
  },
  slate: {
    style: 'slate',
    subtitle: 'Writing slate · shown while recording',
    caption: 'Lines fill as the phrase grows',
    sideNote: 'Drawn by the application itself, over whatever you are typing into.',
    status: 'SYSTEM READY',
  },
  companion: {
    style: 'companion',
    subtitle: 'Companion orb · a window of its own',
    caption: 'Stays on the desktop between dictations',
    sideNote: 'Rendered by a separate process, so a fault in it cannot take the application down.',
    note: {
      text: 'The companion keeps its own window and its own position. It reads the same audio the recorder already has — no second microphone is opened.',
      action: 'Place it',
    },
    status: 'COMPANION RUNNING',
  },
  off: {
    style: 'off',
    subtitle: 'No overlay',
    caption: 'Nothing is drawn while recording',
    sideNote: 'The hotkey still works. Only the visual confirmation is gone.',
    note: {
      tone: 'signal',
      text: 'Without an overlay there is nothing on screen to confirm that the hotkey reached the application. The tray icon is the only signal left.',
    },
    status: 'OVERLAY OFF',
  },
};

/* ── History ───────────────────────────────────────────── */

const HISTORY_STATES = {
  recent: {
    count: '128 entries', scope: 'stored on this device',
    lede: 'Newest first. Cleared entries are removed from disk, not hidden.',
    status: 'SYSTEM READY',
    rows: [
      { text: 'Нужно проверить, как ведёт себя вставка в терминале после отпускания клавиш', detail: 'ru · 6.2 s · 212 ms release', state: 'selected', mark: 'Local' },
      { text: 'The verification receipt avoids re-hashing multi-gigabyte models on every start', detail: 'en · 4.8 s · 1 940 ms release', state: 'remote', mark: 'Groq' },
      { text: 'Переключение на Vulkan должно происходить только после успешного прогрева', detail: 'ru · 5.1 s · 236 ms release', state: 'installed', mark: 'Local' },
      { text: 'Check whether the clipboard restore still wins over a newer user copy', detail: 'en · 3.4 s · 198 ms release', state: 'installed', mark: 'Local' },
      { text: 'Провайдер должен уходить в паузу до сброса лимита, а не до перезапуска', detail: 'ru · 7.0 s · 224 ms release', state: 'installed', mark: 'Local' },
    ],
  },
  'mixed-sources': {
    count: '128 entries', scope: '31 sent to a service',
    scopeTone: 'signal',
    lede: 'Every line says where it was transcribed. What left this device is not something to have to remember.',
    status: 'SYSTEM READY',
    statusMode: 'AUDIO LEAVES DEVICE',
    rows: [
      { text: 'The verification receipt avoids re-hashing multi-gigabyte models on every start', detail: 'en · 4.8 s · 1 940 ms release', state: 'remote', mark: 'Groq' },
      { text: 'Нужно проверить, как ведёт себя вставка в терминале после отпускания клавиш', detail: 'ru · 6.2 s · 2 310 ms release', state: 'remote', mark: 'Cloudflare' },
      { text: 'Переключение на Vulkan должно происходить только после успешного прогрева', detail: 'ru · 5.1 s · 236 ms release', state: 'installed', mark: 'Local' },
      { text: 'Check whether the clipboard restore still wins over a newer user copy', detail: 'en · 3.4 s · 1 780 ms release', state: 'remote', mark: 'Groq' },
      { text: 'Провайдер должен уходить в паузу до сброса лимита, а не до перезапуска', detail: 'ru · 7.0 s · 224 ms release', state: 'installed', mark: 'Local' },
    ],
  },
  empty: {
    count: 'No entries', scope: 'stored on this device',
    lede: 'Newest first. Cleared entries are removed from disk, not hidden.',
    status: 'SYSTEM READY',
    rows: [],
    empty: {
      title: 'Nothing here yet',
      note: 'Dictations appear here after they are transcribed. History is kept on this machine and can be switched off entirely.',
      action: 'Start dictating',
    },
  },
  disabled: {
    count: 'History off', scope: 'nothing is written to disk',
    scopeTone: 'signal',
    lede: 'While this is off, transcripts exist only long enough to be inserted.',
    status: 'HISTORY OFF',
    rows: [],
    empty: {
      title: 'History is switched off',
      note: 'Nothing is written to disk. Insert-last still works for the most recent dictation, because that one is held in memory until the next.',
      action: 'Switch on',
    },
  },
  recovered: {
    count: '126 entries', scope: 'recovered after an interrupted write',
    scopeTone: 'signal',
    lede: 'Two entries could not be read and were moved aside rather than deleted.',
    status: 'HISTORY RECOVERED',
    rows: [
      { text: 'Нужно проверить, как ведёт себя вставка в терминале после отпускания клавиш', detail: 'ru · 6.2 s · 212 ms release', state: 'selected', mark: 'Local' },
      { text: 'The verification receipt avoids re-hashing multi-gigabyte models on every start', detail: 'en · 4.8 s · 1 940 ms release', state: 'remote', mark: 'Groq' },
      { text: 'Переключение на Vulkan должно происходить только после успешного прогрева', detail: 'ru · 5.1 s · 236 ms release', state: 'installed', mark: 'Local' },
    ],
  },
};

/* ── Hotkeys ───────────────────────────────────────────── */

const HOTKEY_STATES = {
  active: {
    summary: { text: 'All four are held by SayFlex', tone: 'ink' },
    status: 'SYSTEM READY',
    rows: [
      { keys: 'Ctrl + Alt + Space', name: 'Dictate', detail: 'Records while held · transcribes and inserts on release', state: 'selected', text: 'Held' },
      { keys: 'Ctrl + Alt + V', name: 'Insert last', detail: 'Places the newest transcript where the caret is now', state: 'installed', text: 'Held' },
      { keys: 'Ctrl + Alt + K', name: 'Correct layout', detail: 'Offers RU/EN candidates for the selection · never automatic', state: 'installed', text: 'Held' },
      { keys: 'Ctrl + Alt + F', name: 'Translate selection', detail: 'Captures the selection once and opens the preview', state: 'installed', text: 'Held' },
      { keys: 'Ctrl + Alt + P', name: 'Pause input features', detail: 'Releases every hotkey until pressed again', state: 'installed', text: 'Held' },
    ],
  },
  paused: {
    summary: { text: 'Paused — no hotkey is held', tone: 'signal' },
    status: 'INPUT PAUSED',
    statusMode: 'INPUT PAUSED',
    rows: [
      { keys: 'Ctrl + Alt + Space', name: 'Dictate', detail: 'Released while paused', state: 'disabled', text: 'Paused' },
      { keys: 'Ctrl + Alt + V', name: 'Insert last', detail: 'Released while paused', state: 'disabled', text: 'Paused' },
      { keys: 'Ctrl + Alt + K', name: 'Correct layout', detail: 'Released while paused', state: 'disabled', text: 'Paused' },
      { keys: 'Ctrl + Alt + F', name: 'Translate selection', detail: 'Released while paused', state: 'disabled', text: 'Paused' },
      { keys: 'Ctrl + Alt + P', name: 'Resume', detail: 'The only combination still held', state: 'active', text: 'Held' },
    ],
  },
  conflict: {
    summary: { text: 'One combination is taken', tone: 'signal' },
    status: 'HOTKEY TAKEN',
    rows: [
      { keys: 'Ctrl + Alt + Space', name: 'Dictate', detail: 'Records while held · transcribes and inserts on release', state: 'selected', text: 'Held' },
      { keys: 'Ctrl + Alt + V', name: 'Insert last', detail: 'Another application registered it first', state: 'refused', text: 'Taken', action: 'Change' },
      { keys: 'Ctrl + Alt + K', name: 'Correct layout', detail: 'Offers RU/EN candidates for the selection · never automatic', state: 'installed', text: 'Held' },
      { keys: 'Ctrl + Alt + F', name: 'Translate selection', detail: 'Captures the selection once and opens the preview', state: 'installed', text: 'Held' },
      { keys: 'Ctrl + Alt + P', name: 'Pause input features', detail: 'Releases every hotkey until pressed again', state: 'installed', text: 'Held' },
    ],
  },
  'protected-field': {
    summary: { text: 'Standing down in a password field', tone: 'signal' },
    status: 'PROTECTED FIELD',
    note: true,
    rows: [
      { keys: 'Ctrl + Alt + Space', name: 'Dictate', detail: 'Refused: the focused field is a password box', state: 'refused', text: 'Refused' },
      { keys: 'Ctrl + Alt + V', name: 'Insert last', detail: 'Refused: the focused field is a password box', state: 'refused', text: 'Refused' },
      { keys: 'Ctrl + Alt + K', name: 'Correct layout', detail: 'Refused: selected text cannot be read here', state: 'refused', text: 'Refused' },
      { keys: 'Ctrl + Alt + F', name: 'Translate selection', detail: 'Refused: selected text cannot be read here', state: 'refused', text: 'Refused' },
      { keys: 'Ctrl + Alt + P', name: 'Pause input features', detail: 'Always available', state: 'installed', text: 'Held' },
    ],
  },
};

/* Which archetype each page belongs to is not decoration: it decides the
   grid. A list gets the width the third column was using. */
const PAGES = {
  overview: { states: Object.keys(OVERVIEW_STATES), number: '011', layout: 'wide' },
  dictation: { states: Object.keys(DICTATION_STATES), number: '013', layout: 'instrument' },
  translation: { states: Object.keys(TRANSLATION_STATES), number: '017', layout: 'wide' },
  models: { states: Object.keys(MODEL_STATES), number: '021', layout: 'wide' },
  providers: { states: Object.keys(PROVIDER_STATES), number: '024', layout: 'wide' },
  visuals: { states: Object.keys(VISUAL_STATES), number: '025', layout: 'instrument' },
  history: { states: Object.keys(HISTORY_STATES), number: '029', layout: 'wide' },
  hotkeys: { states: Object.keys(HOTKEY_STATES), number: '031', layout: 'wide' },
  controls: { states: [], number: '037', layout: 'wide' },
};

/* ── rendering ─────────────────────────────────────────── */

const element = (id) => document.getElementById(id);
let currentPage = 'dictation';
let currentState = 'ready';

function buildBars() {
  [['signal-bars', BAR_COUNT], ['preview-bars', 26]].forEach(([id, count]) => {
    const host = element(id);
    host.innerHTML = '';
    for (let index = 0; index < count; index += 1) {
      host.appendChild(document.createElement('i'));
    }
  });
}

function renderRow(row, index, draggable) {
  const item = document.createElement('li');
  const wrap = document.createElement('div');
  wrap.className = 'row';
  wrap.dataset.state = row.state;

  const lead = document.createElement('div');
  if (draggable) {
    lead.className = 'grip';
    lead.textContent = '⋮⋮';
    lead.title = 'Drag to reorder';
  } else {
    lead.className = 'order';
    lead.textContent = String(index + 1).padStart(2, '0');
  }

  const main = document.createElement('div');
  main.className = 'row-main';
  const title = document.createElement('div');
  title.className = 'row-title';
  const name = document.createElement('b');
  name.textContent = row.name;
  title.appendChild(name);
  if (row.tag) {
    const tag = document.createElement('em');
    tag.textContent = row.tag;
    title.appendChild(tag);
  }
  const detail = document.createElement('p');
  detail.className = 'row-detail';
  detail.textContent = row.detail;
  main.append(title, detail);

  const state = document.createElement('div');
  state.className = 'row-state';
  if (row.size) {
    const size = document.createElement('span');
    size.className = 'row-size';
    size.textContent = row.size;
    state.appendChild(size);
  }
  if (typeof row.progress === 'number') {
    const progress = document.createElement('div');
    progress.className = 'progress';
    const fill = document.createElement('i');
    fill.style.width = `${row.progress}%`;
    progress.appendChild(fill);
    state.appendChild(progress);
  }
  const mark = document.createElement('span');
  mark.className = 'row-mark';
  const text = document.createElement('span');
  text.className = 'state-text';
  text.textContent = row.text;
  state.append(mark, text);
  if (row.action) {
    const action = document.createElement('button');
    action.className = 'row-action';
    action.textContent = row.action;
    state.appendChild(action);
  }

  wrap.append(lead, main, state);
  item.appendChild(wrap);
  return item;
}

function renderRows(host, rows, draggable) {
  host.innerHTML = '';
  rows.forEach((row, index) => host.appendChild(renderRow(row, index, draggable)));
  if (draggable) makeReorderable(host, rows);
}

/* Reordering is the whole point of the provider list, so it is done by
   dragging the row and not by a pair of arrows hidden at the end of it. The
   arrows stay as well: a list that can only be reordered by dragging cannot be
   reordered from the keyboard. */
function makeReorderable(host, rows) {
  host.querySelectorAll('.grip').forEach((grip, index) => {
    grip.addEventListener('mousedown', (event) => {
      event.preventDefault();
      const item = grip.closest('li');
      const items = [...host.children];
      item.classList.add('dragging');
      let target = index;

      const move = (moveEvent) => {
        items.forEach((candidate, candidateIndex) => {
          const box = candidate.getBoundingClientRect();
          if (moveEvent.clientY > box.top && moveEvent.clientY < box.bottom) {
            target = candidateIndex;
          }
          candidate.classList.toggle('drop-target', candidateIndex === target && candidateIndex !== index);
        });
      };
      const drop = () => {
        document.removeEventListener('mousemove', move);
        document.removeEventListener('mouseup', drop);
        item.classList.remove('dragging');
        items.forEach((candidate) => candidate.classList.remove('drop-target'));
        if (target !== index) {
          const [moved] = rows.splice(index, 1);
          rows.splice(target, 0, moved);
          renderRows(host, rows, true);
        }
      };
      document.addEventListener('mousemove', move);
      document.addEventListener('mouseup', drop);
    });
  });
}

function applyDictation(state) {
  const data = DICTATION_STATES[state];
  element('signal').dataset.state = data.signal;
  element('dictation-subtitle').textContent = data.subtitle;
  const value = element('readout-value');
  value.hidden = data.value === null;
  value.textContent = data.value ?? '';
  const label = element('readout-label');
  label.textContent = data.caption;
  // With no number to carry it, the caption becomes the line the eye lands on.
  label.classList.toggle('readout-alone', data.value === null);
  element('engine-note').textContent = data.engineNote || 'CUDA verified by warm-up · 5070 Ti';

  const note = element('dictation-note');
  if (data.note) {
    note.hidden = false;
    note.dataset.tone = data.note.tone || 'ink';
    element('dictation-note-text').textContent = data.note.text;
    const action = element('dictation-note-action');
    action.hidden = !data.note.action;
    action.textContent = data.note.action || '';
  } else {
    note.hidden = true;
  }

  const metrics = element('dictation-metrics');
  if (data.metrics) {
    metrics.hidden = false;
    element('metric-latency').textContent = data.metrics.latency;
    element('metric-target').textContent = data.metrics.target;
    element('metric-source').textContent = data.metrics.source;
  } else {
    metrics.hidden = true;
  }

  setStatus(data.statusMode || 'LOCAL · PRIVATE', data.status);
}

function applyModels(state) {
  renderRows(element('model-rows'), MODEL_STATES[state], false);
  renderRows(element('pack-rows'), PACK_ROWS[state] || PACK_ROWS['at-rest'], false);
  setStatus('LOCAL · PRIVATE', state === 'installing' ? 'DOWNLOADING' : 'SYSTEM READY');
}

function applyProviders(state) {
  const data = PROVIDER_STATES[state];
  renderRows(element('provider-rows'), data.rows, true);
  const summary = element('privacy-summary');
  summary.dataset.tone = data.privacy.tone;
  element('privacy-text').textContent = data.privacy.text;
  setStatus(data.statusMode, state === 'all-out' ? 'NO PROVIDER' : 'SYSTEM READY');
}

/* ── rendering the five remaining pages ────────────────── */

function renderNote(prefix, note) {
  const host = element(`${prefix}-note`);
  if (!host) return;
  if (!note) {
    host.hidden = true;
    return;
  }
  host.hidden = false;
  host.dataset.tone = note.tone || 'ink';
  element(`${prefix}-note-text`).textContent = note.text;
  const action = element(`${prefix}-note-action`);
  action.hidden = !note.action;
  action.textContent = note.action || '';
}

function applyOverview(state) {
  const data = OVERVIEW_STATES[state];
  element('overview-statement').textContent = data.statement;
  element('overview-sub').textContent = data.sub;
  renderNote('overview', data.note);

  const summary = element('overview-privacy');
  summary.dataset.tone = data.privacy.tone;
  element('overview-privacy-text').textContent = data.privacy.text;

  const facts = element('overview-facts');
  facts.innerHTML = '';
  data.facts.forEach((fact) => {
    const wrap = document.createElement('div');
    if (fact.tone) wrap.dataset.tone = fact.tone;
    const term = document.createElement('dt');
    term.textContent = fact.label;
    const value = document.createElement('dd');
    value.textContent = fact.value;
    if (fact.note) {
      const note = document.createElement('small');
      note.textContent = fact.note;
      value.appendChild(note);
    }
    wrap.append(term, value);
    facts.appendChild(wrap);
  });

  setStatus(data.statusMode || 'LOCAL · PRIVATE', data.status);
}

/* Which form rows are a choice rather than a fact. */
const TRANSLATION_MENUS = {
  Direction: 'direction',
  Model: 'translation-model',
  Engine: 'translation-engine',
  Result: 'translation-result',
};

function applyTranslation(state) {
  const data = TRANSLATION_STATES[state];
  renderNote('translation', data.note);

  const summary = element('translation-summary');
  summary.dataset.tone = data.summary.tone;
  element('translation-summary-text').textContent = data.summary.text;

  const host = element('translation-rows');
  host.innerHTML = '';
  data.rows.forEach((row) => {
    const wrap = document.createElement('div');
    if (row.tone) wrap.dataset.tone = row.tone;
    if (row.muted) wrap.style.opacity = '.5';

    const term = document.createElement('dt');
    term.textContent = row.label;
    const hint = document.createElement('small');
    hint.textContent = row.hint;
    term.appendChild(hint);

    const value = document.createElement('dd');
    const menuId = TRANSLATION_MENUS[row.label];
    if (menuId && !row.muted) {
      value.appendChild(menuValueButton(menuId));
    } else if (row.value === 'stepper') {
      const stepper = document.createElement('div');
      stepper.className = 'stepper';
      const less = document.createElement('button');
      less.textContent = '−';
      const amount = document.createElement('b');
      amount.textContent = String(row.step);
      const more = document.createElement('button');
      more.textContent = '+';
      stepper.append(less, amount, more);
      value.appendChild(stepper);
    } else {
      value.textContent = row.value;
    }
    wrap.append(term, value);
    host.appendChild(wrap);
  });

  setStatus('LOCAL · PRIVATE', data.status);
}

function buildSettingRows(hostId, menuIds) {
  const host = element(hostId);
  host.innerHTML = '';
  menuIds.forEach((menuId) => host.appendChild(menuButton(menuId, () => afterSettingChange(menuId))));
}

/* A setting that changes something visible changes it immediately: a control
   that only looks pressed teaches the user not to trust it. */
function afterSettingChange(menuId) {
  if (menuId === 'history' && CHOSEN.history === 'off') {
    setStatus('LOCAL · PRIVATE', 'HISTORY OFF');
  }
  if (menuId === 'palette' || menuId === 'response') {
    element('visuals-side-note').textContent = `${labelOf('palette')} · ${labelOf('response')} response`;
  }
}

function applyVisuals(state) {
  const data = VISUAL_STATES[state];
  element('visual-preview').dataset.style = data.style;
  element('visuals-subtitle').textContent = data.subtitle;
  element('visuals-caption').textContent = data.caption;
  element('visuals-side-note').textContent = data.sideNote;
  renderNote('visuals', data.note);

  document.querySelectorAll('#visual-choices .choice').forEach((choice) => {
    choice.classList.toggle('active', choice.dataset.style === data.style);
  });

  setStatus('LOCAL · PRIVATE', data.status);
}

const ENGINES = {
  cuda: { label: 'CUDA verified by warm-up · 5070 Ti', tile: 0 },
  cpu: { label: 'CPU · always available, slower', tile: 1 },
  auto: { label: 'Auto · tries CUDA, then Vulkan, then CPU', tile: 2 },
  api: { label: 'A service does the work · see Providers', tile: 3 },
};

function buildEngineChoices() {
  const tiles = [...document.querySelectorAll('[data-page="dictation"] .choice')];
  const keys = Object.keys(ENGINES);
  tiles.forEach((tile, index) => {
    tile.dataset.engine = keys[index];
    tile.addEventListener('click', () => {
      if (tile.disabled) return;
      tiles.forEach((other) => other.classList.toggle('active', other === tile));
      // Choosing an engine is not the same as having it: a backend is only
      // trusted after a real warm-up pass, and the note says which is which.
      const key = tile.dataset.engine;
      if (key === 'auto' || key === 'cuda') {
        tile.dataset.busy = 'true';
        element('engine-note').textContent = 'Verifying with a warm-up pass…';
        setTimeout(() => {
          delete tile.dataset.busy;
          element('engine-note').textContent = ENGINES[key].label;
        }, 1400);
      } else {
        element('engine-note').textContent = ENGINES[key].label;
      }
    });
  });
}

function buildVisualChoices() {
  const host = element('visual-choices');
  host.innerHTML = '';
  VISUAL_STYLES.forEach((style) => {
    const button = document.createElement('button');
    button.className = 'choice';
    button.dataset.style = style.preview;
    const mark = document.createElement('i');
    mark.className = style.mark;
    const label = document.createElement('span');
    label.innerHTML = style.name;
    button.append(mark, label);
    button.addEventListener('click', () => {
      const named = Object.keys(VISUAL_STATES).find(
        (key) => VISUAL_STATES[key].style === style.preview,
      );
      if (named) showState(named);
    });
    host.appendChild(button);
  });
}

function applyHistory(state) {
  const data = HISTORY_STATES[state];
  element('history-count').textContent = data.count;
  element('history-scope').textContent = data.scope;
  element('history-summary').dataset.tone = data.scopeTone || 'ink';
  element('history-lede').textContent = data.lede;

  const host = element('history-rows');
  host.innerHTML = '';
  data.rows.forEach((row, index) => {
    const item = document.createElement('li');
    const wrap = document.createElement('div');
    wrap.className = 'row';
    wrap.dataset.state = row.state;

    const order = document.createElement('div');
    order.className = 'order';
    order.textContent = String(index + 1).padStart(2, '0');

    const main = document.createElement('div');
    main.className = 'row-main';
    const quote = document.createElement('div');
    quote.className = 'row-quote';
    quote.textContent = row.text;
    const detail = document.createElement('p');
    detail.className = 'row-detail';
    detail.textContent = row.detail;
    main.append(quote, detail);

    const stateCell = document.createElement('div');
    stateCell.className = 'row-state';
    const mark = document.createElement('span');
    mark.className = 'row-mark';
    const text = document.createElement('span');
    text.className = 'state-text';
    text.textContent = row.mark;
    const action = document.createElement('button');
    action.className = 'row-action';
    action.textContent = 'Copy';
    stateCell.append(mark, text, action);

    wrap.addEventListener('contextmenu', (event) => {
      event.preventDefault();
      openContextMenu([
        { heading: 'Entry' },
        { label: 'Copy text', action: () => setStatus('LOCAL · PRIVATE', 'COPIED') },
        { label: 'Insert where the caret is', action: () => setStatus('LOCAL · PRIVATE', 'INSERTED') },
        { label: 'Copy as plain text' },
        { separator: true },
        { label: 'Delete this entry', danger: true, action: () => { wrap.parentElement.remove(); } },
      ], event.clientX, event.clientY);
    });

    wrap.append(order, main, stateCell);
    item.appendChild(wrap);
    host.appendChild(item);
  });

  const clear = element('history-clear');
  clear.innerHTML = '';
  if (data.rows.length) {
    clear.appendChild(confirmAction('Clear history', 'Delete every entry from disk?', () => {
      showState('empty');
    }));
  }

  const empty = element('history-empty');
  if (data.empty) {
    empty.hidden = false;
    element('history-empty-title').textContent = data.empty.title;
    element('history-empty-note').textContent = data.empty.note;
    const action = element('history-empty-action');
    action.hidden = !data.empty.action;
    action.textContent = data.empty.action || '';
  } else {
    empty.hidden = true;
  }

  setStatus(data.statusMode || 'LOCAL · PRIVATE', data.status);
}

function applyHotkeys(state) {
  const data = HOTKEY_STATES[state];
  const summary = element('hotkeys-summary');
  summary.dataset.tone = data.summary.tone;
  element('hotkeys-summary-text').textContent = data.summary.text;

  const host = element('hotkey-rows');
  host.innerHTML = '';
  data.rows.forEach((row) => {
    const item = document.createElement('li');
    const wrap = document.createElement('div');
    wrap.className = 'row';
    wrap.dataset.state = row.state;

    const keys = document.createElement('div');
    keys.className = 'row-keys';
    keys.textContent = row.keys;

    const main = document.createElement('div');
    main.className = 'row-main';
    const title = document.createElement('div');
    title.className = 'row-title';
    const name = document.createElement('b');
    name.textContent = row.name;
    title.appendChild(name);
    const detail = document.createElement('p');
    detail.className = 'row-detail';
    detail.textContent = row.detail;
    main.append(title, detail);

    const stateCell = document.createElement('div');
    stateCell.className = 'row-state';
    const mark = document.createElement('span');
    mark.className = 'row-mark';
    const text = document.createElement('span');
    text.className = 'state-text';
    text.textContent = row.text;
    stateCell.append(mark, text);
    if (row.action) {
      const action = document.createElement('button');
      action.className = 'row-action';
      action.textContent = row.action;
      stateCell.appendChild(action);
    }

    wrap.append(keys, main, stateCell);
    item.appendChild(wrap);
    host.appendChild(item);
  });

  setStatus(data.statusMode || 'LOCAL · PRIVATE', data.status);
}

function setStatus(mode, line) {
  const copy = document.querySelector('.status-copy');
  copy.dataset.tone = mode === 'LOCAL · PRIVATE' ? 'ink' : 'signal';
  element('status-mode').textContent = mode;
  element('status-line').textContent = line;
}

function showPage(page) {
  currentPage = page;
  element('workspace').classList.toggle('is-list', PAGES[page].layout === 'wide');

  document.querySelectorAll('.page, .page-side').forEach((node) => {
    node.hidden = node.dataset.page !== page;
  });
  document.querySelectorAll('#tool-nav button').forEach((button) => {
    button.classList.toggle('active', button.dataset.page === page);
  });

  element('page-number').textContent = PAGES[page].number;
  buildStateButtons(PAGES[page].states);
  // The sheet shows live controls, so it is rebuilt from whatever the settings
  // are now rather than from what they were at start-up.
  if (page === 'controls') buildControlsSheet();
  if (PAGES[page].states.length) {
    showState(PAGES[page].states[0]);
  } else {
    // The control sheet has no states of its own: every control on it is
    // already showing all of them at once.
    setStatus('LOCAL · PRIVATE', 'CONTROL SHEET');
  }
}

function showState(state) {
  currentState = state;
  document.querySelectorAll('#state-buttons button').forEach((button) => {
    button.classList.toggle('active', button.dataset.state === state);
  });
  const apply = {
    overview: applyOverview,
    dictation: applyDictation,
    translation: applyTranslation,
    models: applyModels,
    providers: applyProviders,
    visuals: applyVisuals,
    history: applyHistory,
    hotkeys: applyHotkeys,
  }[currentPage];
  if (apply) apply(state);
}

function buildStateButtons(states) {
  const host = element('state-buttons');
  host.innerHTML = '';
  states.forEach((state) => {
    const button = document.createElement('button');
    button.dataset.state = state;
    button.textContent = state;
    button.addEventListener('click', () => showState(state));
    host.appendChild(button);
  });
}

/* The trace only moves while something is being heard. */
function animate() {
  const signal = element('signal');
  const bars = signal.querySelectorAll('i');
  const listening = signal.dataset.state === 'listening';
  const processing = signal.dataset.state === 'processing';
  const now = performance.now() / 1000;

  bars.forEach((bar, index) => {
    if (listening) {
      const wave = Math.sin(now * 5.5 + index * 0.45) * Math.sin(now * 1.7 + index * 0.11);
      const envelope = Math.sin((index / BAR_COUNT) * Math.PI);
      bar.style.height = `${6 + Math.abs(wave) * envelope * 74}%`;
    } else if (processing) {
      const sweep = (now * 1.4 + index / BAR_COUNT) % 1;
      bar.style.height = `${4 + Math.max(0, 1 - Math.abs(sweep - 0.5) * 6) * 22}%`;
    } else if (bar.style.height) {
      // Nothing is being heard: hand the height back to the stylesheet rather
      // than leaving the last frame frozen on screen.
      bar.style.height = '';
    }
  });
  animatePreview(now);
  requestAnimationFrame(animate);
}

/* The visualizer preview is always moving: it is showing what a style looks
   like, not what the microphone is doing right now. */
function animatePreview(now) {
  const preview = element('visual-preview');
  if (preview.closest('.page').hidden) return;

  if (preview.dataset.style === 'spectrum') {
    preview.querySelectorAll('.preview-bars i').forEach((bar, index) => {
      const wave = Math.sin(now * 4.2 + index * 0.5) * Math.sin(now * 1.3 + index * 0.16);
      const envelope = Math.sin((index / 26) * Math.PI);
      bar.style.height = `${8 + Math.abs(wave) * envelope * 78}%`;
    });
  }
  if (preview.dataset.style === 'ribbon') {
    const path = element('ribbon-path');
    const offset = Math.sin(now * 1.6) * 14;
    const second = Math.sin(now * 2.1 + 1) * 12;
    path.setAttribute(
      'd',
      `M0 45 Q 25 ${45 - offset} 50 45 T 100 ${45 + second} T 150 45 T 200 45`,
    );
  }
}

function start() {
  const now = new Date();
  element('weekday').textContent = now.toLocaleDateString('en-GB', { weekday: 'short' });
  element('date').textContent = now.toLocaleDateString('en-GB', { day: 'numeric', month: 'long' });
  element('year').textContent = String(now.getFullYear());

  buildBars();
  buildVisualChoices();
  buildEngineChoices();
  buildSettingRows('dictation-settings', ['output', 'release', 'history']);
  buildSettingRows('visuals-settings', ['palette', 'response']);
  buildControlsSheet();

  /* The application lives in the tray, so the tray menu is a real part of the
     design rather than something Windows draws by accident. */
  element('tray-button').addEventListener('click', (event) => {
    event.stopPropagation();
    const box = event.currentTarget.getBoundingClientRect();
    openContextMenu([
      { heading: 'SayFlex' },
      { label: 'Dictate now', note: 'Same as holding Ctrl + Alt + Space' },
      { label: 'Insert last transcript' },
      { label: 'Pause input features', note: 'Releases every hotkey until resumed' },
      { separator: true },
      { label: 'Open settings' },
      { label: 'Open history' },
      { separator: true },
      { label: 'Quit SayFlex', danger: true },
    ], box.right + 8, box.top - 190);
  });
  document.querySelectorAll('#tool-nav button').forEach((button) => {
    button.addEventListener('click', () => showPage(button.dataset.page));
  });
  showPage('dictation');
  requestAnimationFrame(animate);
}

start();
