// static/js/markers.js
import { showToast } from '/static/js/utils.js';
import api from '/static/js/api.js';

// Global für aktuelle Session ID (wird beim Start gesetzt)
let currentSessionId = null;

// 1️⃣ Geladene Marker-Typen speichern (damit wir Farbe/Name kennen beim Stream)
let markerTypesMap = {};

export function setSessionId(sessionId) {
    currentSessionId = sessionId;
    loadMarkerStream();  // Stream sofort laden
}

// 2️⃣ Marker-Typen laden und Buttons bauen
export async function loadSettableMarkers() {
    const markerButtonContainer = document.getElementById("markerControls");

    const markerTypes = await api.getMarkerTypes();
    markerButtonContainer.innerHTML = ""; // clear old

    markerTypes.forEach(marker => {
        const btn = document.createElement("button");

        // → Scene-Style!
        btn.innerHTML = `<div class="text-lg">${marker.name}</div>`;
        btn.style.backgroundColor = marker.color || "#444";

        btn.className = "p-6 rounded-2xl text-white font-bold text-center bg-gray-800 hover:bg-blue-600 transition-all transform hover:scale-105 shadow-lg cursor-pointer m-2";

        // → Funktion bleibt GENAU GLEICH!
        btn.addEventListener("click", () => addMarker(marker.id));

        markerButtonContainer.appendChild(btn);
    });
}




// 4️⃣ Marker setzen
export async function addMarker(markerTypeId) {
    const payload = {
        session_id: currentSessionId,
        marker_type_id: markerTypeId,
        user_id: 1,  // ⚠️ MUSS EINE GÜLTIGE user.id SEIN!
        text: ""     // Optional
    };

    console.log("📍 addMarker payload:", payload);

    try {
        await api.addMarker(payload);
        showToast(`📍 Marker gesetzt!`);
        loadMarkerStream();
    } catch (error) {
        console.error("Fehler beim Setzen des Markers:", error);
    }
}



export function setCurrentSessionId(sessionId) {
    currentSessionId = sessionId;
    console.log("marker.js → currentSessionId gesetzt:", currentSessionId);
}

export async function loadMarkerStream() {
    if (!currentSessionId) {
        console.warn("⚠️ Keine aktive Session → Marker-Stream nicht geladen.");
        return;
    }

    const markers = await api.getMarkers(currentSessionId);
    // → Marker-Stream anzeigen ...
}


function getClientId() {
    let clientId = localStorage.getItem('client_id');
    if (!clientId) {
        clientId = 'client-' + Math.floor(Math.random() * 1000000);  // ODER → richtige UUID nehmen!
        localStorage.setItem('client_id', clientId);
        console.log("Neue client_id generiert:", clientId);
    }
    return clientId;
}
