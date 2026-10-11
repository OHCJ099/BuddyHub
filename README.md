# BuddyHub

轻量 Windows 本地 AI 管理器：WorkBuddy 自动签到、中国版与国际版独立账号池、OpenAI 兼容反代，以及本地服务管理。

常驻后台使用 Rust；管理窗口使用原生 Windows Forms。窗口只在手动打开时运行，关闭窗口后反代与签到继续工作。

## 下载与安装

到 [Releases](https://github.com/OHCJ099/BuddyHub/releases/latest) 下载 `BuddyHub-v1.0.0-windows-x64.zip`，解压到固定目录，双击 `setup.cmd`。

- 支持 Windows 10/11 x64。下载包内含 .NET 运行时，不需要安装 Node.js、Python 或 Rust。
- 安装脚本会创建桌面入口、登录后台启动项、每日备用检查任务及每分钟恢复检查。不会自动打开管理窗口。
- 安装后不要移动解压目录。需要换位置时先运行旧目录中的 `tools/uninstall.ps1`，再在新目录运行 `setup.cmd`。
- 首次运行会生成随机的本地调用密钥、管理密钥及空的加密账号库；发布包不携带任何账号。

## 添加账号

1. 在官方 WorkBuddy 客户端完成登录。
2. 打开 BuddyHub 的中国版或国际版账号页，点击「导入当前登录」。
3. 切换到另一个自己拥有的账号登录后再次导入，已有账号会保留。

使用「打开 WorkBuddy」按钮时，如果尚未记录客户端位置，会提示选择已安装的启动程序。登录或短信验证由用户在官方客户端完成。

自动签到仅处理启用的中国版账号：后台启动后、跨日后每分钟检查，成功后当天跳过；网络失败稍后重试。关机时不能执行任务，恢复运行后补签当天，无法补领前一天的奖励。具体奖励和模型权限以平台实际返回为准。

## 连接 Hermes 或其他客户端

点击「复制客户端连接配置」，使用生成的本地密钥；默认地址为：

```text
Base URL: http://127.0.0.1:8787/v1
中国版模型: deepseek-v4.1-flash-cn
国际版模型: deepseek-v4.1-flash-ai
```

同一区域内轮询账号，中国版与国际版不会交叉回退。模型目录只公布以上两个入口，不展开不同思考强度的别名。思考强度通过客户端的 `reasoning_effort` 传入。

Hermes 中添加 OpenAI 兼容服务商，填入上述地址和密钥，再添加两个模型。仓库中的 `tools/configure-hermes.py` 可用于整理已经存在的 `workbuddy` 服务商配置；运行前需要 Python 和 PyYAML，工具会备份原文件。普通使用无需此脚本。

## 数据与后台

- 数据固定存于 `%USERPROFILE%\.buddyhub`，避开 Windows MSIX 对 AppData 的重定向；高级用法可通过 `BUDDYHUB_DATA` 指定隔离目录。
- 账号库使用 AES-256-GCM 加密，密钥存于同一用户私有目录。安装时限制该目录为当前用户和 SYSTEM 可访问；这不抵御同一用户身份下运行的恶意软件。
- 只监听 `127.0.0.1`；反代端口默认 `8787`，管理端口默认 `8790`，分别验证不同密钥。
- 「保持 WorkBuddy 自启动关闭」会关闭官方客户端自启，并检测更新后恢复自启的情况；用户手动打开客户端不受影响。
- 需要管理其他本地反代时，可在「本地服务」添加可执行文件。新安装不会带入开发者的服务路径。
- 当前版本没有自动更新。更新前备份整个数据目录，关闭后台后替换程序，再运行 `setup.cmd`；不要用别人的数据目录覆盖自己的账号库。
- 卸载自启：在 PowerShell 运行 `powershell -NoProfile -ExecutionPolicy Bypass -File .\tools\uninstall.ps1`。账号库和解压文件保留，确认不再需要后自行删除。

## 从源码构建

需要 Windows x64、Rust stable（MSVC）、Visual Studio C++ Build Tools、.NET 8 SDK。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\tools\build.ps1
```

构建会运行 Rust 测试，输出独立运行的 Windows 包和 SHA-256 校验文件到 `artifacts/`。GitHub Actions 使用相同脚本构建。

```powershell
cargo test --manifest-path native/Cargo.toml --locked
```

源码目录：`native/` 为后台与协议适配，`desktop/` 为管理窗口，`tools/` 为安装、构建及 Hermes 桥接工具。构建产物、账号库、密钥及日志均不提交到 Git。

## 说明与许可证

这是独立的社区项目，与 WorkBuddy、腾讯、DeepSeek 或 Hermes 官方无隶属关系。仅管理用户合法拥有和授权的账号，不提供批量注册、验证码绕过或付费额度绕过。接口由上游控制，可能随客户端更新发生变化。

项目源码采用 [MIT](LICENSE) 许可证。第三方依赖保留其各自许可证，见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
