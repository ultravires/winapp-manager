# winget 打包与提交

本目录保存提交到 [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs) 的清单（manifest），
目录结构与 winget-pkgs 仓库一致：`manifests/<发布者首字母>/<发布者>/<包名>/<版本>/`。

- 包标识符：**`ultravires.WinAppManager`**（winget ID 不允许连字符，故不是 winapp-manager）
- 安装类型：`zip` + `NestedInstallerType: portable`（zip 内单个 `app_manager.exe`）
- 别名：未设 `PortableCommandAlias`，默认取 exe 名 `app_manager`

安装命令（合入后生效）：

```powershell
winget install ultravires.WinAppManager
```

## 首次提交（v0.1.0，手动一次）

1. 用 `ultravires` 账号 Fork [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs)。
2. 在 Fork 中新建分支（如 `ultravires.WinAppManager-0.1.0`）。
3. 把本目录 `manifests/u/ultravires/WinAppManager/0.1.0/` 下的 **3 个 yaml** 原样复制到 Fork 的同路径。
4. 向 `microsoft/winget-pkgs` 的 `master` 分支发 PR，标题格式：

   ```
   Add version 0.1.0 to ultravires.WinAppManager
   ```

5. 机器人会要求签署 Microsoft CLA，在 PR 页面点链接用浏览器签一次即可（仅首次）。
6. 等待自动化校验（winget-pkgs-bot）+ 人工审核，通常 1 天内合入。

## 后续版本（自动）

Release 正式发布后，`.github/workflows/winget.yml` 会自动提交新版本清单。
前置条件（一次性）：

- 仓库 Secrets 配置 `WINGET_TOKEN`：GitHub 经典 PAT，勾选 `public_repo`；
- 该 PAT 对应账号已签 Microsoft CLA；
- 包已通过首次提交进入 winget-pkgs。

## 新版本时要同步更新的文件

1. 新建目录 `manifests/u/ultravires/WinAppManager/<新版本>/`，复制上一版本 3 个 yaml；
2. 修改其中的 `PackageVersion`、`InstallerUrl`（指向新 Release 的 zip）、`InstallerSha256`（新 zip 的 SHA-256，Release 产物 `SHA256SUMS.txt` 里有）；
3. 本地校验：`winget validate --manifest <目录>`；
4. 走上面的 PR 流程（或等自动工作流）。
