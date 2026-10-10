// No credentials or telemetry enter worker messages or persistent browser state.
if ("serviceWorker" in navigator && window.isSecureContext) {
  window.addEventListener("load", () => {
    navigator.serviceWorker.register("./service-worker.js", { scope: "./" })
      .catch(() => { /* The online console remains usable without offline shell. */ });
  });
}

// Only WebAuthn ceremony options use JS JSON. Telemetry is decoded in Rust.
const fabricDecode = (value) => {
  const raw = atob(value.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(raw, (character) => character.charCodeAt(0));
};
const fabricEncode = (buffer) => {
  const bytes = new Uint8Array(buffer);
  let raw = "";
  for (const byte of bytes) raw += String.fromCharCode(byte);
  return btoa(raw).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
};
window.fabricPasskey = async (register, optionsText) => {
  if (!window.isSecureContext || !navigator.credentials) throw new Error("Secure passkey context required");
  const options = JSON.parse(optionsText);
  const publicKey = options.publicKey;
  if (!publicKey || typeof publicKey.challenge !== "string") throw new Error("Invalid ceremony options");
  publicKey.challenge = fabricDecode(publicKey.challenge);
  publicKey.timeout = Math.min(publicKey.timeout || 60000, 60000);
  if (publicKey.user) publicKey.user.id = fabricDecode(publicKey.user.id);
  for (const name of ["allowCredentials", "excludeCredentials"]) {
    if (publicKey[name]) publicKey[name] = publicKey[name].map((credential) => ({...credential, id: fabricDecode(credential.id)}));
  }
  const credential = await navigator.credentials[register ? "create" : "get"]({publicKey});
  if (!credential) throw new Error("Ceremony canceled");
  const response = {clientDataJSON: fabricEncode(credential.response.clientDataJSON)};
  if (register) {
    response.attestationObject = fabricEncode(credential.response.attestationObject);
    if (credential.response.getTransports) response.transports = credential.response.getTransports();
  } else {
    response.authenticatorData = fabricEncode(credential.response.authenticatorData);
    response.signature = fabricEncode(credential.response.signature);
    response.userHandle = credential.response.userHandle ? fabricEncode(credential.response.userHandle) : null;
  }
  return JSON.stringify({id: credential.id, rawId: fabricEncode(credential.rawId), type: credential.type,
    response, extensions: credential.getClientExtensionResults()});
};
// No credential or telemetry payload crosses tabs, and nothing is persisted.
const fabricSessionChannel = typeof BroadcastChannel === "function" ? new BroadcastChannel("fabric-session") : null;
if (fabricSessionChannel) fabricSessionChannel.onmessage = (event) => {
  if (event.data === "logout") window.dispatchEvent(new Event("fabric-session-logout"));
};
window.fabricLogoutBroadcast = () => { if (fabricSessionChannel) fabricSessionChannel.postMessage("logout"); };
