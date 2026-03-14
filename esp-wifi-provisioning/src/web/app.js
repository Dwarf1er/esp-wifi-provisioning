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

togglePass.addEventListener("click", () => {
    const hidden = passIn.type === "password";

    passIn.type = hidden ? "text" : "password";
    eyeIcon.classList.toggle("striked", hidden);

    togglePass.setAttribute(
        "aria-label",
        hidden ? "Hide password" : "Show password"
    );

    togglePass.setAttribute("aria-pressed", hidden ? "true" : "false");
});

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

function renderNetworks(networks) {
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
            announceEl.textContent = `Selected: ${net.ssid}`;
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

    netDiv.querySelector('.net-item')?.focus();
}

async function loadNetworks() {
    netDiv.textContent = 'Scanning networks…';
    rescanBtn.disabled = true;
    rescanBtn.textContent = '↺ Scanning…';

    try {
        const r = await fetch('/networks');
        if (!r.ok) throw new Error('non-200');
        renderNetworks(await r.json());
    } catch {
        if (typeof DEV_NETWORKS !== 'undefined') {
            renderNetworks(DEV_NETWORKS);
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
    loadNetworks();
});

async function handleSubmit() {
    const ssid = ssidIn.value.trim();
    const pass = passIn.value;

    if (!ssid) {
        setStatus('Please enter an SSID.', 'err');
        return;
    }

    btn.disabled = true;
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

loadNetworks();
