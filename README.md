# 应用管理器（Rust for Windows）

用 Rust 编写的 Windows 已安装应用管理工具。纯 Rust 实现（egui GUI），无 Node.js / WebView 依赖。

## 下载

**winget（推荐）**——装完即可在 PATH 中直接运行 `app_manager`：

```powershell
winget install ultravires.WinAppManager
```

**GitHub Releases**——通过 [Releases](https://github.com/ultravires/winapp-manager/releases/latest) 下载 `winapp-manager-vX.Y.Z-x64.zip`，解压后运行 `app_manager.exe`。

附件中的 `SHA256SUMS.txt` 记录了 exe 与 zip 的 SHA-256，下载后可自行核对：

```powershell
Get-FileHash .\winapp-manager-v0.1.0-x64.zip -Algorithm SHA256
```

> exe 未签名，首次运行可能被 SmartScreen / Defender 拦截（对无签名、零信誉新文件的常见误报）：点「更多信息 → 仍要运行」，或在 Defender 保护历史中允许。稳妥做法是先核对 SHA-256 再运行。

## 功能

1. **自动扫描本地应用**：与「控制面板\程序\程序和功能」同源的注册表 Uninstall 列表（HKLM / HKLM-Wow64 / HKCU），过滤系统组件、KB 更新项，同名条目合并去重
   - 启动目标三级解析：`DisplayIcon`（校验排除卸载器）→ 安装目录主程序 exe（名称匹配打分）→ 开始菜单同名快捷方式
   - 运行库 / 驱动 / SDK 等无可启动主程序的条目会显示但双击提示（与程序和功能一致）；便携安装（scoop 等）不在注册表中，可用手动添加
2. **手动添加应用**：工具栏「➕ 添加应用」选择 exe / lnk；若当前处于某分类视图，自动归入该分类
3. **应用分类**：左侧栏「➕ 新建分类」；右键分类可重命名 / 删除（删除后应用移入"未分类"）
4. **双击图标运行**：双击图标通过 `ShellExecuteW` 启动（exe 与 lnk 均支持）；单击不触发，避免误运行
5. **右键管理应用**：运行 / 打开所在位置 / 移动到分类（子菜单）/ 新建分类并移入 / 从列表移除 / 卸载…（调起官方卸载程序）
6. **已移除视图**：「从列表移除」的应用集中在左侧栏「🗑 已移除」里，右键可**恢复**；手动添加的应用在已移除视图中还可「彻底删除」
7. **字体设置**：工具栏「🔤 字体」打开设置对话框，列出系统全部已装字体（HKLM/HKCU Fonts 注册表项），点击即时生效并持久化；微软雅黑始终作为中文回退，任何选择下不会缺字
6. **其他**：搜索过滤、图标从目标文件实时提取（SHGetFileInfoW → GetDIBits），提取失败或无图标源时回退到系统默认类型图标（按扩展名，SHGFI_USEFILEATTRIBUTES）、配置 JSON 持久化（分类归属跨重启保留）、已卸载应用自动从列表移除

## 使用

直接运行 `app_manager.exe`（发布版位于 `target\release\`，亦可复制到任意可写目录）。

- 配置保存在 exe 同目录的 `app_manager_config.json`（不可写时回退到 `%APPDATA%`）
- 「从列表移除」统一进入「已移除」视图（不再直接消失）；恢复后回到原分类。自动扫描的应用若真的被卸载，会在重新扫描时自动清除
- UWP / 微软商店应用不在注册表 Uninstall 列表中（与「程序和功能」一致），暂不支持

## 构建

```powershell
cargo build --release
# 产物：target\release\app_manager.exe
```

依赖：Rust stable (msvc)，Windows 10/11。图标资源编译需要 Windows SDK 的 rc.exe（安装 VS 时勾选 SDK 即有）。

## 技术栈

| 组件 | 选型 |
|---|---|
| GUI | eframe / egui 0.27（自绘图标网格 + 右键菜单） |
| 扫描 | winreg（注册表 Uninstall，程序和功能同源）+ walkdir（快捷方式回退索引） |
| 图标 | winapi：SHGetFileInfoW → GetIconInfo → GetDIBits → RGBA 纹理 |
| 启动 | ShellExecuteW（verb=open） |
| 对话框 | rfd（IFileOpenDialog） |
| 持久化 | serde_json |
| 应用图标 | 内嵌 `assets/app.ico`：build.rs + winresource 编入 exe 资源（资源管理器/任务栏），启动时 image 解码后 `with_icon` 设为窗口图标 |

`assets/app.ico` 由 Python 脚本生成（PIL 绘制 2×2 磁贴图样，含 16–256 共 7 档 BMP 条目）。

中文字体：启动时加载 `C:\Windows\Fonts\msyh.ttc`（微软雅黑），避免 egui 默认字体缺汉字字形；可在「🔤 字体」中切换应用字体，选择持久化于配置文件的 `font` 字段。

## 发布

打 tag 推送即自动构建并发布 Release（`.github/workflows/release.yml`）：

```powershell
# 1. 确认 Cargo.toml 的 version 与 tag 一致（工作流会校验，不一致直接失败）
# 2. 提交代码后打 tag 并推送
git tag v0.1.0
git push origin v0.1.0
```

工作流流程：`cargo build --release --locked` → 打包 zip（exe + `SHA256SUMS.txt`）→ 上传 Actions artifact → 创建同名 Release 并附上产物；重跑同一 tag 会覆盖资产，不会重复建 Release。Actions 页面也可手动触发（workflow_dispatch），仅产出 artifact 供测试，不创建 Release。

Release 正式发布后还会自动向 winget 提交新版本清单（`.github/workflows/winget.yml`，需先配置 `WINGET_TOKEN` secret 并完成首次手动提交，见 `packaging/winget/README.md`）。
