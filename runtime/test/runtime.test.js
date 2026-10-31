import assert from 'node:assert/strict';
import test from 'node:test';
import { JSDOM } from 'jsdom';

const dom = new JSDOM('<!doctype html><html><body><div id="app"></div></body></html>', {
  url: 'http://localhost/',
  pretendToBeVisual: true,
});

function define(name, value) {
  Object.defineProperty(globalThis, name, { value, writable: true, configurable: true });
}

define('window', dom.window);
define('document', dom.window.document);
define('navigator', dom.window.navigator);
define('HTMLElement', dom.window.HTMLElement);
define('Node', dom.window.Node);
define('Event', dom.window.Event);
define('KeyboardEvent', dom.window.KeyboardEvent);
define('localStorage', dom.window.localStorage);
define('sessionStorage', dom.window.sessionStorage);

const P = (await import('../plain.runtime.js')).P;

const flush = () => P.flush();

test('signal, computed and effect form a dependency graph', () => {
  const price = P.signal(10);
  const qty = P.signal(2);
  const total = P.computed(() => price() * qty());
  let seen = [];
  P.scope(() => {
    P.effect(() => seen.push(total()));
  });
  assert.deepEqual(seen, [20]);
  qty.set(3);
  flush();
  assert.deepEqual(seen, [20, 30]);
  // No change: no notification.
  qty.set(3);
  flush();
  assert.deepEqual(seen, [20, 30]);
  price.set(5);
  flush();
  assert.deepEqual(seen, [20, 30, 15]);
});

test('batch collapses writes into one pass', () => {
  const a = P.signal(1);
  const b = P.signal(2);
  let runs = 0;
  P.scope(() => {
    P.effect(() => {
      a();
      b();
      runs++;
    });
  });
  assert.equal(runs, 1);
  P.batch(() => {
    a.set(10);
    b.set(20);
  });
  assert.equal(runs, 2, 'both writes should produce a single re-run');
});

test('scope disposes effects and listeners', () => {
  const count = P.signal(0);
  let runs = 0;
  const handle = P.scope(() => {
    P.effect(() => {
      count();
      runs++;
    });
  });
  count.set(1);
  flush();
  assert.equal(runs, 2);
  handle.dispose();
  count.set(2);
  flush();
  assert.equal(runs, 2, 'disposed effect must not run again');
});

test('setText keeps an element in sync', () => {
  const name = P.signal('world');
  const node = P.el('p', 'p1');
  P.scope(() => P.setText(node, () => `Hello, ${name()}!`));
  assert.equal(node.textContent, 'Hello, world!');
  name.set('Plain');
  flush();
  assert.equal(node.textContent, 'Hello, Plain!');
});

test('setAttr removes falsy attributes and escapes text', () => {
  const node = P.el('button');
  const disabled = P.signal(true);
  P.scope(() => P.setAttr(node, 'disabled', () => disabled()));
  assert.equal(node.hasAttribute('disabled'), true);
  disabled.set(false);
  flush();
  assert.equal(node.hasAttribute('disabled'), false);

  const label = P.el('span');
  P.setText(label, '<img src=x onerror=alert(1)>');
  assert.equal(label.children.length, 0, 'text must never be parsed as markup');
  assert.equal(label.textContent, '<img src=x onerror=alert(1)>');
});

test('when swaps branches and disposes the old scope', () => {
  const parent = P.el('div');
  const loggedIn = P.signal(false);
  let listenerRuns = 0;
  const tick = P.signal(0);
  P.scope(() => {
    P.when(
      parent,
      () => loggedIn(),
      () => {
        const node = P.el('p');
        P.setText(node, 'welcome');
        P.effect(() => {
          tick();
          listenerRuns++;
        });
        return node;
      },
      () => {
        const node = P.el('p');
        P.setText(node, 'please log in');
        return node;
      },
    );
  });
  assert.equal(parent.textContent.trim(), 'please log in');
  loggedIn.set(true);
  flush();
  assert.match(parent.textContent, /welcome/);
  const before = listenerRuns;
  tick.set(1);
  flush();
  assert.equal(listenerRuns, before + 1);

  loggedIn.set(false);
  flush();
  const after = listenerRuns;
  tick.set(2);
  flush();
  assert.equal(listenerRuns, after, 'effects from a removed branch must be disposed');
  assert.equal(parent.querySelectorAll('p').length, 1);
});

test('each keys rows, preserves state and disposes removed rows', () => {
  const parent = P.el('ul');
  const items = P.signal([
    { id: 'a', label: 'apple' },
    { id: 'b', label: 'banana' },
    { id: 'c', label: 'cherry' },
  ]);
  const built = [];
  P.scope(() => {
    P.each(
      parent,
      () => items(),
      (item) => {
        const node = P.el('li');
        P.setText(node, () => item().label);
        built.push(node);
        return node;
      },
      (item) => item.id,
    );
  });
  flush();
  assert.equal(parent.querySelectorAll('li').length, 3);
  assert.equal(parent.textContent.replace(/\s/g, ''), 'applebananacherry');
  assert.equal(built.length, 3);

  // Reorder: no row should be rebuilt.
  items.set([items.peek()[2], items.peek()[0], items.peek()[1]]);
  flush();
  assert.equal(built.length, 3, 'reordering must reuse rows');
  assert.equal(parent.textContent.replace(/\s/g, ''), 'cherryapplebanana');

  // Update one item in place: the row updates without a rebuild.
  const updated = items.peek().map((item) => (item.id === 'a' ? { ...item, label: 'APPLE' } : item));
  items.set(updated);
  flush();
  assert.equal(built.length, 3);
  assert.equal(parent.textContent.replace(/\s/g, ''), 'cherryAPPLEbanana');

  // Remove one.
  items.set(items.peek().filter((item) => item.id !== 'b'));
  flush();
  assert.equal(parent.querySelectorAll('li').length, 2);
  assert.equal(built.length, 3, 'removal must not build rows');

  // Append one.
  items.set([...items.peek(), { id: 'd', label: 'date' }]);
  flush();
  assert.equal(built.length, 4);
  assert.equal(parent.textContent.replace(/\s/g, ''), 'cherryAPPLEdate');
});

test('each over a range and over an unkeyed list', () => {
  const parent = P.el('div');
  P.scope(() => {
    P.each(parent, () => 3, (n) => {
      const node = P.el('span');
      P.setText(node, () => String(n()));
      return node;
    });
  });
  flush();
  assert.equal(parent.querySelectorAll('span').length, 3);
  assert.equal(parent.textContent.replace(/\s+/g, ''), '123');
});

test('bind gives two-way binding for text, checkbox and select', () => {
  const name = P.signal('');
  const agreed = P.signal(false);
  const choice = P.signal('b');

  const input = P.el('input');
  const check = P.el('input');
  check.type = 'checkbox';
  const select = P.el('select');
  for (const value of ['a', 'b']) {
    const option = P.el('option');
    option.value = value;
    select.append(option);
  }

  P.scope(() => {
    P.bind(input, () => name(), (v) => name.set(v), 'value');
    P.bind(check, () => agreed(), (v) => agreed.set(v), 'checked');
    P.bind(select, () => choice(), (v) => choice.set(v), 'select');
  });
  flush();

  input.value = 'Ada';
  input.dispatchEvent(new dom.window.Event('input'));
  assert.equal(name.peek(), 'Ada');

  name.set('Grace');
  flush();
  assert.equal(input.value, 'Grace');

  check.checked = true;
  check.dispatchEvent(new dom.window.Event('change'));
  assert.equal(agreed.peek(), true);

  select.value = 'a';
  select.dispatchEvent(new dom.window.Event('change'));
  assert.equal(choice.peek(), 'a');
  choice.set('b');
  flush();
  assert.equal(select.value, 'b');
});

test('persisted restores from and writes to storage', () => {
  localStorage.setItem('plain:todos', JSON.stringify(['buy milk']));
  const handle = P.scope(() => P.persisted('todos', []));
  const todos = handle.value;
  flush();
  assert.deepEqual(todos.peek(), ['buy milk']);
  todos.set(['buy milk', 'walk dog']);
  flush();
  assert.equal(localStorage.getItem('plain:todos'), '["buy milk","walk dog"]');
  handle.dispose();
});

test('nested mutation through list.update produces a new array', () => {
  const todos = P.signal([{ id: 1, done: false }]);
  todos.set(P.list.update(todos.peek(), (t) => t.id === 1, (t) => ({ ...t, done: true })));
  assert.equal(todos.peek()[0].done, true);
});

test('mount renders a component into a selector', () => {
  const dispose = P.mount('#app', () => {
    const count = P.signal(0);
    const button = P.el('button', 'p1');
    P.setText(button, () => `Count: ${count()}`);
    P.on(button, 'click', () => count.set(count() + 1));
    return button;
  });
  flush();
  const button = document.querySelector('#app button');
  assert.equal(button.textContent, 'Count: 0');
  button.dispatchEvent(new dom.window.Event('click'));
  flush();
  assert.equal(button.textContent, 'Count: 1');
  dispose();
});

test('standard library helpers behave', () => {
  assert.equal(P.str.capitalize('hello'), 'Hello');
  assert.equal(P.str.title('the quick fox'), 'The Quick Fox');
  assert.equal(P.slug('Hello, World!'), 'hello-world');
  assert.equal(P.truncate('abcdefghij', 5), 'abcde…');
  assert.deepEqual(P.list.filter([1, 2, 3, 4], (n) => n % 2 === 0), [2, 4]);
  assert.deepEqual(P.list.sortBy([{ n: 2 }, { n: 1 }], (x) => x.n).map((x) => x.n), [1, 2]);
  assert.deepEqual(P.list.sortBy([{ n: 2 }, { n: 1 }], (x) => x.n, 'desc').map((x) => x.n), [2, 1]);
  assert.equal(P.list.sum([1, 2, 3]), 6);
  assert.equal(P.list.count([1, 2, 3], (n) => n > 1), 2);
  assert.deepEqual(P.list.remove([{ id: 1 }, { id: 2 }], (x) => x.id === 1), [{ id: 2 }]);
  assert.deepEqual(P.list.chunk([1, 2, 3, 4, 5], 2), [[1, 2], [3, 4], [5]]);
  assert.deepEqual(P.list.unique([1, 1, 2]), [1, 2]);
  assert.equal(P.num.clamp(15, 0, 10), 10);
  assert.equal(P.num.mapRange(5, 0, 10, 0, 100), 50);
  assert.equal(P.formatBytes(1536), '1.5 KB');
  assert.equal(P.formatDate(new Date(2025, 0, 15, 14, 30), 'MMM D, YYYY'), 'Jan 15, 2025');
  assert.equal(P.formatDate(new Date(2025, 0, 15, 14, 30), 'h:mm A'), '2:30 PM');
  assert.equal(P.formatOrdinal(3), '3rd');
  assert.equal(P.formatOrdinal(2), '2nd');
  assert.equal(P.color.toHex('rgb(255, 87, 51)'), '#ff5733');
  assert.equal(P.color.alpha('#ff5733', 0.5), 'rgba(255, 87, 51, 0.5)');
  assert.equal(P.contrastColor('#ffffff'), '#000000');
  assert.equal(P.contrastColor('#000000'), '#ffffff');
  assert.ok(P.contrastRatio('#000000', '#ffffff') > 20);
  assert.equal(P.buildUrl('/search', { q: 'a b', page: 2 }), '/search?q=a%20b&page=2');
  assert.equal(P.parseUrl('https://example.com/x?q=1').query.q, '1');
});

test('device exposes reactive viewport state', () => {
  assert.equal(typeof P.device.width, 'function');
  assert.equal(typeof P.device.mobile(), 'boolean');
  assert.equal(typeof P.device.darkMode(), 'boolean');
});

test('update cycles are detected rather than hanging', () => {
  const a = P.signal(0);
  const errors = [];
  P.onError((error) => errors.push(error));
  const handle = P.scope(() => {
    P.effect(() => a.set(a() + 1));
  });
  flush();
  handle.dispose();
  P.onError(null);
  assert.ok(errors.length > 0, 'a self-writing effect should be reported');
  assert.match(String(errors[0].message), /did not settle/);
});
