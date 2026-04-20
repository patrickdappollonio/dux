// Browser entry point for dux remote share.
//
// Loads the dux-web-browser WASM module, manages the pair/terminal screens,
// drives the render loop (FullFrame / FrameDiff cell updates to a DOM grid),
// and captures keyboard input via the Keyboard Lock API with a 500 ms
// hold-Esc gesture to release the lock.
//
// The wire format (bincode RemoteMessage) is owned by the WASM side; here
// we only speak the JSON shape serde_json produces. Variants arrive as
// either bare strings (e.g. "LeaderRequest") or one-key objects
// (e.g. {"FrameDiff": {seq, cells}}) — the dispatch below handles both.

import init, { Session } from "./pkg/dux_web_browser.js";

// ── DOM handles ─────────────────────────────────────────────────────
const $ = (id) => document.getElementById(id);
const pairScreen = $("pair-screen");
const termScreen = $("terminal-screen");
const codeInput = $("code");
const pairStatus = $("pair-status");
const pairForm = $("pair-form");
const gridEl = $("grid");
const leaderEl = $("leader");
const modalEl = $("modal");
const modalText = $("modal-text");
const modalConfirm = $("modal-confirm");
const modalCancel = $("modal-cancel");
const bannerEl = $("banner");

// ── State ───────────────────────────────────────────────────────────
let session = null;
let gridCells = null; // 2D row/col array of cell <span> nodes
let gridCols = 0;
let gridRows = 0;

const keysHeld = new Set();
let escHoldTimer = null;
const ESC_HOLD_MS = 500;

// Keys the Keyboard Lock API should capture. Browser-reserved shortcuts
// that users frequently forward to agent CLIs (Ctrl-W in shells, Ctrl-T to
// cycle tabs in interactive sessions, function keys in vim/emacs).
const RESERVED_KEYS = [
  "Escape", "Tab",
  "F1", "F2", "F3", "F4", "F5", "F6",
  "F7", "F8", "F9", "F10", "F11", "F12",
  "KeyW", "KeyT", "KeyN", "KeyR", "KeyL", "KeyQ",
  "MetaLeft", "MetaRight", "ControlLeft", "ControlRight",
  "AltLeft", "AltRight",
];

// ── Bootstrap ───────────────────────────────────────────────────────
async function boot() {
  try {
    await init();
  } catch (e) {
    pairStatus.textContent = `failed to load WASM module: ${e}`;
    return;
  }

  pairForm.addEventListener("submit", (ev) => {
    ev.preventDefault();
    void onConnect();
  });

  modalConfirm.addEventListener("click", () => confirmDisconnect());
  modalCancel.addEventListener("click", () => dismissModal());

  if (!navigator.keyboard || !navigator.keyboard.lock) {
    showBanner(
      "This browser does not support the Keyboard Lock API — system shortcuts " +
      "(Ctrl-W, Ctrl-T, Alt-Tab) will not reach the host. Chrome, Edge, and " +
      "Chromium on ChromeOS support it.",
    );
  }
}

// ── Connect flow ───────────────────────────────────────────────────
async function onConnect() {
  const code = codeInput.value.trim();
  if (!code) {
    pairStatus.textContent = "paste a pairing code first";
    return;
  }
  pairStatus.textContent = "connecting…";
  try {
    session = await Session.connect(
      code,
      navigator.userAgent || "dux-web-browser",
      null,
    );
  } catch (e) {
    pairStatus.textContent = String(e);
    session = null;
    return;
  }
  enterTerminal();
  setLeader("connected to '" + session.host_label + "'");
  try {
    await navigator.keyboard?.lock?.(RESERVED_KEYS);
  } catch (e) {
    // Non-fatal: the banner already warned the user. Keep going.
    console.warn("keyboard lock failed:", e);
  }
  document.addEventListener("keydown", onKeyDown, true);
  document.addEventListener("keyup", onKeyUp, true);
  void readLoop();
}

function enterTerminal() {
  pairScreen.hidden = true;
  termScreen.hidden = false;
}

function exitTerminal() {
  document.removeEventListener("keydown", onKeyDown, true);
  document.removeEventListener("keyup", onKeyUp, true);
  try {
    navigator.keyboard?.unlock?.();
  } catch {}
  termScreen.hidden = true;
  pairScreen.hidden = false;
  pairStatus.textContent = "";
  codeInput.value = "";
  codeInput.focus();
  session = null;
  clearGrid();
}

// ── Render loop ────────────────────────────────────────────────────
async function readLoop() {
  while (session) {
    let json;
    try {
      json = await session.next_message();
    } catch (e) {
      showBanner("Disconnected: " + String(e));
      exitTerminal();
      return;
    }
    let msg;
    try {
      msg = JSON.parse(json);
    } catch (e) {
      console.error("bad JSON from WASM:", json, e);
      continue;
    }
    handleMessage(msg);
  }
}

// serde_json emits external-tagged enums: unit variants as bare strings
// ("LeaderRequest"), data variants as single-key objects
// ({"FrameDiff": {...}}).  Normalise both shapes into {kind, data}.
function normalise(msg) {
  if (typeof msg === "string") return { kind: msg, data: null };
  const keys = Object.keys(msg);
  if (keys.length === 1) return { kind: keys[0], data: msg[keys[0]] };
  return { kind: "Unknown", data: msg };
}

function handleMessage(raw) {
  const { kind, data } = normalise(raw);
  switch (kind) {
    case "Hello":
      // Host advertises capabilities / peer label; we already stored the
      // label via session.host_label. Nothing else to do.
      break;
    case "FullFrame":
      renderFullFrame(data);
      break;
    case "FrameDiff":
      applyFrameDiff(data.cells);
      break;
    case "PtySnapshotDiff":
      // PTY snapshots go through the same cell grid for now — the host
      // writes ratatui chrome and the PTY cells into the composited
      // FrameDiff stream already, so we ignore this dedicated channel in
      // v1 and let the chrome stream drive all rendering.
      break;
    case "Resize":
      resizeGrid(data.cols, data.rows);
      break;
    case "LeaderChange":
      setLeader(data.leader === "Client" ? "you are driving" : "host is driving");
      break;
    case "LeaderResponse":
      if (!data.granted) {
        showBanner("Host denied your lead-request.");
      }
      break;
    case "Ping":
      // WASM side already replies with Pong — nothing to do in the UI.
      break;
    case "Bye":
      showBanner("Host closed the connection: " + JSON.stringify(data?.reason ?? "Graceful"));
      exitTerminal();
      break;
    default:
      console.debug("unhandled remote message", kind, data);
  }
}

// ── Grid rendering ─────────────────────────────────────────────────
function clearGrid() {
  gridEl.replaceChildren();
  gridCells = null;
  gridCols = 0;
  gridRows = 0;
}

function resizeGrid(cols, rows) {
  if (cols === gridCols && rows === gridRows) return;
  gridCols = cols;
  gridRows = rows;
  gridEl.replaceChildren();
  gridCells = Array.from({ length: rows }, () => Array(cols).fill(null));
  // Cells are added lazily as diffs arrive — we don't pre-populate
  // because empty cells cost DOM nodes for no benefit.
}

function renderFullFrame(frame) {
  const { cols, rows, cells } = frame;
  resizeGrid(cols, rows);
  // A FullFrame is a superset keyframe — wipe the grid first, then apply
  // every cell as a diff.
  for (const row of gridCells) for (let i = 0; i < row.length; i++) row[i] = null;
  gridEl.replaceChildren();
  applyFrameDiff(cells);
}

function applyFrameDiff(cells) {
  if (!gridCells) return;
  for (const c of cells) {
    paintCell(c);
  }
}

function paintCell(c) {
  const { row, col, symbol, fg, bg, modifier } = c;
  if (row >= gridRows || col >= gridCols) return;
  let node = gridCells[row]?.[col];
  if (!node) {
    node = document.createElement("span");
    node.className = "cell";
    node.style.left = `calc(${col} * var(--cell))`;
    node.style.top = `calc(${row} * var(--row))`;
    gridEl.appendChild(node);
    gridCells[row][col] = node;
  }
  node.textContent = symbol;
  node.className = "cell " + classListForAttrs(fg, bg, modifier);
  // Rgb and Indexed colors override class-based styling.
  const fgStyle = inlineColor(fg, "color");
  const bgStyle = inlineColor(bg, "background-color");
  node.style.color = fgStyle;
  node.style.backgroundColor = bgStyle;
}

function classListForAttrs(fg, bg, modifier) {
  const cls = [];
  if (typeof fg === "string") cls.push("fg-" + fg);
  if (typeof bg === "string") cls.push("bg-" + bg);
  // Modifier is a bitmask matching ratatui's Modifier::bits().
  // Match the most common visible attributes; the rest are ignored.
  if (modifier & 0x0001) cls.push("bold");
  if (modifier & 0x0002) cls.push("dim");
  if (modifier & 0x0004) cls.push("italic");
  if (modifier & 0x0008) cls.push("underlined");
  if (modifier & 0x0040) cls.push("reversed");
  if (modifier & 0x0080) cls.push("hidden");
  if (modifier & 0x0100) cls.push("crossedout");
  return cls.join(" ");
}

function inlineColor(c, _kind) {
  if (typeof c === "string") return ""; // class handles it
  if (c && c.Rgb) {
    const [r, g, b] = c.Rgb;
    return `rgb(${r},${g},${b})`;
  }
  if (c && typeof c.Indexed === "number") {
    // 256-color palette indexing is approximated through the basic 16 +
    // 6×6×6 cube + grayscale ramp.  For v1 we map indexed to the basic 16
    // when we can and fall through otherwise.
    return ansi256ToCss(c.Indexed);
  }
  return "";
}

function ansi256ToCss(i) {
  if (i < 16) {
    // Standard / bright ansi — the CSS classes already map these, but we
    // still need a value here because `inlineColor` was called.
    const names = ["#000","#c41b1b","#2a9f54","#c08f1c","#2470c4","#a53da5","#1f9ea5","#b0b6bd","#606770","#ff6b6b","#5fd68f","#ffc759","#5ea7ff","#d97ad9","#5fd7d7","#f0f2f4"];
    return names[i];
  }
  if (i >= 232) {
    // Grayscale ramp: 24 steps from 8 to 238.
    const g = 8 + (i - 232) * 10;
    return `rgb(${g},${g},${g})`;
  }
  // 6×6×6 cube.
  const idx = i - 16;
  const r = Math.floor(idx / 36);
  const g = Math.floor((idx % 36) / 6);
  const b = idx % 6;
  const scale = (v) => (v === 0 ? 0 : 55 + v * 40);
  return `rgb(${scale(r)},${scale(g)},${scale(b)})`;
}

// ── Keyboard capture ───────────────────────────────────────────────
function onKeyDown(e) {
  // Dedupe OS key-repeat; dux's input dispatch doesn't want auto-repeats
  // as distinct presses.
  if (keysHeld.has(e.code)) {
    e.preventDefault();
    return;
  }
  keysHeld.add(e.code);

  if (e.code === "Escape") {
    escHoldTimer = setTimeout(onEscHeld, ESC_HOLD_MS);
  }

  const wireKey = domToWireKey(e);
  if (wireKey && session) {
    void session.send_input_key(JSON.stringify(wireKey)).catch((err) => {
      console.warn("send_input_key failed:", err);
    });
  }
  e.preventDefault();
}

function onKeyUp(e) {
  keysHeld.delete(e.code);
  if (e.code === "Escape" && escHoldTimer !== null) {
    clearTimeout(escHoldTimer);
    escHoldTimer = null;
  }
  e.preventDefault();
}

function onEscHeld() {
  escHoldTimer = null;
  try {
    navigator.keyboard?.unlock?.();
  } catch {}
  keysHeld.clear();
  showModal();
}

function showModal() {
  modalText.textContent = "Disconnect from the host?";
  modalEl.hidden = false;
  modalCancel.focus();
}

function dismissModal() {
  modalEl.hidden = true;
  // Re-engage the lock; the render loop keeps running.
  void navigator.keyboard?.lock?.(RESERVED_KEYS).catch(() => {});
}

function confirmDisconnect() {
  modalEl.hidden = true;
  if (session) {
    session.close().catch(() => {});
  }
  exitTerminal();
}

// DOM KeyboardEvent → WireKeyEvent JSON.
//
// The wire type serialises externally-tagged: unit variants as strings,
// data variants as single-key objects. For example the user pressing 'a'
// yields `{code: {Char: "a"}, modifiers: 0, kind: "Press"}`.
function domToWireKey(e) {
  let code;
  let shiftEat = false;

  switch (e.code) {
    case "Backspace":    code = "Backspace"; break;
    case "Enter":
    case "NumpadEnter":  code = "Enter"; break;
    case "ArrowLeft":    code = "Left"; break;
    case "ArrowRight":   code = "Right"; break;
    case "ArrowUp":      code = "Up"; break;
    case "ArrowDown":    code = "Down"; break;
    case "Home":         code = "Home"; break;
    case "End":          code = "End"; break;
    case "PageUp":       code = "PageUp"; break;
    case "PageDown":     code = "PageDown"; break;
    case "Tab":
      code = e.shiftKey ? "BackTab" : "Tab";
      if (e.shiftKey) shiftEat = true;
      break;
    case "Delete":       code = "Delete"; break;
    case "Insert":       code = "Insert"; break;
    case "Escape":       code = "Esc"; break;
    case "CapsLock":     code = "CapsLock"; break;
    case "ScrollLock":   code = "ScrollLock"; break;
    case "NumLock":      code = "NumLock"; break;
    case "PrintScreen":  code = "PrintScreen"; break;
    case "Pause":        code = "Pause"; break;
    default: {
      if (e.code.startsWith("F") && /^F\d+$/.test(e.code)) {
        code = { F: Number(e.code.slice(1)) };
      } else if (e.key && e.key.length === 1) {
        code = { Char: e.key };
      } else {
        return null;
      }
    }
  }

  let modifiers = 0;
  if (e.shiftKey && !shiftEat) modifiers |= 0x01;
  if (e.ctrlKey) modifiers |= 0x02;
  if (e.altKey) modifiers |= 0x04;
  if (e.metaKey) modifiers |= 0x08;

  const kind = e.repeat ? "Repeat" : "Press";
  return { code, modifiers, kind };
}

// ── UI helpers ─────────────────────────────────────────────────────
function setLeader(text) {
  leaderEl.textContent = text;
}

function showBanner(text) {
  bannerEl.textContent = text;
  bannerEl.hidden = false;
}

// ── Go ─────────────────────────────────────────────────────────────
boot();
