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

/* One row here is one TranscriptionProviderStatus off the wire: the switch,
   whether it can run now, a sentence saying why not when it cannot, and a
   masked tail. Nothing else exists on that message, so nothing else is drawn
   — a remaining-quota number would be a figure this application invents and
   then has to defend. The wording of `detail` is the worker's own. */
const PROVIDER_STATES = {
  'local-only': {
    rows: [
      { id: 'local', name: 'This computer', enabled: true, about: 'Whisper Large v3 Turbo · CUDA' },
      { id: 'groq', name: 'Groq', enabled: false, about: 'Free tier: 2 000 requests a day' },
      { id: 'cloudflare', name: 'Cloudflare Workers AI', enabled: false, about: 'Free tier: 10 000 neurons a day, shared across models' },
      { id: 'azure', name: 'Microsoft Azure Speech', enabled: false, about: 'Free tier F0: 5 hours a month · needs a region' },
      { id: 'deepgram', name: 'Deepgram', enabled: false, about: '$200 of credit, no expiry' },
      { id: 'custom', name: 'Custom endpoint', enabled: false, about: 'Any OpenAI-compatible address · HTTPS only' },
    ],
  },
  'service-first': {
    rows: [
      { id: 'cloudflare', name: 'Cloudflare Workers AI', enabled: true, key: 'cf_live_4f21', extra: 'a1b2c3d4', about: 'Free tier: 10 000 neurons a day, shared across models' },
      { id: 'groq', name: 'Groq', enabled: true, key: 'gsk_live_9c07', about: 'Free tier: 2 000 requests a day' },
      { id: 'local', name: 'This computer', enabled: true, about: 'Whisper Large v3 Turbo · CUDA' },
      { id: 'azure', name: 'Microsoft Azure Speech', enabled: false, about: 'Free tier F0: 5 hours a month · needs a region' },
      { id: 'deepgram', name: 'Deepgram', enabled: false, about: '$200 of credit, no expiry' },
      { id: 'custom', name: 'Custom endpoint', enabled: false, about: 'Any OpenAI-compatible address · HTTPS only' },
    ],
  },
  'quota-spent': {
    rows: [
      { id: 'cloudflare', name: 'Cloudflare Workers AI', enabled: true, key: 'cf_live_4f21', extra: 'a1b2c3d4', detail: 'Daily neurons spent; available again in about 54 min', about: 'Free tier: 10 000 neurons a day, shared across models' },
      { id: 'groq', name: 'Groq', enabled: true, key: 'gsk_live_9c07', about: 'Free tier: 2 000 requests a day' },
      { id: 'local', name: 'This computer', enabled: true, about: 'Whisper Large v3 Turbo · CUDA' },
      { id: 'azure', name: 'Microsoft Azure Speech', enabled: false, about: 'Free tier F0: 5 hours a month · needs a region' },
      { id: 'deepgram', name: 'Deepgram', enabled: false, about: '$200 of credit, no expiry' },
      { id: 'custom', name: 'Custom endpoint', enabled: false, about: 'Any OpenAI-compatible address · HTTPS only' },
    ],
  },
  'key-refused': {
    rows: [
      { id: 'cloudflare', name: 'Cloudflare Workers AI', enabled: true, key: 'cf_dead_0000', extra: 'a1b2c3d4', detail: 'Key refused: the service rejected this token', refused: true, about: 'Free tier: 10 000 neurons a day, shared across models' },
      { id: 'groq', name: 'Groq', enabled: true, key: 'gsk_live_9c07', about: 'Free tier: 2 000 requests a day' },
      { id: 'local', name: 'This computer', enabled: true, about: 'Whisper Large v3 Turbo · CUDA' },
      { id: 'azure', name: 'Microsoft Azure Speech', enabled: false, about: 'Free tier F0: 5 hours a month · needs a region' },
      { id: 'deepgram', name: 'Deepgram', enabled: false, about: '$200 of credit, no expiry' },
      { id: 'custom', name: 'Custom endpoint', enabled: false, about: 'Any OpenAI-compatible address · HTTPS only' },
    ],
  },
  'all-out': {
    rows: [
      { id: 'cloudflare', name: 'Cloudflare Workers AI', enabled: true, key: 'cf_live_4f21', extra: 'a1b2c3d4', detail: 'Daily neurons spent; available again in about 54 min', about: 'Free tier: 10 000 neurons a day, shared across models' },
      { id: 'groq', name: 'Groq', enabled: true, key: 'gsk_live_9c07', detail: 'Daily requests spent; available again in about 3 h', about: 'Free tier: 2 000 requests a day' },
      { id: 'local', name: 'This computer', enabled: false, about: 'Whisper Large v3 Turbo · CUDA' },
      { id: 'azure', name: 'Microsoft Azure Speech', enabled: false, about: 'Free tier F0: 5 hours a month · needs a region' },
      { id: 'deepgram', name: 'Deepgram', enabled: false, about: '$200 of credit, no expiry' },
      { id: 'custom', name: 'Custom endpoint', enabled: false, about: 'Any OpenAI-compatible address · HTTPS only' },
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

/* All eight the product draws. A style that exists in the application and not
   in the design does not get quietly dropped at the port: it either appears
   here or it is removed on purpose, with the removal written down. */
const VISUAL_STYLES = [
  { id: 'orb', name: 'Voice<br>orb', mark: 'spot-mark', preview: 'orb' },
  { id: 'spectrum', name: 'Spectrum', mark: 'area-mark', preview: 'spectrum' },
  { id: 'ribbon', name: 'Fluid<br>ribbon', mark: 'target-mark', preview: 'ribbon' },
  { id: 'mic', name: 'Broadcast<br>mic', mark: 'area-mark', preview: 'mic' },
  { id: 'circular', name: 'Circular', mark: 'target-mark', preview: 'circular' },
  { id: 'sphere', name: 'Flow<br>sphere', mark: 'spot-mark', preview: 'sphere' },
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
  ribbon: {
    style: 'ribbon',
    subtitle: 'Fluid ribbon · shown while recording',
    caption: 'One line that thickens with the voice',
    sideNote: 'Drawn by the application itself, over whatever you are typing into.',
    status: 'SYSTEM READY',
  },
  mic: {
    style: 'mic',
    subtitle: 'Broadcast mic · shown while recording',
    caption: 'Level bars beside a studio microphone',
    sideNote: 'Drawn by the application itself, over whatever you are typing into.',
    status: 'SYSTEM READY',
  },
  circular: {
    style: 'circular',
    subtitle: 'Circular · shown while recording',
    caption: 'The spectrum wrapped into a ring',
    sideNote: 'Drawn on its own native surface, so it can be transparent per pixel.',
    status: 'SYSTEM READY',
  },
  sphere: {
    style: 'sphere',
    subtitle: 'Flow sphere · shown while recording',
    caption: 'A point cloud that accelerates on voice onset',
    sideNote: 'Drawn on its own native surface, so it can be transparent per pixel.',
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

/* Every command SayFlex registers, with the three things that belong to a
   command and were previously spread over two pages: whether it is on, what
   combination calls it, and — where there is a choice — what it does. The
   old Text tools page was those switches with the combinations taken away,
   which is why it is folded in here.
   `mode` names a menu in the control registry. */
const HOTKEY_STATES = {
  active: {
    summary: { tone: 'ink' },
    status: 'SYSTEM READY',
    rows: [
      { keys: 'Ctrl + Alt + Space', name: 'Dictate', on: true, detail: 'Records while held · transcribes and inserts on release', held: true },
      { keys: 'Ctrl + Alt + V', name: 'Insert last', on: true, detail: 'Places the newest transcript where the caret is now', held: true },
      { keys: 'Ctrl + Alt + K', name: 'Correct layout', on: true, detail: 'Offers RU/EN candidates for the selection · never automatic', held: true },
      { keys: 'Ctrl + Alt + F', name: 'Act on selection', on: true, detail: 'Captures the selection once and opens the preview', held: true, mode: 'text-action' },
      { keys: 'Ctrl + Alt + P', name: 'Pause input features', on: true, detail: 'Releases every hotkey until pressed again', held: true },
    ],
  },
  paused: {
    summary: { tone: 'signal', text: 'Paused — no hotkey is held' },
    status: 'INPUT PAUSED',
    statusMode: 'INPUT PAUSED',
    paused: true,
    rows: [
      { keys: 'Ctrl + Alt + Space', name: 'Dictate', on: true, detail: 'Released while paused' },
      { keys: 'Ctrl + Alt + V', name: 'Insert last', on: true, detail: 'Released while paused' },
      { keys: 'Ctrl + Alt + K', name: 'Correct layout', on: true, detail: 'Released while paused' },
      { keys: 'Ctrl + Alt + F', name: 'Act on selection', on: true, detail: 'Released while paused', mode: 'text-action' },
      { keys: 'Ctrl + Alt + P', name: 'Resume', on: true, detail: 'The only combination still held', held: true, resume: true },
    ],
  },
  conflict: {
    summary: { tone: 'signal' },
    status: 'HOTKEY TAKEN',
    rows: [
      { keys: 'Ctrl + Alt + Space', name: 'Dictate', on: true, detail: 'Records while held · transcribes and inserts on release', held: true },
      { keys: 'Ctrl + Alt + V', name: 'Insert last', on: true, detail: 'Another application registered it first', taken: true },
      { keys: 'Ctrl + Alt + K', name: 'Correct layout', on: true, detail: 'Offers RU/EN candidates for the selection · never automatic', held: true },
      { keys: 'Ctrl + Alt + F', name: 'Act on selection', on: true, detail: 'Captures the selection once and opens the preview', held: true, mode: 'text-action' },
      { keys: 'Ctrl + Alt + P', name: 'Pause input features', on: true, detail: 'Releases every hotkey until pressed again', held: true },
    ],
  },
  'switched-off': {
    summary: { tone: 'ink' },
    status: 'SYSTEM READY',
    rows: [
      { keys: 'Ctrl + Alt + Space', name: 'Dictate', on: true, detail: 'Records while held · transcribes and inserts on release', held: true },
      { keys: 'Ctrl + Alt + V', name: 'Insert last', on: true, detail: 'Places the newest transcript where the caret is now', held: true },
      { keys: 'Ctrl + Alt + K', name: 'Correct layout', on: false, detail: 'Switched off · the combination is left to other applications' },
      { keys: 'Ctrl + Alt + F', name: 'Act on selection', on: false, detail: 'Switched off · the combination is left to other applications', mode: 'text-action' },
      { keys: 'Ctrl + Alt + P', name: 'Pause input features', on: true, detail: 'Releases every hotkey until pressed again', held: true },
    ],
  },
  'protected-field': {
    summary: { tone: 'signal', text: 'Standing down in a password field' },
    status: 'PROTECTED FIELD',
    protectedField: true,
    rows: [
      { keys: 'Ctrl + Alt + Space', name: 'Dictate', on: true, detail: 'Refused: the focused field is a password box' },
      { keys: 'Ctrl + Alt + V', name: 'Insert last', on: true, detail: 'Refused: the focused field is a password box' },
      { keys: 'Ctrl + Alt + K', name: 'Correct layout', on: true, detail: 'Refused: selected text cannot be read here' },
      { keys: 'Ctrl + Alt + F', name: 'Act on selection', on: true, detail: 'Refused: selected text cannot be read here', mode: 'text-action' },
      { keys: 'Ctrl + Alt + P', name: 'Pause input features', on: true, detail: 'Always available', held: true },
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
  controls: { states: [], number: '—', layout: 'wide' },
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

function makeReorderable(host, rows, rerender) {
  host.querySelectorAll('.grip').forEach((grip, index) => {
    // A list that can only be reordered with a mouse cannot be reordered at
    // all by someone working from the keyboard, so the handle is a control in
    // its own right rather than a decoration that happens to accept a drag.
    grip.tabIndex = 0;
    grip.setAttribute('role', 'button');
    grip.setAttribute('aria-label', `Move ${rows[index].name}: arrow keys, or drag`);
    grip.addEventListener('keydown', (event) => {
      const step = event.key === 'ArrowUp' ? -1 : event.key === 'ArrowDown' ? 1 : 0;
      if (!step) return;
      event.preventDefault();
      const target = index + step;
      if (target < 0 || target >= rows.length) return;
      const [moved] = rows.splice(index, 1);
      rows.splice(target, 0, moved);
      rerender();
      // The list was rebuilt, so the handle the user was holding is a new
      // node: focus follows the row rather than staying at the position.
      const handles = host.querySelectorAll('.grip');
      if (handles[target]) handles[target].focus();
    });

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
          rerender();
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
    wireAction(action, data.note.action);
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

function applyProviders(state) {
  loadProviderModel(state);
  renderProviders();
}

function applyModels(state) {
  loadModelLists(state);
  renderModelLists();
}

/* What each note offers to do about the thing it is reporting. A note that
   states a problem and then offers a button that does nothing is worse than a
   note with no button at all. */
const NOTE_ACTIONS = {
  Reorder: () => showPage('providers'),
  Details: () => showPage('overview'),
  Install: () => {
    showPage('models');
    const pending = [...speechModel, ...packModel].find((row) => row.state === 'idle');
    if (pending) startDownload(pending);
  },
  'Insert last': () => {
    showPage('dictation');
    showState('inserted');
  },
  Retry: () => showState('warming'),
  'Place it': () => {
    element('visuals-note-text').textContent =
      'Drag the orb anywhere with Ctrl + Alt held down. It remembers where it was put, and checks the position is still on a visible screen at start-up.';
  },
  'Start dictating': () => showPage('dictation'),
  'Switch on': () => {
    CHOSEN.history = 'on';
    showState('recent');
  },
};

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
  wireAction(action, note.action);
}

/* Replacing the node drops the previous handler with it, which is simpler to
   reason about than remembering what was attached last time. */
function wireAction(button, label) {
  if (!label) return;
  const fresh = button.cloneNode(true);
  button.replaceWith(fresh);
  fresh.addEventListener('click', (event) => {
    event.stopPropagation();
    const act = NOTE_ACTIONS[label];
    if (act) act();
  });
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
      // The bound belongs to the control rather than to a complaint after the
      // fact: the button simply stops at the end of the range.
      const step = (delta) => {
        row.step = Math.min(8, Math.max(0, row.step + delta));
        amount.textContent = String(row.step);
        less.disabled = row.step === 0;
        more.disabled = row.step === 8;
      };
      less.addEventListener('click', () => step(-1));
      more.addEventListener('click', () => step(1));
      step(0);
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

/* The ring is thirty-two spokes, the same count the product's circular
   spectrum uses, so the picture is not a different instrument. */
function buildCircularPreview() {
  const host = element('circular-spokes');
  if (!host) return;
  const spokes = 32;
  for (let i = 0; i < spokes; i += 1) {
    const angle = (i / spokes) * Math.PI * 2 - Math.PI / 2;
    const inner = 34;
    const outer = inner + 8 + Math.abs(Math.sin(i * 0.7)) * 16;
    const line = document.createElementNS('http://www.w3.org/2000/svg', 'line');
    line.setAttribute('x1', (60 + Math.cos(angle) * inner).toFixed(2));
    line.setAttribute('y1', (60 + Math.sin(angle) * inner).toFixed(2));
    line.setAttribute('x2', (60 + Math.cos(angle) * outer).toFixed(2));
    line.setAttribute('y2', (60 + Math.sin(angle) * outer).toFixed(2));
    host.appendChild(line);
  }
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
    action.addEventListener('click', (event) => {
      event.stopPropagation();
      action.textContent = 'Copied';
      setStatus('LOCAL · PRIVATE', 'COPIED TO CLIPBOARD');
      setTimeout(() => {
        action.textContent = 'Copy';
      }, 1400);
    });
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
    wireAction(action, data.empty.action);
  } else {
    empty.hidden = true;
  }

  setStatus(data.statusMode || 'LOCAL · PRIVATE', data.status);
}

/* Combinations Windows keeps for itself. Registering one silently fails at
   run time, so it is worth refusing here where the reason can be given. */
const RESERVED_HOTKEYS = new Set([
  'Ctrl + Alt + Delete', 'Ctrl + Shift + Escape', 'Win + L', 'Alt + Tab', 'Alt + F4',
]);

/* Changing a combination listens for one instead of asking the user to type
   its name. Escape leaves it as it was. A clash is refused in the row while it
   keeps listening, because the answer to "that one is taken" is another key,
   not a dialog to dismiss first. */
function captureHotkey(row, wrap, keys, detail, action) {
  const stateText = wrap.querySelector('.state-text');
  const previous = { keys: row.keys, detail: row.detail, taken: row.taken };
  keys.textContent = 'Press a combination…';
  detail.textContent = 'Escape leaves it unchanged.';
  wrap.dataset.state = 'downloading';
  stateText.textContent = 'Listening';
  action.textContent = 'Listening';

  const stop = () => document.removeEventListener('keydown', onKey, true);

  const refuse = (why) => {
    keys.textContent = 'Press a combination…';
    detail.textContent = why;
    wrap.dataset.state = 'refused';
    stateText.textContent = 'Taken';
    setStatus('LOCAL · PRIVATE', 'HOTKEY TAKEN');
  };

  const onKey = (event) => {
    event.preventDefault();
    event.stopPropagation();
    if (event.key === 'Escape') {
      stop();
      Object.assign(row, previous);
      renderHotkeys();
      setStatus('LOCAL · PRIVATE', 'UNCHANGED');
      return;
    }
    if (['Control', 'Alt', 'Shift', 'Meta'].includes(event.key)) return;

    const parts = [];
    if (event.ctrlKey) parts.push('Ctrl');
    if (event.altKey) parts.push('Alt');
    if (event.shiftKey) parts.push('Shift');
    parts.push(event.key.length === 1 ? event.key.toUpperCase() : event.key);
    const combination = parts.join(' + ');

    if (parts.length < 2) {
      refuse('Needs a modifier, or it would fire while you type');
      return;
    }
    if (RESERVED_HOTKEYS.has(combination)) {
      refuse(`Windows keeps ${combination} for itself`);
      return;
    }
    const holder = hotkeyModel.find((other) => other !== row && other.on && other.keys === combination);
    if (holder) {
      refuse(`${combination} is held by ${holder.name} · pick another`);
      return;
    }

    stop();
    row.keys = combination;
    row.detail = 'Registered by SayFlex';
    row.taken = false;
    row.held = true;
    renderHotkeys();
    setStatus('LOCAL · PRIVATE', 'HOTKEY CHANGED');
  };
  document.addEventListener('keydown', onKey, true);
}

/* The line at the top counts what the rows say, so resolving a clash also
   clears the warning about it. */
function refreshHotkeySummary() {
  const summary = element('hotkeys-summary');
  const taken = hotkeyModel.filter((row) => row.on && row.taken).length;
  const held = hotkeyModel.filter((row) => hotkeyStatus(row).text === 'Held').length;
  const WORDS = ['none', 'one', 'two', 'three', 'four', 'five', 'six', 'seven'];
  const spell = (count) => WORDS[count] || String(count);

  let text;
  let tone = 'ink';
  if (taken) {
    tone = 'signal';
    text = `${taken === 1 ? 'One combination is' : `${spell(taken)} combinations are`} taken`;
  } else if (hotkeyContext.summary && hotkeyContext.summary.text) {
    tone = hotkeyContext.summary.tone || 'ink';
    text = hotkeyContext.summary.text;
  } else if (held === hotkeyModel.length) {
    text = `All ${spell(held)} are held by SayFlex`;
  } else {
    text = `${spell(held)} of ${spell(hotkeyModel.length)} held by SayFlex`;
  }
  summary.dataset.tone = tone;
  element('hotkeys-summary-text').textContent = text;
}

let hotkeyModel = [];
let hotkeyContext = {};

function applyHotkeys(state) {
  const data = HOTKEY_STATES[state];
  hotkeyContext = data;
  hotkeyModel = data.rows.map((row) => ({ ...row }));
  renderHotkeys();
  setStatus(data.statusMode || 'LOCAL · PRIVATE', data.status);
}

/** What one row reports about itself, in the order the reasons override. */
function hotkeyStatus(row) {
  if (!row.on) return { state: 'disabled', text: 'Off' };
  if (row.taken) return { state: 'refused', text: 'Taken' };
  if (hotkeyContext.protectedField && !row.held) return { state: 'refused', text: 'Refused' };
  if (hotkeyContext.paused && !row.resume) return { state: 'disabled', text: 'Paused' };
  return { state: row.resume ? 'active' : 'installed', text: 'Held' };
}

function renderHotkeys() {
  const host = element('hotkey-rows');
  host.innerHTML = '';

  hotkeyModel.forEach((row) => {
    const status = hotkeyStatus(row);
    const item = document.createElement('li');
    const wrap = document.createElement('div');
    wrap.className = 'row';
    wrap.dataset.state = status.state;

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

    // A command with a choice about what it does carries that choice, rather
    // than leaving it on a separate page with the combination taken away.
    if (row.mode && row.on) {
      const mode = document.createElement('div');
      mode.className = 'row-mode';
      mode.appendChild(menuValueButton(row.mode));
      main.appendChild(mode);
    }

    const stateCell = document.createElement('div');
    stateCell.className = 'row-state';
    const mark = document.createElement('span');
    mark.className = 'row-mark';
    const text = document.createElement('span');
    text.className = 'state-text';
    text.textContent = status.text;
    stateCell.append(mark, text);

    // Switching a command off leaves its combination to other applications;
    // it is not the same as changing the combination, so both are offered.
    const toggle = toggleControl({ on: 'On', off: 'Off' }, row.on, (next) => {
      row.on = next;
      row.detail = next
        ? 'Registered by SayFlex'
        : 'Switched off · the combination is left to other applications';
      if (!next) row.taken = false;
      renderHotkeys();
      setStatus('LOCAL · PRIVATE', next ? 'HOTKEY HELD' : 'HOTKEY RELEASED');
    });
    toggle.classList.add('row-toggle');
    stateCell.appendChild(toggle);

    if (row.on) {
      const action = document.createElement('button');
      action.className = 'row-action';
      action.textContent = 'Change';
      action.addEventListener('click', (event) => {
        event.stopPropagation();
        captureHotkey(row, wrap, keys, detail, action);
      });
      stateCell.appendChild(action);
    }

    wrap.append(keys, main, stateCell);
    item.appendChild(wrap);
    host.appendChild(item);
  });

  refreshHotkeySummary();
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
  element('controls-tab').classList.toggle('active', page === 'controls');

  element('page-number').textContent = PAGES[page].number;
  numberThePage(page);
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

/* The control sheet is review tooling, like the state switcher beside it, so
   it is reached the way review tooling is reached and never appears in the
   product's own navigation. */
function wireReviewTab() {
  const tab = element('controls-tab');
  tab.addEventListener('click', () => {
    showPage(currentPage === 'controls' ? 'overview' : 'controls');
  });
}

/* Written once, from the order the navigation is built from. Typed into the
   markup it drifted: two pages carried the same number and the review sheet
   claimed to be the ninth of nine. */
function numberThePage(page) {
  const product = Object.keys(PAGES).filter((name) => name !== 'controls');
  const host = document.querySelector(`.page[data-page="${page}"] .index`);
  if (!host) return;
  const at = product.indexOf(page);
  host.textContent = at === -1
    ? 'CONTROL SHEET'
    : `${String(at + 1).padStart(2, '0')} / ${String(product.length).padStart(2, '0')}`;
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

/* A panel is placed against the shell, not against the row, so a column that
   scrolls under it would leave it pointing at nothing. */
function closeMenusOnScroll() {
  document.querySelectorAll('.page, .page-side').forEach((column) => {
    column.addEventListener('scroll', () => closeMenu(), { passive: true });
  });
}

function start() {
  const now = new Date();
  element('weekday').textContent = now.toLocaleDateString('en-GB', { weekday: 'short' });
  element('date').textContent = now.toLocaleDateString('en-GB', { day: 'numeric', month: 'long' });
  element('year').textContent = String(now.getFullYear());

  buildBars();
  buildVisualChoices();
  buildCircularPreview();
  buildEngineChoices();
  buildSettingRows('dictation-settings', ['output', 'release', 'history']);
  buildSettingRows('visuals-settings', ['palette', 'response']);
  buildControlsSheet();
  closeMenusOnScroll();
  wireReviewTab();

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

/* ── the provider list, operated rather than illustrated ──
 *
 * The rows below are a working model, not a picture of one. Adding a key,
 * switching a service off, dragging it up the order and watching the privacy
 * line change are the things that have to be got right before any of this is
 * written in Slint, and they cannot be judged from a still. */

const PROVIDER_FIELDS = {
  cloudflare: { label: 'Account identifier', placeholder: '32 hex characters from the dashboard' },
  azure: { label: 'Region', placeholder: 'westeurope' },
  custom: { label: 'Endpoint', placeholder: 'https://speech.example.com/v1' },
};

const PROVIDER_HELP = {
  groq: 'Free tier: 2 000 requests a day. The key starts with gsk_.',
  cloudflare: 'Free tier: 10 000 neurons a day, shared across every model. Needs the account identifier as well as the token.',
  azure: 'Free tier F0: 5 hours a month. The region must match the one the resource was created in.',
  deepgram: '$200 of credit with no expiry.',
  custom: 'Any endpoint that speaks the OpenAI transcription API. HTTPS only: the key and the audio both travel in this request.',
};

let providerModel = [];
let editingProvider = null;

function loadProviderModel(state) {
  providerModel = PROVIDER_STATES[state].rows.map((row) => ({ ...row }));
  editingProvider = null;
}

/** What the row says about itself, derived rather than stored.
 *
 * The words come from the worker's own vocabulary, because the interface has
 * no other source: Switched off, No key stored, Key refused: …, and
 * "<reason>; available again in about N min" for a service that is cooling
 * down. There is no remaining-quota figure on the wire and none is drawn. */
function providerStatus(provider, index) {
  if (!provider.enabled) return { state: 'disabled', text: 'Switched off' };
  if (provider.id !== 'local' && !provider.key) return { state: 'disabled', text: 'No key stored' };
  if (provider.detail) {
    return { state: provider.refused ? 'refused' : 'exhausted', text: provider.detail };
  }
  // The router hands the recording to the local engine as soon as it reaches
  // it and stops walking, so a row under an enabled local engine is never
  // consulted. Saying so is better than leaving it looking ready.
  const localAt = providerModel.findIndex((row) => row.id === 'local' && usable(row));
  if (localAt !== -1 && index > localAt) {
    return { state: 'disabled', text: 'Never reached' };
  }
  const first = providerModel.find((candidate) => usable(candidate));
  if (first === provider) return { state: 'active', text: 'In use' };
  return { state: 'installed', text: 'Standing by' };
}

function usable(provider) {
  if (!provider.enabled) return false;
  if (provider.id !== 'local' && !provider.key) return false;
  return !provider.detail;
}

function renderProviders() {
  const host = element('provider-rows');
  host.innerHTML = '';

  providerModel.forEach((provider, index) => {
    const status = providerStatus(provider, index);
    const item = document.createElement('li');
    const wrap = document.createElement('div');
    wrap.className = 'row';
    wrap.dataset.state = status.state;

    const grip = document.createElement('div');
    grip.className = 'grip';
    grip.textContent = '⋮⋮';
    grip.title = 'Drag to reorder';

    const main = document.createElement('div');
    main.className = 'row-main';
    const title = document.createElement('div');
    title.className = 'row-title';
    const name = document.createElement('b');
    name.textContent = provider.name;
    title.appendChild(name);
    const detail = document.createElement('p');
    detail.className = 'row-detail';
    detail.textContent = providerDetail(provider);
    main.append(title, detail);

    const stateCell = document.createElement('div');
    stateCell.className = 'row-state';
    const mark = document.createElement('span');
    mark.className = 'row-mark';
    const text = document.createElement('span');
    text.className = 'state-text';
    text.textContent = status.text;
    stateCell.append(mark, text);

    // Switching a row off keeps its place and its key; only the order and the
    // switch decide what is used.
    const toggle = toggleControl(
      { on: 'On', off: 'Off' },
      provider.enabled,
      (next) => {
        provider.enabled = next;
        renderProviders();
      },
    );
    toggle.classList.add('row-toggle');
    stateCell.appendChild(toggle);

    if (provider.id !== 'local') {
      const action = document.createElement('button');
      action.className = 'row-action';
      action.textContent = provider.key ? 'Replace key' : 'Add key';
      action.addEventListener('click', (event) => {
        event.stopPropagation();
        editingProvider = editingProvider === provider.id ? null : provider.id;
        renderProviders();
      });
      stateCell.appendChild(action);
    }

    wrap.append(grip, main, stateCell);
    item.appendChild(wrap);
    if (editingProvider === provider.id) item.appendChild(providerEditor(provider));
    host.appendChild(item);
  });

  makeReorderable(host, providerModel, renderProviders);
  refreshPrivacyLine();

  // Measured once the list is in the document: an editor opened on a lower row
  // is useless if its buttons land under the footer.
  const open = host.querySelector('.row-editor');
  if (open) revealInColumn(open);
}

/* scrollIntoView aligns an edge and stops; what is wanted here is that the
   whole thing clears the bottom of the column, with a little room under it.
   Measured synchronously rather than in an animation frame: those do not run
   while the window is in the background, and the editor would then open
   unreachable in a window the user is about to bring forward. */
function revealInColumn(node) {
  const column = node.closest('.page, .page-side');
  if (!column) return;
  const overshoot = node.getBoundingClientRect().bottom
    - column.getBoundingClientRect().bottom + 18;
  // Instant, not smooth: a smooth scroll is animated by the compositor and does
  // not run at all in a background window, which is exactly the case this
  // guards against.
  if (overshoot > 0) column.scrollTop += overshoot;
}

function providerDetail(provider) {
  if (!provider.key) return provider.about;
  const extra = provider.extra ? ` · ${provider.extra}` : '';
  return `Key ••••${provider.key.slice(-4)}${extra} · ${provider.about}`;
}

/** The form that appears in place of nothing when a key is being added. */
function providerEditor(provider) {
  const editor = document.createElement('div');
  editor.className = 'row-editor';

  const fields = document.createElement('div');
  fields.className = 'editor-fields';

  const keyLabel = document.createElement('span');
  keyLabel.className = 'editor-label';
  keyLabel.textContent = provider.key ? 'Replace the key' : 'API key';
  const keyField = fieldControl({ placeholder: 'Paste the key', masked: true });
  fields.append(keyLabel, keyField);

  let extraField = null;
  const spec = PROVIDER_FIELDS[provider.id];
  if (spec) {
    const label = document.createElement('span');
    label.className = 'editor-label';
    label.textContent = spec.label;
    extraField = fieldControl({ placeholder: spec.placeholder, value: provider.extra || '' });
    fields.append(label, extraField);
  }

  const help = document.createElement('p');
  help.className = 'editor-note';
  help.textContent = PROVIDER_HELP[provider.id] || '';

  const storage = document.createElement('p');
  storage.className = 'editor-note';
  storage.innerHTML = 'Stored <b>encrypted with Windows DPAPI</b>, in its own file and never in settings.json. '
    + 'Once saved it is never shown again — only the last four characters.';

  const actions = document.createElement('div');
  actions.className = 'editor-actions';
  const save = document.createElement('button');
  save.className = 'row-action';
  save.textContent = 'Save';
  const cancel = document.createElement('button');
  cancel.className = 'row-action';
  cancel.textContent = 'Cancel';
  actions.append(save, cancel);
  if (provider.key) {
    actions.appendChild(confirmAction('Remove key', 'Remove the stored key?', () => {
      provider.key = null;
      delete provider.detail;
      delete provider.refused;
      editingProvider = null;
      renderProviders();
    }));
  }

  const problem = document.createElement('p');
  problem.className = 'editor-note';
  problem.style.color = 'var(--signal-text)';
  problem.hidden = true;

  save.addEventListener('click', (event) => {
    event.stopPropagation();
    const key = keyField.querySelector('input').value.trim();
    const extra = extraField ? extraField.querySelector('input').value.trim() : '';

    // The two refusals worth showing: nothing typed, and an endpoint that
    // would carry the key and the audio in the clear.
    if (!key) {
      keyField.dataset.invalid = 'true';
      problem.textContent = 'A key is needed before this service can be used.';
      problem.hidden = false;
      return;
    }
    if (spec && !extra) {
      extraField.dataset.invalid = 'true';
      problem.textContent = `${spec.label} is needed as well as the key.`;
      problem.hidden = false;
      return;
    }
    if (provider.id === 'custom' && !extra.startsWith('https://')) {
      extraField.dataset.invalid = 'true';
      problem.textContent = 'The endpoint must be HTTPS: the key and the audio both travel in this request.';
      problem.hidden = false;
      return;
    }

    provider.key = key;
    provider.extra = extra || null;
    provider.enabled = true;
    delete provider.detail;
    delete provider.refused;
    editingProvider = null;
    renderProviders();
    setStatus(privacyTone() === 'signal' ? 'AUDIO LEAVES DEVICE' : 'LOCAL · PRIVATE', 'KEY STORED');
  });

  cancel.addEventListener('click', (event) => {
    event.stopPropagation();
    editingProvider = null;
    renderProviders();
  });

  editor.append(fields, help, storage, problem, actions);
  return editor;
}

function privacyTone() {
  const first = providerModel.find((provider) => usable(provider));
  return first && first.id !== 'local' ? 'signal' : 'ink';
}

function refreshPrivacyLine() {
  const tone = privacyTone();
  const first = providerModel.find((provider) => usable(provider));
  const summary = element('privacy-summary');
  summary.dataset.tone = tone;
  element('privacy-text').textContent = !first
    ? 'Nothing can transcribe right now'
    : tone === 'signal'
      ? `Audio leaves this device for ${first.name}`
      : 'Audio stays on this device';
  setStatus(
    tone === 'signal' ? 'AUDIO LEAVES DEVICE' : 'LOCAL · PRIVATE',
    first ? 'SYSTEM READY' : 'NO PROVIDER',
  );
}

/* ── the model list, operated rather than illustrated ──── */

let speechModel = [];
let packModel = [];
const downloads = new Map();

function loadModelLists(state) {
  downloads.forEach((timer) => clearInterval(timer));
  downloads.clear();
  speechModel = MODEL_STATES[state].map((row) => ({ ...row }));
  packModel = (PACK_ROWS[state] || PACK_ROWS['at-rest']).map((row) => ({ ...row }));
}

function renderModelLists() {
  renderModelRows(element('model-rows'), speechModel);
  renderModelRows(element('pack-rows'), packModel);
  const installed = [...speechModel, ...packModel].filter((row) => row.state === 'selected' || row.state === 'installed');
  element('models-summary').textContent = `${installed.length} installed`;
}

function renderModelRows(host, list) {
  host.innerHTML = '';
  list.forEach((row, index) => {
    const item = document.createElement('li');
    const wrap = document.createElement('div');
    wrap.className = 'row';
    wrap.dataset.state = row.state;

    const order = document.createElement('div');
    order.className = 'order';
    order.textContent = String(index + 1).padStart(2, '0');

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

    const stateCell = document.createElement('div');
    stateCell.className = 'row-state';
    const size = document.createElement('span');
    size.className = 'row-size';
    size.textContent = row.size;
    stateCell.appendChild(size);

    if (typeof row.progress === 'number') {
      const progress = document.createElement('div');
      progress.className = 'progress';
      const fill = document.createElement('i');
      fill.style.width = `${row.progress}%`;
      progress.appendChild(fill);
      stateCell.appendChild(progress);
    }

    const mark = document.createElement('span');
    mark.className = 'row-mark';
    const text = document.createElement('span');
    text.className = 'state-text';
    text.textContent = row.text;
    stateCell.append(mark, text);

    modelActions(row, list).forEach((action) => stateCell.appendChild(action));

    wrap.append(order, main, stateCell);
    item.appendChild(wrap);
    host.appendChild(item);
  });
}

/* One verb per row at rest; a second appears only while something runs. */
function modelActions(row, list) {
  const make = (label, handler, danger) => {
    const button = document.createElement('button');
    button.className = `row-action${danger ? ' danger' : ''}`;
    button.textContent = label;
    button.addEventListener('click', (event) => {
      event.stopPropagation();
      handler();
    });
    return button;
  };

  if (row.state === 'downloading' || row.state === 'verifying') {
    return [make('Cancel', () => cancelDownload(row))];
  }
  if (row.state === 'selected') {
    return [];
  }
  if (row.state === 'installed') {
    return [
      make('Use', () => {
        list.forEach((other) => {
          if (other.state === 'selected') {
            other.state = 'installed';
            other.text = 'Installed';
          }
        });
        row.state = 'selected';
        row.text = 'Selected';
        renderModelLists();
        setStatus('LOCAL · PRIVATE', 'MODEL SELECTED');
      }),
      make('Remove', () => {
        row.state = 'idle';
        row.text = 'Not installed';
        renderModelLists();
      }, true),
    ];
  }
  return [make(row.state === 'refused' ? 'Retry' : 'Install', () => startDownload(row))];
}

/* A download that only moves when the page is reloaded teaches nothing about
   how the row behaves while it runs. */
function startDownload(row) {
  row.state = 'downloading';
  row.progress = 0;
  row.text = '0 %';
  row.detail = 'Resumes from where it stopped if it is interrupted';
  renderModelLists();
  setStatus('LOCAL · PRIVATE', 'DOWNLOADING');

  const timer = setInterval(() => {
    row.progress = Math.min(100, row.progress + 7);
    row.text = `${row.progress} %`;
    if (row.progress >= 100) {
      clearInterval(timer);
      downloads.delete(row.name);
      row.state = 'verifying';
      row.text = 'Verifying';
      row.detail = 'Checking SHA-256 before the file is put in place';
      renderModelLists();
      setTimeout(() => {
        row.state = 'installed';
        row.text = 'Installed';
        delete row.progress;
        row.detail = 'Verified and installed';
        renderModelLists();
        setStatus('LOCAL · PRIVATE', 'INSTALLED');
      }, 1200);
      return;
    }
    renderModelLists();
  }, 260);
  downloads.set(row.name, timer);
}

function cancelDownload(row) {
  const timer = downloads.get(row.name);
  if (timer) clearInterval(timer);
  downloads.delete(row.name);
  row.state = 'idle';
  row.text = 'Not installed';
  row.detail = 'Cancelled · the partial download is kept and resumes later';
  delete row.progress;
  renderModelLists();
  setStatus('LOCAL · PRIVATE', 'CANCELLED');
}

start();
