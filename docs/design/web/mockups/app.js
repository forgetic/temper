// A small, browser-only state layer for the visual prototype. No request leaves
// the page. The short pending state stands in for a durable engine decision.
(() => {
  'use strict';

  const KEY = 'temper-web-mockup-v1';
  const page = location.pathname.split('/').pop() || 'index.html';
  const $ = (selector, root = document) => root.querySelector(selector);
  const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
  const byText = (text, root = document) => $$('button', root).find(button => button.textContent.trim().includes(text));
  const empty = (parent, text) => {
    let node = $('.empty-state', parent);
    if (!node) {
      node = document.createElement('div');
      node.className = 'empty-state';
      parent.append(node);
    }
    node.textContent = text;
    return node;
  };
  const escapeHtml = value => String(value).replace(/[&<>"']/g, char => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'
  })[char]);

  function load() {
    try {
      return {
        proposal: 'pending', rejectReason: '', choice: null, held: 'pending',
        heldReason: '', messages: {}, chats: [], chatStatus: {}, taskStatus: 'running',
        taskAmend: '', goals: [],
        ...JSON.parse(localStorage.getItem(KEY) || '{}')
      };
    } catch {
      return { proposal: 'pending', rejectReason: '', choice: null, held: 'pending',
        heldReason: '', messages: {}, chats: [], chatStatus: {}, taskStatus: 'running',
        taskAmend: '', goals: [] };
    }
  }
  let state = load();
  let busy = false;
  let toastTimer;

  function save() {
    try { localStorage.setItem(KEY, JSON.stringify(state)); } catch { /* File previews may disable storage. */ }
  }
  function toast(message) {
    let node = $('.demo-toast');
    if (!node) {
      node = document.createElement('div');
      node.className = 'demo-toast';
      node.setAttribute('role', 'status');
      document.body.append(node);
    }
    node.textContent = message;
    node.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { node.hidden = true; }, 3600);
  }
  function commit(message, change, after) {
    if (busy) return;
    busy = true;
    toast('Request pending…');
    setTimeout(() => {
      change();
      save();
      busy = false;
      render();
      toast(message);
      after?.();
    }, 550);
  }

  function dialog({ title, description, fields = '', confirm = 'Continue', danger = false, onConfirm }) {
    const node = document.createElement('dialog');
    node.className = 'demo-dialog';
    node.innerHTML = `<form><h2>${title}</h2><p>${description}</p>${fields}<div class="button-row"><button class="button" type="button" data-cancel>Cancel</button><button class="button ${danger ? 'danger' : 'primary'}" type="submit">${confirm}</button></div></form>`;
    document.body.append(node);
    $('[data-cancel]', node).addEventListener('click', () => node.close());
    $('form', node).addEventListener('submit', event => {
      event.preventDefault();
      if (onConfirm(node) !== false) node.close();
    });
    node.addEventListener('close', () => node.remove(), { once: true });
    node.showModal();
    $('input, textarea', node)?.focus();
  }

  function decideProposal(accept) {
    if (state.proposal !== 'pending') return;
    if (accept) {
      dialog({
        title: 'Accept this goal?',
        description: 'T1 becomes yours, reserves 40 from your pool, and starts a coordinator. Its changes may land into main in ai/temper.',
        confirm: 'Accept goal',
        onConfirm: () => commit('Goal accepted. T1 is on the board.', () => { state.proposal = 'accepted'; })
      });
    } else {
      dialog({
        title: 'Reject this proposal',
        description: 'Tell T0 why, so it can change course or propose again.',
        fields: '<label for="reject-reason">Reason</label><textarea class="field" id="reject-reason" required></textarea>',
        confirm: 'Reject proposal', danger: true,
        onConfirm: node => {
          const reason = $('#reject-reason', node).value.trim();
          if (!reason) return false;
          commit('Proposal rejected. Your reason was sent to T0.', () => {
            state.proposal = 'rejected'; state.rejectReason = reason;
          });
        }
      });
    }
  }

  function renderNav() {
    const count = [state.proposal === 'pending', !state.choice,
      state.held === 'pending' && state.taskStatus !== 'cancelled'].filter(Boolean).length;
    $$('.sidebar .count').forEach(node => { node.textContent = count; node.hidden = count === 0; });
  }

  let inboxKind = 'all';
  let inboxGoal = 'all';
  function renderInbox() {
    const cards = $$('.inbox-card');
    if (!cards.length) return;
    const items = [
      { node: cards[0], kind: 'proposal', goal: 'chat', pending: state.proposal === 'pending' },
      { node: cards[1], kind: 'choice', goal: 'T12', pending: !state.choice },
      { node: cards[2], kind: 'held', goal: 'T21', pending: state.held === 'pending' && state.taskStatus !== 'cancelled' }
    ];
    let shown = 0;
    items.forEach(item => {
      item.node.hidden = !(item.pending && (inboxKind === 'all' || inboxKind === item.kind)
        && (inboxGoal === 'all' || inboxGoal === item.goal));
      if (!item.node.hidden) shown++;
    });
    $('.inbox-split .subtle-count').textContent = items.filter(item => item.pending).length;
    empty(cards[0].parentElement, 'Nothing needs you in this view.').hidden = shown > 0;
    const update = $('.updates-list');
    let decision = $('[data-decision-update]', update);
    if (state.proposal !== 'pending') {
      if (!decision) {
        decision = document.createElement('div');
        decision.className = 'card update';
        decision.dataset.decisionUpdate = '';
        update.prepend(decision);
      }
      decision.textContent = state.proposal === 'accepted'
        ? '✓  T1 Add OAuth login was accepted and is now on the board.'
        : '↳  The OAuth login proposal was rejected with your reason.';
    } else decision?.remove();
  }
  function bindInbox() {
    byText('Accept goal')?.addEventListener('click', () => decideProposal(true));
    byText('Reject with reason')?.addEventListener('click', () => decideProposal(false));
    byText('Answer choice')?.addEventListener('click', () => dialog({
      title: 'Choose an approach',
      description: 'Your choice becomes the result of T16 and reaches the tasks waiting on it.',
      fields: '<label class="choice-option"><input type="radio" name="approach" value="Inline guidance" required> Inline guidance</label><label class="choice-option"><input type="radio" name="approach" value="Summary banner"> Summary banner</label><label for="choice-why">Why</label><textarea class="field" id="choice-why" required></textarea>',
      confirm: 'Answer choice',
      onConfirm: node => {
        const option = $('input[name="approach"]:checked', node)?.value;
        const why = $('#choice-why', node).value.trim();
        if (!option || !why) return false;
        commit('Choice answered. The coordinator can continue.', () => { state.choice = { option, why }; });
      }
    }));
    byText('Release task')?.addEventListener('click', () => dialog({
      title: 'Release T27?',
      description: 'The memory-safety review starts a new run. Its tries count begins again at zero.',
      confirm: 'Release task',
      onConfirm: () => commit('T27 was released and is starting.', () => { state.held = 'released'; })
    }));
    byText('Leave held')?.addEventListener('click', () => dialog({
      title: 'Leave T27 held',
      description: 'It remains held on the goal page and leaves your inbox. Give the coordinator a reason.',
      fields: '<label for="held-reason">Reason</label><textarea class="field" id="held-reason" required></textarea>',
      confirm: 'Leave held',
      onConfirm: node => {
        const reason = $('#held-reason', node).value.trim();
        if (!reason) return false;
        commit('T27 remains held. Your reason was recorded.', () => {
          state.held = 'left'; state.heldReason = reason;
        });
      }
    }));
    const filters = $$('.inbox-split .filter-row .filter');
    filters[0]?.addEventListener('click', () => {
      filters[0].textContent = filters[0].textContent === 'All projects' ? 'temper' : 'All projects';
      toast('Showing the temper project.');
    });
    filters[1]?.addEventListener('click', () => {
      const values = ['all', 'proposal', 'choice', 'held'];
      inboxKind = values[(values.indexOf(inboxKind) + 1) % values.length];
      filters[1].textContent = inboxKind === 'all' ? 'Kind ⌄' : `${inboxKind[0].toUpperCase()}${inboxKind.slice(1)} ⌄`;
      renderInbox();
    });
    filters[2]?.addEventListener('click', () => {
      const values = ['all', 'T21', 'T12', 'chat'];
      inboxGoal = values[(values.indexOf(inboxGoal) + 1) % values.length];
      filters[2].textContent = inboxGoal === 'all' ? 'Goal ⌄' : `${inboxGoal === 'chat' ? 'Chat T0' : inboxGoal} ⌄`;
      renderInbox();
    });
  }

  const chatId = new URLSearchParams(location.search).get('chat') || 'T0';
  function renderChat() {
    const conversation = $('.conversation');
    if (!conversation) return;
    const newChat = state.chats.find(chat => chat.id === chatId);
    if (newChat && !conversation.dataset.dynamic) {
      conversation.dataset.dynamic = 'true';
      conversation.replaceChildren();
      const first = document.createElement('div');
      first.className = 'message user';
      const bubble = document.createElement('div');
      bubble.className = 'user-bubble';
      bubble.textContent = newChat.words;
      first.append(bubble);
      conversation.append(first);
      $('.chat-title').textContent = newChat.title;
      $('.breadcrumbs strong').textContent = newChat.title;
      $('.chat-head .eyebrow').textContent = `Chat · ${chatId}`;
      $('.composer textarea').placeholder = `Message ${chatId}…`;
      $('.composer textarea').setAttribute('aria-label', `Message ${chatId}`);
      $('.chat-aside .aside-panel .kv strong').textContent = `${chatId} · Agent`;
      const proposed = $$('.chat-aside .aside-section')[1];
      proposed.querySelector('.aside-list').textContent = 'Nothing proposed yet.';
      $('.chat-aside .aside-panel .kv:nth-child(3) strong').textContent = '0 / 8';
      $('.chat-aside .progress > span').style.width = '0%';
      $('.chat-aside .mini-stat').textContent = '0 spent · 8 left';
    }
    if (!newChat && chatId !== 'T0') {
      location.replace('chats.html');
      return;
    }
    if (chatId === 'T0') {
      const card = $('.inline-card');
      const tag = $('.tag', card);
      const row = $('.button-row', card);
      let note = $('.decision-note', card);
      tag.textContent = state.proposal === 'pending' ? 'Proposal'
        : state.proposal === 'accepted' ? 'Accepted' : 'Rejected';
      tag.className = `tag ${state.proposal === 'accepted' ? 'green' : state.proposal === 'rejected' ? 'red' : 'accent'}`;
      row.hidden = state.proposal !== 'pending';
      if (state.proposal !== 'pending') {
        if (!note) { note = document.createElement('div'); note.className = 'decision-note'; card.append(note); }
        note.textContent = state.proposal === 'accepted'
          ? 'Pat accepted this goal. T1 is now on the board and belongs to Pat.'
          : `Pat rejected this proposal: “${state.rejectReason}”`;
        $('.divider-label').textContent = state.proposal === 'accepted' ? 'T1 was created' : 'Proposal rejected';
      } else {
        note?.remove();
        $('.divider-label').textContent = 'T0 is waiting for your decision';
      }
      const proposedStatus = $('.chat-aside .aside-section:nth-child(2) small');
      if (proposedStatus) proposedStatus.textContent = state.proposal === 'pending'
        ? 'Waiting for your acceptance' : state.proposal === 'accepted' ? 'Accepted · now yours' : 'Rejected';
    }
    let messages = $('[data-demo-messages]', conversation);
    if (!messages) {
      messages = document.createElement('div');
      messages.dataset.demoMessages = '';
      conversation.append(messages);
    }
    messages.replaceChildren();
    (state.messages[chatId] || []).forEach(words => {
      const message = document.createElement('div');
      message.className = 'message user';
      const wrapper = document.createElement('div');
      const bubble = document.createElement('div');
      bubble.className = 'user-bubble';
      bubble.textContent = words;
      const time = document.createElement('div');
      time.className = 'message-time';
      time.style.textAlign = 'right';
      time.textContent = 'Sent · awaiting the next turn';
      wrapper.append(bubble, time);
      message.append(wrapper);
      messages.append(message);
    });
    const status = state.chatStatus[chatId] || (newChat ? 'starting' : 'waiting');
    const tag = $('.chat-head-bottom .tag');
    tag.textContent = { waiting: 'Waiting for you', starting: 'Starting', stopped: 'Held · stopped', closed: 'Closed' }[status];
    tag.className = `tag ${status === 'closed' ? '' : status === 'stopped' ? 'red' : 'green'}`;
    const composer = $('.composer-wrap');
    composer.hidden = status === 'closed';
    const stop = byText('Stop', $('.chat-head-bottom')) || byText('Release', $('.chat-head-bottom'));
    stop.textContent = status === 'stopped' ? 'Release' : 'Stop';
    stop.hidden = status === 'closed';
    byText('Close', $('.chat-head-bottom')).hidden = status === 'closed';
    conversation.scrollTop = conversation.scrollHeight;
  }
  function bindChat() {
    const composer = $('.composer-wrap');
    const input = $('textarea', composer);
    const send = () => {
      const words = input.value.trim();
      if (!words) return;
      commit('Words sent. The chat will read them at its next turn.', () => {
        (state.messages[chatId] ||= []).push(words);
        if (state.chatStatus[chatId] === 'stopped') state.chatStatus[chatId] = 'waiting';
        input.value = '';
      });
    };
    $('.send-button', composer).addEventListener('click', send);
    input.addEventListener('keydown', event => {
      if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); send(); }
    });
    byText('Accept goal', $('.inline-card'))?.addEventListener('click', () => decideProposal(true));
    byText('Reject with reason', $('.inline-card'))?.addEventListener('click', () => decideProposal(false));
    const controls = $('.chat-head-bottom');
    const stop = byText('Stop', controls);
    stop.addEventListener('click', () => {
      const held = state.chatStatus[chatId] === 'stopped';
      dialog({
        title: held ? 'Release this chat?' : 'Stop this chat?',
        description: held ? 'Its next run resumes from the committed conversation.'
          : 'Its current run stops and the chat is held. Delegates continue.',
        confirm: held ? 'Release chat' : 'Stop chat',
        onConfirm: () => commit(held ? 'Chat released.' : 'Chat stopped and held.', () => {
          state.chatStatus[chatId] = held ? 'waiting' : 'stopped';
        })
      });
    });
    byText('Close', controls).addEventListener('click', () => dialog({
      title: 'Close this chat?',
      description: 'The chat is asked to finish. Its conversation stays readable. This sample has no live delegates to hand over.',
      confirm: 'Close chat',
      onConfirm: () => commit('Chat closed.', () => { state.chatStatus[chatId] = 'closed'; })
    }));
  }

  let chatsFilter = 'live';
  function renderChats() {
    const list = $('.chat-list');
    if (!list) return;
    $$('[data-new-chat]', list).forEach(node => node.remove());
    state.chats.slice().reverse().forEach(chat => {
      const link = document.createElement('a');
      link.className = 'card chat-preview';
      link.dataset.newChat = chat.id;
      link.href = `index.html?chat=${encodeURIComponent(chat.id)}`;
      link.innerHTML = `<div><div class="meta-row"><span class="tag green">${state.chatStatus[chat.id] === 'closed' ? 'Closed' : 'Starting'}</span><span>temper · ${chat.id}</span></div><h3>${escapeHtml(chat.title)}</h3><p>${escapeHtml(chat.words)}</p></div><div class="meta-row"><span>0 spent · no delegates</span><span>Just now</span></div>`;
      list.prepend(link);
    });
    const t0 = $('.chat-preview[href="index.html"]');
    if (t0 && state.proposal !== 'pending') {
      $('p', t0).textContent = state.proposal === 'accepted' ? 'The OAuth goal was accepted.' : 'The goal proposal was rejected.';
    }
    const cards = $$('.chat-preview', list);
    let shown = 0;
    cards.forEach(card => {
      const id = card.dataset.newChat || (card === t0 ? 'T0' : null);
      const closed = id && state.chatStatus[id] === 'closed';
      card.hidden = chatsFilter === 'closed' ? !closed : !!closed;
      if (!card.hidden) shown++;
    });
    empty(list, 'No chats in this view.').hidden = shown > 0;
  }
  function bindChats() {
    const input = $('.welcome textarea');
    const start = () => {
      const words = input.value.trim();
      if (!words) { input.focus(); return; }
      const id = `T${62 + state.chats.length}`;
      commit('Chat created. Its first run is starting.', () => {
        state.chats.push({ id, title: words.split(/\n/)[0].slice(0, 58), words });
      }, () => { location.href = `index.html?chat=${id}`; });
    };
    $('.welcome .send-button').addEventListener('click', start);
    input.addEventListener('keydown', event => {
      if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); start(); }
    });
    const filters = $$('.section-heading .filter', $('.page.narrow'));
    filters.slice(0, 2).forEach((button, index) => button.addEventListener('click', () => {
      chatsFilter = index === 0 ? 'live' : 'closed';
      filters.slice(0, 2).forEach(item => item.classList.toggle('active', item === button));
      renderChats();
    }));
    filters[2]?.addEventListener('click', () => {
      filters[2].textContent = filters[2].textContent.includes('All') ? 'temper ⌄' : 'All projects ⌄';
    });
    $$('.chat-preview[href="index.html"]').slice(1).forEach(link => link.addEventListener('click', event => {
      event.preventDefault();
      toast('This chat is sample data; T0 has the conversation mockup.');
    }));
  }

  let boardFilter = 'live';
  function renderBoard() {
    const stack = $('.section-heading + .stack');
    if (!stack || page !== 'board.html') return;
    $$('[data-demo-goal]', stack).forEach(node => node.remove());
    if (state.proposal === 'accepted') {
      const row = document.createElement('article');
      row.className = 'card board-row';
      row.dataset.demoGoal = 'T1';
      row.innerHTML = '<div class="rank"></div><div><div class="meta-row"><span class="tag accent">Starting</span><span class="tiny">T1 · Goal</span></div><h3 style="margin-top:7px">Add OAuth login</h3><p>Coordinator starting · accepted from chat T0</p></div><div><div class="progress"><span style="width:0%"></span></div><p>0 of 40 spent · no deadline</p></div><div class="right"><a class="text-link" href="index.html">View chat ↗</a></div>';
      stack.children[1]?.before(row);
    }
    state.goals.forEach(goal => {
      const row = document.createElement('article');
      row.className = 'card board-row';
      row.dataset.demoGoal = goal.id;
      row.innerHTML = `<div class="rank"></div><div><div class="meta-row"><span class="tag accent">Starting</span><span class="tiny">${goal.id} · Goal</span></div><h3 style="margin-top:7px"></h3><p>Coordinator starting · set by Pat</p></div><div><div class="progress"><span style="width:0%"></span></div><p>0 of ${escapeHtml(goal.budget)} spent · no deadline</p></div><div class="right"><span class="muted tiny">Just added</span></div>`;
      $('h3', row).textContent = goal.title;
      const waitingGoal = $$('.board-row', stack).find(item => item.textContent.includes('T34'));
      waitingGoal?.before(row);
    });
    const rows = $$('.board-row', stack);
    rows.forEach((row, index) => {
      $('.rank', row).textContent = index + 1;
      let status = 'live';
      if (row.textContent.includes('T34')) status = 'waiting';
      if (row.textContent.includes('T21') && state.taskStatus === 'cancelled') status = 'ended';
      if (row.textContent.includes('T21') && state.taskStatus === 'stopped') status = 'waiting';
      row.hidden = status !== boardFilter;
      if (row.textContent.includes('T21')) {
        const tag = $('.tag', row);
        if (state.taskStatus === 'cancelled') { tag.textContent = 'Cancelled'; tag.className = 'tag red'; }
        else if (state.taskStatus === 'stopped') { tag.textContent = 'Held · stopped'; tag.className = 'tag red'; }
        else { tag.textContent = 'Running'; tag.className = 'tag green'; }
        $('p', row).textContent = state.held === 'released'
          ? '3 changes landed · 2 in flight' : '3 changes landed · 2 in flight · 1 held below';
      }
    });
    const liveGoals = rows.filter(row => !row.textContent.includes('T34') && !(row.textContent.includes('T21') && state.taskStatus === 'cancelled')).length;
    $('.stats-row .stat-card .value').textContent = liveGoals;
    if (state.taskStatus === 'cancelled') {
      $$('.stats-row .stat-card .value')[1].textContent = '5';
      $$('.stats-row .stat-card small')[1].textContent = '1 ready to land';
    }
    empty(stack, 'No goals in this view.').hidden = rows.some(row => !row.hidden);
  }
  function bindBoard() {
    const filters = $$('.section-heading .filter');
    filters.slice(0, 3).forEach((button, index) => button.addEventListener('click', () => {
      boardFilter = ['live', 'waiting', 'ended'][index];
      filters.slice(0, 3).forEach(item => item.classList.toggle('active', item === button));
      renderBoard();
    }));
    byText('Set a goal')?.addEventListener('click', () => dialog({
      title: 'Set a goal',
      description: 'A coordinator starts with this goal, funded from your project pool.',
      fields: '<label for="goal-title">What should it do?</label><textarea class="field" id="goal-title" required></textarea><label for="goal-budget">Budget</label><input class="field" id="goal-budget" type="number" min="1" max="120" value="20" required>',
      confirm: 'Set goal',
      onConfirm: node => {
        const title = $('#goal-title', node).value.trim();
        const budget = Number($('#goal-budget', node).value);
        if (!title || !Number.isFinite(budget) || budget < 1 || budget > 120) return false;
        commit('Goal added to the board.', () => {
          state.goals.push({ id: `T${70 + state.goals.length}`, title, budget });
        });
      }
    }));
  }

  let heldOnly = false;
  function renderTask() {
    if (page !== 'task.html') return;
    const status = $('.task-heading .meta-row:last-child .tag');
    status.textContent = {
      running: 'Running · planning next change',
      stopped: 'Held · stopped by Pat',
      cancelled: 'Cancelled by Pat'
    }[state.taskStatus];
    status.className = `tag ${state.taskStatus === 'running' ? 'green' : 'red'}`;
    const controls = $('.task-toolbar');
    const stop = byText('Stop', controls) || byText('Release', controls);
    stop.textContent = state.taskStatus === 'stopped' ? 'Release' : 'Stop';
    stop.disabled = state.taskStatus === 'cancelled';
    byText('Amend', controls).disabled = state.taskStatus === 'cancelled';
    byText('Cancel', controls).disabled = state.taskStatus === 'cancelled';
    if (state.taskAmend) {
      $('.task-heading .intro').textContent = state.taskAmend;
      $('#overview > p').textContent = state.taskAmend;
    }
    const t27 = $$('.plan-row').find(row => row.textContent.includes('T27'));
    if (t27) {
      const phase = $('.phase', t27);
      phase.textContent = state.taskStatus === 'cancelled' ? 'Cancelled'
        : state.held === 'released' ? 'Starting · tries reset' : 'Held · out of tries';
      phase.className = `phase ${state.taskStatus === 'cancelled' ? '' : state.held === 'released' ? 'good' : 'held'}`;
    }
    const notice = $('.notice.red');
    notice.hidden = state.held === 'released' || state.taskStatus === 'cancelled';
    if (state.held === 'left') notice.querySelector('div').textContent = `T27 remains held. Pat's reason: ${state.heldReason}`;
    const heldCount = $$('.side-card .kv').find(row => row.textContent.includes('Held below'));
    if (heldCount) $('strong', heldCount).textContent = state.held === 'released' || state.taskStatus === 'cancelled' ? '0 tasks' : '1 task';
    const rows = $$('.plan-row');
    rows.forEach(row => {
      row.hidden = heldOnly && !row.textContent.includes('T27');
      if (state.taskStatus === 'cancelled' && !$('.phase.good', row)) $('.phase', row).textContent = 'Cancelled';
    });
  }
  function bindTask() {
    const controls = $('.task-toolbar');
    byText('Amend', controls).addEventListener('click', () => dialog({
      title: 'Amend T21',
      description: 'The coordinator will see the new instructions at its next turn.',
      fields: `<label for="task-amend">Instructions</label><textarea class="field" id="task-amend" required>${escapeHtml(state.taskAmend || $('.task-heading .intro').textContent)}</textarea>`,
      confirm: 'Amend task',
      onConfirm: node => {
        const words = $('#task-amend', node).value.trim();
        if (!words) return false;
        commit('T21 was amended.', () => { state.taskAmend = words; });
      }
    }));
    byText('Stop', controls).addEventListener('click', () => {
      const stopped = state.taskStatus === 'stopped';
      dialog({
        title: stopped ? 'Release T21?' : 'Stop T21?',
        description: stopped ? 'The coordinator resumes from the committed plan.'
          : 'Its live run stops. Delegates continue, and the goal is held for you.',
        confirm: stopped ? 'Release goal' : 'Stop goal',
        onConfirm: () => commit(stopped ? 'T21 was released.' : 'T21 was stopped and held.', () => {
          state.taskStatus = stopped ? 'running' : 'stopped';
        })
      });
    });
    byText('Cancel', controls).addEventListener('click', () => dialog({
      title: 'Cancel T21 and its plan?',
      description: 'This ends the goal and the seven tasks below it. Live runs stop and unlanded changes are withdrawn.',
      confirm: 'Cancel goal', danger: true,
      onConfirm: () => commit('T21 and its plan were cancelled in this demo.', () => { state.taskStatus = 'cancelled'; })
    }));
    byText('Held only', $('#plan')).addEventListener('click', event => {
      heldOnly = !heldOnly;
      event.currentTarget.classList.toggle('active', heldOnly);
      event.currentTarget.textContent = heldOnly ? 'Show all' : 'Held only ⌄';
      renderTask();
    });
    byText('Whole subtree', $('#history')).addEventListener('click', event => {
      const whole = event.currentTarget.textContent.includes('Whole');
      event.currentTarget.textContent = whole ? 'Task only ⌄' : 'Whole subtree ⌄';
      $$('#history .event').slice(0, 2).forEach(row => { row.hidden = whole; });
    });
  }

  function renderChanges() {
    if (page !== 'changes.html') return;
    if (state.proposal === 'accepted') {
      const next = $$('#queue .queue-row')[1];
      $('p', next).textContent = $('p', next).textContent.replace('priority 2', 'priority 3');
    }
    if (state.taskStatus !== 'cancelled') return;
    const queue = $$('#queue .queue-row');
    queue[0].hidden = true;
    $('.queue-no', queue[1]).textContent = '1';
    $('#queue .card-top .tag').textContent = '1 ready';
    $$('#in-flight .event')[0].hidden = true;
    $('#in-flight .card-top .muted').textContent = '2 changes';
    $('#change-detail .tag').textContent = 'Cancelled · T24';
    $('#change-detail .tag').className = 'tag red';
    $('#change-detail .notice').innerHTML = '<span>○</span><div><strong>Change withdrawn.</strong> T21 was cancelled before T24 merged.</div>';
    $$('#change-detail .step').forEach(step => step.classList.remove('current'));
  }

  function bindCommon() {
    $$('.tabbar a').forEach(link => link.addEventListener('click', () => {
      $$('.tabbar a').forEach(item => item.classList.toggle('active', item === link));
    }));
    $$('button[aria-label="Search"]').forEach(button => button.addEventListener('click', () => dialog({
      title: 'Go to a task',
      description: 'Task numbers open the corresponding representative mockup.',
      fields: '<label for="task-number">Task number</label><input class="field" id="task-number" placeholder="T0, T21 or T24" required>',
      confirm: 'Open task',
      onConfirm: node => {
        const number = $('#task-number', node).value.trim().toUpperCase();
        const route = { T0: 'index.html', T21: 'task.html', T24: 'changes.html' }[number];
        if (!route) { toast('This task has no dedicated mockup page. Try T0, T21 or T24.'); return false; }
        location.href = route;
      }
    })));
    $$('button[aria-label^="More"]').forEach(button => button.addEventListener('click', () => dialog({
      title: 'Prototype options',
      description: 'The sample decisions and chats are stored only in this browser.',
      confirm: 'Reset demo data', danger: true,
      onConfirm: () => {
        try { localStorage.removeItem(KEY); } catch { /* no storage available */ }
        if (page === 'index.html' && chatId !== 'T0') location.href = 'chats.html';
        else location.reload();
      }
    })));
    addEventListener('storage', event => {
      if (event.key === KEY) { state = load(); render(); }
    });
  }
  function render() {
    renderNav();
    renderInbox();
    renderChats();
    renderChat();
    renderBoard();
    renderTask();
    renderChanges();
  }

  bindCommon();
  if (page === 'inbox.html') bindInbox();
  if (page === 'chats.html') bindChats();
  if (page === 'index.html') bindChat();
  if (page === 'board.html') bindBoard();
  if (page === 'task.html') bindTask();
  render();
  document.documentElement.dataset.demoReady = 'true';
})();
