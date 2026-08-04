const STORAGE_KEY = "cadence-library-v1";
const SESSION_KEY = "cadence-session-v1";
const PENDING_DELETIONS_KEY = "cadence-pending-deletions-v1";
const DB_NAME = "cadence-audio-v1";
const DB_STORE = "files";
const DEMO_TRACK_ID = "demo-glass-echoes";
const WAVEFORM_PEAK_COUNT = 320;
const TRACK_STAGES = [
  { id: "sound-design", label: "Sound design" },
  { id: "production", label: "Production / arrangement" },
  { id: "mixdown", label: "Mixdown" },
  { id: "mastering", label: "Mastering" }
];
const DEFAULT_TRACK_STAGE = TRACK_STAGES[0].id;

const demoTrack = {
  id: DEMO_TRACK_ID,
  title: "Glass Echoes",
  originalName: "glass-echoes-v3.wav",
  duration: 222,
  size: 0,
  createdAt: "2026-07-31T18:40:00.000Z",
  demo: true,
  favorite: false,
  stage: DEFAULT_TRACK_STAGE,
  notes: [
    { id: "demo-note-1", time: 43.2, body: "Let the vocal breathe here — the delay tail is fighting the first word of the next phrase.", category: "mix", done: false, createdAt: "2026-07-31T18:44:00.000Z" },
    { id: "demo-note-2", time: 91.8, body: "Try a cleaner transition into the second chorus. The energy drops for a beat too long.", category: "arrangement", done: false, createdAt: "2026-07-31T18:49:00.000Z" },
    { id: "demo-note-3", time: 157.4, body: "This bass texture is working. Keep it, but check the low-mid build against the kick.", category: "sound", done: true, createdAt: "2026-07-31T18:55:00.000Z" }
  ]
};

const state = { tracks: [], selectedTrackId: DEMO_TRACK_ID, noteFilter: "open", libraryFilter: "all", search: "", audioUrl: null, audioLoadToken: 0, waveformLoadToken: 0, playbackFrame: 0, loudnessFrame: 0, toastTimer: null, sessionSaveTimer: 0, muted: false, playbackIntent: false, commentDragging: false, suppressCommentClick: false, seekDragging: false, suppressSeekClick: false, loudnessLevel: null, liveLoudnessLevel: null, liveIntegratedLoudnessLevel: null, playbackGraph: null, restoreTrackId: null, restoreTime: 0, restorePlaying: false, suppressSessionPersistence: false, editingNoteId: null };
const $ = (selector) => document.querySelector(selector);
const $$ = (selector) => [...document.querySelectorAll(selector)];
const elements = {
  trackList: $("#trackList"), librarySummary: $("#librarySummary"), allCount: $("#allCount"), needsReviewCount: $("#needsReviewCount"), clearCount: $("#clearCount"), searchInput: $("#searchInput"), clearSearchButton: $("#clearSearchButton"), reviewHeader: $("#reviewHeader"), playerContext: $("#playerContext"), waveformStage: $("#waveformStage"), waveform: $("#waveform"), waveformBars: $("#waveformBars"), playbackLine: $("#playbackLine"), commentRail: $("#commentRail"), commentMarkers: $("#commentMarkers"), commentPreviewDot: $("#commentPreviewDot"), timelineCursor: $("#timelineCursor"), waveformHoverTime: $("#waveformHoverTime"), audio: $("#audioPlayer"), playButton: $("#playButton"), seekInput: $("#seekInput"), currentTime: $("#currentTime"), durationTime: $("#durationTime"), volumeButton: $("#volumeButton"), volumeInput: $("#volumeInput"), audioUnavailable: $("#audioUnavailable"), addNoteButton: $("#addNoteButton"), notesList: $("#notesList"), openNoteCount: $("#openNoteCount"), allNoteCount: $("#allNoteCount"), doneNoteCount: $("#doneNoteCount"), noteComposer: $("#noteComposer"), composerMode: $("#composerMode"), draftTime: $("#draftTime"), noteInput: $("#noteInput"), cancelNoteButton: $("#cancelNoteButton"), trackDetails: $("#trackDetails"), loudnessCard: $("#loudnessCard"), loudnessValue: $("#loudnessValue"), loudnessZone: $("#loudnessZone"), loudnessMarker: $("#loudnessMarker"), loudnessLiveValue: $("#loudnessLiveValue"), loudnessLiveStatus: $("#loudnessLiveStatus"), loudnessLiveMarker: $("#loudnessLiveMarker"), loudnessIntegratedLiveValue: $("#loudnessIntegratedLiveValue"), saveState: $("#saveState"), toast: $("#toast"), fileInput: $("#fileInput"), dropOverlay: $("#dropOverlay")
};

function cloneDemo() { return JSON.parse(JSON.stringify(demoTrack)); }
function trackStage(value) { return TRACK_STAGES.find((stage) => stage.id === value) || TRACK_STAGES[0]; }
function normalizeTrack(track) { return { ...track, favorite: track.favorite === true, stage: trackStage(track.stage).id, notes: Array.isArray(track.notes) ? track.notes : [] }; }
function escapeHTML(value) { return String(value).replace(/[&<>'"]/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", "'": "&#39;", '"': "&quot;" }[char])); }
function formatTime(seconds) { if (!Number.isFinite(seconds) || seconds < 0) return "00:00"; const whole = Math.floor(seconds); const minutes = Math.floor(whole / 60); const secs = whole % 60; return `${String(minutes).padStart(2, "0")}:${String(secs).padStart(2, "0")}`; }
function formatDate(value) { return new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" }).format(new Date(value)); }
function formatBytes(bytes) { if (!bytes) return "Preview"; if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`; return `${(bytes / (1024 * 1024)).toFixed(1)} MB`; }
function activeTrack() { return state.tracks.find((track) => track.id === state.selectedTrackId) || state.tracks[0]; }
function openNotes(track) { return (track?.notes || []).filter((note) => !note.done); }
function saveState() { try { localStorage.setItem(STORAGE_KEY, JSON.stringify(state.tracks)); elements.saveState.innerHTML = '<span class="save-dot"></span> All changes saved'; return true; } catch (error) { elements.saveState.textContent = "Could not save changes locally"; showToast(`Changes could not be saved: ${error.message}`); return false; } }
function loadSessionState() { try { const stored = JSON.parse(localStorage.getItem(SESSION_KEY) || "null"); return stored && typeof stored === "object" ? stored : {}; } catch { return {}; } }
function sessionSnapshot() { const track = activeTrack(); const currentTime = Number(elements.audio.currentTime); const volume = Number(elements.volumeInput.value); return { selectedTrackId: state.selectedTrackId, trackId: track?.id || null, currentTime: Number.isFinite(currentTime) ? currentTime : 0, wasPlaying: Boolean(elements.audio.src && state.playbackIntent), volume: Number.isFinite(volume) ? Math.max(0, Math.min(1, volume)) : 0.8, muted: state.muted, noteFilter: state.noteFilter, libraryFilter: state.libraryFilter, search: state.search }; }
function persistSession() { if (state.suppressSessionPersistence) return; try { localStorage.setItem(SESSION_KEY, JSON.stringify(sessionSnapshot())); } catch { /* Session recovery is best-effort when storage is unavailable. */ } }
function scheduleSessionPersistence() { if (state.suppressSessionPersistence || state.sessionSaveTimer) return; state.sessionSaveTimer = setTimeout(() => { state.sessionSaveTimer = 0; persistSession(); }, 200); }
function loadPendingDeletions() { try { const stored = JSON.parse(localStorage.getItem(PENDING_DELETIONS_KEY) || "[]"); return Array.isArray(stored) ? [...new Set(stored.filter((id) => typeof id === "string" && id))] : []; } catch { return []; } }
function savePendingDeletions(ids, failureMessage = "Removal could not be queued") { try { localStorage.setItem(PENDING_DELETIONS_KEY, JSON.stringify([...new Set(ids)])); return true; } catch (error) { showToast(`${failureMessage}: ${error.message}`); return false; } }
function queuePendingDeletion(id) { const pending = loadPendingDeletions(); return pending.includes(id) || savePendingDeletions([...pending, id]); }
function clearPendingDeletion(id) { const pending = loadPendingDeletions(); return !pending.includes(id) || savePendingDeletions(pending.filter((pendingId) => pendingId !== id), "Pending removal could not be cleared; cleanup will retry on next launch"); }
function showToast(message) { elements.toast.textContent = message; elements.toast.classList.add("is-visible"); clearTimeout(state.toastTimer); state.toastTimer = setTimeout(() => elements.toast.classList.remove("is-visible"), 2800); }

function openAudioDB() {
  return new Promise((resolve, reject) => {
    if (!window.indexedDB) { reject(new Error("IndexedDB unavailable")); return; }
    const request = indexedDB.open(DB_NAME, 1);
    request.onupgradeneeded = () => request.result.createObjectStore(DB_STORE);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error || new Error("Could not open local audio storage"));
  });
}
async function storeFile(id, file) { const db = await openAudioDB(); return new Promise((resolve, reject) => { const tx = db.transaction(DB_STORE, "readwrite"); tx.objectStore(DB_STORE).put(file, id); tx.oncomplete = () => { db.close(); resolve(); }; tx.onerror = () => { db.close(); reject(tx.error); }; }); }
async function readFile(id) { const db = await openAudioDB(); return new Promise((resolve, reject) => { const tx = db.transaction(DB_STORE, "readonly"); const request = tx.objectStore(DB_STORE).get(id); request.onsuccess = () => resolve(request.result || null); request.onerror = () => reject(request.error); tx.oncomplete = () => db.close(); }); }
async function deleteFile(id) { const db = await openAudioDB(); return new Promise((resolve, reject) => { const tx = db.transaction(DB_STORE, "readwrite"); tx.objectStore(DB_STORE).delete(id); tx.oncomplete = () => { db.close(); resolve(); }; tx.onerror = () => { db.close(); reject(tx.error || new Error("Could not remove local audio")); }; }); }

function renderTrackList() {
  const query = state.search.trim().toLowerCase();
  const filtered = state.tracks.filter((track) => {
    const matchesText = !query || `${track.title} ${track.originalName}`.toLowerCase().includes(query);
    const isClear = openNotes(track).length === 0;
    const matchesFilter = state.libraryFilter === "all" || (state.libraryFilter === "clear" ? isClear : !isClear);
    return matchesText && matchesFilter;
  }).sort((a, b) => new Date(b.createdAt) - new Date(a.createdAt));
  elements.trackList.innerHTML = filtered.length ? filtered.map((track) => {
    const isClear = openNotes(track).length === 0;
    const stage = trackStage(track.stage);
    const favoriteLabel = track.favorite ? "Remove from favorites" : "Mark as favorite";
    const removeButton = track.demo ? "" : `<button class="track-delete" data-track-delete type="button" aria-label="Remove ${escapeHTML(track.title)} from library" title="Remove from library">×</button>`;
    return `<div class="track-item ${track.id === state.selectedTrackId ? "is-selected" : ""}" data-track-id="${escapeHTML(track.id)}"><button class="track-open" data-track-open type="button" aria-label="Open ${escapeHTML(track.title)}"><span class="track-art"></span><span class="track-copy"><span class="track-title">${escapeHTML(track.title)}</span><span class="track-meta">${formatTime(track.duration)} · ${track.demo ? "Preview record" : formatBytes(track.size)}</span><span class="track-stage-inline">${escapeHTML(stage.label)}</span></span><span class="track-indicator ${isClear ? "is-clear" : ""}" title="${isClear ? "No open notes" : "Open review notes"}"></span></button><span class="track-actions"><button class="track-favorite ${track.favorite ? "is-active" : ""}" data-track-favorite type="button" aria-pressed="${track.favorite}" aria-label="${favoriteLabel}" title="${favoriteLabel}">${track.favorite ? "★" : "☆"}</button>${removeButton}</span></div>`;
  }).join("") : '<div class="empty-library"><strong>No tracks here yet.</strong><p>Import an audio file or adjust your search.</p></div>';
  const needsReview = state.tracks.filter((track) => openNotes(track).length).length;
  const clear = state.tracks.length - needsReview;
  elements.librarySummary.textContent = `${state.tracks.length} track${state.tracks.length === 1 ? "" : "s"} · local only`;
  elements.allCount.textContent = state.tracks.length; elements.needsReviewCount.textContent = needsReview; elements.clearCount.textContent = clear;
  $$("[data-library-filter]").forEach((button) => { const active = button.dataset.libraryFilter === state.libraryFilter; button.classList.toggle("is-active", active); button.setAttribute("aria-pressed", String(active)); });
}

function renderReviewHeader(track) {
  if (!track) { elements.reviewHeader.innerHTML = "<h2>Your review desk</h2>"; return; }
  const openCount = openNotes(track).length;
  const stage = trackStage(track.stage);
  const favoriteLabel = track.favorite ? "Remove from favorites" : "Mark as favorite";
  const stageOptions = TRACK_STAGES.map((option) => `<option value="${option.id}" ${option.id === stage.id ? "selected" : ""}>${escapeHTML(option.label)}</option>`).join("");
  elements.reviewHeader.innerHTML = `<div><div class="title-eyebrow"><span class="tiny-dot"></span>${track.demo ? "Preview record" : "Local track"}</div><h2>${escapeHTML(track.title)}</h2><div class="header-meta"><span>${track.demo ? "Example workspace" : escapeHTML(track.originalName)}</span><span>${formatTime(track.duration)}</span></div></div>`;
  elements.playerContext.textContent = track.demo ? "A sample workspace to show how the review loop feels" : `${track.originalName} · ${formatBytes(track.size)}`;
  elements.trackDetails.innerHTML = `<div class="details-art"></div><p class="section-label">Track details</p><div class="details-heading"><h3>${escapeHTML(track.title)}</h3><button class="track-favorite track-favorite-large ${track.favorite ? "is-active" : ""}" data-track-favorite type="button" aria-pressed="${track.favorite}" aria-label="${favoriteLabel}" title="${favoriteLabel}">${track.favorite ? "★" : "☆"}</button></div><p class="details-file">${escapeHTML(track.originalName)}</p><div class="track-management"><label class="stage-control" for="trackStageSelect"><span>Progress stage</span><select id="trackStageSelect">${stageOptions}</select></label>${track.demo ? '<span class="details-note">Preview record · imported tracks can be removed here.</span>' : '<button class="danger-button" data-remove-track type="button">Remove from library</button>'}</div><div class="details-stats"><div class="details-stat"><span>Open notes</span><strong>${openCount}</strong></div><div class="details-stat"><span>Duration</span><strong>${formatTime(track.duration)}</strong></div></div>`;
}

function toggleTrackFavorite(id) {
  const track = state.tracks.find((item) => item.id === id);
  if (!track) return;
  const previous = track.favorite === true;
  track.favorite = !previous;
  if (!saveState()) { track.favorite = previous; return; }
  renderTrackList();
  renderReviewHeader(activeTrack());
  showToast(track.favorite ? "Marked as a favorite." : "Favorite removed.");
}

function updateTrackStage(id, value) {
  const track = state.tracks.find((item) => item.id === id);
  if (!track) return;
  const nextStage = trackStage(value).id;
  if (track.stage === nextStage) return;
  const previous = track.stage;
  track.stage = nextStage;
  if (!saveState()) { track.stage = previous; return; }
  renderTrackList();
  renderReviewHeader(activeTrack());
  showToast(`Progress moved to ${trackStage(nextStage).label}.`);
}

async function removeTrack(id) {
  const track = state.tracks.find((item) => item.id === id);
  if (!track) return;
  if (track.demo) { showToast("The preview record cannot be removed."); return; }
  if (!window.confirm(`Remove “${track.title}” from this device?`)) return;
  if (!queuePendingDeletion(id)) return;
  const previousTracks = state.tracks;
  const previousSelectedId = state.selectedTrackId;
  const nextTracks = previousTracks.filter((item) => item.id !== id);
  state.tracks = nextTracks;
  if (previousSelectedId === id) { state.selectedTrackId = nextTracks[0]?.id || DEMO_TRACK_ID; state.playbackIntent = false; }
  if (!saveState()) {
    const pendingCleared = clearPendingDeletion(id);
    state.tracks = previousTracks;
    state.selectedTrackId = previousSelectedId;
    if (pendingCleared) {
      renderAll();
      showToast("Track could not be removed from the local library.");
      return;
    }
    state.tracks = nextTracks;
    if (previousSelectedId === id) { state.selectedTrackId = nextTracks[0]?.id || DEMO_TRACK_ID; state.playbackIntent = false; }
    elements.noteComposer.hidden = true;
    renderAll();
   await loadSelectedAudio();
    persistSession();
   showToast("Removal queued; cleanup will finish on next launch.");
    return;
  }
  elements.noteComposer.hidden = true;
  renderAll();
 await loadSelectedAudio();
  persistSession();
 try {
    await deleteFile(id);
  } catch (error) {
    showToast(`Removal queued; cleanup will retry on next launch (${error.message}).`);
    return;
  }
  const cleared = clearPendingDeletion(id);
  showToast(cleared ? `${track.title} removed from this device.` : `${track.title} removed; cleanup will finish on next launch.`);
}

function fallbackWaveform(count = WAVEFORM_PEAK_COUNT) { return Array.from({ length: count }, (_, index) => Math.max(.08, Math.min(1, Math.abs(Math.sin(index * 1.91) * .32 + Math.sin(index * .47) * .15 + .31)))); }
function paintWaveform(peaks) { elements.waveformBars.innerHTML = peaks.map((peak) => `<span class="wave-bar" style="height:${Math.max(5, Math.round(peak * 100))}%"></span>`).join(""); updateWaveform(); }
function extractWaveformPeaks(buffer, count = WAVEFORM_PEAK_COUNT) { const channels = Array.from({ length: buffer.numberOfChannels }, (_, channel) => buffer.getChannelData(channel)); const blockSize = Math.max(1, Math.floor(buffer.length / count)); const rawPeaks = Array.from({ length: count }, (_, index) => { const start = index * blockSize; const end = index === count - 1 ? buffer.length : Math.min(buffer.length, start + blockSize); const stride = Math.max(1, Math.floor((end - start) / 180)); let peak = 0; for (let sample = start; sample < end; sample += stride) { let amplitude = 0; channels.forEach((channel) => { amplitude += Math.abs(channel[sample] || 0); }); peak = Math.max(peak, amplitude / channels.length); } return peak; }); const sortedPeaks = [...rawPeaks].sort((a, b) => a - b); const ceiling = sortedPeaks[Math.max(0, Math.floor(sortedPeaks.length * .95))] || 1; return rawPeaks.map((peak) => Math.max(.04, Math.min(1, Math.pow(peak / ceiling, .75)))); }
function loudnessReading(level) {
  let zone = "soft"; let label = "TOO SOFT";
  if (level > -5.99) { zone = level > -3 ? "clip" : "hot"; label = level > -3 ? "CLIP RISK" : "HOT"; }
  else if (level >= -7.01) { zone = "target"; label = "TECHNO MASTER"; }
  else if (level > -12) { zone = "mixdown"; label = "MIXDOWN"; }
  else if (level > -18) { zone = "headroom"; label = "MIX HEADROOM"; }
  return { zone, label, position: Math.max(0, Math.min(100, ((level + 24) / 24) * 100)) };
}
function formatLoudness(level) { return level <= -59.9 ? "-∞" : level.toFixed(1); }
function resetLiveLoudnessSession() {
  if (state.playbackGraph) { state.playbackGraph.energyFrames = []; state.playbackGraph.integratedBlocks = []; state.playbackGraph.lastIntegratedBlockAt = 0; }
  state.liveLoudnessLevel = null;
  state.liveIntegratedLoudnessLevel = null;
  elements.loudnessLiveValue.textContent = "—";
  elements.loudnessIntegratedLiveValue.textContent = "—";
  elements.loudnessLiveMarker.hidden = true;
  elements.loudnessCard.removeAttribute("data-live-zone");
}
function resetLoudnessMeter(status = "NO SIGNAL") { stopLiveLoudness(status); resetLiveLoudnessSession(); state.loudnessLevel = null; elements.loudnessValue.textContent = "—"; elements.loudnessZone.textContent = status; elements.loudnessCard.removeAttribute("data-zone"); elements.loudnessMarker.hidden = true; elements.loudnessMarker.style.left = "0%"; elements.loudnessLiveStatus.textContent = status; }
function setLoudnessPending() { stopLiveLoudness("ANALYZING"); resetLiveLoudnessSession(); state.loudnessLevel = null; elements.loudnessValue.textContent = "…"; elements.loudnessZone.textContent = "ANALYZING"; elements.loudnessCard.dataset.zone = "pending"; elements.loudnessLiveStatus.textContent = "ANALYZING"; elements.loudnessMarker.hidden = true; }
function setLoudnessLevel(level) {
  if (!Number.isFinite(level)) { resetLoudnessMeter("UNAVAILABLE"); return; }
  state.loudnessLevel = level;
  const reading = loudnessReading(level);
  elements.loudnessValue.textContent = formatLoudness(level);
  elements.loudnessZone.textContent = reading.label;
  elements.loudnessCard.dataset.zone = reading.zone;
  elements.loudnessMarker.hidden = false;
  elements.loudnessMarker.style.left = `${reading.position}%`;
  if (elements.audio.paused) elements.loudnessLiveStatus.textContent = "PAUSED";
}
function setLoudnessLiveLevel(level, status = "PAUSED") {
  state.liveLoudnessLevel = Number.isFinite(level) ? level : null;
  if (!Number.isFinite(level)) { elements.loudnessLiveValue.textContent = "—"; elements.loudnessLiveStatus.textContent = status; elements.loudnessLiveMarker.hidden = true; elements.loudnessCard.removeAttribute("data-live-zone"); return; }
  const reading = loudnessReading(level);
  elements.loudnessLiveValue.textContent = formatLoudness(level);
  elements.loudnessLiveStatus.textContent = reading.label;
  elements.loudnessLiveMarker.hidden = false;
  elements.loudnessLiveMarker.style.left = `${reading.position}%`;
  elements.loudnessCard.dataset.liveZone = reading.zone;
}
function setLoudnessIntegratedLiveLevel(level) { state.liveIntegratedLoudnessLevel = Number.isFinite(level) ? level : null; elements.loudnessIntegratedLiveValue.textContent = Number.isFinite(level) ? formatLoudness(level) : "—"; }
const ITU_K_WEIGHTING_48K = { pre: { feedforward: [1.53512485958697, -2.69169618940638, 1.19839281085285], feedback: [1, -1.69065929318241, 0.73248077421585] }, rlb: { feedforward: [1, -2, 1], feedback: [1, -1.99004745483398, 0.99007225036621] } };
function createKWeightingChain(context, source, destination) {
  if (Math.abs(context.sampleRate - 48000) < .5 && context.createIIRFilter) {
    const preFilter = context.createIIRFilter(ITU_K_WEIGHTING_48K.pre.feedforward, ITU_K_WEIGHTING_48K.pre.feedback);
    const rlbFilter = context.createIIRFilter(ITU_K_WEIGHTING_48K.rlb.feedforward, ITU_K_WEIGHTING_48K.rlb.feedback);
    source.connect(preFilter).connect(rlbFilter).connect(destination);
    return;
  }
  const highPass = context.createBiquadFilter(); highPass.type = "highpass"; highPass.frequency.value = 38.1358; highPass.Q.value = 0.5;
  const highShelf = context.createBiquadFilter(); highShelf.type = "highshelf"; highShelf.frequency.value = 1681.974; highShelf.gain.value = 4;
  source.connect(highPass).connect(highShelf).connect(destination);
}
async function analyzeKWeightedLufs(buffer) {
  const OfflineAudioContextClass = window.OfflineAudioContext || window.webkitOfflineAudioContext;
  if (!OfflineAudioContextClass || !buffer?.length || !buffer.numberOfChannels) return null;
  const sampleRate = 48000;
  const context = new OfflineAudioContextClass(buffer.numberOfChannels, Math.max(1, Math.ceil(buffer.duration * sampleRate)), sampleRate);
  const source = context.createBufferSource(); source.buffer = buffer;
  createKWeightingChain(context, source, context.destination); source.start(0);
  const weighted = await context.startRendering();
  const blockSize = Math.max(1, Math.round(weighted.sampleRate * .4));
  const hopSize = Math.max(1, Math.round(blockSize * .25));
  const channels = Array.from({ length: weighted.numberOfChannels }, (_, channel) => weighted.getChannelData(channel));
  const blocks = [];
  for (let start = 0; start + blockSize <= weighted.length; start += hopSize) {
    let energy = 0;
    channels.forEach((channel, index) => { const channelGain = weighted.numberOfChannels === 6 ? ([1, 1, 1, 0, 1.41, 1.41][index] || 0) : 1; if (!channelGain) return; let channelEnergy = 0; for (let sample = start; sample < start + blockSize; sample += 1) channelEnergy += channel[sample] * channel[sample]; energy += channelGain * channelGain * channelEnergy / blockSize; });
    const lufs = energy > 1e-12 ? -0.691 + 10 * Math.log10(energy) : -60;
    if (lufs > -70) blocks.push({ energy, lufs });
  }
  if (!blocks.length) return -60;
  const ungatedEnergy = blocks.reduce((sum, block) => sum + block.energy, 0) / blocks.length;
  const relativeGate = -0.691 + 10 * Math.log10(Math.max(1e-12, ungatedEnergy)) - 10;
  const gatedBlocks = blocks.filter((block) => block.lufs > relativeGate);
  const integratedEnergy = (gatedBlocks.length ? gatedBlocks : blocks).reduce((sum, block) => sum + block.energy, 0) / (gatedBlocks.length || blocks.length);
  return Math.max(-60, Math.min(3, -0.691 + 10 * Math.log10(Math.max(1e-12, integratedEnergy))));
}
function updateAuditionOutput() {
  const level = Math.max(0, Math.min(1, Number(elements.volumeInput.value) || 0));
  if (state.playbackGraph) {
    state.playbackGraph.auditionGain.gain.value = state.muted ? 0 : level;
    elements.audio.volume = 1;
    elements.audio.muted = false;
  } else {
    elements.audio.volume = level;
    elements.audio.muted = state.muted;
  }
}
function ensurePlaybackGraph() {
  if (state.playbackGraph) return state.playbackGraph;
  const AudioContextClass = window.AudioContext || window.webkitAudioContext;
  if (!AudioContextClass || !elements.audio.src) return null;
  let context;
  try { context = new AudioContextClass({ sampleRate: 48000 }); } catch { try { context = new AudioContextClass(); } catch { return null; } }
  try {
    const source = context.createMediaElementSource(elements.audio);
    const auditionGain = context.createGain();
    const splitter = context.createChannelSplitter(6);
    const analyserSink = context.createGain();
    analyserSink.gain.value = 0;
    source.connect(auditionGain).connect(context.destination);
    const analysers = Array.from({ length: 6 }, (_, channel) => {
      const channelInput = context.createGain();
      const analyser = context.createAnalyser();
      splitter.connect(channelInput, channel, 0);
      analyser.fftSize = 2048;
      analyser.smoothingTimeConstant = 0;
      createKWeightingChain(context, channelInput, analyser);
      analyser.connect(analyserSink);
      return analyser;
    });
    source.connect(splitter);
    analyserSink.connect(context.destination);
    state.playbackGraph = { context, auditionGain, analysers, data: analysers.map((analyser) => new Float32Array(analyser.fftSize)), channelGains: [1, 1, 1, 0, 1.41, 1.41], energyFrames: [], integratedBlocks: [], lastIntegratedBlockAt: 0 };
    elements.audio.volume = 1;
    elements.audio.muted = false;
    updateAuditionOutput();
    return state.playbackGraph;
  } catch { context.close().catch(() => {}); return null; }
}
async function preparePlaybackGraph() { const graph = ensurePlaybackGraph(); if (graph?.context.state === "suspended") await graph.context.resume().catch(() => {}); updateAuditionOutput(); return graph; }
function stopLiveLoudness(status = "PAUSED") {
  if (state.loudnessFrame) cancelAnimationFrame(state.loudnessFrame);
  state.loudnessFrame = 0;
  if (state.playbackGraph) state.playbackGraph.energyFrames = [];
  if (state.liveLoudnessLevel !== null) elements.loudnessLiveStatus.textContent = status;
}
function updateLiveIntegratedLoudness(graph, now) {
  const blockFrames = graph.energyFrames.filter((frame) => frame.time >= now - 400);
  if (now - graph.lastIntegratedBlockAt < 100 || !blockFrames.length || now - blockFrames[0].time < 400) return;
  const blockEnergy = blockFrames.reduce((sum, frame) => sum + frame.energy, 0) / blockFrames.length;
  const blockLufs = blockEnergy > 1e-12 ? -0.691 + 10 * Math.log10(blockEnergy) : -60;
  if (blockLufs > -70) graph.integratedBlocks.push({ energy: blockEnergy, lufs: blockLufs });
  graph.lastIntegratedBlockAt = now;
  if (!graph.integratedBlocks.length) return;
  const ungatedEnergy = graph.integratedBlocks.reduce((sum, block) => sum + block.energy, 0) / graph.integratedBlocks.length;
  const relativeGate = -0.691 + 10 * Math.log10(Math.max(1e-12, ungatedEnergy)) - 10;
  const gatedBlocks = graph.integratedBlocks.filter((block) => block.lufs > relativeGate);
  const integratedEnergy = (gatedBlocks.length ? gatedBlocks : graph.integratedBlocks).reduce((sum, block) => sum + block.energy, 0) / (gatedBlocks.length || graph.integratedBlocks.length);
  setLoudnessIntegratedLiveLevel(Math.max(-60, Math.min(3, -0.691 + 10 * Math.log10(Math.max(1e-12, integratedEnergy)))));
}
function sampleLiveLoudness() {
  const graph = state.playbackGraph;
  if (!graph || elements.audio.paused || elements.audio.ended) return;
  let energy = 0;
  graph.analysers.forEach((analyser, channel) => {
    analyser.getFloatTimeDomainData(graph.data[channel]);
    let channelEnergy = 0;
    for (const sample of graph.data[channel]) channelEnergy += sample * sample;
    energy += (graph.channelGains[channel] || 0) ** 2 * channelEnergy / graph.data[channel].length;
  });
  const now = performance.now();
  graph.energyFrames.push({ time: now, energy });
  while (graph.energyFrames.length && graph.energyFrames[0].time < now - 3000) graph.energyFrames.shift();
  const shortTermFrames = graph.energyFrames.filter((frame) => frame.time >= now - 3000);
  const averageEnergy = shortTermFrames.reduce((sum, frame) => sum + frame.energy, 0) / Math.max(1, shortTermFrames.length);
  setLoudnessLiveLevel(Math.max(-60, -0.691 + 10 * Math.log10(Math.max(1e-12, averageEnergy))));
  updateLiveIntegratedLoudness(graph, now);
}
function startLiveLoudness() {
  stopLiveLoudness("PLAYING");
  const tick = () => { sampleLiveLoudness(); if (!elements.audio.paused && !elements.audio.ended) state.loudnessFrame = requestAnimationFrame(tick); else state.loudnessFrame = 0; };
  tick();
}
async function renderWaveform() {
  const track = activeTrack();
  const requestToken = ++state.waveformLoadToken;
  const cachedPeaks = track?.waveformVersion === 3 ? track.waveformPeaks : null;
  const cachedLoudness = track?.loudnessVersion === 2 && Number.isFinite(track.loudnessLufs) ? track.loudnessLufs : null;
  paintWaveform(Array.isArray(cachedPeaks) && cachedPeaks.length ? cachedPeaks : fallbackWaveform());
  if (!track || track.demo || (!window.AudioContext && !window.webkitAudioContext)) { resetLoudnessMeter(track?.demo ? "PREVIEW ONLY" : "UNAVAILABLE"); return; }
  if (cachedLoudness !== null) setLoudnessLevel(cachedLoudness); else setLoudnessPending();
  if (cachedPeaks?.length && cachedLoudness !== null) return;
  let context;
  try {
    const blob = await readFile(track.id);
    if (!blob) {
      if (requestToken === state.waveformLoadToken && activeTrack()?.id === track.id) resetLoudnessMeter("UNAVAILABLE");
      return;
    }
    const AudioContextClass = window.AudioContext || window.webkitAudioContext;
    context = new AudioContextClass();
    const buffer = await context.decodeAudioData(await blob.arrayBuffer());
    const peaks = extractWaveformPeaks(buffer);
    const loudness = cachedLoudness ?? await analyzeKWeightedLufs(buffer);
    await context.close(); context = null;
    if (requestToken !== state.waveformLoadToken || activeTrack()?.id !== track.id) return;
    track.waveformVersion = 3;
    track.waveformPeaks = peaks;
    track.loudnessVersion = 2;
    track.loudnessLufs = loudness;
    saveState();
    paintWaveform(peaks);
    setLoudnessLevel(loudness);
  } catch { if (context) await context.close().catch(() => {}); if (requestToken === state.waveformLoadToken && activeTrack()?.id === track.id) resetLoudnessMeter("UNAVAILABLE"); }
}
function updatePlaybackLine() { const duration = elements.audio.duration || activeTrack()?.duration || 0; const current = elements.audio.currentTime || 0; const progress = duration ? Math.max(0, Math.min(1, current / duration)) : 0; elements.playbackLine.style.left = `${progress * 100}%`; elements.playbackLine.hidden = !duration; }
function stopPlaybackAnimation() { if (state.playbackFrame) cancelAnimationFrame(state.playbackFrame); state.playbackFrame = 0; updatePlaybackLine(); }
function startPlaybackAnimation() { stopPlaybackAnimation(); const tick = () => { updatePlaybackLine(); if (!elements.audio.paused && !elements.audio.ended) state.playbackFrame = requestAnimationFrame(tick); else state.playbackFrame = 0; }; tick(); }
function updateWaveform() { const duration = elements.audio.duration || activeTrack()?.duration || 0; const current = elements.audio.currentTime || 0; const progress = duration ? current / duration : 0; $$(".wave-bar").forEach((bar, index, bars) => bar.classList.toggle("is-played", index / bars.length <= progress)); updatePlaybackLine(); }
function renderCommentLane(track = activeTrack()) { const notes = [...(track?.notes || [])].sort((a, b) => a.time - b.time); const duration = track?.duration || elements.audio.duration || 1; if (!track) { elements.commentMarkers.innerHTML = ""; return; } elements.commentMarkers.innerHTML = notes.map((note) => `<button class="comment-marker ${note.done ? "is-done" : ""}" data-marker-note-id="${escapeHTML(note.id)}" type="button" style="left:${Math.max(0, Math.min(100, note.time / duration * 100))}%" aria-label="Jump to ${formatTime(note.time)} note"><span></span><strong>${formatTime(note.time)}</strong></button>`).join(""); }
function timelineRatio(event, rail) { const rect = rail.getBoundingClientRect(); if (!rect.width) return 0; return Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width)); }
function timelineSeconds(event, rail) { const duration = elements.audio.duration || activeTrack()?.duration || 0; return timelineRatio(event, rail) * duration; }
function updateTimelineCursor(event) { const rail = event.currentTarget; const railRect = rail.getBoundingClientRect(); const waveformRect = elements.waveform.getBoundingClientRect(); if (!railRect.width || !waveformRect.width) return; const ratio = timelineRatio(event, rail); const position = Math.max(0, Math.min(100, (railRect.left + ratio * railRect.width - waveformRect.left) / waveformRect.width * 100)); const seconds = timelineSeconds(event, rail); elements.timelineCursor.style.left = `${position}%`; elements.timelineCursor.hidden = false; elements.waveformHoverTime.textContent = formatTime(seconds); elements.waveformHoverTime.style.left = `${position}%`; elements.waveformHoverTime.hidden = false; }
function updateCommentPreview(event) { updateTimelineCursor(event); const ratio = timelineRatio(event, elements.commentRail); elements.commentPreviewDot.style.left = `${ratio * 100}%`; elements.commentPreviewDot.hidden = false; elements.commentPreviewDot.classList.toggle("is-dragging", state.commentDragging); }
function hideTimelineCursor() { if (state.commentDragging) return; elements.timelineCursor.hidden = true; elements.waveformHoverTime.hidden = true; elements.commentPreviewDot.hidden = true; elements.commentPreviewDot.classList.remove("is-dragging"); }
function startSeekDrag(event) { if (event.target.closest("#commentRail") || !activeTrack() || activeTrack().demo || !elements.audio.src) return; state.seekDragging = true; elements.waveform.classList.add("is-seeking"); elements.waveform.setPointerCapture?.(event.pointerId); seekTo(timelineSeconds(event, elements.waveform)); updateTimelineCursor(event); event.preventDefault(); event.stopPropagation(); }
function moveSeekDrag(event) { if (!state.seekDragging) return; seekTo(timelineSeconds(event, elements.waveform)); updateTimelineCursor(event); event.preventDefault(); }
async function playFromScrubPoint() { if (!activeTrack() || activeTrack().demo || !elements.audio.src) return; await preparePlaybackGraph(); elements.audio.play().catch(() => showToast("Playback was blocked — press play again.")); }
function finishSeekDrag(event) { if (!state.seekDragging) return; seekTo(timelineSeconds(event, elements.waveform)); state.seekDragging = false; elements.waveform.classList.remove("is-seeking"); if (elements.waveform.hasPointerCapture?.(event.pointerId)) elements.waveform.releasePointerCapture(event.pointerId); state.suppressSeekClick = true; playFromScrubPoint(); setTimeout(() => { state.suppressSeekClick = false; }, 0); event.preventDefault(); event.stopPropagation(); }
function cancelSeekDrag(event) { if (!state.seekDragging) return; state.seekDragging = false; elements.waveform.classList.remove("is-seeking"); if (event && elements.waveform.hasPointerCapture?.(event.pointerId)) elements.waveform.releasePointerCapture(event.pointerId); event?.stopPropagation(); }
function startCommentDrag(event) { if (event.target.closest("[data-marker-note-id]")) return; state.commentDragging = true; elements.commentRail.classList.add("is-dragging"); elements.commentRail.setPointerCapture?.(event.pointerId); updateCommentPreview(event); event.preventDefault(); event.stopPropagation(); }
function moveCommentDrag(event) { if (state.commentDragging) updateCommentPreview(event); }
function finishCommentDrag(event) { if (!state.commentDragging) return; updateCommentPreview(event); state.commentDragging = false; elements.commentRail.classList.remove("is-dragging"); if (elements.commentRail.hasPointerCapture?.(event.pointerId)) elements.commentRail.releasePointerCapture(event.pointerId); state.suppressCommentClick = true; beginNoteAt(timelineSeconds(event, elements.commentRail), event); setTimeout(() => { state.suppressCommentClick = false; }, 0); event.preventDefault(); event.stopPropagation(); }
function cancelCommentDrag(event) { if (!state.commentDragging) return; state.commentDragging = false; elements.commentRail.classList.remove("is-dragging"); if (event && elements.commentRail.hasPointerCapture?.(event.pointerId)) elements.commentRail.releasePointerCapture(event.pointerId); event?.stopPropagation(); hideTimelineCursor(); }

function renderNotes() {
  const track = activeTrack();
  renderCommentLane(track);
  if (!track) { elements.notesList.innerHTML = '<div class="empty-notes"><strong>Select a track to see its notes.</strong></div>'; return; }
  const notes = [...(track.notes || [])].sort((a, b) => a.time - b.time);
  const visibleNotes = notes.filter((note) => state.noteFilter === "all" || (state.noteFilter === "done" ? note.done : !note.done));
  elements.openNoteCount.textContent = openNotes(track).length; elements.allNoteCount.textContent = notes.length; elements.doneNoteCount.textContent = notes.filter((note) => note.done).length;
  $$("[data-note-filter]").forEach((button) => { const active = button.dataset.noteFilter === state.noteFilter; button.classList.toggle("is-active", active); button.setAttribute("aria-pressed", String(active)); });
  if (!visibleNotes.length) { const message = state.noteFilter === "done" ? "No completed notes yet." : state.noteFilter === "open" ? "Nothing open on this pass." : "No notes on this track yet."; elements.notesList.innerHTML = `<div class="empty-notes"><div class="empty-notes-icon">⌖</div><strong>${message}</strong><p>${track.demo ? "Import the actual audio to start leaving your own time-stamped notes." : "Start playback, then capture the exact moment that needs attention."}</p></div>`; return; }
  elements.notesList.innerHTML = "";
  visibleNotes.forEach((note) => {
    const row = document.createElement("div"); row.className = `note-row ${note.done ? "is-done" : ""}`; row.dataset.noteId = note.id;
    row.innerHTML = `<input class="note-check" type="checkbox" aria-label="Mark note done" ${note.done ? "checked" : ""}><button class="timestamp-button" type="button">${formatTime(note.time)}</button><div class="note-body"><p></p><div class="note-meta"><span>${formatDate(note.createdAt)}</span></div></div><div class="note-actions"><button class="note-edit" type="button" aria-label="Edit note" title="Edit note">Edit</button><button class="note-delete" type="button" aria-label="Delete note" title="Delete note">×</button></div>`;
    row.querySelector(".note-body p").textContent = note.body; elements.notesList.appendChild(row);
  });
}

function renderAll() { renderTrackList(); renderReviewHeader(activeTrack()); renderNotes(); renderWaveform(); }
function resetPlayer() { stopPlaybackAnimation(); stopLiveLoudness(); elements.audio.pause(); elements.audio.removeAttribute("src"); elements.audio.dataset.trackId = ""; elements.audio.load(); elements.playButton.disabled = true; elements.seekInput.disabled = true; elements.volumeButton.disabled = true; elements.volumeInput.disabled = true; elements.addNoteButton.disabled = true; elements.audioUnavailable.hidden = true; elements.durationTime.textContent = formatTime(activeTrack()?.duration || 0); elements.currentTime.textContent = "00:00"; if (state.audioUrl) { URL.revokeObjectURL(state.audioUrl); state.audioUrl = null; } updatePlaybackLine(); }

async function loadSelectedAudio() {
  const track = activeTrack(); const requestToken = ++state.audioLoadToken; const requestedTrackId = track?.id; resetPlayer(); if (!track || track.demo) { if (track?.demo) elements.audioUnavailable.hidden = false; renderWaveform(); return; }
  try {
    const blob = await readFile(track.id); if (!blob) throw new Error("Audio file is not available in local storage.");
    if (requestToken !== state.audioLoadToken || activeTrack()?.id !== requestedTrackId) return;
    state.audioUrl = URL.createObjectURL(blob); elements.audio.dataset.trackId = requestedTrackId; elements.audio.src = state.audioUrl; elements.audio.load(); ensurePlaybackGraph(); elements.playButton.disabled = false; elements.seekInput.disabled = false; elements.volumeButton.disabled = false; elements.volumeInput.disabled = false; elements.addNoteButton.disabled = false; updateAuditionOutput();
  } catch (error) { if (requestToken !== state.audioLoadToken || activeTrack()?.id !== requestedTrackId) return; elements.audioUnavailable.hidden = false; elements.audioUnavailable.querySelector("strong").textContent = "Audio file unavailable"; elements.audioUnavailable.querySelector("p").textContent = error.message; }
}
async function selectTrack(id) { if (id === state.selectedTrackId) return; persistSession(); state.selectedTrackId = id; state.playbackIntent = false; state.draftTime = null; state.restoreTrackId = id; state.restoreTime = 0; state.restorePlaying = false; cancelNote(); state.suppressSessionPersistence = true; try { renderAll(); await loadSelectedAudio(); } finally { state.suppressSessionPersistence = false; persistSession(); } }
function updateTransport() { const duration = elements.audio.duration || activeTrack()?.duration || 0; const current = elements.audio.currentTime || 0; elements.currentTime.textContent = formatTime(current); elements.durationTime.textContent = formatTime(duration); elements.seekInput.max = duration; elements.seekInput.value = current; elements.playButton.querySelector(".play-glyph").textContent = elements.audio.paused ? "▶" : "Ⅱ"; elements.playButton.setAttribute("aria-label", elements.audio.paused ? "Play track" : "Pause track"); updateWaveform(); }
function seekTo(seconds) { if (!Number.isFinite(seconds)) return; const duration = elements.audio.duration || activeTrack()?.duration || 0; const nextTime = Math.max(0, Math.min(seconds, duration)); if (Math.abs(nextTime - (elements.audio.currentTime || 0)) > .25) resetLiveLoudnessSession(); elements.audio.currentTime = nextTime; updateTransport(); scheduleSessionPersistence(); }
async function togglePlayback() { if (!activeTrack() || activeTrack().demo || !elements.audio.src) return; if (elements.audio.paused) { await preparePlaybackGraph(); elements.audio.play().catch(() => showToast("Playback was blocked — press play again.")); } else { state.playbackIntent = false; elements.audio.pause(); scheduleSessionPersistence(); } }
function positionComposer(event) { const stageRect = elements.waveformStage.getBoundingClientRect(); const width = Math.min(360, Math.max(250, stageRect.width - 24)); const pointerX = event ? event.clientX - stageRect.left : stageRect.width / 2; const pointerY = event ? event.clientY - stageRect.top : stageRect.height * .55; const left = Math.max(12, Math.min(stageRect.width - width - 12, pointerX - width / 2)); const top = Math.max(42, Math.min(stageRect.height - 180, pointerY + 12)); elements.noteComposer.style.width = `${width}px`; elements.noteComposer.style.left = `${left}px`; elements.noteComposer.style.top = `${top}px`; }
function beginNoteAt(seconds, event) { const track = activeTrack(); if (!track || track.demo || !elements.audio.src) { showToast("Import the actual audio to capture a listening point."); return; } state.editingNoteId = null; elements.composerMode.textContent = "captured point"; elements.noteComposer.querySelector('[type="submit"]').textContent = "Save note"; const duration = elements.audio.duration || track.duration || 0; state.draftTime = Math.max(0, Math.min(duration, seconds)); seekTo(state.draftTime); elements.draftTime.textContent = formatTime(state.draftTime); elements.noteComposer.hidden = false; elements.commentPreviewDot.hidden = true; elements.commentPreviewDot.classList.remove("is-dragging"); positionComposer(event); elements.noteInput.value = ""; elements.noteInput.focus(); }
function beginNote() { beginNoteAt(elements.audio.currentTime || 0); }
function beginEditNote(note) { if (!note) return; state.editingNoteId = note.id; state.draftTime = note.time; elements.draftTime.textContent = formatTime(note.time); elements.composerMode.textContent = "editing note"; elements.noteComposer.querySelector('[type="submit"]').textContent = "Save changes"; elements.noteComposer.hidden = false; elements.commentPreviewDot.hidden = true; elements.commentPreviewDot.classList.remove("is-dragging"); positionComposer(); elements.noteInput.value = note.body; elements.noteInput.focus(); elements.noteInput.select(); }
function cancelNote() { state.draftTime = null; state.editingNoteId = null; elements.noteComposer.hidden = true; elements.noteInput.value = ""; elements.composerMode.textContent = "captured point"; elements.noteComposer.querySelector('[type="submit"]').textContent = "Save note"; }
function saveComposerNote() {
  const body = elements.noteInput.value.trim();
  if (!body || state.draftTime === null) { showToast("Write a note before saving."); return; }
  const track = activeTrack();
  if (!track) return;
  if (state.editingNoteId) {
    const note = track.notes.find((item) => item.id === state.editingNoteId);
    if (!note) { cancelNote(); return; }
    const previousBody = note.body;
    note.body = body;
    const saved = saveState();
    if (!saved) note.body = previousBody;
    cancelNote();
    renderTrackList(); renderNotes(); renderReviewHeader(track);
    showToast(saved ? "Review note updated." : "Note could not be saved locally.");
    return;
  }
  const note = { id: `note-${Date.now()}`, time: state.draftTime, body, done: false, createdAt: new Date().toISOString() };
  track.notes.push(note);
  track.notes.sort((a, b) => a.time - b.time);
  const saved = saveState();
  if (!saved) track.notes = track.notes.filter((item) => item.id !== note.id);
  cancelNote();
  state.noteFilter = "open";
  renderTrackList(); renderNotes(); renderReviewHeader(track);
  showToast(saved ? "Review note saved." : "Note could not be saved locally.");
}

async function importFiles(fileList) {
  const files = [...fileList].filter((file) => file.type.startsWith("audio/") || /\.(mp3|wav|m4a|aac|flac|ogg)$/i.test(file.name));
  if (!files.length) { showToast("Choose an audio file to import."); return; }
  let importedCount = 0;
  for (const file of files) {
    const id = `track-${crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random()}`}`;
    const title = file.name.replace(/\.[^/.]+$/, "").replace(/[_-]+/g, " ").trim() || "Untitled track";
    const tempUrl = URL.createObjectURL(file); const duration = await new Promise((resolve) => { const probe = new Audio(); probe.onloadedmetadata = () => { URL.revokeObjectURL(tempUrl); resolve(Number.isFinite(probe.duration) ? probe.duration : 0); }; probe.onerror = () => { URL.revokeObjectURL(tempUrl); resolve(0); }; probe.src = tempUrl; });
    try { await storeFile(id, file); } catch (error) { showToast(`Could not store ${file.name}: ${error.message}`); continue; }
    state.tracks.push({ id, title, originalName: file.name, duration, size: file.size, createdAt: new Date().toISOString(), favorite: false, stage: DEFAULT_TRACK_STAGE, notes: [] }); importedCount += 1;
    state.selectedTrackId = id;
  }
  const saved = importedCount > 0 && saveState();
  if (importedCount) { state.restoreTrackId = null; state.restoreTime = 0; state.restorePlaying = false; state.playbackIntent = false; }
  renderAll();
  await loadSelectedAudio();
  if (importedCount) showToast(`${importedCount} track${importedCount === 1 ? "" : "s"} imported locally${saved ? "." : ", but metadata could not be saved."}`);
  persistSession();
  elements.fileInput.value = "";
}

function bindEvents() {
  $("#uploadButton").addEventListener("click", () => elements.fileInput.click()); $("#headerUploadButton").addEventListener("click", () => elements.fileInput.click()); $("#aboutButton").addEventListener("click", () => showToast("Cadence is a private, local-first track review desk.")); elements.fileInput.addEventListener("change", () => importFiles(elements.fileInput.files));
  elements.trackList.addEventListener("click", (event) => { const item = event.target.closest("[data-track-id]"); if (!item) return; if (event.target.closest("[data-track-favorite]")) { toggleTrackFavorite(item.dataset.trackId); return; } if (event.target.closest("[data-track-delete]")) { removeTrack(item.dataset.trackId); return; } if (event.target.closest("[data-track-open]")) selectTrack(item.dataset.trackId); });
  elements.trackDetails.addEventListener("click", (event) => { if (event.target.closest("[data-track-favorite]")) { toggleTrackFavorite(activeTrack()?.id); return; } if (event.target.closest("[data-remove-track]")) removeTrack(activeTrack()?.id); });
  elements.trackDetails.addEventListener("change", (event) => { if (event.target.matches("#trackStageSelect")) updateTrackStage(activeTrack()?.id, event.target.value); });
  elements.searchInput.addEventListener("input", () => { state.search = elements.searchInput.value; renderTrackList(); scheduleSessionPersistence(); }); elements.clearSearchButton.addEventListener("click", () => { elements.searchInput.value = ""; state.search = ""; renderTrackList(); scheduleSessionPersistence(); elements.searchInput.focus(); });
  $$("[data-library-filter]").forEach((button) => button.addEventListener("click", () => { state.libraryFilter = button.dataset.libraryFilter; renderTrackList(); scheduleSessionPersistence(); }));
  $$("[data-note-filter]").forEach((button) => button.addEventListener("click", () => { state.noteFilter = button.dataset.noteFilter; renderNotes(); scheduleSessionPersistence(); }));
  elements.playButton.addEventListener("click", togglePlayback);
  elements.audio.addEventListener("timeupdate", () => { updateTransport(); scheduleSessionPersistence(); });
  elements.audio.addEventListener("loadedmetadata", () => {
    const sourceTrackId = elements.audio.dataset.trackId;
    const track = activeTrack();
    if (!track || sourceTrackId !== track.id) return;
    if (!track.duration) { track.duration = elements.audio.duration; saveState(); renderTrackList(); renderReviewHeader(track); }
    if (state.restoreTrackId === track.id) {
      const duration = elements.audio.duration || track.duration || 0;
      const resumeTime = Math.max(0, Math.min(duration, Number(state.restoreTime) || 0));
      const shouldResume = state.restorePlaying;
      state.restoreTrackId = null;
      state.restoreTime = 0;
      state.restorePlaying = false;
      elements.audio.currentTime = resumeTime;
      if (shouldResume) preparePlaybackGraph().then(() => elements.audio.play()).catch(() => { state.playbackIntent = false; showToast("Playback is ready to resume — press play."); persistSession(); });
    }
    updateTransport();
  });
  elements.audio.addEventListener("play", () => { state.playbackIntent = true; preparePlaybackGraph(); updateTransport(); startPlaybackAnimation(); startLiveLoudness(); scheduleSessionPersistence(); });
  elements.audio.addEventListener("pause", () => { updateTransport(); stopPlaybackAnimation(); stopLiveLoudness(); scheduleSessionPersistence(); });
  elements.audio.addEventListener("ended", () => { state.playbackIntent = false; updateTransport(); stopPlaybackAnimation(); stopLiveLoudness("ENDED"); persistSession(); });
  elements.seekInput.addEventListener("input", () => seekTo(Number(elements.seekInput.value)));
  elements.volumeInput.addEventListener("input", () => { state.muted = Number(elements.volumeInput.value) === 0; updateAuditionOutput(); elements.volumeButton.textContent = state.muted ? "◌" : "⌁"; scheduleSessionPersistence(); }); elements.volumeButton.addEventListener("click", () => { state.muted = !state.muted; updateAuditionOutput(); elements.volumeButton.textContent = state.muted ? "◌" : "⌁"; elements.volumeButton.setAttribute("aria-label", state.muted ? "Unmute track" : "Mute track"); scheduleSessionPersistence(); });
  window.addEventListener("pagehide", persistSession); window.addEventListener("beforeunload", persistSession);
  elements.waveform.addEventListener("pointerdown", startSeekDrag); elements.waveform.addEventListener("pointermove", moveSeekDrag); elements.waveform.addEventListener("pointerup", finishSeekDrag); elements.waveform.addEventListener("pointercancel", cancelSeekDrag); elements.waveform.addEventListener("click", (event) => { if (state.suppressSeekClick) { state.suppressSeekClick = false; return; } if (!event.target.closest("#commentRail")) seekTo(timelineSeconds(event, elements.waveform)); }); elements.waveform.addEventListener("keydown", (event) => { if (["ArrowLeft", "ArrowRight"].includes(event.key)) { event.preventDefault(); seekTo((elements.audio.currentTime || 0) + (event.key === "ArrowRight" ? 5 : -5)); } }); elements.commentRail.addEventListener("pointerdown", startCommentDrag); elements.commentRail.addEventListener("pointermove", moveCommentDrag); elements.commentRail.addEventListener("pointerup", finishCommentDrag); elements.commentRail.addEventListener("pointercancel", cancelCommentDrag); elements.commentRail.addEventListener("click", (event) => { event.stopPropagation(); if (state.suppressCommentClick) return; const marker = event.target.closest("[data-marker-note-id]"); if (marker) { const note = activeTrack()?.notes.find((item) => item.id === marker.dataset.markerNoteId); if (note) seekTo(note.time); return; } beginNoteAt(timelineSeconds(event, elements.commentRail), event); }); elements.waveform.addEventListener("pointermove", updateTimelineCursor); elements.commentRail.addEventListener("pointermove", updateCommentPreview); elements.waveformStage.addEventListener("pointerleave", hideTimelineCursor);
  elements.addNoteButton.addEventListener("click", beginNote); elements.cancelNoteButton.addEventListener("click", cancelNote); elements.noteComposer.addEventListener("submit", (event) => { event.preventDefault(); saveComposerNote(); });
  elements.notesList.addEventListener("click", (event) => { const row = event.target.closest("[data-note-id]"); if (!row) return; const track = activeTrack(); const note = track.notes.find((item) => item.id === row.dataset.noteId); if (!note) return; if (event.target.closest(".timestamp-button")) { seekTo(note.time); return; } if (event.target.closest(".note-edit")) { beginEditNote(note); return; } if (event.target.closest(".note-delete")) { const originalNotes = track.notes; track.notes = track.notes.filter((item) => item.id !== note.id); const saved = saveState(); if (!saved) track.notes = originalNotes; if (state.editingNoteId === note.id) cancelNote(); renderAll(); showToast(saved ? "Review note deleted." : "Note could not be deleted locally."); } }); elements.notesList.addEventListener("change", (event) => { const row = event.target.closest("[data-note-id]"); if (!row || !event.target.matches(".note-check")) return; const note = activeTrack().notes.find((item) => item.id === row.dataset.noteId); const previousDone = note.done; note.done = event.target.checked; const saved = saveState(); if (!saved) note.done = previousDone; renderTrackList(); renderNotes(); renderReviewHeader(activeTrack()); showToast(saved ? (note.done ? "Note marked complete." : "Note reopened.") : "Note completion could not be saved locally."); });
  $("#plannerButton").addEventListener("click", () => showToast("The planner is next — your review notes are already ready for it."));
  document.addEventListener("keydown", (event) => { const activeElement = document.activeElement; const typing = ["INPUT", "TEXTAREA", "SELECT"].includes(activeElement.tagName); const interactive = typing || ["BUTTON", "A"].includes(activeElement.tagName) || activeElement.isContentEditable; if (event.key === "/" && !typing) { event.preventDefault(); elements.searchInput.focus(); } if (event.key.toLowerCase() === "u" && !interactive) elements.fileInput.click(); if (event.key === " " && !interactive && activeTrack() && !activeTrack().demo) { event.preventDefault(); togglePlayback(); } if (event.key.toLowerCase() === "n" && !interactive) { event.preventDefault(); beginNote(); } if (event.key === "Escape" && !elements.noteComposer.hidden) cancelNote(); if (event.key === "Enter" && document.activeElement === elements.noteInput && !event.shiftKey) { event.preventDefault(); elements.noteComposer.requestSubmit(); } });
  let dragDepth = 0; document.addEventListener("dragenter", (event) => { if ([...event.dataTransfer.types].includes("Files")) { event.preventDefault(); dragDepth += 1; elements.dropOverlay.hidden = false; } }); document.addEventListener("dragover", (event) => { if ([...event.dataTransfer.types].includes("Files")) event.preventDefault(); }); document.addEventListener("dragleave", (event) => { if ([...event.dataTransfer.types].includes("Files")) { dragDepth -= 1; if (dragDepth <= 0) { dragDepth = 0; elements.dropOverlay.hidden = true; } } }); document.addEventListener("drop", (event) => { if ([...event.dataTransfer.types].includes("Files")) { event.preventDefault(); dragDepth = 0; elements.dropOverlay.hidden = true; importFiles(event.dataTransfer.files); } });
}

function loadTracks() { try { const stored = JSON.parse(localStorage.getItem(STORAGE_KEY) || "null"); return Array.isArray(stored) && stored.length ? stored.map(normalizeTrack) : [cloneDemo()]; } catch { return [cloneDemo()]; } }
async function reconcilePendingDeletions() {
  const storedPending = loadPendingDeletions();
  const pending = storedPending.filter((id) => id !== DEMO_TRACK_ID);
  if (pending.length !== storedPending.length) savePendingDeletions(pending, "Preview cleanup marker could not be cleared; it will be ignored");
  for (const id of pending) {
    const previousTracks = state.tracks;
    if (previousTracks.some((track) => track.id === id)) {
      state.tracks = previousTracks.filter((track) => track.id !== id);
      if (!saveState()) {
        state.tracks = previousTracks;
        continue;
      }
    }
    try {
      await deleteFile(id);
    } catch {
      continue;
    }
    clearPendingDeletion(id);
  }
}
async function init() {
  const session = loadSessionState();
  state.tracks = loadTracks();
  if (!state.tracks.some((track) => track.id === DEMO_TRACK_ID)) state.tracks.push(cloneDemo());
  state.selectedTrackId = typeof session.selectedTrackId === "string" ? session.selectedTrackId : DEMO_TRACK_ID;
  state.noteFilter = ["open", "all", "done"].includes(session.noteFilter) ? session.noteFilter : "open";
  state.libraryFilter = ["all", "needs-review", "clear"].includes(session.libraryFilter) ? session.libraryFilter : "all";
  state.search = typeof session.search === "string" ? session.search : "";
  state.muted = session.muted === true;
  const sessionVolume = Number(session.volume);
  if (Number.isFinite(sessionVolume)) elements.volumeInput.value = String(Math.max(0, Math.min(1, sessionVolume)));
  state.restoreTrackId = typeof session.trackId === "string" ? session.trackId : state.selectedTrackId;
  state.restoreTime = Number.isFinite(Number(session.currentTime)) ? Math.max(0, Number(session.currentTime)) : 0;
  state.restorePlaying = session.wasPlaying === true;
  state.playbackIntent = state.restorePlaying;
  await reconcilePendingDeletions();
  const restoredTrack = state.tracks.find((track) => track.id === state.selectedTrackId);
  state.selectedTrackId = restoredTrack?.id || state.tracks[0]?.id || DEMO_TRACK_ID;
  if (state.restoreTrackId !== state.selectedTrackId) { state.restoreTrackId = null; state.restoreTime = 0; state.restorePlaying = false; }
  if (!elements.searchInput.value) elements.searchInput.value = state.search;
  bindEvents();
  renderAll();
  state.suppressSessionPersistence = true;
  try { await loadSelectedAudio(); } finally { state.suppressSessionPersistence = false; }
 updateAuditionOutput();
  elements.volumeButton.textContent = state.muted ? "◌" : "⌁";
  elements.volumeButton.setAttribute("aria-label", state.muted ? "Unmute track" : "Mute track");
 persistSession();
}
init();
