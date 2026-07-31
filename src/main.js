import { LineChart } from './chart.js';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

let currentMode = 'live'; // 'live' or 'offline'
let isMonitoring = false;
let chart = null;
let logData = []; // Sparar loggobjekt { timestamp, file_name, pixel_diff, ocr_text, is_changed, image_base64 }

// DOM-element
const statusDot = document.getElementById('status-dot');
const statusText = document.getElementById('status-text');
const btnMainAction = document.getElementById('btn-main-action');
const btnSelectDir = document.getElementById('btn-select-dir');
const btnRefreshSources = document.getElementById('btn-refresh-sources');
const btnClearLog = document.getElementById('btn-clear-log');
const sourceType = document.getElementById('source-type');
const sourceId = document.getElementById('source-id');
const enableCrop = document.getElementById('enable-crop');
const cropCoords = document.getElementById('crop-coords');
const intervalSecs = document.getElementById('interval-secs');
const saveDir = document.getElementById('save-dir');
const enableOcr = document.getElementById('enable-ocr');
const enableOcrCrop = document.getElementById('enable-ocr-crop');
const ocrCoords = document.getElementById('ocr-coords');
const ocrCropCheckboxContainer = document.getElementById('ocr-crop-checkbox-container');
const diffThreshold = document.getElementById('diff-threshold');
const thresholdVal = document.getElementById('threshold-val');
const previewPlaceholder = document.getElementById('preview-placeholder');
const previewImg = document.getElementById('preview-img');
const logTbody = document.getElementById('log-tbody');

// Stats-element
const statTotal = document.getElementById('stat-total');
const statChanges = document.getElementById('stat-changes');
const statAvg = document.getElementById('stat-avg');

// Koordinat-element
const cropX = document.getElementById('crop-x');
const cropY = document.getElementById('crop-y');
const cropW = document.getElementById('crop-w');
const cropH = document.getElementById('crop-h');
const ocrX = document.getElementById('ocr-x');
const ocrY = document.getElementById('ocr-y');
const ocrW = document.getElementById('ocr-w');
const ocrH = document.getElementById('ocr-h');

// Initiera appen
document.addEventListener('DOMContentLoaded', () => {
  chart = new LineChart('diff-chart');
  
  loadSources();
  setupEventListeners();
  checkCurrentStatus();
});

// Kontrollera om en backend-loop redan körs (t.ex. om appen laddas om)
async function checkCurrentStatus() {
  try {
    const running = await invoke('get_capture_status');
    if (running) {
      setMonitoringState(true);
    }
  } catch (err) {
    console.error('Kunde inte läsa status:', err);
  }
}

// Lyssna på Tauri IPC-events
async function setupEventListeners() {
  // Lyssna på lyckade skärmklippshändelser
  await listen('capture-result', (event) => {
    const result = event.payload;
    handleNewResult(result);
  });

  // Lyssna på klippsfel
  await listen('capture-error', (event) => {
    console.error('Klippfel från backend:', event.payload);
    alert(event.payload);
  });
}

function handleNewResult(result) {
  logData.push(result);
  
  // Uppdatera grafen
  chart.addData(result.pixel_diff, result.timestamp);
  
  // Uppdatera preview-bilden
  if (result.image_base64) {
    previewPlaceholder.classList.add('hidden');
    previewImg.classList.remove('hidden');
    previewImg.src = result.image_base64;
  }
  
  // Uppdatera loggtabell och statistik
  updateStats();
  appendLogEntry(result, logData.length - 1);
}

function updateStats() {
  if (logData.length === 0) {
    statTotal.textContent = '0';
    statChanges.textContent = '0';
    statAvg.textContent = '0.0%';
    return;
  }

  const total = logData.length;
  const changes = logData.filter(d => d.is_changed).length;
  
  // Beräkna genomsnittlig diff
  const sumDiff = logData.reduce((acc, curr) => acc + curr.pixel_diff, 0);
  const avgDiff = (sumDiff / total) * 100;

  statTotal.textContent = total;
  statChanges.textContent = changes;
  statAvg.textContent = `${avgDiff.toFixed(2)}%`;
}

function appendLogEntry(entry, index) {
  // Ta bort "Tom logg"-rad om det är första posten
  if (logData.length === 1) {
    logTbody.innerHTML = '';
  }

  const tr = document.createElement('tr');
  tr.id = `log-row-${index}`;
  tr.onclick = () => previewLogEntry(index);

  const tdTime = document.createElement('td');
  // Visa bara klockslag för renare gränssnitt
  tdTime.textContent = entry.timestamp.split(' ')[1] || entry.timestamp;
  
  const tdFile = document.createElement('td');
  tdFile.textContent = entry.file_name;
  
  const tdDiff = document.createElement('td');
  tdDiff.textContent = `${(entry.pixel_diff * 100).toFixed(2)}%`;
  
  const tdOcr = document.createElement('td');
  tdOcr.textContent = entry.ocr_text || '-';
  tdOcr.style.fontFamily = entry.ocr_text ? 'monospace' : 'inherit';
  
  const tdStatus = document.createElement('td');
  const badge = document.createElement('span');
  badge.className = 'badge ' + (entry.is_changed ? 'success' : 'neutral');
  badge.textContent = entry.is_changed ? 'Ändrad' : 'Stabil';
  tdStatus.appendChild(badge);

  tr.appendChild(tdTime);
  tr.appendChild(tdFile);
  tr.appendChild(tdDiff);
  tr.appendChild(tdOcr);
  tr.appendChild(tdStatus);

  // Lägg till högst upp i tabellen (nyast först)
  logTbody.insertBefore(tr, logTbody.firstChild);
}

// Låter användaren klicka i loggen för att förhandsgranska bilder i efterhand
function previewLogEntry(index) {
  const entry = logData[index];
  if (!entry) return;

  // Markera aktiv rad
  document.querySelectorAll('#log-tbody tr').forEach(row => {
    row.style.background = '';
  });
  const row = document.getElementById(`log-row-${index}`);
  if (row) {
    row.style.background = 'rgba(99, 102, 241, 0.1)';
  }

  if (entry.image_base64) {
    previewPlaceholder.classList.add('hidden');
    previewImg.classList.remove('hidden');
    previewImg.src = entry.image_base64;
  }
}

// Konfigurera event listeners för kontroller
function setupEventListeners() {
  // Spara mapp val
  btnSelectDir.onclick = async () => {
    const selected = await invoke('select_folder');
    if (selected) {
      saveDir.value = selected;
    }
  };

  // Typ av källa (skärm vs fönster)
  sourceType.onchange = () => loadSources();

  // Uppdatera källor
  btnRefreshSources.onclick = () => loadSources();

  // Rensa loggen
  btnClearLog.onclick = () => {
    logData = [];
    logTbody.innerHTML = `
      <tr>
        <td colspan="5" style="text-align: center; color: var(--text-muted); padding: 30px;">
          Loggen är tom. Ingen övervakning har gjorts.
        </td>
      </tr>
    `;
    chart.clear();
    updateStats();
    previewImg.classList.add('hidden');
    previewPlaceholder.classList.remove('hidden');
  };

  // Beskärningskryssruta
  enableCrop.onchange = (e) => {
    if (e.target.checked) {
      cropCoords.classList.remove('hidden');
    } else {
      cropCoords.classList.add('hidden');
    }
  };

  // OCR-kryssruta
  enableOcr.onchange = (e) => {
    if (e.target.checked) {
      ocrCropCheckboxContainer.classList.remove('hidden');
      if (enableOcrCrop.checked) {
        ocrCoords.classList.remove('hidden');
      }
    } else {
      ocrCropCheckboxContainer.classList.add('hidden');
      ocrCoords.classList.add('hidden');
    }
  };

  // OCR separat beskärning kryssruta
  enableOcrCrop.onchange = (e) => {
    if (e.target.checked && enableOcr.checked) {
      ocrCoords.classList.remove('hidden');
    } else {
      ocrCoords.classList.add('hidden');
    }
  };

  // Känslighets-slider
  diffThreshold.oninput = (e) => {
    thresholdVal.textContent = `${parseFloat(e.target.value).toFixed(1)}%`;
  };

  // Huvudknapp (Start/Stopp eller Analysera)
  btnMainAction.onclick = () => {
    if (currentMode === 'live') {
      toggleMonitoring();
    } else {
      runOfflineAnalysis();
    }
  };
}

// Hämta skärmar eller programfönster från Rust
async function loadSources() {
  const type = sourceType.value;
  sourceId.innerHTML = '<option value="">Laddar källor...</option>';
  
  try {
    if (type === 'screen') {
      const monitors = await invoke('list_monitors');
      sourceId.innerHTML = '';
      monitors.forEach(m => {
        const opt = document.createElement('option');
        opt.value = m.id;
        opt.textContent = `${m.name} [${m.width}x${m.height}]`;
        sourceId.appendChild(opt);
      });
    } else {
      const windows = await invoke('list_windows');
      sourceId.innerHTML = '';
      windows.forEach(w => {
        const opt = document.createElement('option');
        opt.value = w.id;
        opt.textContent = `${w.app_name} - ${w.title.substring(0, 45)}`;
        sourceId.appendChild(opt);
      });
    }
  } catch (err) {
    sourceId.innerHTML = `<option value="">Kunde inte ladda källor: ${err}</option>`;
  }
}

// Växlar mellan Realtid och Efterhandsanalys
function setMode(mode) {
  currentMode = mode;
  document.getElementById('tab-live').classList.toggle('active', mode === 'live');
  document.getElementById('tab-offline').classList.toggle('active', mode === 'offline');

  const intervalGroup = document.getElementById('interval-group');
  const targetSelectionGroup = document.getElementById('target-selection-group');
  const sensitivityGroup = document.getElementById('sensitivity-group');
  const dirLabel = document.getElementById('dir-label');

  if (mode === 'live') {
    intervalGroup.classList.remove('hidden');
    targetSelectionGroup.classList.remove('hidden');
    sensitivityGroup.classList.remove('hidden');
    dirLabel.textContent = 'Spara skärmklipp i mapp';
    btnMainAction.textContent = isMonitoring ? 'Stoppa övervakning' : 'Starta övervakning';
  } else {
    intervalGroup.classList.add('hidden');
    targetSelectionGroup.classList.add('hidden');
    sensitivityGroup.classList.add('hidden');
    dirLabel.textContent = 'Analysera skärmklipp i mapp';
    btnMainAction.textContent = 'Analysera mapp';
    btnMainAction.className = 'btn-primary';
  }
}
window.setMode = setMode; // Gör globalt tillgänglig för HTML-tabbar

// Slår på/av realtidsövervakning
async function toggleMonitoring() {
  if (isMonitoring) {
    // Stoppa
    try {
      await invoke('stop_capture');
      setMonitoringState(false);
    } catch (err) {
      alert(`Kunde inte stoppa övervakning: ${err}`);
    }
  } else {
    // Starta
    if (!saveDir.value) {
      alert('Vänligen välj en mapp att spara skärmklippen i först.');
      return;
    }
    if (!sourceId.value) {
      alert('Vänligen välj en källa att klippa ifrån.');
      return;
    }

    const settings = {
      source_type: sourceType.value,
      source_id: parseInt(sourceId.value),
      interval_secs: parseInt(intervalSecs.value),
      save_dir: saveDir.value,
      crop_area: enableCrop.checked ? [
        parseInt(cropX.value),
        parseInt(cropY.value),
        parseInt(cropW.value),
        parseInt(cropH.value)
      ] : null,
      enable_ocr: enableOcr.checked,
      ocr_area: (enableOcr.checked && enableOcrCrop.checked) ? [
        parseInt(ocrX.value),
        parseInt(ocrY.value),
        parseInt(ocrW.value),
        parseInt(ocrH.value)
      ] : null,
      diff_threshold: parseFloat(diffThreshold.value) / 100.0 // Konvertera % till 0.0 - 1.0
    };

    try {
      await invoke('start_capture', { settings });
      setMonitoringState(true);
    } catch (err) {
      alert(`Kunde inte starta övervakning: ${err}`);
    }
  }
}

function setMonitoringState(active) {
  isMonitoring = active;
  if (active) {
    statusDot.classList.add('active');
    statusText.textContent = 'Aktiv';
    btnMainAction.textContent = 'Stoppa övervakning';
    btnMainAction.classList.add('running');
  } else {
    statusDot.classList.remove('active');
    statusText.textContent = 'Inaktiv';
    btnMainAction.textContent = 'Starta övervakning';
    btnMainAction.classList.remove('running');
  }
}

// Kör offlineanalys på en mapp
async function runOfflineAnalysis() {
  if (!saveDir.value) {
    alert('Vänligen välj mappen med skärmklipp som ska analyseras.');
    return;
  }

  btnMainAction.disabled = true;
  btnMainAction.textContent = 'Analyserar...';
  statusText.textContent = 'Analyserar';
  statusDot.className = 'status-dot active';

  const args = {
    dirPath: saveDir.value,
    cropArea: enableCrop.checked ? [
      parseInt(cropX.value),
      parseInt(cropY.value),
      parseInt(cropW.value),
      parseInt(cropH.value)
    ] : null,
    enableOcr: enableOcr.checked,
    ocrArea: (enableOcr.checked && enableOcrCrop.checked) ? [
      parseInt(ocrX.value),
      parseInt(ocrY.value),
      parseInt(ocrW.value),
      parseInt(ocrH.value)
    ] : null,
    diffThreshold: parseFloat(diffThreshold.value) / 100.0
  };

  try {
    const results = await invoke('run_offline_analysis', args);
    
    // Töm loggen och grafen först
    logData = [];
    logTbody.innerHTML = '';
    chart.clear();

    if (results.length === 0) {
      logTbody.innerHTML = `
        <tr>
          <td colspan="5" style="text-align: center; color: var(--text-muted); padding: 30px;">
            Hittade inga kompatibla skärmklipp (.png/.jpg) i den valda mappen.
          </td>
        </tr>
      `;
    } else {
      results.forEach((res, idx) => {
        // Skapa en fullt kompatibel logg-post
        const entry = {
          timestamp: res.file_name, // Använd filnamn som tidsstämpel i offline-läge
          file_name: res.file_name,
          pixel_diff: res.pixel_diff,
          ocr_text: res.ocr_text,
          is_changed: res.is_changed,
          image_base64: res.image_base64
        };

        logData.push(entry);
        chart.addData(res.pixel_diff, res.file_name);
        appendLogEntry(entry, idx);
      });

      // Förhandsgranska den sista bilden automatiskt
      previewLogEntry(results.length - 1);
    }

    updateStats();
    alert(`Analys klar! Bearbetade ${results.length} skärmklipp.`);
  } catch (err) {
    alert(`Fel vid offlineanalys: ${err}`);
  } finally {
    btnMainAction.disabled = false;
    btnMainAction.textContent = 'Analysera mapp';
    statusText.textContent = 'Inaktiv';
    statusDot.className = 'status-dot';
  }
}
