(() => {
  'use strict';

  const KEY = 'temper-web-mockup-v1';
  const chatId = new URLSearchParams(location.search).get('chat') || 'T0';
  const $ = (selector, root = document) => root.querySelector(selector);
  const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
  const input = $('.terminal-composer textarea');
  const thread = $('.terminal-thread');
  let busy = false;
  let toastTimer;

  function load() {
    try {
      return {
        proposal: 'pending', rejectReason: '', choice: null, held: 'pending',
        messages: {}, chats: [], chatStatus: {}, taskStatus: 'running',
        ...JSON.parse(localStorage.getItem(KEY) || '{}')
      };
    } catch {
      return { proposal: 'pending', rejectReason: '', choice: null, held: 'pending',
        messages: {}, chats: [], chatStatus: {}, taskStatus: 'running' };
    }
  }
  let state = load();
  const save = () => {
    try { localStorage.setItem(KEY, JSON.stringify(state)); } catch { /* Direct file previews may disable storage. */ }
  };

  function toast(message) {
    let node = $('.terminal-toast');
    if (!node) {
      node = document.createElement('div');
      node.className = 'terminal-toast';
      node.setAttribute('role', 'status');
      document.body.append(node);
    }
    node.textContent = message;
    node.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { node.hidden = true; }, 3200);
  }
  function commit(message, change) {
    if (busy) return;
    busy = true;
    toast('Request pending…');
    setTimeout(() => {
      change();
      save();
      busy = false;
      render();
      toast(message);
    }, 550);
  }
  function dialog(title, description, confirmText, onConfirm, askReason = false) {
    const node = document.createElement('dialog');
    node.className = 'terminal-dialog';
    const form = document.createElement('form');
    const heading = document.createElement('h2');
    heading.textContent = title;
    const words = document.createElement('p');
    words.textContent = description;
    form.append(heading, words);
    if (askReason) {
      const label = document.createElement('label');
      label.htmlFor = 'terminal-reason';
      label.textContent = 'Reason';
      const textarea = document.createElement('textarea');
      textarea.id = 'terminal-reason';
      textarea.required = true;
      form.append(label, textarea);
    }
    const actions = document.createElement('div');
    actions.className = 'terminal-dialog-actions';
    const cancel = document.createElement('button');
    cancel.type = 'button';
    cancel.textContent = 'Cancel';
    cancel.addEventListener('click', () => node.close());
    const confirm = document.createElement('button');
    confirm.type = 'submit';
    confirm.textContent = confirmText;
    actions.append(cancel, confirm);
    form.append(actions);
    form.addEventListener('submit', event => {
      event.preventDefault();
      const reason = askReason ? $('#terminal-reason', node).value.trim() : '';
      if (askReason && !reason) return;
      node.close();
      onConfirm(reason);
    });
    node.append(form);
    node.addEventListener('close', () => node.remove(), { once: true });
    document.body.append(node);
    node.showModal();
    if (askReason) $('#terminal-reason', node).focus();
  }

  function autosize() {
    input.style.height = '22px';
    const height = Math.min(Math.max(input.scrollHeight, 22), 160);
    input.style.height = `${height}px`;
    input.style.overflowY = input.scrollHeight > 160 ? 'auto' : 'hidden';
  }
  function addTurn(words, label = 'You', time = 'Sent · awaiting next turn') {
    const section = document.createElement('section');
    section.className = 'terminal-turn terminal-turn-user';
    const role = document.createElement('div');
    role.className = 'terminal-role';
    role.append(document.createTextNode(label + ' '));
    const clock = document.createElement('time');
    clock.textContent = time;
    role.append(clock);
    const content = document.createElement('div');
    content.className = 'terminal-turn-content';
    const prompt = document.createElement('span');
    prompt.className = 'terminal-prompt';
    prompt.textContent = '›';
    const paragraph = document.createElement('p');
    paragraph.textContent = words;
    content.append(prompt, paragraph);
    section.append(role, content);
    return section;
  }

  function render() {
    const newChat = state.chats.find(chat => chat.id === chatId);
    if (!newChat && chatId !== 'T0') { location.replace('chats.html'); return; }
    $$('.terminal-count').forEach(count => {
      const pending = [state.proposal === 'pending', !state.choice,
        state.held === 'pending' && state.taskStatus !== 'cancelled'].filter(Boolean).length;
      count.textContent = pending;
      count.hidden = pending === 0;
    });

    if (newChat) {
      $('.terminal-number').textContent = chatId;
      $('.terminal-title').textContent = newChat.title;
      document.title = `${newChat.title} · temper`;
      $('.terminal-seed').hidden = true;
      $('.terminal-chat-meta span:nth-child(2)').textContent = '0 / 8 spent';
      input.placeholder = `Message ${chatId}…`;
      input.setAttribute('aria-label', `Message ${chatId}`);
      $('.terminal-added-messages').replaceChildren(addTurn(newChat.words, 'You', 'First words'));
    } else {
      const proposal = $('.terminal-proposal');
      const type = $('.terminal-proposal-type', proposal);
      const result = $('.terminal-proposal-result', proposal);
      const actions = $('.terminal-proposal-actions', proposal);
      type.textContent = state.proposal === 'pending' ? 'PROPOSAL'
        : state.proposal === 'accepted' ? 'ACCEPTED' : 'REJECTED';
      $('.terminal-proposal-wait', proposal).textContent = state.proposal === 'pending' ? 'waiting 4 min' : 'decided';
      actions.hidden = state.proposal !== 'pending';
      result.hidden = state.proposal === 'pending';
      result.classList.toggle('rejected', state.proposal === 'rejected');
      result.textContent = state.proposal === 'accepted'
        ? 'Pat accepted this goal. T1 now belongs to Pat and appears on the board.'
        : state.proposal === 'rejected' ? `Pat rejected the proposal: “${state.rejectReason}”` : '';
      $('.terminal-added-messages').replaceChildren();
    }
    const added = $('.terminal-added-messages');
    (state.messages[chatId] || []).forEach(words => added.append(addTurn(words)));

    const status = state.chatStatus[chatId] || (newChat ? 'starting' : 'waiting');
    const phase = $('.terminal-phase');
    phase.className = `terminal-phase ${status === 'stopped' ? 'held' : status === 'closed' ? 'closed' : ''}`;
    phase.innerHTML = '<i class="pill-dot"></i>';
    phase.append(document.createTextNode({
      waiting: 'Waiting for you', starting: 'Starting', stopped: 'Held · stopped', closed: 'Closed'
    }[status]));
    $('.terminal-composer-area').hidden = status === 'closed';
    $('[data-chat-stop]').textContent = status === 'stopped' ? 'Release' : 'Stop';
    $('[data-chat-stop]').hidden = status === 'closed';
    $('[data-chat-close]').hidden = status === 'closed';
    thread.scrollTop = thread.scrollHeight;
  }

  function send() {
    const words = input.value.trim();
    if (!words || state.chatStatus[chatId] === 'closed') return;
    commit('Words sent. The chat reads them at its next turn.', () => {
      (state.messages[chatId] ||= []).push(words);
      if (state.chatStatus[chatId] === 'stopped') state.chatStatus[chatId] = 'waiting';
      input.value = '';
      autosize();
    });
  }
  input.addEventListener('input', autosize);
  input.addEventListener('keydown', event => {
    if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); send(); }
  });
  $('.terminal-send').addEventListener('click', send);
  $('[data-accept]').addEventListener('click', () => dialog(
    'Accept this goal?',
    'T1 becomes yours, reserves 40 from your pool, and starts a coordinator.',
    'Accept goal',
    () => commit('Goal accepted. T1 is on the board.', () => { state.proposal = 'accepted'; })
  ));
  $('[data-reject]').addEventListener('click', () => dialog(
    'Reject this proposal',
    'Tell T0 why so it can change course or propose again.',
    'Reject proposal',
    reason => commit('Proposal rejected. Your reason was sent to T0.', () => {
      state.proposal = 'rejected'; state.rejectReason = reason;
    }),
    true
  ));
  $('[data-chat-stop]').addEventListener('click', () => {
    const held = state.chatStatus[chatId] === 'stopped';
    dialog(held ? 'Release this chat?' : 'Stop this chat?',
      held ? 'The next run resumes from the committed conversation.' : 'The live run stops and the chat is held. Delegates continue.',
      held ? 'Release chat' : 'Stop chat',
      () => commit(held ? 'Chat released.' : 'Chat stopped and held.', () => {
        state.chatStatus[chatId] = held ? 'waiting' : 'stopped';
      }));
  });
  $('[data-chat-close]').addEventListener('click', () => dialog(
    'Close this chat?',
    'The chat is asked to finish. This sample has no live delegates. Its conversation stays readable.',
    'Close chat',
    () => commit('Chat closed.', () => { state.chatStatus[chatId] = 'closed'; })
  ));
  addEventListener('storage', event => {
    if (event.key === KEY) { state = load(); render(); }
  });
  render();
  autosize();
  addEventListener('load', () => { thread.scrollTop = thread.scrollHeight; });
  document.fonts?.ready.then(() => { thread.scrollTop = thread.scrollHeight; });
})();
