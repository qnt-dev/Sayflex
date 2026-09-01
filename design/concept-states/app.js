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

const PAGES = {
  dictation: { states: Object.keys(DICTATION_STATES), number: '013' },
  models: { states: Object.keys(MODEL_STATES), number: '021' },
  providers: { states: Object.keys(PROVIDER_STATES), number: '024' },
};

const BLANK_PAGES = {
  overview: { index: '01 / 08', title: 'Overview', archetype: 'сводка' },
  translation: { index: '03 / 08', title: 'Translation', archetype: 'форма' },
  visuals: { index: '06 / 08', title: 'Visuals', archetype: 'прибор' },
  history: { index: '07 / 08', title: 'History', archetype: 'список' },
  hotkeys: { index: '08 / 08', title: 'Hotkeys', archetype: 'форма' },
};

/* ── rendering ─────────────────────────────────────────── */

const element = (id) => document.getElementById(id);
let currentPage = 'dictation';
let currentState = 'ready';

function buildBars() {
  const host = element('signal-bars');
  host.innerHTML = '';
  for (let index = 0; index < BAR_COUNT; index += 1) {
    host.appendChild(document.createElement('i'));
  }
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

function setStatus(mode, line) {
  const copy = document.querySelector('.status-copy');
  copy.dataset.tone = mode === 'LOCAL · PRIVATE' ? 'ink' : 'signal';
  element('status-mode').textContent = mode;
  element('status-line').textContent = line;
}

function showPage(page) {
  currentPage = page;
  const known = Object.keys(PAGES).includes(page);
  const workspace = element('workspace');
  workspace.classList.toggle('is-list', page === 'models' || page === 'providers');

  document.querySelectorAll('.page, .page-side').forEach((node) => {
    node.hidden = node.dataset.page !== page;
  });
  document.querySelectorAll('#tool-nav button').forEach((button) => {
    button.classList.toggle('active', button.dataset.page === page);
  });

  if (!known) {
    const blank = BLANK_PAGES[page];
    document.querySelector('[data-page="blank"]').hidden = false;
    element('blank-index').textContent = blank.index;
    element('blank-title').textContent = blank.title;
    element('blank-archetype').textContent = blank.archetype;
    element('page-number').textContent = '—';
    buildStateButtons([]);
    setStatus('LOCAL · PRIVATE', 'NOT DESIGNED YET');
    return;
  }

  element('page-number').textContent = PAGES[page].number;
  buildStateButtons(PAGES[page].states);
  showState(PAGES[page].states[0]);
}

function showState(state) {
  currentState = state;
  document.querySelectorAll('#state-buttons button').forEach((button) => {
    button.classList.toggle('active', button.dataset.state === state);
  });
  if (currentPage === 'dictation') applyDictation(state);
  if (currentPage === 'models') applyModels(state);
  if (currentPage === 'providers') applyProviders(state);
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
  requestAnimationFrame(animate);
}

function start() {
  const now = new Date();
  element('weekday').textContent = now.toLocaleDateString('en-GB', { weekday: 'short' });
  element('date').textContent = now.toLocaleDateString('en-GB', { day: 'numeric', month: 'long' });
  element('year').textContent = String(now.getFullYear());

  buildBars();
  document.querySelectorAll('#tool-nav button').forEach((button) => {
    button.addEventListener('click', () => showPage(button.dataset.page));
  });
  showPage('dictation');
  requestAnimationFrame(animate);
}

start();
