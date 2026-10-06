// The page: notes of the session on the left, a collaborative editor on the right.
// Documents are Y.Docs here; the wasm `Session` only carries their y-sync messages.
import * as Y from "yjs";
import * as sync from "y-protocols/sync";
import * as awarenessProtocol from "y-protocols/awareness";
import * as encoding from "lib0/encoding";
import * as decoding from "lib0/decoding";
import { EditorView, minimalSetup } from "codemirror";
import { EditorState } from "@codemirror/state";
import { markdown } from "@codemirror/lang-markdown";
import { yCollab } from "y-codemirror.next";
import init, { Session } from "../pkg/deepika_sync_web.js";

const MESSAGE_SYNC = 0, MESSAGE_AWARENESS = 1, MESSAGE_QUERY_AWARENESS = 3;
const REMOTE = Symbol("remote");
/** A sheet of prose rather than a code editor: no gutter, a measured column. */
const SHEET = EditorView.theme({
  "&": { height: "100%", fontSize: "15px", backgroundColor: "transparent", color: "inherit" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "1.75" },
  ".cm-content": { maxWidth: "48rem", margin: "0 auto", padding: "2rem 1.5rem", caretColor: "currentColor" },
  ".cm-cursor": { borderLeftColor: "currentColor" },
});
const COLORS = ["#b63257", "#386ac6", "#25806a", "#9861b2", "#b36b20"];

const $ = (id) => document.getElementById(id);
/** The status pill: "busy" while something is under way, then "ok" or "error". */
const state = (text, kind = "busy") => { $("state-text").textContent = text; $("state").dataset.kind = kind; };

let session;
/** Every note of the session: id -> { ydoc, meta, awareness?, ready, synced? }. */
const docs = new Map();
let view, current;

function frame(write) {
  const encoder = encoding.createEncoder();
  write(encoder);
  return encoding.toUint8Array(encoder);
}

/**
 * Track a note. Path and tombstone live in the document's own `metadata` map, so holding
 * every document is what keeps the list live: a note created, renamed or deleted anywhere
 * reaches this page as an ordinary update.
 */
function track(id, local = false) {
  let doc = docs.get(id);
  if (doc) return doc;
  const ydoc = new Y.Doc();
  doc = { ydoc, meta: ydoc.getMap("metadata"), ready: local };
  docs.set(id, doc);
  ydoc.on("update", (update, origin) => {
    if (origin !== REMOTE) session.send(id, frame((e) => { encoding.writeVarUint(e, MESSAGE_SYNC); sync.writeUpdate(e, update); }));
  });
  doc.meta.observe(() => metadataChanged(id));
  // Ask the session for everything it has about this note.
  if (!local) session.send(id, frame((e) => { encoding.writeVarUint(e, MESSAGE_SYNC); sync.writeSyncStep1(e, ydoc); }));
  return doc;
}

function awarenessOf(id) {
  const doc = track(id);
  if (!doc.awareness) {
    doc.awareness = new awarenessProtocol.Awareness(doc.ydoc);
    doc.awareness.on("change", () => { if (id === current) drawPeople(); });
    doc.awareness.on("update", ({ added, updated, removed }, origin) => {
      if (origin === REMOTE) return;
      const changed = added.concat(updated, removed);
      session.send(id, frame((e) => {
        encoding.writeVarUint(e, MESSAGE_AWARENESS);
        encoding.writeVarUint8Array(e, awarenessProtocol.encodeAwarenessUpdate(doc.awareness, changed));
      }));
    });
  }
  return doc.awareness;
}

function onMessage(docId, bytes) {
  const doc = track(docId);
  const decoder = decoding.createDecoder(bytes);
  const type = decoding.readVarUint(decoder);
  if (type === MESSAGE_SYNC) {
    const reply = encoding.createEncoder();
    encoding.writeVarUint(reply, MESSAGE_SYNC);
    const kind = sync.readSyncMessage(decoder, reply, doc.ydoc, REMOTE);
    if (encoding.length(reply) > 1) session.send(docId, encoding.toUint8Array(reply));
    if (kind !== sync.messageYjsSyncStep1 && !doc.ready) { doc.ready = true; doc.synced?.(); doc.synced = undefined; }
  } else if (type === MESSAGE_AWARENESS && doc.awareness) {
    awarenessProtocol.applyAwarenessUpdate(doc.awareness, decoding.readVarUint8Array(decoder), REMOTE);
  }
}

const pathOf = (id) => docs.get(id)?.meta.get("path") || "";
const isLive = (id) => !!pathOf(id) && docs.get(id).meta.get("deleted") !== true;

let drawing = false;
function metadataChanged(id) {
  if (id === current) {
    const winner = docs.get(id).meta.get("superseded_by");
    if (winner) openNote(winner); // an identical copy was merged into another note
    else if (!isLive(id)) closeNote(`« ${$("title").textContent} » a été supprimée.`);
    else $("title").textContent = pathOf(id);
  }
  if (drawing) return;
  drawing = true;
  requestAnimationFrame(() => { drawing = false; drawList(); });
}

function drawList() {
  const wanted = $("filter").value.trim().toLowerCase();
  const all = [...docs.keys()].filter(isLive);
  const ids = all.filter((id) => pathOf(id).toLowerCase().includes(wanted)).sort((a, b) => pathOf(a).localeCompare(pathOf(b)));
  $("notes").replaceChildren(...ids.map((id) => {
    const path = pathOf(id), cut = path.lastIndexOf("/") + 1;
    const item = document.createElement("li");
    item.dataset.id = id;
    item.className = "group mb-0.5 cursor-pointer truncate rounded-lg px-2.5 py-1.5 text-sm transition " + (id === current
      ? "bg-indigo-50 font-medium text-indigo-700 dark:bg-indigo-950 dark:text-indigo-300"
      : "text-zinc-700 hover:bg-zinc-100 dark:text-zinc-300 dark:hover:bg-zinc-800");
    // Folder dimmed, name in front; together they still read as the path.
    const folder = Object.assign(document.createElement("span"), { textContent: path.slice(0, cut), className: "text-zinc-400 dark:text-zinc-500" });
    item.append(folder, path.slice(cut));
    item.title = path;
    item.addEventListener("click", () => openNote(id));
    return item;
  }));
  $("count").textContent = wanted ? `${ids.length} sur ${all.length} notes` : `${all.length} note${all.length > 1 ? "s" : ""}`;
}

/** Who else has this note open, as initials in their cursor's color. */
function drawPeople() {
  const awareness = docs.get(current)?.awareness;
  const others = awareness ? [...awareness.getStates()].filter(([client, s]) => client !== awareness.clientID && s.user?.name) : [];
  $("people").replaceChildren(...others.map(([, s]) => {
    const chip = Object.assign(document.createElement("span"), { textContent: s.user.name.trim().slice(0, 2).toUpperCase(), title: s.user.name });
    chip.className = "grid size-7 place-items-center rounded-full text-[11px] font-semibold text-white ring-2 ring-white dark:ring-zinc-900";
    chip.style.background = s.user.color ?? "#71717a";
    return chip;
  }));
}

function closeNote(message) {
  docs.get(current)?.awareness?.setLocalState(null);
  current = undefined;
  view?.destroy();
  view = undefined;
  $("title-empty").textContent = message;
  delete document.body.dataset.open;
  drawPeople();
  window.deepikaSync = { docs };
  drawList();
}

function openNote(id) {
  if (current && current !== id) docs.get(current)?.awareness?.setLocalState(null);
  current = id;
  const doc = track(id);
  const awareness = awarenessOf(id);
  const name = $("name").value;
  const color = COLORS[Math.abs([...name].reduce((h, c) => (Math.imul(h, 31) + c.charCodeAt(0)) | 0, 0)) % COLORS.length];
  awareness.setLocalStateField("user", { name, color, colorLight: color + "40" });
  // Awareness travels as changes: ask who is already in the note.
  session.send(id, frame((e) => encoding.writeVarUint(e, MESSAGE_QUERY_AWARENESS)));

  const show = () => {
    if (current !== id) return;
    view?.destroy();
    const ytext = doc.ydoc.getText("content");
    view = new EditorView({
      parent: $("editor"),
      state: EditorState.create({
        doc: ytext.toString(),
        extensions: [minimalSetup, SHEET, markdown(), EditorView.lineWrapping, yCollab(ytext, awareness)],
      }),
    });
    $("title").textContent = pathOf(id);
    document.body.dataset.open = id;
    drawPeople();
    window.deepikaSync = { view, doc, docs }; // handy in the console, and for the automated check
    drawList();
  };
  // Show the editor once the note's text has arrived, so it never starts empty by mistake.
  if (doc.ready) show(); else doc.synced = show;
}

/** A path as the daemon stores it: relative, `/`-separated, ending in `.md`. */
function notePath(input) {
  const parts = input.trim().replaceAll("\\", "/").split("/").map((p) => p.trim()).filter(Boolean);
  if (!parts.length || parts.some((p) => p === "." || p === ".." || p.startsWith(".collab"))) return null;
  const path = parts.join("/");
  return /\.md$/i.test(path) ? path : `${path}.md`;
}

function taken(path, except) {
  return [...docs.keys()].some((id) => id !== except && isLive(id) && pathOf(id) === path);
}

function askPath(question, initial, except) {
  const answer = window.prompt(question, initial);
  if (answer === null) return null;
  const path = notePath(answer);
  if (!path) return state("Nom de note invalide.", "error"), null;
  if (taken(path, except)) return state(`« ${path} » existe déjà.`, "error"), null;
  return path;
}

function createNote() {
  const path = askPath("Chemin de la nouvelle note (les dossiers se créent tout seuls) :", "Sans titre.md");
  if (!path) return;
  const id = crypto.randomUUID();
  const doc = track(id, true);
  // One transaction, so the session learns the note and where it lives at once.
  doc.ydoc.transact(() => { doc.meta.set("path", path); doc.meta.set("deleted", false); doc.ydoc.getText("content"); });
  openNote(id);
}

function renameNote() {
  const path = askPath("Nouveau chemin de la note :", pathOf(current), current);
  if (path && path !== pathOf(current)) docs.get(current).meta.set("path", path);
}

function deleteNote() {
  if (window.confirm(`Supprimer « ${pathOf(current)} » pour tout le monde ?`)) docs.get(current).meta.set("deleted", true);
}

async function start() {
  // Changing only the fragment does not reload a page: do it ourselves, so an invitation
  // pasted into the address bar of an already open page is taken into account.
  window.addEventListener("hashchange", () => location.reload());
  const invitation = decodeURIComponent(location.hash.slice(1)).trim();
  $("name").value = localStorage.getItem("deepika-sync.name") || `Invité ${Math.floor(Math.random() * 90 + 10)}`;
  $("name").addEventListener("change", () => {
    localStorage.setItem("deepika-sync.name", $("name").value);
    const awareness = docs.get(current)?.awareness;
    awareness?.setLocalStateField("user", { ...awareness.getLocalState()?.user, name: $("name").value });
  });
  // Also shown again when joining fails, to try another invitation.
  $("ask").addEventListener("submit", (event) => {
    event.preventDefault();
    // Accept the bare invitation as well as a whole link containing it.
    const pasted = $("invitation").value.trim();
    const next = pasted.includes("#") ? pasted.slice(pasted.indexOf("#") + 1) : pasted;
    if (next === invitation) location.reload(); else location.hash = next;
  });
  if (!invitation) {
    document.body.dataset.view = "ask";
    return state("En attente d'une invitation", "busy");
  }
  try {
    await init();
    state("Connexion à l'hôte par le relais…");
    const began = performance.now();
    session = await Session.connect(invitation, onMessage, (reason) => state(`Connexion perdue : ${reason}. Rechargez la page.`, "error"));
    state(`Connecté · session jointe en ${Math.round(performance.now() - began)} ms`, "ok");
    for (const note of JSON.parse(session.manifest)) if (!note.deleted) track(note.id);
    document.body.dataset.view = "session";
    $("filter").addEventListener("input", drawList);
    $("new").addEventListener("click", createNote);
    $("rename").addEventListener("click", renameNote);
    $("delete").addEventListener("click", deleteNote);
    drawList();
    document.body.dataset.result = "ok";
  } catch (error) {
    document.body.dataset.view = "ask";
    state(`Échec : ${error.message ?? error}`, "error");
    document.body.dataset.result = "error";
  }
}
start();
