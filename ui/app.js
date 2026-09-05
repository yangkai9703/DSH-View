// 壳前端：窗口控制 + 轮询 DSH 状态 → iframe 加载。

const { invoke } = window.__TAURI__.core;

// ---------- 窗口控制（最小化/最大化/关闭） ----------

document.getElementById("btn-min").addEventListener("click", () => {
  invoke("plugin:window|minimize", { label: "main" });
});
document.getElementById("btn-max").addEventListener("click", () => {
  invoke("plugin:window|toggle_maximize", { label: "main" });
});
document.getElementById("btn-close").addEventListener("click", () => {
  invoke("plugin:window|close", { label: "main" });
});

// ---------- DSH 启动状态 ----------

const frame = document.getElementById("dsh-frame");
const placeholder = document.getElementById("placeholder");
const spinner = document.getElementById("ph-spinner");
const phTitle = document.getElementById("ph-title");
const phDetail = document.getElementById("ph-detail");
const phError = document.getElementById("ph-error");
const phRetry = document.getElementById("ph-retry");

// 轮询 Rust 侧 DSH 状态（比事件更可靠：无需时序对齐，慢一拍也能拿到最终态）。
let ready = false;

async function pollStatus() {
  if (ready) return;
  try {
    const s = await invoke("dsh_status");
    if (s.stage === "ready" && s.url) {
      ready = true;
      placeholder.hidden = true;
      frame.hidden = false;
      frame.src = s.url;
      return;
    }
    if (s.stage === "error") {
      showError(s.message || "未知错误");
      return; // 停止轮询，等用户点重试
    }
    if (s.stage === "waiting") {
      phTitle.textContent = "正在等待 DSH 服务就绪…";
    }
  } catch (e) {
    // invoke 失败（前端刚加载时可能发生），下个周期再试
  }
  setTimeout(pollStatus, 500);
}

// 重试：复位 UI 后重新开始轮询（Rust 侧 supervisor 线程只跑一次，若上次已
// 失败，轮询会立刻拿到 error——由用户重启整个应用来重拉 DSH）。
phRetry.addEventListener("click", () => {
  showPlaceholder();
  setTimeout(pollStatus, 300);
});

showPlaceholder();
pollStatus();

function showPlaceholder() {
  placeholder.hidden = false;
  frame.hidden = true;
  spinner.classList.remove("hidden");
  phTitle.textContent = "正在启动 DeepSeek Harness…";
  phDetail.textContent = "首次运行需要下载依赖，可能需要一两分钟";
  phError.hidden = true;
  phRetry.hidden = true;
}

function showError(message) {
  spinner.classList.add("hidden");
  phTitle.textContent = "启动失败";
  phDetail.textContent = "";
  phError.textContent = message;
  phError.hidden = false;
  phRetry.hidden = false;
}

showPlaceholder();
pollStatus();