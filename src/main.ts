import "./style.css";

declare global {
  interface Window {
    __TAURI__?: {
      core: {
        invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
      };
    };
  }
}
interface Endpoint {
  display: string;
  label: string;
}
interface Pair {
  id: string;
  local: Endpoint;
  remote: Endpoint;
  automatic: boolean;
  ignore_patterns: string[];
}
interface Device {
  serial: string;
  name: string;
  state: string;
}
interface Folder {
  path: string;
  name: string;
}
interface RemoteSelection {
  serial: string;
  path: string;
  label: string;
  display: string;
}
interface Item {
  size: number;
  modified: number;
  hash: string;
}
interface Conflict {
  path: string;
  local: Item;
  usb: Item;
}
interface Preview {
  token: string;
  conflicts: Conflict[];
  merge_recovery: boolean;
}
interface Status {
  phase: string;
  detail: string;
  busy: boolean;
  pair: Pair | null;
  last_success: string | null;
  preview: Preview | null;
  failures: { path: string; reason: string }[];
  progress: { completed: number; total: number } | null;
  elapsed_ms: number;
}
interface Entry {
  seq: number;
  time: string;
  level: string;
  message: string;
}
const invoke = <T>(cmd: string, args?: Record<string, unknown>) =>
  window.__TAURI__
    ? window.__TAURI__.core.invoke<T>(cmd, args)
    : Promise.reject(new Error("请通过 MSync 桌面程序打开此界面。"));
const app = document.querySelector<HTMLDivElement>("#app")!;
const icon = (paths: string) =>
  `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${paths}</svg>`;
const icons = {
  sync: icon('<path d="M4 7h15l-4-4M20 17H5l4 4M19 7l-4 4M5 17l4-4"/>'),
  computer: icon('<rect x="3" y="4" width="18" height="13" rx="2"/><path d="M8 21h8m-4-4v4"/>'),
  tablet: icon('<rect x="5" y="2" width="14" height="20" rx="2"/><path d="M11 18h2"/>'),
  folder: icon('<path d="M3 7V5a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/>'),
  more: icon('<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>'),
  back: icon('<path d="m14 6-6 6 6 6"/>'),
  next: icon('<path d="m10 6 6 6-6 6"/>'),
  refresh: icon('<path d="M20 7v5h-5M4 17v-5h5M6 6a8 8 0 0 1 14 6M4 12a8 8 0 0 0 14 6"/>'),
  close: icon('<path d="m6 6 12 12M6 18 18 6"/>'),
};
app.innerHTML = `
  <main>
    <div id="editing-bar" hidden><span>更换文件夹</span><button id="cancel-change" class="text-button">取消</button></div>
    <section class="folders" aria-label="同步文件夹">
      <div class="folder"><span class="folder-symbol">${icons.computer}</span><div class="folder-text"><h2>电脑</h2><p id="local-path">未选择文件夹</p></div><button id="local-pick" aria-label="选择电脑文件夹">选择</button></div>
      <div class="connection" aria-hidden="true"><span>${icons.sync}</span></div>
      <div class="folder"><span class="folder-symbol">${icons.tablet}</span><div class="folder-text"><h2>平板</h2><p id="usb-path">未选择文件夹</p></div><button id="usb-pick" aria-label="选择平板文件夹">选择</button></div>
    </section>
    <div class="sync-row">
      <section id="status" class="status" aria-live="polite" hidden><span id="status-dot" class="dot"></span><div><strong id="phase">尚未设置</strong><p id="last-success" hidden></p></div></section>
      <button id="sync" class="primary">开始同步</button>
      <button id="stop" hidden>停止同步</button>
    </div>
    <section id="sync-progress" aria-label="本轮同步进度" hidden><div class="progress-text"><span id="progress-count"></span><span id="sync-time"></span></div><progress id="progress-bar" aria-label="已处理文件"></progress></section>
    <p id="detail" role="status" hidden></p>
    <details id="failures-panel" hidden><summary id="failures-summary"></summary><ul id="failures-list"></ul></details>
    <dialog id="confirmation" hidden aria-labelledby="confirm-title" aria-describedby="confirm-text"><h2 id="confirm-title">选择保留的版本</h2><p id="confirm-text">未保留的版本会存入备份。</p><div id="conflict-body" class="dialog-body"><div id="conflicts"></div><p id="confirm-error" role="alert"></p></div><div class="confirm-actions"><button id="dismiss">稍后处理</button><button id="confirm" class="primary">同步所选版本</button></div></dialog>
    <footer>
      <div class="footer-links"><button id="backup" class="text-button">打开备份</button><button id="logs-toggle" class="text-button" aria-controls="logs-panel" aria-expanded="false">查看日志</button></div>
      <button id="more" class="icon-button" aria-label="更多设置" title="更多设置" aria-controls="settings" aria-expanded="false">${icons.more}</button>
      <section id="settings" aria-label="设置" hidden>
        <label class="automatic" title="每分钟检查一次；关闭窗口后在托盘中继续运行。">自动同步<input type="checkbox" id="automatic" role="switch"></label>
        <button id="ignore-settings">忽略文件</button>
        <button id="change">更换文件夹</button>
      </section>
    </footer>
    <dialog id="logs-panel" aria-labelledby="logs-title" hidden><div class="log-toolbar"><h2 id="logs-title">运行日志</h2><div><button id="logs-folder" class="text-button">打开目录</button><button id="copy-log">复制日志</button><button id="logs-close" class="icon-button" aria-label="关闭日志" title="关闭日志">${icons.close}</button></div></div><pre id="log-output" tabindex="0" aria-label="运行日志"></pre><p id="log-error" role="alert"></p></dialog>
    <p id="notice" role="alert"></p>
  </main>
  <dialog id="ignore-dialog" aria-labelledby="ignore-title" hidden><h2 id="ignore-title">忽略文件</h2><label for="ignore-patterns">正则表达式</label><p id="ignore-hint">每行一条，匹配所有子文件夹中的文件名。</p><textarea id="ignore-patterns" rows="4" spellcheck="false" aria-describedby="ignore-hint ignore-note"></textarea><p id="ignore-note">默认 <code>^~\\$</code> 匹配 ~$ 开头。已有文件保留，留空则不忽略。</p><p id="ignore-error" role="alert"></p><div class="confirm-actions"><button id="ignore-cancel">取消</button><button id="ignore-save" class="primary">保存</button></div></dialog>
  <dialog id="device-dialog" aria-labelledby="device-title">
    <h2 id="device-title">选择平板文件夹</h2>
    <div class="device-toolbar"><button id="device-back" class="icon-button" aria-label="上一级" title="上一级">${icons.back}</button><p id="device-location"></p><button id="device-refresh" class="icon-button" aria-label="刷新" title="刷新">${icons.refresh}</button></div>
    <div id="device-browser" class="dialog-body"><div id="device-folders"></div><p id="device-warning" role="status"></p><details class="connection-help"><summary>首次连接</summary><p>开启平板的 USB 调试，解锁并允许这台电脑。选择 FolderSync 使用的工作文件夹。</p></details></div>
    <div class="confirm-actions"><button id="device-close">取消</button><button id="device-select" class="primary" disabled>选择此文件夹</button></div>
  </dialog>`;
const el = <T extends HTMLElement = HTMLElement>(id: string) =>
  document.getElementById(id) as T;
const button = (id: string, fn: () => Promise<void> | void) =>
  el(id).addEventListener("click", () => {
    Promise.resolve().then(fn).catch(error);
  });
function error(e: unknown) {
  el(
    el<HTMLDialogElement>("confirmation").open ? "confirm-error"
      : el<HTMLDialogElement>("logs-panel").open ? "log-error"
      : el<HTMLDialogElement>("ignore-dialog").open ? "ignore-error" : "notice",
  ).textContent = e instanceof Error ? e.message : String(e);
}
let selected: [string | null, string | null] = [null, null];
let remote: RemoteSelection | null = null;
let status: Status | null = null;
let editing = false;
let submitting = false;
let shownToken = "";
let shownConflicts = "";
const conflictKey = (p: Preview) => JSON.stringify(p.conflicts.map(c =>
  [c.path, ...[c.local, c.usb].map(item => [item.hash, item.modified, item.size])],
));
function render(s: Status) {
  status = s;
  const quiet = ["尚未设置", "已就绪", "已完成"].includes(s.phase);
  el("phase").textContent = s.phase;
  el("detail").textContent = s.detail;
  // 正常状态由阶段和时间表达；仅在需要处理时展开原因。
  el("detail").hidden = s.busy || quiet || editing || !s.detail;
  el("status").hidden = (!s.pair && !s.busy) || editing;
  el("editing-bar").hidden = !editing;
  el("status-dot").className =
    `dot ${s.busy ? "busy" : !quiet || s.preview || s.failures.length ? "warning" : ""}`;
  for (const [i, key] of ["local", "usb"].entries()) {
    el(`${key}-path`).textContent =
      selected[i] ??
      (s.pair && !editing ? [s.pair.local, s.pair.remote][i].display : null) ??
      "未选择文件夹";
    el(`${key}-path`).title = el(`${key}-path`).textContent ?? "";
    el<HTMLButtonElement>(`${key}-pick`).disabled =
      s.busy || (!!s.pair && !editing);
    el(`${key}-pick`).hidden = !!s.pair && !editing;
  }
  el<HTMLButtonElement>("sync").disabled =
    s.busy || submitting || ((!s.pair || editing) && !selected.every(Boolean));
  el("sync").hidden = s.busy;
  el("sync").textContent = submitting
    ? "正在开始…"
    : s.pair && !editing
      ? s.preview
        ? "处理冲突"
        : "立即同步"
      : "开始同步";
  el<HTMLButtonElement>("confirm").disabled = s.busy;
  el("stop").hidden = !s.busy;
  el<HTMLButtonElement>("backup").disabled = !s.pair || s.busy;
  el<HTMLButtonElement>("change").disabled = !s.pair || s.busy;
  el<HTMLButtonElement>("ignore-settings").disabled = !s.pair || s.busy;
  el<HTMLButtonElement>("ignore-save").disabled = s.busy;
  el<HTMLInputElement>("automatic").checked = s.pair?.automatic ?? true;
  el<HTMLInputElement>("automatic").disabled = !s.pair || s.busy;
  el("last-success").textContent = s.last_success
    ? `上次成功 ${new Date(s.last_success).toLocaleString("zh-CN")}`
    : "";
  el("last-success").hidden = !s.last_success;
  el("sync-progress").hidden = editing || (!s.busy && !s.elapsed_ms);
  const seconds = Math.floor(s.elapsed_ms / 1000);
  const duration = seconds < 60 ? `${seconds} 秒`
    : seconds < 3600 ? `${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`
    : `${Math.floor(seconds / 3600)} 小时 ${Math.floor(seconds % 3600 / 60)} 分`;
  el("sync-time").textContent = `${s.busy ? "已用时" : "本轮用时"} ${duration}`;
  const progress = el<HTMLProgressElement>("progress-bar");
  if (s.progress && (s.progress.total || !s.busy)) {
    progress.max = s.progress.total || 1;
    progress.value = s.progress.total ? s.progress.completed : 1;
    const percent = Math.floor(progress.value / progress.max * 100);
    el("progress-count").textContent = `已处理 ${s.progress.completed}/${s.progress.total} · ${percent}%`;
  } else {
    progress.removeAttribute("value");
    el("progress-count").textContent = s.busy ? "正在检查文件" : s.phase;
  }
  el("failures-panel").hidden = !s.failures.length;
  el("failures-summary").textContent =
    `${s.failures.length} 项未完成`;
  el("failures-list").replaceChildren(
    ...s.failures.map((f) => {
      const row = document.createElement("li");
      row.textContent = `${f.path}：${f.reason}`;
      return row;
    }),
  );
  if (s.preview) {
    shownToken = s.preview.token;
    if (conflictKey(s.preview) !== shownConflicts) showPreview(s.preview);
  }
  if (!s.preview && shownToken) {
    shownToken = "";
    shownConflicts = "";
    closeConfirmation();
  }
}
function closeConfirmation() {
  el<HTMLDialogElement>("confirmation").close();
  el("confirmation").hidden = true;
}
function showPreview(p: Preview) {
  el("confirmation").hidden = false;
  el("confirm-error").textContent = "";
  const key = conflictKey(p);
  // 后台只更新令牌时，保留用户的选择和弹窗关闭状态。
  if (key !== shownConflicts) {
    shownConflicts = key;
    el("conflicts").replaceChildren(...p.conflicts.map((c, index) => {
      const row = document.createElement("fieldset");
      row.className = "conflict";
      row.dataset.path = c.path;
      const name = document.createElement("legend");
      name.textContent = c.path;
      row.append(name);
      const versions = document.createElement("div");
      versions.className = "versions";
      [c.local, c.usb].forEach((item, side) => {
        const source = side ? "平板" : "电脑";
        const version = document.createElement("div");
        version.className = "version";
        const label = document.createElement("label");
        label.innerHTML = '<span class="version-heading"><input type="radio"><strong></strong></span><span class="version-time"></span><span class="version-size"></span>';
        const radio = label.querySelector("input")!;
        radio.name = `conflict-${index}`;
        radio.value = String(side);
        radio.setAttribute("aria-label", `保留${source}版`);
        label.querySelector("strong")!.textContent = source;
        label.querySelector(".version-time")!.textContent = item.modified
          ? new Date(item.modified).toLocaleString("zh-CN") : "时间未知";
        const size = label.querySelector<HTMLElement>(".version-size")!;
        size.textContent = formatSize(item.size);
        size.title = `${item.size.toLocaleString("zh-CN")} 字节`;
        const open = document.createElement("button");
        open.className = "open-version";
        open.textContent = "打开";
        open.title = `打开${source}版副本`;
        open.setAttribute("aria-label", `打开${source}版 ${c.path}`);
        open.onclick = async () => {
          open.disabled = true;
          open.textContent = "打开中…";
          el("confirm-error").textContent = "";
          try {
            await invoke("open_conflict", { token: shownToken, path: c.path, side });
          } catch (e) {
            error(e);
          } finally {
            open.disabled = false;
            open.textContent = "打开";
          }
        };
        version.append(label, open);
        versions.append(version);
      });
      row.append(versions);
      return row;
    }));
  }
  if (!el<HTMLDialogElement>("confirmation").open)
    el<HTMLDialogElement>("confirmation").showModal();
}
function formatSize(bytes: number) {
  if (bytes < 1024) return `${bytes} 字节`;
  const unit = Math.min(Math.floor(Math.log2(bytes) / 10), 3);
  return `${(bytes / 1024 ** unit).toLocaleString("zh-CN", { maximumFractionDigits: 1 })} ${["字节", "KB", "MB", "GB"][unit]}`;
}
button("local-pick", async () => {
  const path = await invoke<string | null>("choose_folder");
  if (path) selected[0] = path;
  if (status) render(status);
});
let browsingDevice: Device | null = null;
let crumbs: Folder[] = [];
async function browseDevice() {
  el("device-folders").textContent = "正在读取…";
  el("device-warning").textContent = "";
  el<HTMLButtonElement>("device-select").disabled = true;
  el<HTMLButtonElement>("device-back").disabled = !browsingDevice;
  el("device-location").textContent =
    [browsingDevice?.name, ...crumbs.map((c) => c.name)]
      .filter(Boolean)
      .join(" / ") || "已连接设备";
  try {
    if (!browsingDevice) {
      const devices = await invoke<Device[]>("list_devices");
      el("device-folders").replaceChildren();
      if (!devices.length)
        el("device-folders").textContent =
          "未发现 ADB 设备。请连接平板、开启 USB 调试后刷新；仍未发现时检查电脑上的 ADB 驱动。";
      for (const device of devices) {
        const b = document.createElement("button");
        const state =
          device.state === "device"
            ? "已授权"
            : device.state === "unauthorized"
              ? "请在平板上允许 USB 调试"
              : "设备离线";
        b.innerHTML = `${icons.tablet}<span></span>${icons.next}`;
        b.querySelector("span")!.textContent = device.state === "device"
          ? device.name
          : `${device.name} · ${state}`;
        b.title = device.serial;
        b.setAttribute("aria-label", `${device.name} ${device.serial} ${state}`);
        if (devices.length > 1) b.querySelector("span")!.append(` · ${device.serial}`);
        b.onclick = () => {
          browsingDevice = device;
          void browseDevice();
        };
        el("device-folders").append(b);
      }
    } else {
      const result = await invoke<{ folders: Folder[] }>("list_folders", {
        serial: browsingDevice.serial,
        parent: crumbs.at(-1)?.path ?? "/storage/emulated/0",
      });
      el("device-folders").replaceChildren();
      if (!result.folders.length)
        el("device-folders").textContent = "此处没有子文件夹";
      for (const folder of result.folders) {
        const b = document.createElement("button");
        b.innerHTML = `${icons.folder}<span></span>${icons.next}`;
        b.querySelector("span")!.textContent = folder.name;
        b.onclick = () => {
          crumbs.push(folder);
          void browseDevice();
        };
        el("device-folders").append(b);
      }
      el<HTMLButtonElement>("device-select").disabled = crumbs.length < 1;
    }
  } catch (e) {
    el("device-folders").textContent = "读取失败";
    el("device-warning").textContent = String(e);
  }
}
button("usb-pick", async () => {
  browsingDevice = null;
  crumbs = [];
  el<HTMLDialogElement>("device-dialog").showModal();
  await browseDevice();
});
button("device-close", () => {
  el<HTMLDialogElement>("device-dialog").close();
});
button("device-refresh", async () => {
  await browseDevice();
});
button("device-back", async () => {
  if (crumbs.length) crumbs.pop();
  else browsingDevice = null;
  await browseDevice();
});
button("device-select", () => {
  if (!browsingDevice || crumbs.length < 1) return;
  remote = {
    serial: browsingDevice.serial,
    path: crumbs.at(-1)!.path,
    label: browsingDevice.name,
    display: [browsingDevice.name, ...crumbs.map((c) => c.name)].join(" / "),
  };
  selected[1] = remote.display;
  el<HTMLDialogElement>("device-dialog").close();
  if (status) render(status);
});
button("sync", async () => {
  if (!status || submitting) return;
  if (status.preview && !editing) {
    showPreview(status.preview);
    return;
  }
  el("notice").textContent = "";
  submitting = true;
  render(status);
  try {
    if (!status.pair || editing) {
      if (!remote) throw new Error("请先选择平板文件夹。");
      await invoke("bind", { local: selected[0], ...remote });
      selected = [null, null];
      editing = false;
    }
    await invoke("start_sync");
  } finally {
    submitting = false;
    await refresh();
  }
});
button("confirm", async () => {
  const choices: Record<string, number> = {};
  for (const row of el("conflicts").querySelectorAll<HTMLFieldSetElement>("fieldset")) {
    const selectedVersion = row.querySelector<HTMLInputElement>("input:checked");
    if (!selectedVersion)
      throw new Error("请为每个冲突文件选择保留的版本。");
    choices[row.dataset.path!] = Number(selectedVersion.value);
  }
  await invoke("confirm_sync", { token: shownToken, choices });
  closeConfirmation();
  await refresh();
});
button("dismiss", closeConfirmation);
el("confirmation").addEventListener("cancel", (event) => {
  event.preventDefault();
  el<HTMLButtonElement>("dismiss").click();
});
button("stop", async () => {
  await invoke("stop_sync");
});
button("more", () => {
  setSettingsOpen(el("settings").hidden);
});
function setSettingsOpen(open: boolean) {
  el("settings").hidden = !open;
  el("more").setAttribute("aria-expanded", String(open));
}
document.addEventListener("click", (event) => {
  if (!(event.target as Element).closest("#settings, #more")) setSettingsOpen(false);
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !el("settings").hidden) {
    setSettingsOpen(false);
    el("more").focus();
  }
});
button("change", () => {
  editing = true;
  selected = [null, null];
  remote = null;
  if (status) render(status);
  setSettingsOpen(false);
  el("local-pick").focus();
});
button("cancel-change", () => {
  editing = false;
  selected = [null, null];
  remote = null;
  if (status) render(status);
  el("more").focus();
});
button("ignore-settings", () => {
  el<HTMLTextAreaElement>("ignore-patterns").value = status?.pair?.ignore_patterns.join("\n") ?? "";
  el("ignore-error").textContent = "";
  setSettingsOpen(false);
  const dialog = el<HTMLDialogElement>("ignore-dialog");
  dialog.hidden = false;
  dialog.showModal();
});
button("ignore-cancel", () => el<HTMLDialogElement>("ignore-dialog").close());
el("ignore-dialog").addEventListener("close", () => {
  el("ignore-dialog").hidden = true;
  el("more").focus();
});
button("ignore-save", async () => {
  el("ignore-error").textContent = "";
  el<HTMLButtonElement>("ignore-save").disabled = true;
  try {
    await invoke("set_ignore_patterns", { patterns: el<HTMLTextAreaElement>("ignore-patterns").value.split(/\r?\n/) });
    await refresh();
    el<HTMLDialogElement>("ignore-dialog").close();
    el("notice").textContent = "忽略设置已保存，下次同步生效。";
  } finally {
    el<HTMLButtonElement>("ignore-save").disabled = status?.busy ?? false;
  }
});
el("automatic").addEventListener("change", () => {
  invoke("set_automatic", {
    enabled: el<HTMLInputElement>("automatic").checked,
  }).catch(error);
});
button("backup", async () => {
  await invoke("open_backups");
});
button("logs-folder", async () => {
  await invoke("open_logs");
});
button("logs-toggle", async () => {
  const panel = el<HTMLDialogElement>("logs-panel");
  panel.hidden = false;
  el("log-error").textContent = "";
  el("copy-log").textContent = "复制日志";
  panel.showModal();
  el("logs-toggle").setAttribute("aria-expanded", "true");
  await refreshLogs();
});
button("logs-close", () => el<HTMLDialogElement>("logs-panel").close());
el("logs-panel").addEventListener("close", () => {
  el("logs-panel").hidden = true;
  el("logs-toggle").setAttribute("aria-expanded", "false");
});
button("copy-log", async () => {
  await invoke("copy_logs");
  el("copy-log").textContent = "已复制";
});
let logSeq = 0;
async function refreshLogs() {
  if (el("logs-panel").hidden) return;
  const logs = await invoke<Entry[]>("get_logs");
  const last = logs.at(-1)?.seq ?? 0;
  if (last === logSeq) return;
  logSeq = last;
  const output = el("log-output");
  const atBottom =
    output.scrollTop + output.clientHeight >= output.scrollHeight - 32;
  output.textContent = logs
    .map((l) => `${l.time} [${l.level}] ${l.message}`)
    .join("\n");
  if (atBottom) output.scrollTop = output.scrollHeight;
}
async function refresh() {
  render(await invoke<Status>("get_status"));
  await refreshLogs();
}
async function poll() {
  try {
    await refresh();
  } catch (e) {
    error(e);
  }
  setTimeout(poll, 1000);
}
void poll();
