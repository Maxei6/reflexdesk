// extension/firefox/popup.js
document.addEventListener("DOMContentLoaded", () => {
  const secretInput = document.getElementById("secret");
  const saveBtn = document.getElementById("save");
  const statusDiv = document.getElementById("status");

  chrome.storage.local.get(["pairingSecret"], (res) => {
    if (res.pairingSecret) {
      secretInput.value = res.pairingSecret;
    }
  });

  saveBtn.addEventListener("click", () => {
    const secret = secretInput.value.trim();
    chrome.runtime.sendMessage({ type: "setPairingSecret", secret }, (res) => {
      statusDiv.textContent = "Saved. Connecting to bridge...";
      statusDiv.className = "status ok";
      setTimeout(() => window.close(), 1200);
    });
  });
});
