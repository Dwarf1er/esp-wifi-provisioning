// START_DEV
const DEV_NETWORKS = [
  { ssid: "DevNetwork_5G",  rssi: -42 },
  { ssid: "DevNetwork_2G",  rssi: -61 },
  { ssid: "Neighbor_WiFi",  rssi: -74 },
  { ssid: "WeakSignal",     rssi: -88 },
];
// END_DEV

const netDiv  = document.getElementById('networks');
const ssidIn  = document.getElementById('ssid');
const passIn  = document.getElementById('pass');
const btn     = document.getElementById('btn');
const statusEl = document.getElementById('status');
const spinner = document.getElementById('spinner');

function rssiToBars(rssi) {
  if (rssi >= -55) return 4;
  if (rssi >= -65) return 3;
  if (rssi >= -75) return 2;
  return 1;
}

function barsHtml(n) {
  const heights = [5, 8, 11, 14];
  return '<div class="bars">' +
    heights.map((h, i) =>
      `<div class="bar${i < n ? '' : ' off'}" style="height:${h}px"></div>`
    ).join('') + '</div>';
}

function renderNetworks(networks) {
  netDiv.innerHTML = '';
  networks.forEach(net => {
    const el = document.createElement('div');
    el.className = 'net-item';
    el.innerHTML =
      `<span class="net-ssid">${net.ssid}</span>` +
      `<span style="display:flex;align-items:center">` +
      `<span class="net-rssi">${net.rssi} dBm</span>` +
      barsHtml(rssiToBars(net.rssi)) +
      `</span>`;
    el.onclick = () => {
      document.querySelectorAll('.net-item').forEach(e => e.classList.remove('selected'));
      el.classList.add('selected');
      ssidIn.value = net.ssid;
      passIn.focus();
    };
    netDiv.appendChild(el);
  });
}

async function loadNetworks() {
  try {
    const r = await fetch('/networks');
    if (!r.ok) throw new Error('non-200');
    const networks = await r.json();
    renderNetworks(networks);
  } catch {
    renderNetworks(DEV_NETWORKS);
  }
}

async function submit() {
  const ssid = ssidIn.value.trim();
  const pass = passIn.value;
  if (!ssid) { setStatus('Please enter an SSID.', 'err'); return; }

  btn.disabled = true;
  spinner.style.display = 'block';
  setStatus('Connecting\u2026', '');

  try {
    const r = await fetch('/connect', {
      method: 'POST',
      headers: { 'Content-Type': 'text/plain' },
      body: `${ssid}\n${pass}`
    });
    const j = await r.json();
    if (j.success) {
      setStatus('Credentials saved. Device will reboot and attempt to connect.', 'ok');
    } else {
      setStatus(j.message || 'Connection failed. Check credentials.', 'err');
      btn.disabled = false;
    }
  } catch {
    setStatus('Request failed. Try again.', 'err');
    btn.disabled = false;
  } finally {
    spinner.style.display = 'none';
  }
}

function setStatus(msg, cls) {
  statusEl.textContent = msg;
  statusEl.className = cls;
}

loadNetworks();
