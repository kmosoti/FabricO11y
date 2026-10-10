// No credentials or telemetry enter worker messages or persistent browser state.
if ("serviceWorker" in navigator && window.isSecureContext) {
  window.addEventListener("load", () => {
    navigator.serviceWorker.register("./service-worker.js", { scope: "./" })
      .catch(() => { /* The online console remains usable without offline shell. */ });
  });
}
