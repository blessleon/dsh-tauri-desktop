# DeepSeek Harness（Tauri 2 套壳）

一个极简的 DeepSeek Harness 桌面壳：用 Tauri 2 的 WebView 包装
`npx -y @deepseek-ai/dsh web` 启动的本地界面。

## 工作流程

1. 启动后优先使用应用私有的 Node.js 24。
2. 没有则自动从 nodejs.org 下载**最新的 Node.js 24.x**，解压到应用数据目录
   （`%APPDATA%/com.dsh.tauri-desktop/nodejs`），无需管理员权限、不污染系统。
3. 系统中已有的 Node.js 版本不会被使用，避免与 dsh 的依赖版本不兼容。
4. 以空闲端口运行 `npx -y @deepseek-ai/dsh web --port <port>`。
5. 从子进程输出中解析本地地址（如 `http://localhost:PORT`），
   解析不到时自动轮询端口兜底。
6. 主窗口加载该地址；关闭应用时自动结束整个进程树。

## 开发

```bash
npm install          # 安装 @tauri-apps/cli
npx tauri dev        # 开发运行（窗口 + 实时编译）
```

## 构建安装包

```bash
npm run build        # 等价于 npx tauri build
```

产物在 `src-tauri/target/release/bundle/`。

## 目录结构

```
src/                 # 前端（纯静态 HTML/CSS/JS 启动页）
src-tauri/src/
  lib.rs             # 启动流程编排（检查→下载→启动→导航→退出清理）
  node.rs            # Node.js 检测 / 下载 / 解压 / 定位
  autostart.rs       # 当前用户的开机自启动注册
  server.rs          # npx 子进程管理、URL 解析、端口轮询、进程树清理
src-tauri/tauri.conf.json   # 窗口与打包配置
```

## 说明

- 首次运行会先初始化 `@deepseek-ai/dsh` 的 web profile（自动下载依赖），
  启动页会显示当前阶段。
- 仅 release 构建会注册开机自启动（当前用户，Windows 不需要管理员权限）；
  `tauri dev` 调试运行不会写入自启动项。
- 应用数据保存在系统应用数据目录，卸载时删除
  `%APPDATA%/com.dsh.tauri-desktop` 即可。
