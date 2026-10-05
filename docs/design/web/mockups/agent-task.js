// Representative agent-task transcripts. The live run and steering are a local demo.
(() => {
  'use strict';

  const KEY = 'temper-web-mockup-v1';
  const taskId = new URLSearchParams(location.search).get('task') || 'T26';
  const tasks = {
    T22: {
      title: 'Spike: rotation behavior', role: 'Agent · investigator', status: 'done', spend: '2.1 / 4 spent',
      brief: 'Trace how the provider rotates refresh tokens and report the failure cases the plan must cover.',
      meta: 'Asked by T21 · Result: report · Reads ai/temper',
      runs: [{ title: 'Run 1 · completed yesterday', meta: 'First activation · resumed from the task brief · 2.1 spent',
        events: [
          ['agent', 'I’ll follow refresh from the callback through session persistence, then check which errors leave a usable session.'],
          ['tool', 'Read provider refresh and session paths', 'auth/refresh.rs · session/store.rs · provider response handling'],
          ['agent', 'The provider may issue a new refresh token with the access token. A failed persistence step can leave the browser with an old token while the provider has already rotated it.'],
          ['result', 'Report sent to T21', 'Persist the new token pair together. Keep the prior valid session through transient failures, then surface a clear final sign-in failure.']
        ], end: 'Finished · report committed · 2.1 spent' }]
    },
    T23: {
      title: 'Session storage design', role: 'Agent · designer', status: 'done', spend: '2.6 / 5 spent',
      brief: 'Design an atomic session update for rotated tokens, using T22’s failure cases as inputs.',
      meta: 'Asked by T21 · After T22 · Result: report',
      runs: [{ title: 'Run 1 · completed at 09:54', meta: 'Woken by T22’s report · started fresh from a brief · 2.6 spent',
        events: [
          ['event', 'Input received', 'T22 reported a provider rotation and persistence gap.'],
          ['agent', 'I’ll compare the current session write with an atomic replacement so a partial write cannot split the token pair.'],
          ['tool', 'Inspect session writes and tests', 'session/store.rs · auth/refresh.rs · tests/session_refresh.rs'],
          ['agent', 'The replacement can commit the access and refresh tokens as one session revision. A retry should compare the revision it read before writing.'],
          ['result', 'Design sent to T21', 'Use one revisioned session record. Retry transient provider failures without discarding a valid session; distinguish an exhausted refresh token from a temporary outage.']
        ], end: 'Finished · report committed · 2.6 spent' }]
    },
    T26: {
      title: 'Integration tests', role: 'Agent · test writer', status: 'running', spend: '2.3 / 6 spent',
      brief: 'Exercise token rotation, a transient provider failure, and final refresh failure without weakening existing sign-in tests.',
      meta: 'Asked by T21 · After T23 · Result: report · Writes tests in ai/temper',
      runs: [
        { title: 'Run 1 · parked at 10:03', meta: 'First activation · resumed from the task brief · 0.8 spent',
          events: [
            ['agent', 'I found the existing sign-in fixtures. I’ll wait for T23’s storage design before asserting the replacement behavior.'],
            ['tool', 'Inspect sign-in fixtures', 'tests/sign_in.rs · tests/session_refresh.rs · no files changed']
          ], end: 'Parked · waiting for T23 · transcript kept' },
        { title: 'Run 2 · live now', meta: 'Woken by T23’s result · resumed the transcript · 1.5 spent so far',
          events: [
            ['event', 'Input received', 'T23’s design: replace both tokens in one revisioned session record.'],
            ['agent', 'I’m adding a provider fixture that rotates its refresh token, then checking that the stored pair changes together.'],
            ['tool', 'Edit and run focused tests', 'Updated tests/session_refresh.rs · ran refresh_rotation and transient_failure · 2 passing']
          ] }
      ]
    },
    T27: {
      title: 'Memory-safety review', role: 'Agent · reviewer', status: 'held', spend: '3.1 / 5 spent',
      brief: 'Review the session refresh implementation for unsafe sharing, stale token exposure, and recovery after worker loss.',
      meta: 'Asked by T21 · Result: verdict · Reads ai/temper',
      runs: [
        { title: 'Run 1 · failed at 10:02', meta: 'First activation · started from the task brief · 1.7 spent',
          events: [['agent', 'I’m checking how a replaced token pair crosses the session boundary.'], ['tool', 'Read session boundary', 'Worker lost its checkout before the next turn committed.']], end: 'Failed · worker lost · attempt 1 of 2' },
        { title: 'Run 2 · failed at 10:14', meta: 'Retry after worker loss · resumed committed transcript · 1.4 spent',
          events: [['agent', 'The token swap needs a final check against concurrent refreshes.'], ['tool', 'Inspect refresh locking', 'Worker lost its checkout again before a verdict was committed.']], end: 'Held · out of tries · no verdict yet' }
      ]
    },
    T28: {
      title: 'Release notes', role: 'Agent · writer', status: 'waiting', spend: '0 / 3 spent',
      brief: 'Write release notes for the token refresh change after T24 lands, naming the behavior users will notice.',
      meta: 'Asked by T21 · After T24 lands · Result: report',
      runs: []
    }
  };
  const task = tasks[taskId];
  if (!task) { location.replace('task.html#plan'); return; }

  const $ = selector => document.querySelector(selector);
  const node = (tag, className, words) => {
    const element = document.createElement(tag);
    if (className) element.className = className;
    if (words !== undefined) element.textContent = words;
    return element;
  };
  const thread = $('.terminal-thread');
  const transcript = $('.agent-transcript');
  const liveSlot = $('.agent-live-slot');
  const input = $('.terminal-composer textarea');
  let state;
  let busy = false;
  let streamTimer;
  let streamStart;
  let streamPlayed = false;
  let provisional;
  let toastTimer;

  function load() {
    try { return JSON.parse(localStorage.getItem(KEY) || '{}'); }
    catch { return {}; }
  }
  state = load();
  const save = () => { try { localStorage.setItem(KEY, JSON.stringify(state)); } catch { /* Direct file previews may disable storage. */ } };
  const taskState = () => {
    const item = (state.agentTasks ||= {})[taskId] ||= {};
    item.activity ||= [];
    item.nextRun ||= 3;
    return item;
  };
  const activity = () => (taskState().activity ||= []);
  function status() {
    if (state.taskStatus === 'cancelled') return 'cancelled';
    if (taskId === 'T27') return taskState().status || (state.held === 'released' ? 'running' : 'held');
    return taskState().status || task.status;
  }
  function nearEnd() { return thread.scrollHeight - thread.scrollTop - thread.clientHeight < 100; }
  function scrollEnd() { thread.scrollTop = thread.scrollHeight; }
  function appendAndFollow(element) { const follow = nearEnd(); liveSlot.append(element); if (follow) scrollEnd(); }
  function toast(message) {
    let item = $('.terminal-toast');
    if (!item) { item = node('div', 'terminal-toast'); item.setAttribute('role', 'status'); document.body.append(item); }
    item.textContent = message;
    item.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { item.hidden = true; }, 3200);
  }
  function commit(message, change) {
    if (busy) return;
    busy = true;
    toast('Request pending…');
    setTimeout(() => { change(); save(); busy = false; renderState(); toast(message); }, 550);
  }
  function dialog(title, description, confirmText, onConfirm, withText = false) {
    const modal = node('dialog', 'terminal-dialog');
    const form = node('form');
    form.append(node('h2', '', title), node('p', '', description));
    if (withText) {
      const label = node('label', '', 'Instructions');
      label.htmlFor = 'agent-amend-text';
      const field = node('textarea');
      field.id = 'agent-amend-text';
      field.required = true;
      field.value = taskState().amendment || task.brief;
      form.append(label, field);
    }
    const actions = node('div', 'terminal-dialog-actions');
    const cancel = node('button', '', 'Cancel');
    cancel.type = 'button';
    cancel.addEventListener('click', () => modal.close());
    const confirm = node('button', '', confirmText);
    confirm.type = 'submit';
    actions.append(cancel, confirm);
    form.append(actions);
    form.addEventListener('submit', event => {
      event.preventDefault();
      const value = withText ? modal.querySelector('textarea').value.trim() : '';
      if (withText && !value) return;
      modal.close();
      onConfirm(value);
    });
    modal.append(form);
    modal.addEventListener('close', () => modal.remove(), { once: true });
    document.body.append(modal);
    modal.showModal();
    if (withText) modal.querySelector('textarea').focus();
  }

  function turn(who, words, time = '', provisionalTurn = false) {
    const section = node('section', `terminal-turn ${who === 'You' ? 'terminal-turn-user' : ''} ${provisionalTurn ? 'agent-stream' : ''}`);
    const role = node('div', 'terminal-role', who);
    if (time) role.append(node('time', '', time));
    const content = node('div', 'terminal-turn-content');
    content.append(node('span', `terminal-prompt ${who === 'You' ? '' : 'agent'}`, who === 'You' ? '›' : '✳'), node('p', '', words));
    section.append(role, content);
    return section;
  }
  function renderEvent(event) {
    const [kind, title, detail] = event;
    if (kind === 'agent') return turn('Agent', title);
    if (kind === 'tool') {
      const item = node('details', 'agent-tool');
      const summary = node('summary', '', title);
      summary.append(node('span', '', 'view work'));
      item.append(summary, node('pre', '', detail));
      return item;
    }
    if (kind === 'result') {
      const item = node('div', 'agent-result');
      item.append(node('strong', '', title.toUpperCase()), node('p', '', detail));
      return item;
    }
    const item = node('div', 'agent-event');
    item.append(node('strong', '', `${title} · `), document.createTextNode(detail));
    return item;
  }
  function renderTranscript() {
    document.title = `${taskId} · ${task.title} · temper`;
    $('.agent-lineage-current').textContent = `${taskId} ${task.title}`;
    $('.terminal-number').textContent = taskId;
    $('.terminal-title').textContent = task.title;
    $('.agent-charter').textContent = task.role;
    $('.agent-spend').textContent = task.spend;
    $('.agent-brief-text').textContent = task.brief;
    $('.agent-brief-meta').textContent = task.meta;
    if (!task.runs.length) transcript.append(node('div', 'agent-empty', 'No run yet. This task will start after T24 lands.'));
    task.runs.forEach(run => {
      const heading = node('div', 'agent-run-heading');
      heading.append(node('strong', '', run.title));
      transcript.append(heading, node('div', 'agent-run-meta', run.meta));
      run.events.forEach(event => transcript.append(renderEvent(event)));
      if (run.end) transcript.append(node('div', 'agent-run-end', run.end));
    });
    activity().forEach(appendActivity);
  }
  function appendActivity(entry) {
    if (entry.kind === 'person') {
      const item = turn('You', entry.text, entry.read ? 'Read by agent' : 'Sent · waiting to be read');
      item.dataset.messageId = entry.id;
      appendAndFollow(item);
    } else if (entry.kind === 'agent') {
      appendAndFollow(turn('Agent', entry.text, entry.time || 'Next turn'));
    } else if (entry.kind === 'run') {
      const heading = node('div', 'agent-run-heading');
      heading.append(node('strong', '', `Run ${entry.number} · live now`));
      appendAndFollow(heading);
      appendAndFollow(node('div', 'agent-run-meta', 'Released by Pat · resumed the committed transcript'));
    } else {
      appendAndFollow(node('div', 'agent-event', entry.text));
    }
  }
  function record(entry) {
    activity().push(entry);
    appendActivity(entry);
  }
  function renderState() {
    const current = status();
    const labels = { running: taskId === 'T27' ? 'Running · review restarted' : 'Running · testing refresh failures', stopped: 'Held · stopped by Pat', held: 'Held · out of tries', waiting: 'Waiting for T24', done: 'Done · report sent', cancelled: 'Cancelled by Pat' };
    const phase = $('.terminal-phase');
    phase.className = `terminal-phase ${current === 'held' || current === 'stopped' || current === 'cancelled' ? 'held' : current === 'waiting' ? 'waiting' : ''}`;
    phase.replaceChildren(node('i', 'pill-dot'), document.createTextNode(labels[current]));
    const count = [state.proposal !== 'accepted' && state.proposal !== 'rejected', !state.choice, state.held !== 'released' && state.held !== 'left' && state.taskStatus !== 'cancelled'].filter(Boolean).length;
    $('.terminal-count').textContent = count;
    $('.terminal-count').hidden = count === 0;
    $('[data-amend]').hidden = current === 'done' || current === 'cancelled';
    $('[data-stop]').hidden = current === 'done' || current === 'waiting' || current === 'cancelled';
    $('[data-stop]').textContent = current === 'held' || current === 'stopped' ? 'Release' : 'Stop';
    $('.agent-steer-area').hidden = current === 'done' || current === 'held' || current === 'stopped' || current === 'cancelled';
    $('.agent-steer-note').textContent = current === 'waiting'
      ? 'Steer this task · words are saved now and read when it runs.'
      : 'Steer this task · words reach its live run at the next turn.';
    const runHeadings = [...document.querySelectorAll('.agent-run-heading strong')];
    if (runHeadings.length && ['T26', 'T27'].includes(taskId)) {
      const laterRuns = activity().filter(entry => entry.kind === 'run');
      if (laterRuns.length) {
        runHeadings[task.runs.length - 1].textContent = `Run ${task.runs.length} · ${taskId === 'T27' ? 'held at 10:14' : 'stopped by Pat'}`;
      }
      if (laterRuns.length) {
        laterRuns.forEach((run, index) => {
          runHeadings[task.runs.length + index].textContent = `Run ${run.number} · ${index === laterRuns.length - 1 && current === 'running' ? 'live now' : 'stopped by Pat'}`;
        });
      } else if (taskId === 'T26' && current === 'stopped') {
        runHeadings.at(-1).textContent = 'Run 2 · stopped by Pat';
      }
    }
    if (current !== 'running') stopStream();
    else if (['T26', 'T27'].includes(taskId) && !streamPlayed) startStream();
  }
  function stopStream() {
    clearInterval(streamTimer);
    clearTimeout(streamStart);
    streamTimer = null;
    streamStart = null;
    if (provisional) { provisional.remove(); provisional = null; }
  }
  function startStream() {
    streamPlayed = true;
    const words = taskId === 'T27'
      ? 'I’m resuming the memory-safety review from the last committed turn. I’ll inspect concurrent refreshes before giving a verdict.'
      : 'The rotation fixture passes. I’m testing a temporary provider outage now: the old session must remain usable while a retry is scheduled.';
    const chunks = words.split(' ');
    let index = 0;
    streamStart = setTimeout(() => {
      streamStart = null;
      if (status() !== 'running') return;
      provisional = turn('Agent', '', 'Streaming · uncommitted', true);
      const paragraph = provisional.querySelector('p');
      const caret = node('span', 'agent-stream-caret');
      paragraph.append(caret);
      appendAndFollow(provisional);
      streamTimer = setInterval(() => {
        if (status() !== 'running') { stopStream(); return; }
        const follow = nearEnd();
        paragraph.insertBefore(document.createTextNode(`${chunks[index++]} `), caret);
        if (follow) scrollEnd();
        if (index === chunks.length) {
          clearInterval(streamTimer);
          streamTimer = null;
          caret.remove();
          provisional.classList.remove('agent-stream');
          provisional.querySelector('time').textContent = 'Turn committed';
          provisional = null;
          appendAndFollow(node('div', 'agent-run-end', 'Turn committed · the agent is continuing its run.'));
        }
      }, 190);
    }, 650);
  }
  function autosize() {
    input.style.height = '22px';
    input.style.height = `${Math.min(Math.max(input.scrollHeight, 22), 160)}px`;
    input.style.overflowY = input.scrollHeight > 160 ? 'auto' : 'hidden';
  }
  function sendWords() {
    const words = input.value.trim();
    if (!words || !['running', 'waiting'].includes(status())) return;
    commit('Words sent to this task.', () => {
      const message = { kind: 'person', id: String(Date.now()), text: words, read: false };
      record(message);
      input.value = '';
      autosize();
      if (status() === 'running') setTimeout(() => {
        if (status() !== 'running') return;
        message.read = true;
        save();
        const item = liveSlot.querySelector(`[data-message-id="${message.id}"]`);
        if (item) item.querySelector('time').textContent = 'Read by agent';
        record({ kind: 'agent', text: 'I have your guidance. I’ll apply it at the next step of this run.' });
        save();
      }, 1600);
    });
  }
  input.addEventListener('input', autosize);
  input.addEventListener('keydown', event => { if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); sendWords(); } });
  $('.terminal-send').addEventListener('click', sendWords);
  $('[data-amend]').addEventListener('click', () => dialog(
    `Amend ${taskId}`, 'The agent will hear the new instructions. A live run reads them at its next turn.', 'Amend task',
    text => commit('Task amended.', () => { taskState().amendment = text; record({ kind: 'event', text: `Instructions amended by Pat · ${text}` }); }), true
  ));
  $('[data-stop]').addEventListener('click', () => {
    const release = ['held', 'stopped'].includes(status());
    dialog(release ? `Release ${taskId}?` : `Stop ${taskId}?`,
      release ? 'The task starts a new run from its committed transcript. For T27, its tries begin again at zero.'
        : 'Its live run stops now. Committed turns and saved work remain; the task is held for you.',
      release ? 'Release task' : 'Stop task', () => commit(release ? 'Task released.' : 'Task stopped.', () => {
        const wasStreaming = !!provisional;
        if (taskId === 'T27' && release) state.held = 'released';
        taskState().status = release ? 'running' : 'stopped';
        if (release) streamPlayed = false;
        record({ kind: 'event', text: release ? `Pat released ${taskId}. A new run is starting.` : `Pat stopped ${taskId}. The task is held.` });
        if (wasStreaming && !release) record({ kind: 'event', text: 'Uncommitted live text was discarded when the run stopped.' });
        if (release) record({ kind: 'run', number: taskState().nextRun++ });
      })
    );
  });
  addEventListener('storage', event => { if (event.key === KEY) { state = load(); renderState(); } });
  renderTranscript();
  renderState();
  scrollEnd();
})();
