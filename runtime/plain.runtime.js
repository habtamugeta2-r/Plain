/**
 * Plain runtime — v0.1
 *
 * This file is embedded into the compiler (`crates/plain-lang/src/runtime.rs`
 * uses `include_str!`) and emitted next to the generated application code, so
 * it has to stay dependency-free and readable.
 *
 * Design notes
 * ------------
 * Reactivity is *fine grained*: a signal knows which computations read it, and
 * an update re-runs only those. The generated code therefore never diffs a
 * virtual tree — it creates real DOM nodes once and wraps every dynamic part
 * (text, attribute, style, list, branch) in an effect.
 *
 * Everything dynamic is passed around as a zero-argument accessor (`() => x`).
 * That keeps the compiler simple: it does not need a dependency analyser, the
 * runtime discovers dependencies by observing reads.
 *
 * Security: the runtime only ever uses `textContent`, `setAttribute` and
 * `style.setProperty`. It never writes markup as a string, so interpolated values
 * cannot inject markup. Escape hatches that need raw HTML are a later phase and
 * will require an explicit, sanitised API.
 */

/* ------------------------------------------------------------------------- *
 * Reactivity
 * ------------------------------------------------------------------------- */

/** Currently running computation (effect or computed), or null. */
let current = null;
/** Array of disposer functions for the scope being built, or null. */
let currentOwner = null;
/** Effects waiting to run. */
const pending = new Set();
let batchDepth = 0;
let flushScheduled = false;
let errorHandler = null;
/** Guards against `a = a + 1` inside an effect that reads `a`. */
const MAX_PASSES = 1000;

function reportError(error) {
  if (errorHandler) {
    try {
      errorHandler(error);
      return;
    } catch (_) {
      /* a broken error handler must not hide the original problem */
    }
  }
  if (typeof console !== 'undefined') console.error('[plain]', error);
}

/** Registers `fn` as a cleanup for the enclosing scope. */
export function own(disposer) {
  if (currentOwner) currentOwner.push(disposer);
  return disposer;
}

function link(node, subscribers) {
  subscribers.add(node);
  node.sources.add(subscribers);
}

function unlinkAll(node) {
  for (const subscribers of node.sources) subscribers.delete(node);
  node.sources.clear();
}

function notify(subscribers) {
  for (const node of Array.from(subscribers)) {
    if (node.disposed) continue;
    if (node.kind === 'computed') node.mark();
    else pending.add(node);
  }
  schedule();
}

function schedule() {
  if (batchDepth > 0 || flushScheduled) return;
  flushScheduled = true;
  queueMicrotask(flush);
}

/** Runs every queued effect. Exported so tests can be synchronous. */
export function flush() {
  flushScheduled = false;
  let passes = 0;
  while (pending.size > 0) {
    if (++passes > MAX_PASSES) {
      pending.clear();
      reportError(
        new Error(`reactive update did not settle after ${MAX_PASSES} passes; check for an assignment cycle`),
      );
      return;
    }
    const jobs = Array.from(pending);
    pending.clear();
    for (const job of jobs) {
      if (!job.disposed) job.run();
    }
  }
}

/** Batches several writes into one update pass. */
export function batch(fn) {
  batchDepth++;
  try {
    return fn();
  } finally {
    batchDepth--;
    if (batchDepth === 0) flush();
  }
}

/** Reads signals inside `fn` without subscribing to them. */
export function untrack(fn) {
  const previous = current;
  current = null;
  try {
    return fn();
  } finally {
    current = previous;
  }
}

/**
 * Creates a scope: every effect, listener and timer created while `fn` runs is
 * collected so `dispose()` can tear them all down. Returns `{ value, dispose }`.
 */
export function scope(fn) {
  const disposers = [];
  const previousOwner = currentOwner;
  currentOwner = disposers;
  let value;
  try {
    value = fn();
  } finally {
    currentOwner = previousOwner;
  }
  return {
    value,
    dispose() {
      for (let i = disposers.length - 1; i >= 0; i--) {
        try {
          disposers[i]();
        } catch (error) {
          reportError(error);
        }
      }
      disposers.length = 0;
    },
  };
}

/**
 * A writable reactive value. Reading it inside an effect or computed subscribes
 * that computation; `.set()` schedules the subscribers.
 */
export function signal(initial) {
  let value = initial;
  const subscribers = new Set();

  function read() {
    if (current !== null) link(current, subscribers);
    return value;
  }
  read.set = function set(next) {
    if (Object.is(next, value)) return value;
    value = next;
    notify(subscribers);
    return value;
  };
  read.peek = () => value;
  read.isSignal = true;
  return read;
}

/** A read-only value derived from other reactive values. Recomputed lazily. */
export function computed(fn) {
  let value;
  let dirty = true;
  const subscribers = new Set();
  const node = {
    kind: 'computed',
    sources: new Set(),
    disposed: false,
    mark() {
      if (dirty) return;
      dirty = true;
      notify(subscribers);
    },
  };

  function recompute() {
    unlinkAll(node);
    const previous = current;
    current = node;
    try {
      value = fn();
    } catch (error) {
      reportError(error);
      value = undefined;
    } finally {
      current = previous;
      dirty = false;
    }
  }

  function read() {
    if (dirty) recompute();
    if (current !== null) link(current, subscribers);
    return value;
  }
  read.peek = () => {
    if (dirty) recompute();
    return value;
  };
  return read;
}

/** Runs `fn` immediately, then again whenever a value it read changes. */
export function effect(fn) {
  const node = {
    kind: 'effect',
    sources: new Set(),
    disposed: false,
    run() {
      if (node.disposed) return;
      unlinkAll(node);
      const previous = current;
      current = node;
      try {
        fn();
      } catch (error) {
        reportError(error);
      } finally {
        current = previous;
      }
    },
  };
  node.run();
  own(() => {
    node.disposed = true;
    unlinkAll(node);
    pending.delete(node);
  });
  return node;
}

/* ------------------------------------------------------------------------- *
 * DOM construction
 * ------------------------------------------------------------------------- */

const UNITLESS = new Set([
  'opacity',
  'z-index',
  'flex',
  'flex-grow',
  'flex-shrink',
  'order',
  'grid-column',
  'grid-row',
  'font-weight',
  'line-height',
  'zoom',
  'orphans',
  'widows',
  'animation-iteration-count',
]);

/**
 * The string a value shows as. `nothing` and `null` show as nothing at all,
 * which is why a page never prints "undefined" where a value has not arrived
 * yet.
 */
export function text(value) {
  return value === null || value === undefined ? '' : String(value);
}

/** Normalises a factory result into a flat array of DOM nodes. */
export function toNodes(value) {
  if (value === null || value === undefined || value === false) return [];
  if (Array.isArray(value)) {
    const out = [];
    for (const item of value) out.push(...toNodes(item));
    return out;
  }
  if (typeof value !== 'object') return [];
  if (value.nodeType === 11) return Array.from(value.childNodes);
  if (value.nodeType) return [value];
  return [];
}

/** Creates an element, optionally with its generated scope class. */
export function el(tag, className) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  return node;
}

export function comment(label) {
  return document.createComment(label || '');
}

export function append(parent, child) {
  for (const node of toNodes(child)) parent.append(node);
  return parent;
}

/** Reactive (or static) text content. */
export function setText(node, value) {
  if (typeof value === 'function') effect(() => (node.textContent = text(value())));
  else node.textContent = text(value);
  return node;
}

function applyAttribute(node, name, value) {
  if (value === null || value === undefined || value === false) node.removeAttribute(name);
  else node.setAttribute(name, value === true ? '' : text(value));
}

/** Reactive (or static) attribute. `false`/`null` remove it. */
export function setAttr(node, name, value) {
  if (typeof value === 'function') effect(() => applyAttribute(node, name, value()));
  else applyAttribute(node, name, value);
  return node;
}

/** Reactive (or static) DOM property, e.g. `value`, `checked`, `disabled`. */
export function setProp(node, name, value) {
  if (typeof value === 'function') effect(() => (node[name] = value()));
  else node[name] = value;
  return node;
}

function cssValue(property, value) {
  if (typeof value === 'number' && !UNITLESS.has(property)) return `${value}px`;
  return text(value);
}

/** Reactive (or static) inline style property. Used for dynamic style values. */
export function setStyle(node, property, value) {
  if (typeof value === 'function') {
    effect(() => node.style.setProperty(property, cssValue(property, value())));
  } else {
    node.style.setProperty(property, cssValue(property, value));
  }
  return node;
}

/** Adds classes reactively; `value` may be a string, array or record. */
export function setClass(node, base, value) {
  const apply = (next) => {
    const names = base ? base.split(' ') : [];
    if (typeof next === 'string') names.push(...next.split(' '));
    else if (Array.isArray(next)) names.push(...next.filter(Boolean));
    else if (next && typeof next === 'object') {
      for (const key of Object.keys(next)) if (next[key]) names.push(key);
    }
    node.className = names.filter(Boolean).join(' ');
  };
  if (typeof value === 'function') effect(() => apply(value()));
  else apply(value);
  return node;
}

/**
 * Attaches an event listener and registers its removal with the current scope.
 * `filter` receives the event and may return false to ignore it (used for
 * `on keydown.enter` and friends).
 */
export function on(node, type, handler, options, filter) {
  const listener = (event) => {
    if (filter && !filter(event)) return;
    handler(event);
  };
  node.addEventListener(type, listener, options || false);
  own(() => node.removeEventListener(type, listener, options || false));
  return node;
}

function inputEventFor(kind) {
  return kind === 'value' ? 'input' : 'change';
}

/**
 * Two-way binding. `get` reads state, `set` writes it. `kind` is `value`,
 * `checked` or `select`.
 */
export function bind(node, get, set, kind) {
  const mode = kind || 'value';
  const property = mode === 'checked' ? 'checked' : 'value';
  effect(() => {
    const value = get();
    const next = mode === 'checked' ? Boolean(value) : text(value);
    if (node[property] !== next) node[property] = next;
  });
  on(node, inputEventFor(mode), () => {
    set(mode === 'checked' ? node.checked : node.value);
  });
  return node;
}

function removeNodes(nodes) {
  for (const node of nodes) if (node.parentNode) node.parentNode.removeChild(node);
}

/**
 * Conditional rendering. `then`/`otherwise` are factories; their scope is
 * disposed when the branch is torn down, which is how Plain guarantees
 * listener and timer cleanup.
 */
export function when(parent, condition, then, otherwise) {
  const end = comment('if');
  parent.append(end);
  let state = null;

  effect(() => {
    const branch = condition() ? 'then' : otherwise ? 'else' : 'none';
    if (state && state.branch === branch) return;
    if (state) {
      removeNodes(state.nodes);
      state.handle.dispose();
      state = null;
    }
    if (branch === 'none') return;
    const factory = branch === 'then' ? then : otherwise;
    const handle = scope(factory);
    const nodes = toNodes(handle.value);
    for (const node of nodes) parent.insertBefore(node, end);
    state = { branch, nodes, handle };
  });

  return end;
}

/**
 * Keyed list rendering.
 *
 * Each row owns a scope and an index signal; the row's item accessor is a
 * computed over `(list, index)`, so replacing or reordering the array updates
 * rows in place instead of rebuilding them. Rows that survive keep their local
 * state and listeners; rows that disappear are disposed.
 */
export function each(parent, list, factory, keyFn) {
  const end = comment('each');
  parent.append(end);
  let rows = [];

  function readItems() {
    const value = list();
    if (value === null || value === undefined) return [];
    if (typeof value === 'number') {
      const out = [];
      for (let i = 1; i <= value; i++) out.push(i);
      return out;
    }
    if (typeof value === 'string') return value.split('');
    return value;
  }

  function createRow(index) {
    const idx = signal(index);
    const handle = scope(() => {
      const item = computed(() => {
        const items = readItems();
        return items[idx()];
      });
      return { item, node: factory(item, idx) };
    });
    return { key: '', index, idx, nodes: toNodes(handle.value.node), handle };
  }

  effect(() => {
    const items = readItems();
    const next = [];
    const claimed = new Set();

    for (let i = 0; i < items.length; i++) {
      const key = keyFn ? text(keyFn(items[i], i)) : `#${i}`;
      let row = null;
      for (const candidate of rows) {
        if (candidate.key === key && !claimed.has(candidate)) {
          row = candidate;
          break;
        }
      }
      if (row) {
        claimed.add(row);
        if (row.index !== i) {
          row.index = i;
          row.idx.set(i);
        }
      } else {
        row = createRow(i);
        claimed.add(row);
      }
      row.key = key;
      next.push(row);
    }

    for (const row of rows) {
      if (claimed.has(row)) continue;
      removeNodes(row.nodes);
      row.handle.dispose();
    }
    rows = next;

    // Walk backwards and only move nodes whose successor is not already right.
    let cursor = end;
    for (let i = rows.length - 1; i >= 0; i--) {
      const first = rows[i].nodes[0];
      if (!first) continue;
      if (first.nextSibling !== cursor) {
        for (const node of rows[i].nodes) parent.insertBefore(node, cursor);
      }
      cursor = first;
    }
  });

  return end;
}

/** Runs `fn` after the current scope's nodes exist in the document. */
export function afterMount(fn) {
  if (typeof window === 'undefined' || typeof window.requestAnimationFrame !== 'function') {
    queueMicrotask(() => fn());
    return;
  }
  let cancelled = false;
  window.requestAnimationFrame(() => {
    if (!cancelled) fn();
  });
  own(() => {
    cancelled = true;
  });
}

/** Mounts a component factory into `target` (selector or node). */
export function mount(target, factory) {
  const node = typeof target === 'string' ? document.querySelector(target) : target;
  if (!node) throw new Error(`mount target not found: ${target}`);
  const handle = scope(factory);
  append(node, handle.value);
  flush();
  return handle.dispose;
}

/* ------------------------------------------------------------------------- *
 * Persistence and platform capabilities
 * ------------------------------------------------------------------------- */

const STORAGE_PREFIX = 'plain:';
const memoryStore = new Map();

function memoryBackend() {
  return {
    getItem: (key) => (memoryStore.has(key) ? memoryStore.get(key) : null),
    setItem: (key, value) => memoryStore.set(key, String(value)),
    removeItem: (key) => memoryStore.delete(key),
    clear: () => memoryStore.clear(),
  };
}

function backend(kind) {
  if (typeof window === 'undefined') return memoryBackend();
  try {
    const candidate = kind === 'session' ? window.sessionStorage : window.localStorage;
    const probe = `${STORAGE_PREFIX}__probe__`;
    candidate.setItem(probe, '1');
    candidate.removeItem(probe);
    return candidate;
  } catch (_) {
    // Private browsing, disabled storage, or a non-browser target.
    return memoryBackend();
  }
}

function makeStore(kind) {
  return {
    get(key, fallback) {
      const raw = backend(kind).getItem(STORAGE_PREFIX + key);
      if (raw === null) return fallback === undefined ? null : fallback;
      try {
        return JSON.parse(raw);
      } catch (_) {
        return raw;
      }
    },
    set(key, value) {
      try {
        backend(kind).setItem(STORAGE_PREFIX + key, JSON.stringify(value));
      } catch (error) {
        reportError(error);
      }
      return value;
    },
    remove(key) {
      backend(kind).removeItem(STORAGE_PREFIX + key);
    },
    clear() {
      const store = backend(kind);
      for (const key of Object.keys(store)) {
        if (key.startsWith(STORAGE_PREFIX)) store.removeItem(key);
      }
    },
    has(key) {
      return backend(kind).getItem(STORAGE_PREFIX + key) !== null;
    },
  };
}

/** `localStorage` wrapper with JSON values and a `plain:` key prefix. */
export const store = makeStore('local');
/** `sessionStorage` wrapper. */
export const session = makeStore('session');

/**
 * A signal restored from storage and written back on every change. `initial`
 * is used when nothing is stored yet.
 */
export function persisted(key, initial, area) {
  // `session` keeps the value for the tab; anything else outlives it.
  const backend = area === 'session' ? session : store;
  const value = backend.get(key, initial);
  const state = signal(value);
  effect(() => backend.set(key, state()));
  if (typeof window !== 'undefined' && window.addEventListener) {
    const onStorage = (event) => {
      if (event.key !== STORAGE_PREFIX + key || event.newValue === null) return;
      try {
        state.set(JSON.parse(event.newValue));
      } catch (error) {
        reportError(error);
      }
    };
    window.addEventListener('storage', onStorage);
    own(() => window.removeEventListener('storage', onStorage));
  }
  return state;
}

/** Clipboard write. Requires a user gesture and a secure context in browsers. */
export const clipboard = {
  write(value) {
    if (typeof navigator !== 'undefined' && navigator.clipboard) {
      return navigator.clipboard.writeText(text(value));
    }
    return Promise.reject(new Error('clipboard API unavailable'));
  },
  read() {
    if (typeof navigator !== 'undefined' && navigator.clipboard) {
      return navigator.clipboard.readText();
    }
    return Promise.reject(new Error('clipboard API unavailable'));
  },
};

function makeDevice() {
  const hasWindow = typeof window !== 'undefined';
  const width = signal(hasWindow ? window.innerWidth : 1024);
  const height = signal(hasWindow ? window.innerHeight : 768);
  const online = signal(typeof navigator === 'undefined' ? true : navigator.onLine !== false);
  const dark = signal(false);
  const reducedMotion = signal(false);

  if (hasWindow && typeof window.matchMedia === 'function') {
    const watch = (query, state) => {
      const media = window.matchMedia(query);
      state.set(media.matches);
      const listener = (event) => state.set(event.matches);
      if (typeof media.addEventListener === 'function') media.addEventListener('change', listener);
      else if (typeof media.addListener === 'function') media.addListener(listener);
      return media;
    };
    watch('(prefers-color-scheme: dark)', dark);
    watch('(prefers-reduced-motion: reduce)', reducedMotion);
  }

  if (hasWindow) {
    const onResize = () => {
      width.set(window.innerWidth);
      height.set(window.innerHeight);
    };
    window.addEventListener('resize', onResize);
    own(() => window.removeEventListener('resize', onResize));
  }
  if (typeof window !== 'undefined' && window.addEventListener) {
    const onLine = () => online.set(true);
    const offLine = () => online.set(false);
    window.addEventListener('online', onLine);
    window.addEventListener('offline', offLine);
    own(() => {
      window.removeEventListener('online', onLine);
      window.removeEventListener('offline', offLine);
    });
  }

  const state = {
    width,
    height,
    online,
    darkMode: dark,
    reducedMotion,
    mobile: computed(() => width() < 640),
    tablet: computed(() => width() >= 640 && width() < 1024),
    desktop: computed(() => width() >= 1024),
    touch: signal(
      typeof window !== 'undefined' &&
        ('ontouchstart' in window || (typeof navigator !== 'undefined' && navigator.maxTouchPoints > 0)),
    ),
    language: signal(typeof navigator === 'undefined' ? 'en-US' : navigator.language || 'en-US'),
    pixelRatio: signal(hasWindow && window.devicePixelRatio ? window.devicePixelRatio : 1),
  };

  // Every field is a getter over its accessor, so `device.width` reads like a
  // plain value and still subscribes whatever effect read it. Plain puts no
  // parentheses on the page, and neither should the viewport.
  const api = {};
  for (const name of Object.keys(state)) {
    Object.defineProperty(api, name, { get: state[name], enumerable: true });
  }
  return api;
}

/** Reactive viewport, connectivity, colour-scheme and motion-preference state. */
export const device = makeDevice();

/* ------------------------------------------------------------------------- *
 * Focus management (accessibility)
 * ------------------------------------------------------------------------- */

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function focusFirst(container) {
  if (!container) return null;
  const target = container.querySelector(FOCUSABLE);
  if (target) target.focus();
  return target;
}

/**
 * Traps Tab inside `container`. Returns nothing; the listener is disposed with
 * the enclosing scope.
 */
export function trapFocus(container) {
  if (!container) return;
  const listener = (event) => {
    if (event.key !== 'Tab') return;
    const items = Array.from(container.querySelectorAll(FOCUSABLE)).filter(
      (node) => node.offsetParent !== null || node === document.activeElement,
    );
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };
  container.addEventListener('keydown', listener);
  own(() => container.removeEventListener('keydown', listener));
}

/** Moves focus to an element, falling back to `tabindex="-1"` for containers. */
export function focus(node) {
  if (!node) return;
  if (!node.hasAttribute('tabindex')) node.setAttribute('tabindex', '-1');
  node.focus();
}

/* ------------------------------------------------------------------------- *
 * Standard library: strings
 * ------------------------------------------------------------------------- */

export const str = {
  upper: (value) => text(value).toUpperCase(),
  lower: (value) => text(value).toLowerCase(),
  capitalize: (value) => {
    const s = text(value);
    return s.length === 0 ? s : s[0].toUpperCase() + s.slice(1);
  },
  title: (value) =>
    text(value)
      .split(/(\s+)/)
      .map((part) => (/^\s+$/.test(part) ? part : str.capitalize(part)))
      .join(''),
  trim: (value) => text(value).trim(),
  contains: (value, needle) => text(value).includes(text(needle)),
  startsWith: (value, needle) => text(value).startsWith(text(needle)),
  endsWith: (value, needle) => text(value).endsWith(text(needle)),
  replace: (value, needle, next) => text(value).replace(text(needle), text(next)),
  replaceAll: (value, needle, next) => text(value).split(text(needle)).join(text(next)),
  slice: (value, from, to) => text(value).slice(from, to === undefined ? undefined : to),
  split: (value, separator) => text(value).split(separator === undefined ? ',' : text(separator)),
  join: (value, separator) => (Array.isArray(value) ? value.join(separator === undefined ? ',' : text(separator)) : text(value)),
  reverse: (value) => text(value).split('').reverse().join(''),
  repeat: (value, count) => text(value).repeat(Math.max(0, count | 0)),
  padLeft: (value, width, fill) => text(value).padStart(width, fill === undefined ? ' ' : text(fill)),
  padRight: (value, width, fill) => text(value).padEnd(width, fill === undefined ? ' ' : text(fill)),
  length: (value) => text(value).length,
};

export function slug(value) {
  return text(value)
    .toLowerCase()
    .normalize('NFKD')
    .replace(/[\u0300-\u036f]/g, '')
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
}

export function truncate(value, max, ellipsis) {
  const s = text(value);
  const limit = Math.max(0, max | 0);
  if (s.length <= limit) return s;
  return `${s.slice(0, limit).trimEnd()}${ellipsis === undefined ? '…' : text(ellipsis)}`;
}

/* ------------------------------------------------------------------------- *
 * Standard library: lists
 * ------------------------------------------------------------------------- */

function toArray(value) {
  if (value === null || value === undefined) return [];
  return Array.isArray(value) ? value : Array.from(value);
}

function identity(item) {
  return item;
}

export const list = {
  filter: (items, predicate) => toArray(items).filter(predicate || identity),
  map: (items, fn) => toArray(items).map(fn || identity),
  flatMap: (items, fn) => toArray(items).flatMap(fn || identity),
  sortBy: (items, keyFn, direction) => {
    const factor = direction === 'desc' || direction === 'descending' ? -1 : 1;
    const key = keyFn || identity;
    return toArray(items).slice().sort((a, b) => compare(key(a), key(b)) * factor);
  },
  reverse: (items) => toArray(items).slice().reverse(),
  unique: (items, keyFn) => {
    const key = keyFn || identity;
    const seen = new Set();
    return toArray(items).filter((item) => {
      const id = key(item);
      const idText = typeof id === 'object' ? JSON.stringify(id) : String(id);
      if (seen.has(idText)) return false;
      seen.add(idText);
      return true;
    });
  },
  flatten: (items) => toArray(items).flat(Infinity),
  chunk: (items, size) => {
    const step = Math.max(1, size | 0);
    const source = toArray(items);
    const out = [];
    for (let i = 0; i < source.length; i += step) out.push(source.slice(i, i + step));
    return out;
  },
  group: (items, keyFn) => {
    const key = keyFn || identity;
    const out = new Map();
    for (const item of toArray(items)) {
      const id = key(item);
      if (!out.has(id)) out.set(id, []);
      out.get(id).push(item);
    }
    return out;
  },
  take: (items, count) => toArray(items).slice(0, Math.max(0, count | 0)),
  skip: (items, count) => toArray(items).slice(Math.max(0, count | 0)),
  paginate: (items, page, perPage) => {
    const size = Math.max(1, perPage | 0);
    const index = Math.max(0, (page | 0) - 1);
    return toArray(items).slice(index * size, index * size + size);
  },
  find: (items, predicate) => toArray(items).find(predicate || identity),
  findIndex: (items, predicate) => toArray(items).findIndex(predicate || identity),
  indexOf: (items, value) => toArray(items).indexOf(value),
  includes: (items, value) => toArray(items).includes(value),
  any: (items, predicate) => toArray(items).some(predicate || identity),
  all: (items, predicate) => toArray(items).every(predicate || identity),
  none: (items, predicate) => !toArray(items).some(predicate || identity),
  count: (items, predicate) => toArray(items).filter(predicate || identity).length,
  sum: (items, keyFn) => toArray(items).reduce((total, item) => total + (Number((keyFn || identity)(item)) || 0), 0),
  average: (items, keyFn) => {
    const source = toArray(items);
    return source.length === 0 ? 0 : list.sum(source, keyFn) / source.length;
  },
  minBy: (items, keyFn) => reduceBest(items, keyFn, (a, b) => a <= b),
  maxBy: (items, keyFn) => reduceBest(items, keyFn, (a, b) => a >= b),
  reduce: (items, fn, initial) => toArray(items).reduce(fn, initial),
  zip: (items, other) => toArray(items).map((item, index) => [item, toArray(other)[index]]),
  insert: (items, index, value) => {
    const out = toArray(items).slice();
    out.splice(index, 0, value);
    return out;
  },
  remove: (items, predicate) => toArray(items).filter((item, index) => !(predicate || identity)(item, index)),
  removeAt: (items, index) => toArray(items).filter((_, i) => i !== index),
  move: (items, from, to) => {
    const out = toArray(items).slice();
    const [item] = out.splice(from, 1);
    out.splice(to, 0, item);
    return out;
  },
  update: (items, predicate, fn) =>
    toArray(items).map((item) => ((predicate || identity)(item) ? fn(item) : item)),
  first: (items) => toArray(items)[0],
  last: (items) => {
    const source = toArray(items);
    return source[source.length - 1];
  },
};

function reduceBest(items, keyFn, better) {
  const key = keyFn || identity;
  let best;
  let bestKey;
  for (const item of toArray(items)) {
    const candidate = key(item);
    if (best === undefined || better(candidate, bestKey)) {
      best = item;
      bestKey = candidate;
    }
  }
  return best;
}

function compare(a, b) {
  if (a === b) return 0;
  if (a === null || a === undefined) return -1;
  if (b === null || b === undefined) return 1;
  if (typeof a === 'number' && typeof b === 'number') return a < b ? -1 : 1;
  return String(a).localeCompare(String(b), undefined, { numeric: true });
}

/* ------------------------------------------------------------------------- *
 * Standard library: numbers, math, dates
 * ------------------------------------------------------------------------- */

export const num = {
  clamp: (value, min, max) => Math.min(Math.max(Number(value), min), max),
  lerp: (from, to, t) => from + (to - from) * t,
  mapRange: (value, inMin, inMax, outMin, outMax) =>
    inMax === inMin ? outMin : outMin + ((value - inMin) * (outMax - outMin)) / (inMax - inMin),
  round: (value, digits) => {
    const factor = Math.pow(10, digits || 0);
    return Math.round(Number(value) * factor) / factor;
  },
  floor: (value) => Math.floor(Number(value)),
  ceil: (value) => Math.ceil(Number(value)),
  abs: (value) => Math.abs(Number(value)),
  min: (...values) => Math.min(...values.flat().map(Number)),
  max: (...values) => Math.max(...values.flat().map(Number)),
  pow: (value, exponent) => Math.pow(Number(value), Number(exponent)),
  sqrt: (value) => Math.sqrt(Number(value)),
  random: (min, max) =>
    min === undefined ? Math.random() : min + Math.random() * ((max === undefined ? 1 : max) - min),
  distance: (x1, y1, x2, y2) => Math.hypot(x2 - x1, y2 - y1),
};

export function formatNumber(value, options) {
  return new Intl.NumberFormat(undefined, options).format(Number(value) || 0);
}

export function formatCurrency(value, currency, locale) {
  return new Intl.NumberFormat(locale, { style: 'currency', currency: currency || 'USD' }).format(
    Number(value) || 0,
  );
}

export function formatPercent(value, digits) {
  return new Intl.NumberFormat(undefined, {
    style: 'percent',
    minimumFractionDigits: digits || 0,
    maximumFractionDigits: digits === undefined ? 1 : digits,
  }).format(Number(value) || 0);
}

export function formatCompact(value) {
  return new Intl.NumberFormat(undefined, { notation: 'compact' }).format(Number(value) || 0);
}

export function formatOrdinal(value) {
  const n = Math.trunc(Number(value) || 0);
  if (typeof Intl.PluralRules === 'function') {
    const rules = new Intl.PluralRules(undefined, { type: 'ordinal' });
    const suffix = { one: 'st', two: 'nd', few: 'rd', other: 'th' }[rules.select(n)] || 'th';
    return `${n}${suffix}`;
  }
  const rest = n % 100;
  const suffix = rest >= 11 && rest <= 13 ? 'th' : ['th', 'st', 'nd', 'rd'][n % 10] || 'th';
  return `${n}${suffix}`;
}

export function formatBytes(value) {
  let size = Math.abs(Number(value) || 0);
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let index = 0;
  while (size >= 1024 && index < units.length - 1) {
    size /= 1024;
    index++;
  }
  return `${index === 0 ? size : num.round(size, 1)} ${units[index]}`;
}

function toDate(value) {
  if (value instanceof Date) return value;
  if (typeof value === 'number') return new Date(value);
  return new Date(text(value));
}

const MONTHS = [
  'January', 'February', 'March', 'April', 'May', 'June',
  'July', 'August', 'September', 'October', 'November', 'December',
];

/**
 * Formats a date with a small pattern language: YYYY YY MMMM MMM MM M DD D
 * HH H hh h mm m ss s A a.
 */
export function formatDate(value, pattern) {
  const date = toDate(value);
  if (Number.isNaN(date.getTime())) return '';
  const spec = pattern || 'MMM D, YYYY';
  const hours24 = date.getHours();
  const hours12 = hours24 % 12 === 0 ? 12 : hours24 % 12;
  const tokens = {
    YYYY: date.getFullYear(),
    YY: String(date.getFullYear()).slice(-2),
    MMMM: MONTHS[date.getMonth()],
    MMM: MONTHS[date.getMonth()].slice(0, 3),
    MM: String(date.getMonth() + 1).padStart(2, '0'),
    M: date.getMonth() + 1,
    DD: String(date.getDate()).padStart(2, '0'),
    D: date.getDate(),
    HH: String(hours24).padStart(2, '0'),
    H: hours24,
    hh: String(hours12).padStart(2, '0'),
    h: hours12,
    mm: String(date.getMinutes()).padStart(2, '0'),
    m: date.getMinutes(),
    ss: String(date.getSeconds()).padStart(2, '0'),
    s: date.getSeconds(),
    A: hours24 < 12 ? 'AM' : 'PM',
    a: hours24 < 12 ? 'am' : 'pm',
  };
  return spec.replace(/YYYY|YY|MMMM|MMM|MM|M|DD|D|HH|H|hh|h|mm|m|ss|s|A|a/g, (match) =>
    String(tokens[match]),
  );
}

const RELATIVE_UNITS = [
  ['year', 31536000000],
  ['month', 2592000000],
  ['week', 604800000],
  ['day', 86400000],
  ['hour', 3600000],
  ['minute', 60000],
  ['second', 1000],
];

export function relativeTime(value, now) {
  const date = toDate(value);
  if (Number.isNaN(date.getTime())) return '';
  const reference = now === undefined ? Date.now() : toDate(now).getTime();
  const delta = date.getTime() - reference;
  if (typeof Intl.RelativeTimeFormat !== 'function') {
    const seconds = Math.round(delta / 1000);
    return `${seconds}s${seconds < 0 ? ' ago' : ''}`;
  }
  const format = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' });
  const magnitude = Math.abs(delta);
  for (const [unit, ms] of RELATIVE_UNITS) {
    if (magnitude >= ms || unit === 'second') {
      return format.format(Math.round(delta / ms), unit);
    }
  }
  return format.format(0, 'second');
}

export const datetime = {
  now: () => new Date(),
  parse: (value) => toDate(value),
  add: (value, amount, unit) => {
    const date = new Date(toDate(value).getTime());
    const step = Number(amount) || 0;
    switch (text(unit)) {
      case 'years': case 'year': date.setFullYear(date.getFullYear() + step); break;
      case 'months': case 'month': date.setMonth(date.getMonth() + step); break;
      case 'days': case 'day': date.setDate(date.getDate() + step); break;
      case 'hours': case 'hour': date.setHours(date.getHours() + step); break;
      case 'minutes': case 'minute': date.setMinutes(date.getMinutes() + step); break;
      case 'seconds': case 'second': date.setSeconds(date.getSeconds() + step); break;
      default: date.setTime(date.getTime() + step);
    }
    return date;
  },
  diff: (a, b, unit) => {
    const ms = toDate(a).getTime() - toDate(b).getTime();
    const divisors = { years: 31536000000, months: 2592000000, days: 86400000, hours: 3600000, minutes: 60000, seconds: 1000 };
    const divisor = divisors[`${text(unit)}s`] || divisors[text(unit)] || 1;
    return ms / divisor;
  },
  startOf: (value, unit) => {
    const date = new Date(toDate(value).getTime());
    if (text(unit).startsWith('month')) date.setDate(1);
    if (text(unit).startsWith('day') || text(unit).startsWith('month')) date.setHours(0, 0, 0, 0);
    if (text(unit).startsWith('year')) date.setMonth(0, 1);
    if (text(unit).startsWith('hour')) date.setMinutes(0, 0, 0);
    return date;
  },
  isToday: (value) => formatDate(value, 'YYYY-MM-DD') === formatDate(new Date(), 'YYYY-MM-DD'),
  isBefore: (a, b) => toDate(a).getTime() < toDate(b).getTime(),
  isAfter: (a, b) => toDate(a).getTime() > toDate(b).getTime(),
};

/* ------------------------------------------------------------------------- *
 * Standard library: colours
 * ------------------------------------------------------------------------- */

function clampByte(value) {
  return Math.max(0, Math.min(255, Math.round(value)));
}

/** Parses `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()`/`rgba()` or a CSS name. */
export function parseColor(value) {
  const input = text(value).trim();
  if (input.startsWith('#')) {
    const hex = input.slice(1);
    if (hex.length === 3 || hex.length === 4) {
      const parts = hex.split('').map((digit) => parseInt(digit + digit, 16));
      return { r: parts[0], g: parts[1], b: parts[2], a: parts.length === 4 ? parts[3] / 255 : 1 };
    }
    if (hex.length === 6 || hex.length === 8) {
      return {
        r: parseInt(hex.slice(0, 2), 16),
        g: parseInt(hex.slice(2, 4), 16),
        b: parseInt(hex.slice(4, 6), 16),
        a: hex.length === 8 ? parseInt(hex.slice(6, 8), 16) / 255 : 1,
      };
    }
    return null;
  }
  const match = /^rgba?\(([^)]+)\)$/.exec(input);
  if (match) {
    const parts = match[1].split(/[,\s/]+/).filter(Boolean).map(Number);
    return { r: parts[0], g: parts[1], b: parts[2], a: parts.length > 3 ? parts[3] : 1 };
  }
  return null;
}

function toHsl({ r, g, b }) {
  const rn = r / 255;
  const gn = g / 255;
  const bn = b / 255;
  const max = Math.max(rn, gn, bn);
  const min = Math.min(rn, gn, bn);
  const delta = max - min;
  let hue = 0;
  if (delta !== 0) {
    if (max === rn) hue = ((gn - bn) / delta) % 6;
    else if (max === gn) hue = (bn - rn) / delta + 2;
    else hue = (rn - gn) / delta + 4;
  }
  hue = (hue * 60 + 360) % 360;
  const lightness = (max + min) / 2;
  const saturation = delta === 0 ? 0 : delta / (1 - Math.abs(2 * lightness - 1));
  return { h: hue, s: saturation * 100, l: lightness * 100 };
}

function fromHsl({ h, s, l }, alpha) {
  const saturation = s / 100;
  const lightness = l / 100;
  const c = (1 - Math.abs(2 * lightness - 1)) * saturation;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = lightness - c / 2;
  const segment = Math.floor(h / 60) % 6;
  const table = [
    [c, x, 0], [x, c, 0], [0, c, x], [0, x, c], [x, 0, c], [c, 0, x],
  ][segment] || [0, 0, 0];
  return {
    r: clampByte((table[0] + m) * 255),
    g: clampByte((table[1] + m) * 255),
    b: clampByte((table[2] + m) * 255),
    a: alpha === undefined ? 1 : alpha,
  };
}

function toCss({ r, g, b, a }) {
  const alpha = a === undefined ? 1 : a;
  return alpha >= 1
    ? `#${[r, g, b].map((v) => clampByte(v).toString(16).padStart(2, '0')).join('')}`
    : `rgba(${clampByte(r)}, ${clampByte(g)}, ${clampByte(b)}, ${num.round(alpha, 3)})`;
}

function percentAmount(amount) {
  const n = typeof amount === 'string' ? parseFloat(amount) : Number(amount);
  return Number.isFinite(n) ? n : 0;
}

export const color = {
  lighten: (value, amount) => {
    const parsed = parseColor(value);
    if (!parsed) return text(value);
    const hsl = toHsl(parsed);
    hsl.l = num.clamp(hsl.l + percentAmount(amount), 0, 100);
    return toCss(fromHsl(hsl, parsed.a));
  },
  darken: (value, amount) => {
    const parsed = parseColor(value);
    if (!parsed) return text(value);
    const hsl = toHsl(parsed);
    hsl.l = num.clamp(hsl.l - percentAmount(amount), 0, 100);
    return toCss(fromHsl(hsl, parsed.a));
  },
  saturate: (value, amount) => {
    const parsed = parseColor(value);
    if (!parsed) return text(value);
    const hsl = toHsl(parsed);
    hsl.s = num.clamp(hsl.s + percentAmount(amount), 0, 100);
    return toCss(fromHsl(hsl, parsed.a));
  },
  desaturate: (value, amount) => color.saturate(value, -percentAmount(amount)),
  alpha: (value, amount) => {
    const parsed = parseColor(value);
    if (!parsed) return text(value);
    parsed.a = num.clamp(Number(amount), 0, 1);
    return toCss(parsed);
  },
  mix: (a, b, amount) => {
    const first = parseColor(a);
    const second = parseColor(b);
    if (!first || !second) return text(a);
    const t = amount === undefined ? 0.5 : num.clamp(percentAmount(amount) > 1 ? percentAmount(amount) / 100 : percentAmount(amount), 0, 1);
    return toCss({
      r: first.r + (second.r - first.r) * t,
      g: first.g + (second.g - first.g) * t,
      b: first.b + (second.b - first.b) * t,
      a: first.a + (second.a - first.a) * t,
    });
  },
  complement: (value) => {
    const parsed = parseColor(value);
    if (!parsed) return text(value);
    const hsl = toHsl(parsed);
    hsl.h = (hsl.h + 180) % 360;
    return toCss(fromHsl(hsl, parsed.a));
  },
  grayscale: (value) => {
    const parsed = parseColor(value);
    if (!parsed) return text(value);
    const hsl = toHsl(parsed);
    hsl.s = 0;
    return toCss(fromHsl(hsl, parsed.a));
  },
  toHex: (value) => {
    const parsed = parseColor(value);
    return parsed ? toCss({ ...parsed, a: 1 }) : text(value);
  },
};

function channelLuminance(value) {
  const v = value / 255;
  return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
}

/** WCAG 2.x relative luminance. */
export function luminance(value) {
  const parsed = parseColor(value);
  if (!parsed) return 0;
  return (
    0.2126 * channelLuminance(parsed.r) +
    0.7152 * channelLuminance(parsed.g) +
    0.0722 * channelLuminance(parsed.b)
  );
}

/** WCAG contrast ratio between two colours, from 1 to 21. */
export function contrastRatio(a, b) {
  const la = luminance(a);
  const lb = luminance(b);
  const lighter = Math.max(la, lb);
  const darker = Math.min(la, lb);
  return (lighter + 0.05) / (darker + 0.05);
}

/** Black or white, whichever reads better on `background`. */
export function contrastColor(background) {
  return contrastRatio(background, '#ffffff') >= contrastRatio(background, '#000000')
    ? '#ffffff'
    : '#000000';
}

/* ------------------------------------------------------------------------- *
 * Standard library: URLs and timers
 * ------------------------------------------------------------------------- */

export function parseUrl(value) {
  const input = text(value);
  try {
    const url = new URL(input, typeof window === 'undefined' ? 'http://localhost/' : window.location.href);
    const query = {};
    for (const [key, item] of url.searchParams.entries()) query[key] = item;
    return {
      href: url.href,
      origin: url.origin,
      host: url.host,
      hostname: url.hostname,
      port: url.port,
      path: url.pathname,
      query,
      hash: url.hash,
    };
  } catch (_) {
    return { href: input, origin: '', host: '', hostname: '', port: '', path: input, query: {}, hash: '' };
  }
}

export function buildUrl(path, params) {
  const query = params
    ? Object.keys(params)
        .filter((key) => params[key] !== null && params[key] !== undefined)
        .map((key) => `${encodeURIComponent(key)}=${encodeURIComponent(text(params[key]))}`)
        .join('&')
    : '';
  return query ? `${text(path)}?${query}` : text(path);
}

export const encodeUri = (value) => encodeURIComponent(text(value));
export const decodeUri = (value) => decodeURIComponent(text(value));

/** `setTimeout` wrapper that is disposed with the enclosing scope. */
export function after(delay, fn) {
  const id = setTimeout(() => {
    if (typeof fn === 'function') fn();
  }, Number(delay) || 0);
  own(() => clearTimeout(id));
  return id;
}

/** `setInterval` wrapper that is disposed with the enclosing scope. */
export function every(delay, fn) {
  const id = setInterval(() => {
    if (typeof fn === 'function') fn();
  }, Number(delay) || 0);
  own(() => clearInterval(id));
  return id;
}

/* ------------------------------------------------------------------------- *
 * Components
 * ------------------------------------------------------------------------- */

/**
 * Read a prop.
 *
 * Generated code passes props as an object of accessors — `Card({ title: () =>
 * "Hello" })` — and reads them with `P.prop(props, "title", () => "")`. The
 * result is always an accessor, never a value, so reading it inside an effect
 * keeps that effect subscribed to the parent: a prop is a wire, not a snapshot.
 *
 * A prop that was not passed, or that is currently `nothing`, gets `fallback`.
 * The fallback is applied on every read rather than once at creation, so a prop
 * that arrives later still shows up.
 *
 * A plain value is accepted too and wrapped in a constant accessor. That is what
 * makes a component callable from ordinary JavaScript without a build step.
 */
export function prop(props, name, fallback) {
  const given = props ? props[name] : undefined;
  if (typeof given === 'function') {
    if (!fallback) return given;
    return () => {
      const value = given();
      return value === undefined ? fallback() : value;
    };
  }
  if (given !== undefined) return () => given;
  return fallback || (() => undefined);
}

/**
 * Read the nodes a parent put in a slot.
 *
 * Children are built eagerly in the parent's own scope and handed down, so this
 * only gives them back — a component cannot decide *when* to build them, only
 * where to put them. `P.slot(props)` is the default slot; `P.slot(props, name)`
 * is a named one, and an unfilled slot is empty rather than an error.
 */
export function slot(props, name) {
  if (!props) return [];
  if (name === undefined || name === 'children') return props.children || [];
  const slots = props.slots;
  return (slots && slots[name]) || [];
}

/**
 * An inclusive range: `each page in 1..10` gives ten rows, and `10..1` counts
 * down. A range is materialised when it is read because `each` needs an array,
 * so it is meant for pagination, star ratings and skeleton rows — not for
 * counting to a million. A range that big is refused with an error instead of
 * hanging the tab.
 */
export function range(from, to, step) {
  const start = Number(from) || 0;
  const end = Number(to) || 0;
  const by = Math.abs(Number(step)) || 1;
  const count = Math.floor(Math.abs(end - start) / by) + 1;
  if (count > MAX_RANGE) {
    reportError(
      new Error(
        `range ${start}..${end} would produce ${count} items; ` +
          `Plain refuses to build more than ${MAX_RANGE}`,
      ),
    );
    return [];
  }
  const out = [];
  if (start <= end) for (let i = start; i <= end; i += by) out.push(i);
  else for (let i = start; i >= end; i -= by) out.push(i);
  return out;
}

/** The largest range `each` will build. */
const MAX_RANGE = 100000;

function setIn(node, keys, depth, value) {
  const key = keys[depth];
  const child =
    depth === keys.length - 1
      ? value
      : setIn(node == null ? undefined : node[key], keys, depth + 1, value);
  if (Array.isArray(node)) {
    const copy = node.slice();
    copy[Number(key)] = child;
    return copy;
  }
  const copy = node && typeof node === 'object' ? Object.assign({}, node) : {};
  copy[key] = child;
  return copy;
}

/**
 * Write to a path inside reactive state.
 *
 * `user.name = "Ada"` becomes `P.assign(user, ["name"], "Ada")` and
 * `items[i].done = true` becomes `P.assign(items, [i(), "done"], true)`.
 *
 * Values are treated as immutable: every level on the way down is copied and the
 * root signal is written once. Nothing that already read the old value sees it
 * change underneath, which is what keeps derived values and list rows honest —
 * and what makes a change detectable at all, since a signal that is handed the
 * same object back has nothing to report.
 */
export function assign(target, keys, value) {
  if (!target || typeof target.set !== 'function') {
    reportError(new Error('cannot assign to something that is not reactive state'));
    return value;
  }
  if (!keys || keys.length === 0) {
    target.set(value);
    return value;
  }
  const next = setIn(target(), keys, 0, value);
  target.set(next);
  return next;
}

/* ------------------------------------------------------------------------- *
 * Public namespace
 * ------------------------------------------------------------------------- */

export function onError(handler) {
  errorHandler = handler;
}

/** The namespace generated Plain code imports. */
export const P = {
  // reactivity
  signal,
  computed,
  effect,
  batch,
  flush,
  untrack,
  scope,
  own,
  // dom
  el,
  comment,
  append,
  toNodes,
  setText,
  setAttr,
  setProp,
  setStyle,
  setClass,
  on,
  bind,
  when,
  each,
  afterMount,
  mount,
  focus,
  focusFirst,
  trapFocus,
  // components
  prop,
  slot,
  range,
  assign,
  // platform
  store,
  session,
  persisted,
  clipboard,
  device,
  after,
  every,
  onError,
  text,
  // standard library
  str,
  list,
  num,
  color,
  datetime,
  slug,
  truncate,
  formatNumber,
  formatCurrency,
  formatPercent,
  formatCompact,
  formatOrdinal,
  formatBytes,
  formatDate,
  relativeTime,
  parseColor,
  luminance,
  contrastRatio,
  contrastColor,
  parseUrl,
  buildUrl,
  encodeUri,
  decodeUri,
};

export default P;
