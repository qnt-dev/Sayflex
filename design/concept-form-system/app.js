const modeContent = {
  overview: {
    index: '01 / 06', title: 'Overview', subtitle: 'SayFlex · all systems nominal', panel: 'Runtime',
    choices: [['Ready','System'],['5070 Ti','GPU'],['RU → EN','Route'],['3','Models']],
    labels: ['GPU load','Memory allocation'], values: [27,48], angle: '98%', angleLabel: 'Availability', page: '011'
  },
  dictation: {
    index: '02 / 06', title: 'Dictation', subtitle: 'Local speech · ready', panel: 'Engine',
    choices: [['Local','GPU'],['Local','CPU'],['Auto'],['API']],
    labels: ['Sensitivity','Silence threshold'], values: [62,31], angle: '35°', angleLabel: 'Input angle', page: '013'
  },
  translation: {
    index: '03 / 06', title: 'Translation', subtitle: 'Selection → contextual translation', panel: 'Layout',
    choices: [['Translation'],['Original'],['Dual'],['Pinned']],
    labels: ['Glass blur','Window opacity'], values: [22,74], angle: 'F', angleLabel: 'Ctrl + Alt + F', page: '017'
  },
  models: {
    index: '04 / 06', title: 'Models', subtitle: 'Local inference · CUDA active', panel: 'Model',
    choices: [['Whisper','Large'],['Whisper','Medium'],['Qwen','Translate'],['Custom']],
    labels: ['Decoder speed','GPU reserve'], values: [84,66], angle: '90 t/s', angleLabel: 'Decode rate', page: '021'
  },
  visuals: {
    index: '05 / 06', title: 'Visuals', subtitle: 'Voice state · native overlay', panel: 'Style',
    choices: [['Flow','Sphere'],['Writing','Slate'],['Circular'],['Off']],
    labels: ['Response','Motion intensity'], values: [71,42], angle: '01', angleLabel: 'Flow sphere', page: '025'
  },
  history: {
    index: '06 / 06', title: 'History', subtitle: 'Private · stored on this device', panel: 'Filter',
    choices: [['Today'],['Dictation'],['Translate'],['Pinned']],
    labels: ['Retention','Result length'], values: [45,73], angle: '128', angleLabel: 'Local records', page: '029'
  },
  settings: {
    index: 'S / 06', title: 'Settings', subtitle: 'Application · hardware · privacy', panel: 'System',
    choices: [['General'],['Hotkeys'],['Devices'],['Privacy']],
    labels: ['Interface scale','Background blur'], values: [50,20], angle: '1.0×', angleLabel: 'Interface scale', page: '031'
  }
};

const ui = {
  title: document.querySelector('#section-title'), subtitle: document.querySelector('#section-subtitle'),
  index: document.querySelector('#section-index'), panel: document.querySelector('#panel-title'),
  grid: document.querySelector('#choice-grid'), label1: document.querySelector('#slider-one-label'),
  label2: document.querySelector('#slider-two-label'), slider1: document.querySelector('#sensitivity'),
  slider2: document.querySelector('#threshold'), angle: document.querySelector('#orbit-value'),
  angleLabel: document.querySelector('#orbit-label'), page: document.querySelector('#page-number'),
  object: document.querySelector('#signal-object'), listen: document.querySelector('#listen-button'),
  listenLabel: document.querySelector('#listen-label'), readout: document.querySelector('#readout-value'),
  status: document.querySelector('#status-text')
};

function choiceMark(index) {
  return ['spot-mark','area-mark','target-mark','sun-mark'][index];
}

function setMode(mode) {
  const data = modeContent[mode] || modeContent.dictation;
  document.querySelectorAll('[data-mode]').forEach(el => el.classList.toggle('active', el.dataset.mode === mode));
  ui.index.textContent = data.index;
  ui.title.textContent = data.title;
  ui.subtitle.textContent = data.subtitle;
  ui.panel.textContent = data.panel;
  ui.label1.textContent = data.labels[0];
  ui.label2.textContent = data.labels[1];
  ui.slider1.value = data.values[0];
  ui.slider2.value = data.values[1];
  ui.angle.textContent = data.angle;
  ui.angleLabel.textContent = data.angleLabel;
  ui.page.textContent = data.page;
  ui.grid.innerHTML = data.choices.map((choice, index) =>
    `<button class="choice ${index === 0 ? 'active' : ''}" data-choice="${choice.join(' ')}"><i class="${choiceMark(index)}"></i><span>${choice.join('<br>')}</span></button>`
  ).join('');
  wireChoices();
  ui.object.style.transform = 'translate(-50%,-50%) rotate(0deg)';
}

function wireChoices() {
  document.querySelectorAll('.choice').forEach(choice => choice.addEventListener('click', () => {
    document.querySelectorAll('.choice').forEach(item => item.classList.remove('active'));
    choice.classList.add('active');
    ui.status.textContent = `${choice.dataset.choice.toUpperCase()} SELECTED`;
  }));
}

document.querySelectorAll('[data-mode]').forEach(button => button.addEventListener('click', () => setMode(button.dataset.mode)));

let timer = null;
let elapsed = 0;
ui.listen.addEventListener('click', () => {
  const listening = ui.listen.getAttribute('aria-pressed') !== 'true';
  ui.listen.setAttribute('aria-pressed', String(listening));
  ui.object.classList.toggle('listening', listening);
  ui.listenLabel.textContent = listening ? 'Stop listening' : 'Start listening';
  ui.status.textContent = listening ? 'LISTENING / LOCAL' : 'SYSTEM READY';
  clearInterval(timer);
  if (listening) {
    elapsed = 0;
    timer = setInterval(() => {
      elapsed += 1;
      const mins = String(Math.floor(elapsed / 60)).padStart(2,'0');
      const secs = String(elapsed % 60).padStart(2,'0');
      ui.readout.textContent = `${mins}:${secs}`;
      ui.angle.textContent = `${18 + Math.round(Math.random() * 64)}%`;
    }, 1000);
  } else {
    ui.angle.textContent = modeContent.dictation.angle;
  }
});

ui.slider1.addEventListener('input', event => {
  const value = Number(event.target.value);
  ui.object.style.transform = `translate(-50%,-50%) rotate(${(value - 50) * .32}deg)`;
  ui.angle.textContent = `${value}%`;
});
ui.slider2.addEventListener('input', event => {
  ui.object.style.filter = `drop-shadow(0 18px ${4 + Number(event.target.value) / 3}px rgba(20,22,25,.${Math.max(8,Math.round(Number(event.target.value)/5))}))`;
});

document.querySelector('#brand-mark').addEventListener('click', () => {
  ui.status.textContent = 'LOCAL ENGINE / ONLINE';
  ui.object.animate([
    {transform:'translate(-50%,-50%) scale(1)'},
    {transform:'translate(-50%,-50%) scale(.96)'},
    {transform:'translate(-50%,-50%) scale(1)'}
  ], {duration:380,easing:'cubic-bezier(.2,.8,.2,1)'});
});

const now = new Date();
document.querySelector('#weekday').textContent = new Intl.DateTimeFormat('en', {weekday:'short'}).format(now);
document.querySelector('#date').textContent = new Intl.DateTimeFormat('en', {day:'numeric',month:'long'}).format(now);
document.querySelector('#year').textContent = now.getFullYear();

wireChoices();
