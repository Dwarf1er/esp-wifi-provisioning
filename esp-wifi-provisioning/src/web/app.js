// START_DEV
const DEV_NETWORKS = [{
        ssid: "DevNetwork_5G",
        rssi: -42,
        secure: true
    },
    {
        ssid: "DevNetwork_2G",
        rssi: -61,
        secure: true
    },
    {
        ssid: "Neighbor_WiFi",
        rssi: -74,
        secure: false
    },
    {
        ssid: "WeakSignal",
        rssi: -88,
        secure: false
    },
    {
        ssid: "LooooooooooooooooooooooooongSSID",
        rssi: -99,
        secure: false
    },
];
// END_DEV

const SSID_LEN_MAX = 32;
const WPA_PASS_LEN_MIN = 8;
const WPA_PASS_LEN_MAX = 63;

const netDiv = document.getElementById('networks');
const ssidIn = document.getElementById('ssid');
const passIn = document.getElementById('pass');
const togglePass = document.getElementById("toggle-pass");
const eyeIcon = document.querySelector(".eye-icon");
const btn = document.getElementById('btn');
const form = document.getElementById('wifi-form');
const statusEl = document.getElementById('status');
const spinner = document.getElementById('spinner');
const rescanBtn = document.getElementById('rescan-btn');
const announceEl = document.getElementById('network-announce');
const ssidHint = document.getElementById('ssid-hint');
const passHint = document.getElementById('pass-hint');

togglePass.addEventListener("click", () => {
    const hidden = passIn.type === "password";

    passIn.type = hidden ? "text" : "password";
    eyeIcon.classList.toggle("striked", !hidden);

    togglePass.setAttribute(
        "aria-label",
        hidden ? "Hide password" : "Show password"
    );

    togglePass.setAttribute("aria-pressed", hidden ? "true" : "false");
});

ssidIn.addEventListener("input", () => validateSsid(false));
passIn.addEventListener("input", () => validatePass(false));

function validateSsid(isFinal) {
    const val = ssidIn.value.trim();

    if (val.length === 0) {
        if (isFinal) setSsidHint('Network name is required', 'err');
        else setSsidHint('', '');
        return isFinal ? false : true;
    }

    if (val.length > SSID_LEN_MAX) {
        setSsidHint(`Network name is too long (max ${SSID_LEN_MAX} characters)`, 'err');
        return false;
    }

    setSsidHint('', '');
    return true;
}

function validatePass(isFinal) {
    const val = passIn.value;

    if (val.length === 0) {
        setPassHint('', '');
        return true;
    }

    if (val.length < WPA_PASS_LEN_MIN) {
        const remaining = WPA_PASS_LEN_MIN - val.length;
        const charWord  = remaining === 1 ? 'character' : 'characters';
        setPassHint(`WPA password needs ${remaining} more ${charWord} (min ${WPA_PASS_LEN_MIN})`, 'err');
        return false
    }

    if (val.length > WPA_PASS_LEN_MAX) {
        setPassHint(`Password is too long (max ${WPA_PASS_LEN_MAX} characters)`, 'err');
        return false;
    }

    setPassHint(`Password length between 8 and 63: (${val.length}/${WPA_PASS_LEN_MAX})`, 'ok');
    return true;
}

function setSsidHint(msg, cls) {
    ssidHint.textContent = msg;
    ssidHint.className   = `pass-hint${cls ? ` pass-hint--${cls}` : ''}`;
}

function setPassHint(msg, cls) {
    passHint.textContent = msg;
    passHint.className   = `pass-hint${cls ? ` pass-hint--${cls}` : ''}`;
}

form.addEventListener("submit", e => {
    e.preventDefault();
    handleSubmit();
});

const rssiToBars = rssi =>
    rssi >= -55 ? 4 :
    rssi >= -65 ? 3 :
    rssi >= -75 ? 2 : 1;

function barsHtml(n) {
    return '<div class="bars">' + [0, 1, 2, 3].map(i =>
            `<div class="bar bar-${i}${i < n ? '' : ' off'}"></div>`
        ).join('') +
        '</div>';
}

function renderNetworks(networks, moveFocus) {
    netDiv.innerHTML = '';
    netDiv.setAttribute('aria-activedescendant', '');

    networks.forEach((net, index) => {
        const id = `net-item-${index}`;
        const el = document.createElement('div');
        el.className = 'net-item';
        el.id = id;
        el.setAttribute('tabindex', '0');
        el.setAttribute('role', 'option');
        el.setAttribute('aria-selected', 'false');

        const barsCount = rssiToBars(net.rssi);
        const strengthLabel = ['weak', 'weak', 'fair', 'good', 'excellent'][barsCount];
        const secureLabel = net.secure ? 'secured' : 'open';

        el.setAttribute('aria-label', `${net.ssid}, ${secureLabel}, ${strengthLabel} signal`);

        el.innerHTML =
            `<span class="net-ssid" data-lock="${net.secure ? '🔒' : ''}">${net.ssid}</span>` +
            `<span class="net-meta">` +
            `<span class="net-rssi">${net.rssi} dBm</span>` +
            barsHtml(barsCount) +
            `</span>`;

        function selectNetwork(jumpFocus) {
            document.querySelectorAll('.net-item').forEach(e => {
                e.classList.remove('selected');
                e.setAttribute('aria-selected', 'false');
            });
            el.classList.add('selected');
            el.setAttribute('aria-selected', 'true');
            netDiv.setAttribute('aria-activedescendant', id);
            ssidIn.value = net.ssid;
            setSsidHint("", "");
            announceEl.textContent = `Selected: ${net.ssid}`;

            if (!net.secure) {
                passIn.value = "";
                setPassHint("");
            } else {
                validatePass(false);
            }
            if (jumpFocus) passIn.focus();
        }

        el.addEventListener('click', () => selectNetwork(true));

        el.addEventListener('keydown', e => {
            if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                selectNetwork(true);
            }
            if (e.key === 'ArrowDown') {
                e.preventDefault();
                const next = el.nextElementSibling || netDiv.firstElementChild;
                next.focus();
            }
            if (e.key === 'ArrowUp') {
                e.preventDefault();
                const prev = el.previousElementSibling || netDiv.lastElementChild;
                prev.focus();
            }
        });

        netDiv.appendChild(el);
    });

    if (moveFocus) {
        netDiv.querySelector('.net-item')?.focus();
    }
}

async function loadNetworks(moveFocus = false) {
    netDiv.textContent = 'Scanning networks…';
    rescanBtn.disabled = true;
    rescanBtn.textContent = '↺ Scanning…';

    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), 15_000);

    try {
        const r = await fetch('/networks', { signal: controller.signal });
        if (!r.ok) throw new Error('non-200');
        renderNetworks(await r.json(), moveFocus);
    } catch {
        if (typeof DEV_NETWORKS !== 'undefined') {
            renderNetworks(DEV_NETWORKS, moveFocus);
        } else {
            netDiv.innerHTML =
                '<p class="networks-err">Could not load networks. Try rescanning.</p>';
        }
    } finally {
        rescanBtn.disabled = false;
        rescanBtn.textContent = '↺ Rescan';
    }
}

rescanBtn.addEventListener('click', () => {
    setStatus('', '');
    loadNetworks(false);
});

async function handleSubmit() {
    const ssid = ssidIn.value.trim();
    const pass = passIn.value;

    if (!validateSsid(true)) {
        ssidIn.focus();
        return;
    }

    if (!validatePass(true)) {
        passIn.focus();
        return;
    }

    btn.disabled = true;
    rescanBtn.disabled = true;
    spinner.style.display = 'block';
    setStatus('Connecting…', '');

    try {
        const r = await fetch('/connect', {
            method: 'POST',
            headers: {
                'Content-Type': 'text/plain'
            },
            body: `${ssid}\n${pass}`
        });

        const j = await r.json();

        if (j.success) {
            setStatus(
                'Credentials saved. Device will reboot and attempt to connect.',
                'ok'
            );
        } else {
            setStatus(j.message || 'Connection failed.', 'err');
            btn.disabled = false;
        }
    } catch {
        setStatus('Request failed. Try again.', 'err');
        btn.disabled = false;
    } finally {
        spinner.style.display = 'none';
        rescanBtn.disabled = false;
    }
}

function setStatus(msg, cls) {
    statusEl.textContent = msg;
    statusEl.className = cls;
    if (msg) statusEl.scrollIntoView({
        behavior: 'smooth',
        block: 'nearest'
    });
}

async function checkLastError() {
    try {
        const r = await fetch('/status');
        const j = await r.json();
        if (j.error) {
            document.getElementById('last-error-msg').textContent = j.error;
            document.getElementById('last-error').removeAttribute('hidden');
        }
    } catch { 
        // non-fatal, banner stays hidden if /status is unreachable
    }
}

checkLastError();
loadNetworks(true);
