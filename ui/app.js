"use strict";

const { invoke, Channel } = window.__TAURI__.core;
const { open: openDialog } = window.__TAURI__.dialog;

const STRATA = ["#5566d0", "#f2ae2e", "#5fb9a5", "#dd7419", "#97a3ee", "#14917f", "#c9d0f5"];
const MARKS = ["#97a3ee", "#f2ae2e", "#5fb9a5", "#ec9a5e", "#c9d0f5", "#7fd1c2"];
const ICONS = {
  home: "M3 11l9-8 9 8M5 10v10h14V10",
  clean: "M12 3l9 5-9 5-9-5 9-5zM3 13l9 5 9-5M3 17.5l9 5 9-5",
  dups: "M9 9h11v11H9zM5 15H4V4h11v1",
  programs: "M4 4h6v6H4zM14 4h6v6h-6zM4 14h6v6H4zM14 14h6v6h-6z",
  startup: "M12 3v9M6.3 6.3a8 8 0 1 0 11.4 0",
  perf: "M13 2L4 14h7l-1 8 9-12h-7l1-8z",
  soft: "M12 3v12M7 11l5 5 5-5M4 21h16",
  drivers: "M7 7h10v10H7zM10 3v4M14 3v4M10 17v4M14 17v4M3 10h4M3 14h4M17 10h4M17 14h4",
};

function icon(path) {
  const namespace = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(namespace, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("class", "nav-icon");
  svg.setAttribute("aria-hidden", "true");
  const shape = document.createElementNS(namespace, "path");
  shape.setAttribute("d", path);
  svg.append(shape);
  return svg;
}

/* Icons already asked for; null means the file has none and the initial stays. */
const icons = new Map();

async function requestIcons(sources) {
  const missing = [...new Set(sources.filter((source) => source && !icons.has(source)))];
  if (!missing.length) return;
  for (const source of missing) icons.set(source, null);
  try {
    const found = await invoke("file_icons", { sources: missing });
    for (const [source, image] of Object.entries(found)) icons.set(source, image);
    render();
  } catch {
    // Icons are decorative: the coloured initials stay in place.
  }
}

const bareName = (name) => name.toLowerCase()
  .replace(/\(.*?\)/g, " ")
  .replace(/\bv?\d[\d.+-]*/g, " ")
  .replace(/[^\p{L}\p{N}]+/gu, " ")
  .trim();

/* winget only gives a name, so the icon comes from the installed program of that name. */
function installedIconSource(name) {
  const wanted = bareName(name);
  const programs = (state.programs.items || []).filter((program) => program.icon_source);
  const exact = programs.find((program) => bareName(program.name) === wanted);
  if (exact || wanted.length < 4) return exact?.icon_source || null;
  return programs.find((program) => {
    const candidate = bareName(program.name);
    return candidate.length >= 4 && (candidate.startsWith(wanted) || wanted.startsWith(candidate));
  })?.icon_source || null;
}

/* The program's icon, or a coloured initial, stable for a given name. */
function mark(name, source) {
  const image = source ? icons.get(source) : null;
  if (image) return h("span", { class: "mark has-icon", "aria-hidden": "true" }, h("img", { src: image, alt: "" }));
  const letter = (name.match(/[\p{L}\p{N}]/u) || ["?"])[0].toUpperCase();
  let hash = 0;
  for (const character of name) hash = (hash * 31 + character.codePointAt(0)) >>> 0;
  return h("span", { class: "mark", style: `background: ${MARKS[hash % MARKS.length]}`, "aria-hidden": "true" }, letter);
}

const stat = (value, label) => h("div", { class: "stat" }, h("span", { class: "stat-value" }, String(value)), h("span", { class: "stat-label" }, label));

/* Helpers: all content goes through text nodes, never through HTML strings. */
function h(tag, attrs, ...children) {
  const element = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs || {})) {
    if (value === null || value === undefined || value === false) continue;
    if (key.startsWith("on")) element.addEventListener(key.slice(2), value);
    else if (key === "class") element.className = value;
    else if (key === "style") element.style.cssText = value;
    else if (key in element && key !== "list") element[key] = value;
    else element.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat()) {
    if (child !== null && child !== undefined && child !== false) element.append(child);
  }
  return element;
}

const number = (value, digits) =>
  value.toLocaleString("fr-FR", { maximumFractionDigits: digits, minimumFractionDigits: digits });

function size(bytes) {
  if (bytes === null || bytes === undefined) return "";
  if (bytes < 1024) return `${bytes} o`;
  const units = ["Ko", "Mo", "Go", "To"];
  let value = bytes;
  let unit = -1;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${number(value, 1)} ${units[unit]}`;
}

const plural = (count, word) => `${count.toLocaleString("fr-FR")} ${word}${count > 1 ? "s" : ""}`;
const agree = (count, word) => `${word}${count > 1 ? "s" : ""}`;

function confirmAction(title, body, action) {
  const dialog = document.getElementById("confirm");
  document.getElementById("confirm-title").textContent = title;
  document.getElementById("confirm-body").textContent = body;
  const yes = document.getElementById("confirm-yes");
  const no = document.getElementById("confirm-no");
  yes.textContent = action;
  return new Promise((resolve) => {
    const finish = (answer) => {
      yes.onclick = null;
      no.onclick = null;
      dialog.onclose = null;
      dialog.close();
      resolve(answer);
    };
    yes.onclick = () => finish(true);
    no.onclick = () => finish(false);
    dialog.onclose = () => finish(false);
    dialog.showModal();
    no.focus();
  });
}

function outcomeNotice(outcome) {
  const parts = [`${plural(outcome.moved_files, "fichier")} (${size(outcome.moved_bytes)}) ${outcome.moved_files > 1 ? "déplacés" : "déplacé"} vers la corbeille.${outcome.moved_files ? " Videz-la pour libérer réellement l'espace." : ""}`];
  if (outcome.skipped) parts.push(`${plural(outcome.skipped, "fichier")} ${agree(outcome.skipped, "laissé")} en place, ${agree(outcome.skipped, "modifié")} depuis l'analyse.`);
  if (outcome.blocked_rules.length) parts.push(`Ignoré, programme ouvert entre-temps : ${outcome.blocked_rules.join(", ")}.`);
  if (outcome.failed) parts.push(`${plural(outcome.failed, "fichier")} non ${agree(outcome.failed, "déplacé")}, souvent ${outcome.failed > 1 ? "parce qu'ils sont" : "parce qu'il est"} en cours d'utilisation.`);
  return { kind: outcome.failed ? "err" : "ok", text: parts.join(" "), items: outcome.failed_sample };
}

/* Page state survives navigation. */
const state = {
  home: { items: null },
  clean: { rules: null, selected: new Set(), scan: null, open: null, files: [], lit: null },
  dups: { roots: [], min: 1048576, report: null, picked: new Set() },
  startup: { items: null, query: "", pending: null },
  programs: { items: null, query: "", order: "name" },
  perf: { items: null, query: "", pending: null },
  soft: { items: null, running: new Set(), results: new Map(), queue: null },
  drivers: { items: null, picked: new Set() },
};
for (const page of Object.values(state)) {
  page.busy = null;
  page.notice = null;
  page.progress = null;
}

let current = "home";

const focusName = (element) => element.getAttribute("aria-label") || element.textContent;

function savedFocus() {
  const element = document.activeElement;
  const root = document.getElementById("page");
  if (!element || element === root || !root.contains(element)) return null;
  return {
    tag: element.tagName.toLowerCase(),
    name: focusName(element),
    caret: element.type === "search" ? element.selectionStart : null,
  };
}

function restoreFocus(saved) {
  if (!saved) return;
  const match = [...document.querySelectorAll(`#page ${saved.tag}`)].find((element) => focusName(element) === saved.name);
  if (!match || match.disabled) return;
  match.focus({ preventScroll: true });
  if (saved.caret !== null) match.setSelectionRange(saved.caret, saved.caret);
}

function render() {
  const page = pages[current];
  const root = document.getElementById("page");
  const scroll = root.scrollTop;
  const focus = savedFocus();
  root.replaceChildren(page.view());
  root.scrollTop = scroll;
  restoreFocus(focus);
  paintDock();
  paintNav();
}

/* Messages stay in view at the bottom, wherever the list is scrolled. */
let dockedNotice = null;

function paintDock() {
  const page = state[current];
  document.getElementById("page").classList.toggle("has-dock", Boolean(page.notice));
  if (page.notice === dockedNotice) return;
  dockedNotice = page.notice;
  document.getElementById("dock").replaceChildren(...(page.notice ? [noticeView(page)] : []));
}

function pendingUpdates(page) {
  return page.items.filter((item) => page.results.get(item.id)?.kind !== "ok");
}

function navBadge(name) {
  const page = state[name];
  if (name === "clean") return page.scan?.bytes ? size(page.scan.bytes) : "";
  if (name === "dups") return page.report?.wasted_bytes ? size(page.report.wasted_bytes) : "";
  if (name === "soft") return page.items ? pendingUpdates(page).length || "" : "";
  if (name === "drivers") return page.items?.length || "";
  return "";
}

function paintNav() {
  for (const item of document.querySelectorAll(".nav-item")) {
    const name = item.dataset.page;
    const page = state[name];
    const working = Boolean(page.busy || page.queue || page.running?.size);
    item.classList.toggle("working", working);
    item.classList.toggle("failed", page.notice?.kind === "err");
    item.querySelector(".badge").textContent = working ? "" : navBadge(name);
  }
}

/* Runs a backend task, tracking its progress label and reporting failures. */
async function task(page, label, work) {
  page.busy = label;
  page.notice = null;
  render();
  try {
    await work();
  } catch (error) {
    page.notice = { kind: "err", text: String(error) };
  }
  page.busy = null;
  page.progress = null;
  render();
}

function paintProgress(box, { percent, title, detail }) {
  const ring = box.querySelector(".ring");
  ring.classList.toggle("spinning", percent === null);
  if (percent === null) {
    ring.removeAttribute("aria-valuenow");
    ring.style.removeProperty("--done");
  } else {
    ring.setAttribute("aria-valuenow", percent);
    ring.style.setProperty("--done", percent);
  }
  box.querySelector(".ring-value").textContent = percent === null ? "" : `${percent} %`;
  box.querySelector("strong").textContent = title;
  box.querySelector(".detail").textContent = detail;
}

function progressPanel(id, info, { stacked = false, action = null } = {}) {
  const box = h("div", { class: `progress-panel${stacked ? " stacked" : ""}`, id },
    h("div", { class: "ring", role: "progressbar", "aria-label": info.title, "aria-valuemin": 0, "aria-valuemax": 100 },
      h("span", { class: "ring-value" })),
    h("div", { class: "progress-text" }, h("strong"), h("div", { class: "detail" })),
    action);
  paintProgress(box, info);
  return box;
}

/* Updates the ring in place: a full render on every message would reset the page. */
function updateProgress(page, id, info) {
  if (!page.progress) return;
  page.progress = info;
  const box = document.getElementById(id);
  if (box) paintProgress(box, info);
}

function searchBox(page) {
  return h("input", {
    type: "search",
    placeholder: "Rechercher",
    "aria-label": "Rechercher",
    value: page.query,
    oninput: (event) => {
      page.query = event.target.value;
      render();
    },
  });
}

function header(title, lede, ...actions) {
  return h("div", { class: "head" },
    h("div", null, h("h1", null, title), h("p", null, lede)),
    h("div", { class: "actions" }, actions));
}

function noticeView(page) {
  const { kind, text, items } = page.notice;
  return h("div", { class: `notice ${kind}` },
    h("div", { class: "notice-text" }, text,
      items && items.length ? h("ul", null, items.map((item) => h("li", null, item))) : null),
    h("button", {
      class: "dismiss",
      type: "button",
      "aria-label": "Fermer ce message",
      onclick: () => {
        page.notice = null;
        render();
      },
    }, "×"));
}

const button = (cls, label, onclick, disabled) =>
  h("button", { class: cls, type: "button", onclick, disabled: Boolean(disabled) }, label);

const empty = (text, action) => h("div", { class: "empty" }, h("p", null, text), action);

/* Overview */
async function loadHome() {
  const page = state.home;
  await task(page, "Lecture…", async () => { page.items = await invoke("list_disks"); });
}

function diskCard(disk) {
  const free = disk.free_bytes;
  const used = disk.total_bytes - free;
  // Temporary files and caches live on the system drive.
  const reclaim = disk.mount.toUpperCase().startsWith("C:") ? Math.min(state.clean.scan?.bytes || 0, used) : 0;
  const label = disk.mount.replace(/\\$/, "");
  return h("section", { class: "disk" },
    h("div", { class: "disk-head" },
      h("strong", null, disk.name ? `${label} ${disk.name}` : label),
      h("span", { class: "num" }, `${size(free)} libres sur ${size(disk.total_bytes)}`)),
    h("div", { class: "disk-bar", "aria-hidden": "true" },
      h("div", { class: "disk-used", style: `flex: ${used - reclaim} 1 0` }),
      reclaim ? h("div", { class: "disk-reclaim", style: `flex: ${reclaim} 1 0` }) : null,
      h("div", { class: "disk-free", style: `flex: ${free} 1 0` })));
}

function tile(name, figure, caption, label, action) {
  return h("section", { class: "tile" },
    h("h2", null, icon(ICONS[name]), pages[name].label),
    h("div", { class: "tile-figure" }, String(figure)),
    h("p", null, caption),
    button("quiet mini", label, action));
}

function startCleaningAnalysis() {
  go("clean");
  if (!state.clean.busy && state.clean.selected.size) analyse();
}

function homeTiles() {
  const { clean, dups, programs, startup, soft, drivers } = state;
  const updates = soft.items ? pendingUpdates(soft).length : null;
  const active = startup.items ? startup.items.filter((item) => item.enabled).length : null;
  return [
    tile("clean", clean.scan ? size(clean.scan.bytes) : "—", clean.scan ? "à récupérer dans les fichiers temporaires" : "Analyse pas encore lancée",
      clean.scan ? "Voir le détail" : "Analyser", clean.scan ? () => go("clean") : startCleaningAnalysis),
    tile("dups", dups.report ? size(dups.report.wasted_bytes) : "—", dups.report ? "récupérables en doublons" : "Recherche pas encore lancée",
      "Ouvrir", () => go("dups")),
    tile("programs", programs.items ? programs.items.length : "…", "programmes installés", "Ouvrir", () => go("programs")),
    tile("startup", active ?? "…", startup.items ? `${agree(active, "actif")} sur ${startup.items.length} au démarrage` : "Lecture en cours", "Gérer", () => go("startup")),
    tile("soft", updates ?? "…", soft.items ? `${agree(updates, "mise")} à jour ${agree(updates, "disponible")}` : "Recherche en cours", "Voir", () => go("soft")),
    tile("drivers", drivers.items ? drivers.items.length : "—", drivers.items ? `${agree(drivers.items.length, "pilote")} ${agree(drivers.items.length, "proposé")}` : "Recherche pas encore lancée",
      drivers.items ? "Voir" : "Chercher", () => { go("drivers"); if (!drivers.items && !drivers.busy) loadDrivers(); }),
  ];
}

function homeView() {
  const page = state.home;
  return h("div", null,
    header("Votre PC", "L'état de vos disques et ce qui peut être fait en un coup d'œil."),
    page.items ? h("div", { class: "disks" }, page.items.map(diskCard),
      h("div", { class: "legend", "aria-hidden": "true" },
        h("span", { class: "key used" }, "Utilisé"),
        state.clean.scan?.bytes ? h("span", { class: "key reclaim" }, "Récupérable") : null,
        h("span", { class: "key free" }, "Libre"))) : empty("Lecture des disques…"),
    h("div", { class: "tiles" }, homeTiles()));
}

/* Cleaning */
function resetScan() {
  state.clean.scan = null;
  state.clean.open = null;
  state.clean.files = [];
}

async function analyse() {
  const page = state.clean;
  page.progress = { percent: null, title: "Analyse", detail: "Lecture des dossiers" };
  await task(page, "Analyse en cours…", async () => {
    resetScan();
    page.scan = await invoke("scan_cleaning", { ids: [...page.selected] });
  });
}

async function runCleaning() {
  const page = state.clean;
  const { file_count: count, bytes } = page.scan;
  const accepted = await confirmAction(
    "Envoyer à la corbeille ?",
    `${plural(count, "fichier")} (${size(bytes)}) ${count > 1 ? "iront" : "ira"} à la corbeille. Vous pourrez ${count > 1 ? "les" : "le"} restaurer tant qu'elle n'est pas vidée.`,
    "Envoyer à la corbeille",
  );
  if (!accepted) return;
  const describe = ({ done, total }) => ({
    percent: total ? Math.floor((done * 100) / total) : 100,
    title: "Envoi à la corbeille",
    detail: `${done.toLocaleString("fr-FR")} sur ${plural(total, "fichier")}`,
  });
  const onProgress = new Channel();
  onProgress.onmessage = (progress) => updateProgress(page, "clean-progress", describe(progress));
  page.progress = describe({ done: 0, total: count });
  await task(page, "Déplacement en cours…", async () => {
    const outcome = await invoke("run_cleaning", { onProgress });
    resetScan();
    page.notice = outcomeNotice(outcome);
  });
}

async function toggleFiles(id) {
  const page = state.clean;
  if (page.open === id) {
    page.open = null;
    render();
    return;
  }
  try {
    page.files = await invoke("cleaning_files", { ruleId: id });
    page.open = id;
  } catch (error) {
    page.notice = { kind: "err", text: String(error) };
  }
  render();
}

function ruleRow(rule, result, index) {
  const page = state.clean;
  let sub = null;
  let end = null;
  if (result) {
    if (result.status === "scanned" && !result.file_count) {
      sub = "Rien à nettoyer";
    } else if (result.status === "scanned") {
      end = h("span", { class: "num" }, size(result.bytes));
      sub = plural(result.file_count, "fichier");
      if (result.unreadable) sub += `, ${plural(result.unreadable, "chemin")} ${agree(result.unreadable, "illisible")}`;
    } else if (result.status === "blocked") {
      sub = `Fermez ${result.detail} pour analyser cette catégorie`;
      end = h("span", { class: "tag warn" }, "Programme ouvert");
    } else if (result.status === "not_detected") {
      sub = "Non installé sur ce PC";
    } else {
      sub = result.detail;
    }
  }
  const checkbox = h("input", {
    type: "checkbox",
    checked: page.selected.has(rule.id),
    "aria-label": rule.name,
    onchange: (event) => {
      if (event.target.checked) page.selected.add(rule.id);
      else page.selected.delete(rule.id);
      resetScan();
      render();
    },
  });
  const canList = result && result.status === "scanned" && result.file_count > 0;
  return h("div", {
    class: `row check${page.lit === rule.id ? " lit" : ""}${result && result.status !== "scanned" || result && !result.file_count ? " zero" : ""}`,
    onmouseenter: () => lightLayer(rule.id, true),
    onmouseleave: () => lightLayer(rule.id, false),
    "data-rule": rule.id,
  },
    checkbox,
    h("div", { class: "title" }, rule.name),
    h("div", { class: "end" },
      canList ? h("button", { class: "link", type: "button", onclick: () => toggleFiles(rule.id) }, page.open === rule.id ? "Masquer" : "Voir les fichiers") : null,
      end),
    sub ? h("div", { class: "sub" }, sub) : null,
    page.open === rule.id
      ? h("div", { class: "files" }, page.files.map((file) =>
          h("div", null, h("span", null, file.path), h("span", null, size(file.bytes)))))
      : null);
}

function lightLayer(id, on) {
  for (const element of document.querySelectorAll(`[data-rule="${CSS.escape(id)}"]`)) {
    element.classList.toggle("lit", on);
  }
}

function strata(page) {
  if (page.progress) return h("div", { class: "strata" }, progressPanel("clean-progress", page.progress, { stacked: true }));
  const layers = page.scan ? page.scan.rules.filter((rule) => rule.bytes > 0) : [];
  return h("div", { class: "strata" },
    h("div", { class: "sum" }, page.scan ? size(page.scan.bytes) : "—"),
    h("div", { class: "sum-label" }, page.scan ? "à récupérer" : "Lancez une analyse"),
    h("div", { class: `column${layers.length ? "" : " idle"}`, "aria-hidden": "true" },
      layers.map((rule, index) => h("div", {
        class: "layer",
        title: `${rule.name} : ${size(rule.bytes)}`,
        "data-rule": rule.id,
        style: `flex: ${rule.bytes} 1 0; background: ${STRATA[index % STRATA.length]}; animation-delay: ${index * 60}ms`,
        onmouseenter: () => lightLayer(rule.id, true),
        onmouseleave: () => lightLayer(rule.id, false),
      }))));
}

function cleanView() {
  const page = state.clean;
  const results = new Map((page.scan ? page.scan.rules : []).map((rule) => [rule.id, rule]));
  const canClean = page.scan && page.scan.file_count > 0;
  return h("div", null,
    header("Nettoyage", "Fichiers temporaires et caches. Tout passe par la corbeille, et les fichiers temporaires de moins de 24 heures restent en place.",
      button(canClean ? "quiet" : "primary", page.busy === "Analyse en cours…" ? page.busy : "Analyser", analyse, page.busy || !page.selected.size),
      canClean ? button("primary", page.busy === "Déplacement en cours…" ? page.busy : `Envoyer ${size(page.scan.bytes)} à la corbeille`, runCleaning, page.busy) : null),
    h("div", { class: "clean-grid" },
      page.rules
        ? h("div", { class: "list" }, page.rules.map((rule, index) => ruleRow(rule, results.get(rule.id), index)))
        : empty("Chargement des catégories…"),
      strata(page)));
}

/* Duplicates */
async function addFolders() {
  const page = state.dups;
  const chosen = await openDialog({ directory: true, multiple: true });
  if (!chosen) return;
  for (const path of [].concat(chosen)) {
    if (!page.roots.includes(path)) page.roots.push(path);
  }
  render();
}

async function searchDuplicates() {
  const page = state.dups;
  const onProgress = new Channel();
  onProgress.onmessage = (progress) => updateProgress(page, "search-progress", describeSearch(progress));
  page.progress = describeSearch({ stage: "listing", files: 0 });
  await task(page, "Recherche en cours…", async () => {
    page.picked.clear();
    page.report = await invoke("find_duplicates", { roots: page.roots, minBytes: page.min, onProgress });
  });
}

function describeSearch(progress) {
  if (progress.stage === "listing") {
    return {
      percent: null,
      title: "Inventaire des fichiers",
      detail: `${plural(progress.files, "fichier")} ${agree(progress.files, "trouvé")}`,
    };
  }
  const { done_bytes: done, total_bytes: total } = progress;
  return {
    percent: total ? Math.floor((done * 100) / total) : 100,
    title: "Comparaison du contenu",
    detail: `${size(done)} sur ${size(total)}`,
  };
}

function pickAllButFirst() {
  const page = state.dups;
  page.picked.clear();
  for (const group of page.report.groups) {
    for (const path of group.paths.slice(1)) page.picked.add(path);
  }
  render();
}

async function recycleDuplicates() {
  const page = state.dups;
  const paths = [...page.picked];
  const accepted = await confirmAction(
    "Envoyer les copies à la corbeille ?",
    `${plural(paths.length, "fichier")} ${paths.length > 1 ? "iront" : "ira"} à la corbeille. Une copie de chaque groupe reste toujours en place.`,
    "Envoyer à la corbeille",
  );
  if (!accepted) return;
  await task(page, "Déplacement en cours…", async () => {
    const result = await invoke("recycle_duplicates", { paths });
    page.report = result.report;
    page.picked.clear();
    page.notice = outcomeNotice(result.outcome);
  });
}

function groupCard(group) {
  const page = state.dups;
  const pickedInGroup = group.paths.filter((path) => page.picked.has(path)).length;
  return h("section", { class: "group-card" },
    h("header", null,
      h("span", null, `${group.paths.length} copies identiques de ${size(group.bytes_each)}`),
      h("span", { class: "num" }, `${size(group.bytes_each * (group.paths.length - 1))} récupérables`)),
    group.paths.map((path) => {
      const checked = page.picked.has(path);
      // The last unchecked copy stays locked so that one copy always remains.
      const locked = !checked && pickedInGroup === group.paths.length - 1;
      return h("label", null,
        h("input", {
          type: "checkbox",
          checked,
          disabled: locked,
          "aria-label": path,
          onchange: (event) => {
            if (event.target.checked) page.picked.add(path);
            else page.picked.delete(path);
            render();
          },
        }),
        h("span", null,
          h("span", { class: "folder" }, path.slice(0, path.lastIndexOf("\\") + 1)),
          h("span", { class: "file" }, path.slice(path.lastIndexOf("\\") + 1))));
    }));
}

function dupsView() {
  const page = state.dups;
  const report = page.report;
  return h("div", null,
    header("Doublons", "Compare le contenu réel des fichiers. Les dossiers AppData, .git et node_modules sont ignorés.",
      page.roots.length ? button("primary", page.busy || "Rechercher", searchDuplicates, page.busy) : null),
    h("div", { class: "tools" },
      page.roots.length ? button("quiet", "Ajouter un dossier", addFolders, page.busy) : null,
      page.roots.map((root) => h("span", { class: "chip" }, root,
        h("button", { type: "button", "aria-label": `Retirer ${root}`, onclick: () => { page.roots = page.roots.filter((item) => item !== root); render(); } }, "×"))),
      h("label", null, "Taille minimale ",
        h("select", { onchange: (event) => { page.min = Number(event.target.value); } },
          [[1024, "1 Ko"], [1048576, "1 Mo"], [10485760, "10 Mo"], [104857600, "100 Mo"]].map(([value, label]) =>
            h("option", { value, selected: page.min === value }, label))))),
    page.progress
      ? progressPanel("search-progress", page.progress)
      : report
      ? h("div", null,
          report.refused.map((reason) => h("div", { class: "notice err" }, `Dossier refusé. ${reason}`)),
          h("div", { class: "tools sticky" },
            h("strong", null, report.group_count
              ? `${size(report.wasted_bytes)} récupérables, ${plural(report.group_count, "groupe")}, sur ${plural(report.scanned_files, "fichier")} examinés`
              : `Aucun doublon parmi ${plural(report.scanned_files, "fichier")} examinés`),
            report.group_count ? button("quiet mini", "Cocher tout sauf la première copie", pickAllButFirst, page.busy) : null,
            page.picked.size ? button("primary mini", `Envoyer ${plural(page.picked.size, "fichier")} à la corbeille`, recycleDuplicates, page.busy) : null),
          report.groups.map(groupCard),
          report.group_count > report.groups.length ? h("p", null, `Affichage des ${report.groups.length} groupes les plus lourds.`) : null)
      : empty(page.roots.length ? "Lancez la recherche pour comparer ces dossiers." : "Choisissez les dossiers à comparer.",
          page.roots.length ? null : button("primary", "Choisir un dossier", addFolders, page.busy)));
}

/* Startup */
async function loadStartup() {
  const page = state.startup;
  await task(page, "Lecture…", async () => {
    page.items = await invoke("list_startup");
    requestIcons(page.items.map((item) => item.command));
  });
}

async function toggleStartup(item, enabled) {
  const page = state.startup;
  // Keeps the switch where the user put it while Windows asks for authorization.
  page.pending = { id: item.id, enabled };
  await task(page, "Modification…", async () => {
    try {
      await invoke("set_startup", { id: item.id, enabled });
      page.items = await invoke("list_startup");
      page.notice = { kind: "ok", text: `${item.name} ${enabled ? "se lancera" : "ne se lancera plus"} au démarrage de Windows.` };
    } finally {
      page.pending = null;
    }
  });
}

function commandFile(command) {
  const match = command.match(/([^\\/"]+\.(?:exe|bat|cmd|com|lnk|ps1|vbs))/i);
  return match ? match[1] : command;
}

function startupRow(page, item) {
  const pending = page.pending?.id === item.id ? page.pending : null;
  return h("div", { class: "row with-mark" },
    mark(item.name, item.command),
    h("div", { class: "title" }, item.name),
    h("div", { class: "end", style: "grid-row: span 2" },
      pending && item.needs_admin
        ? h("span", { class: "tag warn" }, "En attente de Windows…")
        : item.needs_admin ? h("span", { class: "tag" }, "Autorisation Windows") : null,
      h("input", {
        type: "checkbox",
        class: "switch",
        role: "switch",
        checked: pending ? pending.enabled : item.enabled,
        disabled: Boolean(page.busy),
        "aria-label": `Lancer ${item.name} au démarrage`,
        onchange: (event) => toggleStartup(item, event.target.checked),
      })),
    h("div", { class: "sub", title: item.command }, `${item.location} · ${commandFile(item.command)}`));
}

function startupView() {
  const page = state.startup;
  const query = page.query.toLowerCase();
  const items = (page.items || []).filter((item) => `${item.name} ${item.command}`.toLowerCase().includes(query));
  const active = (page.items || []).filter((item) => item.enabled).length;
  return h("div", null,
    header("Démarrage", "Programmes lancés à l'ouverture de Windows. Les désactiver ne les supprime pas.",
      page.items ? stat(`${active} / ${page.items.length}`, agree(active, "actif")) : null,
      button("quiet", page.busy === "Lecture…" ? page.busy : "Actualiser", loadStartup, page.busy)),
    page.items && page.items.length ? h("div", { class: "tools" }, searchBox(page)) : null,
    !page.items ? empty("Lecture des programmes de démarrage…")
      : !page.items.length ? empty("Aucun programme ne se lance au démarrage.")
      : !items.length ? empty("Aucun programme ne correspond à cette recherche.")
      : h("div", { class: "list" }, items.map((item) => startupRow(page, item))));
}

/* Programs */
async function loadPrograms() {
  const page = state.programs;
  await task(page, "Lecture…", async () => {
    page.items = await invoke("list_programs");
    requestIcons(page.items.map((item) => item.icon_source));
  });
}

async function uninstallProgram(item) {
  const page = state.programs;
  const accepted = await confirmAction(
    `Désinstaller ${item.name} ?`,
    "Le désinstalleur de l'éditeur va s'ouvrir. Cette opération ne passe pas par la corbeille.",
    "Désinstaller",
  );
  if (!accepted) return;
  await task(page, "Lancement…", async () => {
    await invoke("uninstall_program", { id: item.id });
    page.notice = { kind: "ok", text: `Désinstalleur de ${item.name} lancé. Une fois terminé, cliquez sur Actualiser.` };
  });
}

function formatInstallDate(value) {
  return /^\d{8}$/.test(value) ? `${value.slice(6, 8)}/${value.slice(4, 6)}/${value.slice(0, 4)}` : "";
}

const PROGRAM_ORDERS = {
  name: { label: "Nom", compare: (a, b) => a.name.localeCompare(b.name, "fr") },
  size: { label: "Taille", compare: (a, b) => (b.bytes || 0) - (a.bytes || 0) },
  date: { label: "Date d'installation", compare: (a, b) => (b.installed_on || "").localeCompare(a.installed_on || "") },
};

function programsView() {
  const page = state.programs;
  const query = page.query.toLowerCase();
  const items = (page.items || [])
    .filter((item) => `${item.name} ${item.publisher}`.toLowerCase().includes(query))
    .sort(PROGRAM_ORDERS[page.order].compare);
  return h("div", null,
    header("Programmes", "Lance le désinstalleur officiel de chaque programme.",
      page.items ? stat(page.items.length, agree(page.items.length, "programme")) : null,
      button("quiet", page.busy === "Lecture…" ? page.busy : "Actualiser", loadPrograms, page.busy)),
    h("div", { class: "tools" },
      searchBox(page),
      h("label", null, "Trier par ",
        h("select", { onchange: (event) => { page.order = event.target.value; render(); } },
          Object.entries(PROGRAM_ORDERS).map(([value, { label }]) => h("option", { value, selected: page.order === value }, label))))),
    !page.items ? empty("Lecture des programmes installés…")
      : !items.length ? empty("Aucun programme ne correspond à cette recherche.")
      : h("div", { class: "list" }, items.map((item) => h("div", { class: "row with-mark" },
          mark(item.name, item.icon_source),
          h("div", { class: "title" }, item.name),
          h("div", { class: "end", style: "grid-row: span 2" },
            h("span", { class: "num" }, size(item.bytes)),
            button("danger", "Désinstaller", () => uninstallProgram(item), page.busy)),
          h("div", { class: "sub" }, [item.publisher, item.version, formatInstallDate(item.installed_on)].filter(Boolean).join(" · "))))));
}

/* Performance */
async function loadApps() {
  const page = state.perf;
  await task(page, "Mesure…", async () => {
    page.items = await invoke("list_apps");
    requestIcons(page.items.map((item) => item.exe_path));
  });
}

async function toggleEfficiency(item, enabled) {
  const page = state.perf;
  page.pending = { name: item.name, enabled };
  await task(page, "Modification…", async () => {
    try {
      await invoke("set_efficiency", { name: item.name, enabled });
      page.items = await invoke("list_apps");
      page.notice = { kind: "ok", text: `Mode économie ${enabled ? "activé" : "désactivé"} pour ${appName(item.name)}.` };
    } finally {
      page.pending = null;
    }
  });
}

const appName = (name) => name.replace(/\.exe$/i, "");

function perfRow(page, item) {
  const pending = page.pending?.name === item.name ? page.pending : null;
  return h("div", { class: "row with-mark" },
    mark(item.name, item.exe_path),
    h("div", { class: "title" }, appName(item.name)),
    h("div", { class: "end", style: "grid-row: span 2" },
      h("span", { class: "num cell" }, `${number(item.cpu_percent, 1)} %`),
      h("span", { class: "num cell wide" }, size(item.memory_bytes)),
      h("span", { class: "cell switch-cell" },
        h("input", {
          type: "checkbox",
          class: "switch",
          role: "switch",
          checked: pending ? pending.enabled : item.efficiency,
          disabled: Boolean(page.busy),
          "aria-label": `Mode économie pour ${appName(item.name)}`,
          onchange: (event) => toggleEfficiency(item, event.target.checked),
        }))),
    h("div", { class: "sub" }, `${item.processes} processus`));
}

function perfView() {
  const page = state.perf;
  const query = page.query.toLowerCase();
  const items = (page.items || []).filter((item) => item.name.toLowerCase().includes(query));
  return h("div", null,
    header("Performances", "Le mode économie laisse le processeur aux programmes que vous utilisez, jusqu'à la fermeture du programme ralenti. Il ne libère pas de mémoire.",
      page.items ? stat(size(page.items.reduce((sum, item) => sum + item.memory_bytes, 0)), "de mémoire utilisée") : null,
      button("quiet", page.busy === "Mesure…" ? page.busy : "Actualiser", loadApps, page.busy)),
    page.items ? h("div", { class: "tools" }, searchBox(page),
      h("span", { class: "hint" }, "Certains programmes se mettent seuls en économie : leur interrupteur est déjà allumé.")) : null,
    !page.items ? empty("Mesure des programmes ouverts…")
      : !items.length ? empty("Aucun programme ne correspond à cette recherche.")
      : h("div", { class: "list" },
          h("div", { class: "row columns with-mark", "aria-hidden": "true" },
            h("div"),
            h("div"),
            h("div", { class: "end" },
              h("span", { class: "cell" }, "Processeur"),
              h("span", { class: "cell wide" }, "Mémoire"),
              h("span", { class: "cell switch-cell" }, "Économie"))),
          items.map((item) => perfRow(page, item))));
}

/* Software updates */
async function loadSoftware() {
  const page = state.soft;
  await task(page, "Recherche…", async () => {
    page.items = await invoke("list_software_updates");
    page.results.clear();
  });
}

/* A new first number on a dotted version; years and plain build numbers don't count. */
function isMajorJump(item) {
  const [from, to] = [item.current, item.available].map((version) => (/^\d+\.\d/.test(version) ? Number.parseInt(version, 10) : null));
  return from !== null && to !== null && to > from && from < 1000;
}

async function upgradeOne(item) {
  const page = state.soft;
  page.running.add(item.id);
  page.results.delete(item.id);
  render();
  try {
    await invoke("upgrade_software", { id: item.id });
    page.results.set(item.id, { kind: "ok" });
  } catch (error) {
    page.results.set(item.id, { kind: "err", text: String(error) });
  }
  page.running.delete(item.id);
  render();
}

async function upgradeAsked(item) {
  if (isMajorJump(item)) {
    const accepted = await confirmAction(
      `Passer ${item.name} en version ${item.available} ?`,
      "Une version majeure peut modifier vos données ou vos réglages. Sauvegardez d'abord ce qui compte.",
      "Mettre à jour",
    );
    if (!accepted) return;
  }
  await upgradeOne(item);
}

const bulkUpdates = (page) => pendingUpdates(page).filter((item) => item.installable && !isMajorJump(item));

async function upgradeAll() {
  const page = state.soft;
  const items = bulkUpdates(page);
  const majors = pendingUpdates(page).filter((item) => item.installable && isMajorJump(item)).length;
  const accepted = await confirmAction(
    `Mettre à jour ${plural(items.length, "logiciel")} ?`,
    `Les installations se suivent et Windows peut demander des autorisations. Fermez d'abord les logiciels concernés.${majors ? ` Les versions majeures (${majors}) restent à lancer une par une.` : ""}`,
    "Tout mettre à jour",
  );
  if (!accepted) return;
  page.notice = null;
  page.queue = { done: 0, total: items.length, name: "", stop: false };
  for (const item of items) {
    if (page.queue.stop) break;
    page.queue.name = item.name;
    await upgradeOne(item);
    page.queue.done += 1;
  }
  const count = (kind) => items.filter((item) => page.results.get(item.id)?.kind === kind).length;
  const failed = count("err");
  page.notice = {
    kind: failed ? "err" : "ok",
    text: `${plural(count("ok"), "logiciel")} mis à jour.${failed ? ` ${failed} en échec : la raison est indiquée sur chaque ligne.` : ""}`,
  };
  page.queue = null;
  render();
}

function queuePanel(page) {
  const { done, total, name, stop } = page.queue;
  return progressPanel("queue-progress", {
    percent: Math.floor((done * 100) / total),
    title: `Mise à jour : ${name}`,
    detail: `${done} sur ${plural(total, "logiciel")} terminés`,
  }, {
    action: button("quiet mini", stop ? "Arrêt après celui-ci…" : "Arrêter après celui-ci", () => { page.queue.stop = true; render(); }, stop),
  });
}

function softRow(page, item) {
  const result = page.results.get(item.id);
  const running = page.running.has(item.id);
  const major = isMajorJump(item);
  let action;
  if (result?.kind === "ok") action = h("span", { class: "tag ok" }, "Mis à jour");
  else if (!item.installable) action = h("span", { class: "tag" }, item.manual_note ? "À faire à la main" : "Nom tronqué par winget");
  else action = button("quiet mini", running ? "Mise à jour…" : result ? "Réessayer" : "Mettre à jour", () => upgradeAsked(item), page.running.size);
  let sub = h("div", { class: "sub" }, item.id);
  if (result?.kind === "err") sub = h("div", { class: "sub note failure" }, result.text);
  else if (item.manual_note) sub = h("div", { class: "sub note" }, item.manual_note);
  return h("div", { class: `row with-mark${result?.kind === "ok" ? " zero" : ""}` },
    mark(item.name, installedIconSource(item.name)),
    h("div", { class: "title" }, item.name),
    h("div", { class: "end", style: "grid-row: span 2" },
      major && result?.kind !== "ok" ? h("span", { class: "tag warn" }, "Version majeure") : null,
      running ? h("span", { class: "spin", "aria-hidden": "true" }) : null,
      h("span", { class: "num versions" }, `${item.current} → `, h("b", null, item.available)),
      action),
    sub);
}

function softOrder(page) {
  if (page.queue || !page.results.size) return page.items;
  const rank = (item) => ({ err: 0, ok: 2 })[page.results.get(item.id)?.kind] ?? 1;
  return [...page.items].sort((a, b) => rank(a) - rank(b));
}

function softView() {
  const page = state.soft;
  const bulk = page.items ? bulkUpdates(page).length : 0;
  const locked = page.busy || page.running.size;
  return h("div", null,
    header("Logiciels", "Mises à jour proposées par winget, le gestionnaire de paquets de Windows.",
      page.items && page.items.length ? stat(pendingUpdates(page).length, "à mettre à jour") : null,
      button(bulk ? "quiet" : "primary", page.busy || "Chercher les mises à jour", loadSoftware, locked),
      bulk ? button("primary", `Tout mettre à jour (${bulk})`, upgradeAll, locked) : null),
    page.queue ? queuePanel(page) : null,
    !page.items ? empty(page.busy ? "Recherche des mises à jour…" : "Lancez la recherche pour voir les mises à jour disponibles.")
      : !page.items.length ? empty("Tous les logiciels suivis par winget sont à jour.")
      : h("div", { class: "list" }, softOrder(page).map((item) => softRow(page, item))));
}

/* Drivers */
async function loadDrivers() {
  const page = state.drivers;
  await task(page, "Recherche…", async () => {
    page.items = await invoke("list_driver_updates");
    page.picked = new Set(page.items.filter((item) => !item.firmware).map((item) => item.id));
  });
}

async function installDrivers() {
  const page = state.drivers;
  const ids = [...page.picked];
  const accepted = await confirmAction(
    `Installer ${plural(ids.length, "pilote")} ?`,
    "Windows va demander une autorisation. Un point de restauration est tenté avant l'installation, qui peut prendre plusieurs minutes.",
    "Installer",
  );
  if (!accepted) return;
  page.progress = {
    percent: null,
    title: `Installation de ${plural(ids.length, "pilote")}`,
    detail: "Acceptez la fenêtre Windows si elle s'affiche. Cela peut prendre plusieurs minutes.",
  };
  await task(page, "Installation…", async () => {
    let restart;
    try {
      restart = await invoke("install_drivers", { ids });
    } finally {
      // A failed batch may still have installed some drivers.
      page.items = await invoke("list_driver_updates").catch(() => page.items);
      page.picked.clear();
    }
    page.notice = { kind: "ok", text: restart ? "Pilotes installés. Redémarrez le PC pour terminer." : "Pilotes installés." };
  });
}

function driversView() {
  const page = state.drivers;
  return h("div", null,
    header("Pilotes", "Pilotes proposés par Windows Update pour ce PC. Les micrologiciels ne sont jamais cochés d'office.",
      page.items && page.items.length ? stat(page.items.length, agree(page.items.length, "pilote")) : null,
      page.items ? button("quiet", page.busy === "Recherche…" ? page.busy : "Chercher à nouveau", loadDrivers, page.busy) : null,
      page.picked.size ? button("primary", page.busy === "Installation…" ? page.busy : `Installer ${plural(page.picked.size, "pilote")}`, installDrivers, page.busy) : null),
    page.progress ? progressPanel("driver-progress", page.progress) : null,
    !page.items ? empty("Windows Update peut mettre une minute à répondre.", button("primary", page.busy || "Chercher les pilotes", loadDrivers, page.busy))
      : !page.items.length ? empty("Windows Update ne propose aucun pilote pour ce PC.")
      : h("div", { class: "list" }, page.items.map((item) => h("div", { class: "row check" },
          h("input", {
            type: "checkbox",
            checked: page.picked.has(item.id),
            "aria-label": item.title,
            onchange: (event) => {
              if (event.target.checked) page.picked.add(item.id);
              else page.picked.delete(item.id);
              render();
            },
          }),
          h("div", { class: "title" }, item.title),
          h("div", { class: "end" },
            item.firmware ? h("span", { class: "tag warn" }, "Micrologiciel") : null,
            h("span", { class: "num" }, size(item.size))),
          h("div", { class: "sub" }, item.firmware
            ? "Une coupure pendant l'installation peut rendre l'appareil inutilisable."
            : [item.maker, item.class].filter(Boolean).join(" · "))))));
}

/* Navigation */
const pages = {
  home: { label: "Vue d'ensemble", view: homeView, load: loadHome },
  clean: { label: "Nettoyage", view: cleanView },
  dups: { label: "Doublons", view: dupsView },
  programs: { label: "Programmes", view: programsView, load: loadPrograms },
  startup: { label: "Démarrage", view: startupView, load: loadStartup },
  perf: { label: "Performances", view: perfView, load: loadApps },
  soft: { label: "Logiciels", view: softView, load: loadSoftware },
  drivers: { label: "Pilotes", view: driversView },
};

const navigation = [
  [null, ["home"]],
  ["Libérer de la place", ["clean", "dups", "programs"]],
  ["Accélérer", ["startup", "perf"]],
  ["Mettre à jour", ["soft", "drivers"]],
];

function go(name) {
  current = name;
  for (const item of document.querySelectorAll(".nav-item")) {
    if (item.dataset.page === name) item.setAttribute("aria-current", "page");
    else item.removeAttribute("aria-current");
  }
  document.getElementById("page").scrollTop = 0;
  render();
  if (pages[name].load && state[name].items === null && !state[name].busy) pages[name].load();
}

document.getElementById("nav").append(...navigation.map(([title, names]) =>
  h("div", { class: "group" }, title ? h("h2", null, title) : null, names.map((name) =>
    h("button", { class: "nav-item", type: "button", "data-page": name, onclick: () => go(name) },
      h("span", { class: "label" }, icon(ICONS[name]), pages[name].label),
      h("span", { class: "badge" }))))));

(async () => {
  go("home");
  try {
    const rules = await invoke("list_rules");
    state.clean.rules = rules;
    state.clean.selected = new Set(rules.filter((rule) => rule.enabled_by_default).map((rule) => rule.id));
  } catch (error) {
    state.clean.notice = { kind: "err", text: String(error) };
  }
  render();
  // Read-only lists, loaded ahead so the overview can show their totals.
  for (const name of ["startup", "programs", "soft"]) pages[name].load();
})();
