/* SayFlex — the control system.
 *
 * Everything the user can actually press. Kept apart from the page data
 * because these are the parts that have to behave the same wherever they
 * appear: a menu opens the same way on Dictation and on Translation, a
 * destructive action asks the same question everywhere.
 *
 * The rule the whole set shares: the choice in force is an ink block, exactly
 * as in the navigation. Whatever is chosen right now is the darkest thing in
 * view, and nothing else competes for that weight. */

/* ── menus ─────────────────────────────────────────────── */

const MENUS = {
  output: {
    label: 'Output',
    options: [
      { value: 'original', label: 'Original language', note: 'Whatever you spoke, written as spoken' },
      { value: 'english', label: 'Translate to English', note: 'Only for models trained for it' },
    ],
  },
  release: {
    label: 'Release',
    options: [
      { value: 'fastest', label: 'Fastest', note: 'Prepares results during natural pauses' },
      { value: 'efficient', label: 'Efficient', note: 'Less GPU and battery, slightly slower' },
    ],
  },
  history: {
    label: 'History',
    options: [
      { value: 'on', label: 'Keep on this device' },
      { value: 'off', label: 'Do not keep', note: 'Insert-last still works for the newest one' },
      { separator: true },
      { value: 'clear', label: 'Clear everything now', danger: true, keepSelection: true },
    ],
  },
  'translation-model': {
    label: 'Model',
    options: [
      { value: 'fast', label: 'Fast · Qwen3.5 4B', note: '2.80 GB · installed' },
      { value: 'quality', label: 'Quality · Qwen3.5 9B', note: '5.70 GB · not installed', disabled: true, reason: 'Install it first' },
    ],
  },
  'translation-engine': {
    label: 'Engine',
    options: [
      { value: 'warm', label: 'Keep warm', note: 'Lowest latency, holds memory' },
      { value: 'demand', label: 'Load on demand', note: 'Frees memory, first request waits' },
    ],
  },
  'translation-result': {
    label: 'Result',
    options: [
      { value: 'preview', label: 'Show a preview', note: 'A floating window next to the caret' },
      { value: 'replace', label: 'Replace the selection', note: 'The original goes to history' },
      { value: 'copy', label: 'Copy to clipboard' },
    ],
  },
  'text-action': {
    label: 'On selected text',
    options: [
      { value: 'adaptive', label: 'Decide from the text', note: 'Translates foreign text, corrects a wrong layout' },
      { value: 'translate', label: 'Always translate' },
      { value: 'layout', label: 'Always correct the layout' },
    ],
  },
  direction: {
    label: 'Direction',
    options: [
      { value: 'auto', label: 'Decide by content', note: 'Russian goes to English and back' },
      { value: 'ru-en', label: 'Always RU → EN' },
      { value: 'en-ru', label: 'Always EN → RU' },
    ],
  },
  palette: {
    label: 'Palette',
    options: [
      { value: 'mono', label: 'Monochrome' },
      { value: 'arctic', label: 'Arctic' },
      { value: 'signal', label: 'Signal' },
    ],
  },
  response: {
    label: 'Response',
    options: [
      { value: 'fast', label: 'Fast', note: 'Follows every syllable' },
      { value: 'calm', label: 'Calm', note: 'Smooths the level over time' },
    ],
  },
};

const CHOSEN = {
  output: 'original',
  release: 'fastest',
  history: 'on',
  'translation-model': 'fast',
  'translation-engine': 'warm',
  'translation-result': 'preview',
  direction: 'auto',
  palette: 'mono',
  response: 'fast',
};

let openMenu = null;

function labelOf(menuId) {
  const menu = MENUS[menuId];
  const option = menu.options.find((item) => item.value === CHOSEN[menuId]);
  return option ? option.label : '—';
}

/** A row that opens a menu: the setting on the left, the choice in force on the right. */
function menuButton(menuId, onChange) {
  const button = document.createElement('button');
  button.className = 'menu-button';
  button.type = 'button';
  button.setAttribute('aria-haspopup', 'menu');
  button.setAttribute('aria-expanded', 'false');
  const name = document.createElement('span');
  name.textContent = MENUS[menuId].label;
  const value = document.createElement('b');
  value.textContent = labelOf(menuId);
  button.append(name, value);
  button.addEventListener('click', (event) => {
    event.stopPropagation();
    toggleMenu(button, menuId, () => {
      value.textContent = labelOf(menuId);
      if (onChange) onChange(CHOSEN[menuId]);
    });
  });
  return button;
}

function toggleMenu(anchor, menuId, onChange) {
  if (openMenu && openMenu.anchor === anchor) {
    closeMenu();
    return;
  }
  closeMenu();

  const panel = document.createElement('div');
  panel.className = 'menu-panel';
  panel.setAttribute('role', 'menu');

  const heading = document.createElement('div');
  heading.className = 'menu-label';
  heading.textContent = MENUS[menuId].label;
  panel.appendChild(heading);

  MENUS[menuId].options.forEach((option) => {
    if (option.separator) {
      const rule = document.createElement('div');
      rule.className = 'menu-separator';
      panel.appendChild(rule);
      return;
    }
    const item = document.createElement('button');
    item.className = `menu-item${option.danger ? ' danger' : ''}`;
    item.type = 'button';
    item.setAttribute('role', 'menuitemradio');
    item.setAttribute('aria-checked', String(!option.keepSelection && CHOSEN[menuId] === option.value));
    if (option.disabled) item.disabled = true;

    const mark = document.createElement('i');
    const body = document.createElement('span');
    body.textContent = option.label;
    if (option.note || option.reason) {
      const note = document.createElement('small');
      note.textContent = option.reason || option.note;
      body.appendChild(note);
    }
    item.append(mark, body);
    item.addEventListener('click', (event) => {
      event.stopPropagation();
      if (option.disabled) return;
      if (!option.keepSelection) CHOSEN[menuId] = option.value;
      closeMenu();
      if (onChange) onChange(option.value);
    });
    panel.appendChild(item);
  });

  document.querySelector('.app-shell').appendChild(panel);
  placePanel(panel, anchor);
  anchor.setAttribute('aria-expanded', 'true');
  openMenu = { anchor, panel, menuId };

  const first = panel.querySelector('.menu-item[aria-checked="true"]') || panel.querySelector('.menu-item');
  if (first) first.focus();
}

function placePanel(panel, anchor) {
  const shell = document.querySelector('.app-shell').getBoundingClientRect();
  const box = anchor.getBoundingClientRect();
  panel.style.minWidth = `${Math.max(220, box.width)}px`;
  const height = panel.offsetHeight;
  // Open downwards unless the bottom of the window is closer than the panel is
  // tall; a menu that runs off the screen is a menu with missing options.
  const below = box.bottom - shell.top + 4;
  const above = box.top - shell.top - height - 4;
  panel.style.top = `${below + height < shell.height ? below : Math.max(4, above)}px`;
  panel.style.left = `${Math.min(box.left - shell.left, shell.width - panel.offsetWidth - 8)}px`;
}

function closeMenu() {
  if (!openMenu) return;
  openMenu.panel.remove();
  openMenu.anchor.setAttribute('aria-expanded', 'false');
  openMenu = null;
}

/** Context menus carry actions rather than a choice, so they take a list. */
function openContextMenu(items, x, y) {
  closeMenu();
  const panel = document.createElement('div');
  panel.className = 'menu-panel';
  panel.setAttribute('role', 'menu');

  items.forEach((item) => {
    if (item.separator) {
      const rule = document.createElement('div');
      rule.className = 'menu-separator';
      panel.appendChild(rule);
      return;
    }
    if (item.heading) {
      const heading = document.createElement('div');
      heading.className = 'menu-label';
      heading.textContent = item.heading;
      panel.appendChild(heading);
      return;
    }
    const button = document.createElement('button');
    button.className = `menu-item${item.danger ? ' danger' : ''}`;
    button.type = 'button';
    button.setAttribute('role', 'menuitem');
    if (item.disabled) button.disabled = true;
    const mark = document.createElement('i');
    const body = document.createElement('span');
    body.textContent = item.label;
    if (item.note) {
      const note = document.createElement('small');
      note.textContent = item.note;
      body.appendChild(note);
    }
    button.append(mark, body);
    button.addEventListener('click', (event) => {
      event.stopPropagation();
      closeMenu();
      if (item.action) item.action();
    });
    panel.appendChild(button);
  });

  const shell = document.querySelector('.app-shell');
  shell.appendChild(panel);
  const box = shell.getBoundingClientRect();
  panel.style.left = `${Math.min(x - box.left, box.width - panel.offsetWidth - 8)}px`;
  panel.style.top = `${Math.min(y - box.top, box.height - panel.offsetHeight - 8)}px`;
  openMenu = { anchor: document.body, panel, menuId: null };
  const first = panel.querySelector('.menu-item:not([disabled])');
  if (first) first.focus();
}

/* Anywhere outside closes; Escape closes and returns focus; the arrows walk
   the list. A menu that can only be dismissed by choosing something is a trap. */
document.addEventListener('click', () => closeMenu());
document.addEventListener('keydown', (event) => {
  if (!openMenu) return;
  const items = [...openMenu.panel.querySelectorAll('.menu-item:not([disabled])')];
  const index = items.indexOf(document.activeElement);
  if (event.key === 'Escape') {
    const anchor = openMenu.anchor;
    closeMenu();
    if (anchor && anchor.focus) anchor.focus();
    event.preventDefault();
  }
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    const next = event.key === 'ArrowDown'
      ? items[(index + 1) % items.length]
      : items[(index - 1 + items.length) % items.length];
    if (next) next.focus();
    event.preventDefault();
  }
});
window.addEventListener('resize', closeMenu);

/* ── toggle ────────────────────────────────────────────── */

function toggleControl(label, pressed, onChange) {
  const button = document.createElement('button');
  button.className = 'toggle';
  button.type = 'button';
  button.setAttribute('aria-pressed', String(pressed));
  const track = document.createElement('span');
  track.className = 'toggle-track';
  const text = document.createElement('b');
  const paint = () => {
    text.textContent = button.getAttribute('aria-pressed') === 'true' ? label.on : label.off;
  };
  paint();
  button.append(track, text);
  button.addEventListener('click', () => {
    const next = button.getAttribute('aria-pressed') !== 'true';
    button.setAttribute('aria-pressed', String(next));
    paint();
    if (onChange) onChange(next);
  });
  return button;
}

/* ── text field ────────────────────────────────────────── */

function fieldControl({ placeholder, value = '', masked = false, invalid = false }) {
  const wrap = document.createElement('label');
  wrap.className = 'field';
  if (invalid) wrap.dataset.invalid = 'true';
  const input = document.createElement('input');
  input.type = masked ? 'password' : 'text';
  input.placeholder = placeholder;
  input.value = value;
  wrap.appendChild(input);
  return wrap;
}

/* ── destructive confirm ───────────────────────────────── */

/* A destructive action asks once, in place, and the question replaces the
   button rather than covering the page. Nothing here needs a modal. */
function confirmAction(label, question, onConfirm) {
  const wrap = document.createElement('span');
  wrap.className = 'confirm';
  const render = (asking) => {
    wrap.innerHTML = '';
    if (!asking) {
      const button = document.createElement('button');
      button.className = 'row-action danger';
      button.textContent = label;
      button.addEventListener('click', (event) => {
        event.stopPropagation();
        render(true);
      });
      wrap.appendChild(button);
      return;
    }
    const text = document.createElement('span');
    text.className = 'confirm-question';
    text.textContent = question;
    const yes = document.createElement('button');
    yes.className = 'row-action danger';
    yes.textContent = 'Yes';
    yes.addEventListener('click', (event) => {
      event.stopPropagation();
      render(false);
      if (onConfirm) onConfirm();
    });
    const no = document.createElement('button');
    no.className = 'row-action';
    no.textContent = 'No';
    no.addEventListener('click', (event) => {
      event.stopPropagation();
      render(false);
    });
    wrap.append(text, yes, no);
  };
  render(false);
  return wrap;
}

/** The same menu, for places where the label is already on the row. */
function menuValueButton(menuId, onChange) {
  const button = document.createElement('button');
  button.className = 'menu-button compact';
  button.type = 'button';
  button.setAttribute('aria-haspopup', 'menu');
  button.setAttribute('aria-expanded', 'false');
  const value = document.createElement('b');
  value.textContent = labelOf(menuId);
  button.appendChild(value);
  button.addEventListener('click', (event) => {
    event.stopPropagation();
    toggleMenu(button, menuId, () => {
      value.textContent = labelOf(menuId);
      if (onChange) onChange(CHOSEN[menuId]);
    });
  });
  return button;
}

/* ── the sheet ─────────────────────────────────────────── */

/* Every control the application needs, in every state it can be in. This page
   exists because a control drawn once, in its resting state, is not designed:
   what it looks like while it is busy, refused, or disabled is where the work
   actually is. */
const SHEET = [
  {
    title: 'Menu',
    note: 'For a setting with a small, fixed set of answers. The one in force is an ink block; the panel opens under the row and closes on Escape, on a click outside, or on a choice.',
    build: () => [
      ['closed', () => menuButton('output')],
      ['compact, inside a row', () => menuValueButton('translation-result')],
      ['with an option that cannot be chosen yet', () => menuValueButton('translation-model')],
      ['carrying a destructive action', () => menuButton('history')],
    ],
  },
  {
    title: 'Segmented choice',
    note: 'For three or four alternatives worth showing at once. Used for the speech engine, where seeing all of them matters more than saving the space.',
    build: () => [
      ['at rest, one chosen', () => segmentedDemo('rest')],
      ['verifying a backend', () => segmentedDemo('busy')],
      ['one unavailable', () => segmentedDemo('disabled')],
    ],
  },
  {
    title: 'Toggle',
    note: 'For something that is either on or off and takes effect immediately. The label states what is true now, not what pressing it would do.',
    build: () => [
      ['on', () => toggleControl({ on: 'Kept on this device', off: 'Not kept' }, true)],
      ['off', () => toggleControl({ on: 'Kept on this device', off: 'Not kept' }, false)],
    ],
  },
  {
    title: 'Stepper',
    note: 'For a small bounded number where the exact value matters and a slider would only approximate it.',
    build: () => [['blur radius, 0 to 8', () => stepperDemo()]],
  },
  {
    title: 'Text field',
    note: 'For a key or an address. A stored key is never shown again: the field carries a masked tail, and typing replaces it.',
    build: () => [
      ['empty', () => fieldControl({ placeholder: 'Paste an API key' })],
      ['holding a stored key', () => fieldControl({ placeholder: 'Paste an API key', value: '••••9c07' })],
      ['refused by the service', () => {
        const wrap = document.createElement('div');
        wrap.append(
          fieldControl({ placeholder: 'Paste an API key', value: '••••4f21', invalid: true }),
          noteLine('The service refused this key. Waiting will not fix it.'),
        );
        return wrap;
      }],
    ],
  },
  {
    title: 'Row action',
    note: 'The verb belongs in the row it acts on. One action per row at rest; a second appears only while something is running.',
    build: () => [
      ['ordinary', () => actionDemo('Install')],
      ['while running', () => actionDemo('Cancel', true)],
      ['destructive, asked in place', () => confirmAction('Clear history', 'Delete every entry from disk?', () => {})],
    ],
  },
  {
    title: 'Context menu',
    note: 'A right-click on a row, and the tray icon. Actions only, never a setting: a menu that both does things and remembers things is two menus.',
    build: () => [
      ['on a history entry', () => contextDemo('history')],
      ['the tray', () => contextDemo('tray')],
    ],
  },
];

function noteLine(text) {
  const note = document.createElement('p');
  note.className = 'field-note';
  note.textContent = text;
  return note;
}

function segmentedDemo(mode) {
  const grid = document.createElement('div');
  grid.className = 'choice-grid';
  [['Local<br>GPU', 'spot-mark'], ['Local<br>CPU', 'area-mark'], ['Auto', 'target-mark'], ['API', 'sun-mark']]
    .forEach(([label, mark], index) => {
      const button = document.createElement('button');
      button.className = `choice${index === 0 ? ' active' : ''}`;
      if (mode === 'busy' && index === 0) button.dataset.busy = 'true';
      if (mode === 'disabled' && index === 3) button.disabled = true;
      button.addEventListener('click', () => {
        if (button.disabled) return;
        grid.querySelectorAll('.choice').forEach((other) => other.classList.remove('active'));
        button.classList.add('active');
      });
      const icon = document.createElement('i');
      icon.className = mark;
      const text = document.createElement('span');
      text.innerHTML = label;
      button.append(icon, text);
      grid.appendChild(button);
    });
  return grid;
}

function stepperDemo() {
  const wrap = document.createElement('div');
  wrap.className = 'stepper';
  const less = document.createElement('button');
  less.textContent = '−';
  const value = document.createElement('b');
  let amount = 3;
  value.textContent = String(amount);
  const more = document.createElement('button');
  more.textContent = '+';
  less.addEventListener('click', () => { amount = Math.max(0, amount - 1); value.textContent = String(amount); });
  more.addEventListener('click', () => { amount = Math.min(8, amount + 1); value.textContent = String(amount); });
  wrap.append(less, value, more);
  return wrap;
}

function actionDemo(label, running) {
  const wrap = document.createElement('div');
  wrap.style.display = 'flex';
  wrap.style.alignItems = 'center';
  wrap.style.gap = '10px';
  if (running) {
    const progress = document.createElement('div');
    progress.className = 'progress';
    const fill = document.createElement('i');
    fill.style.width = '43%';
    progress.appendChild(fill);
    wrap.appendChild(progress);
  }
  const button = document.createElement('button');
  button.className = 'row-action';
  button.textContent = label;

  // A specimen that does nothing when pressed is a picture of a control. This
  // one runs, so the second state can be seen rather than read about.
  let timer = null;
  let done = running ? 43 : 0;
  const bar = wrap.querySelector('.progress i');

  const stop = () => {
    clearInterval(timer);
    timer = null;
    done = 0;
    button.textContent = 'Install';
    if (bar) bar.style.width = '0%';
  };

  const run = () => {
    button.textContent = 'Cancel';
    timer = setInterval(() => {
      done = Math.min(100, done + 6);
      if (bar) bar.style.width = `${done}%`;
      if (done >= 100) stop();
    }, 220);
  };

  button.addEventListener('click', () => (timer ? stop() : run()));
  // The specimen captioned "while running" should be running, or it is a
  // picture of the state it claims to show.
  if (running) run();

  wrap.appendChild(button);
  return wrap;
}

function contextDemo(kind) {
  const button = document.createElement('button');
  button.className = 'row-action';
  button.textContent = kind === 'tray' ? 'Open the tray menu' : 'Right-click me';
  const items = kind === 'tray'
    ? [
      { heading: 'SayFlex' },
      { label: 'Dictate now', note: 'Same as holding Ctrl + Alt + Space' },
      { label: 'Insert last transcript' },
      { label: 'Pause input features' },
      { separator: true },
      { label: 'Open settings' },
      { separator: true },
      { label: 'Quit SayFlex', danger: true },
    ]
    : [
      { heading: 'Entry' },
      { label: 'Copy text' },
      { label: 'Insert where the caret is' },
      { separator: true },
      { label: 'Delete this entry', danger: true },
    ];
  const open = (event) => {
    event.preventDefault();
    event.stopPropagation();
    openContextMenu(items, event.clientX, event.clientY);
  };
  button.addEventListener('click', open);
  button.addEventListener('contextmenu', open);
  return button;
}

function buildControlsSheet() {
  const host = document.getElementById('controls-sheet');
  host.innerHTML = '';
  SHEET.forEach((group) => {
    const section = document.createElement('section');
    const title = document.createElement('h2');
    title.textContent = group.title;
    const note = document.createElement('p');
    note.textContent = group.note;
    const demo = document.createElement('div');
    demo.className = 'demo';
    group.build().forEach(([caption, make]) => {
      const label = document.createElement('span');
      label.className = 'state-caption';
      label.textContent = caption;
      demo.append(label, make());
    });
    section.append(title, note, demo);
    host.appendChild(section);
  });
}
