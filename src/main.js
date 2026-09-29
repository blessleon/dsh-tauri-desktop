// Splash / bootstrap status view.
// Listens for `dsh-status` events emitted by the Rust side.

const statusEl = document.getElementById("status");
const detailEl = document.getElementById("detail");
const progressWrap = document.getElementById("progress-wrap");
const progressBar = document.getElementById("progress-bar");
const logEl = document.getElementById("log");

const logLines = [];
function pushLog(line) {
  logLines.push(String(line));
  while (logLines.length > 14) logLines.shift();
  logEl.hidden = false;
  logEl.textContent = logLines.join("\n");
}

const STAGE_HINTS = {
  checking: "正在检查 Node.js…",
  downloading: "正在下载 Node.js…",
  installing: "正在安装 Node.js…",
  starting: "正在启动 DeepSeek Harness 服务…",
  loading: "正在加载界面…",
  ready: "DeepSeek Harness 已就绪",
  error: "启动失败",
};

function apply(payload) {
  const { stage, message, detail, progress } = payload || {};

  const text = message || STAGE_HINTS[stage] || "正在初始化…";
  statusEl.textContent = text;
  statusEl.classList.toggle("error", stage === "error");

  detailEl.textContent = detail || "";

  if (typeof progress === "number" && progress >= 0) {
    progressWrap.classList.add("visible");
    progressBar.style.width = Math.min(100, Math.max(0, progress)) + "%";
  } else {
    progressWrap.classList.remove("visible");
  }
}

async function init() {
  const tauri = window.__TAURI__;
  if (tauri && tauri.event && typeof tauri.event.listen === "function") {
    await tauri.event.listen("dsh-status", (event) => apply(event.payload));
    await tauri.event.listen("dsh-log", (event) => pushLog(event.payload));
  }
  // If the event system is not available (e.g. opened in a plain browser),
  // just show the idle state.
  apply({ stage: "checking", message: "正在检查运行环境…" });
}

init();
