# BuddyHub

轻量 Windows 本地 AI 管理器：WorkBuddy 自动签到、中国版与国际版独立账号池、OpenAI 兼容反代，以及本地服务管理。

常驻后台使用 Rust；管理窗口使用原生 Windows Forms。窗口只在手动打开时运行，关闭窗口后反代与签到继续工作。

## 下载与安装

到 [Releases](https://github.com/OHCJ099/BuddyHub/releases/latest) 下载 `BuddyHub-v1.0.1-windows-x64.zip`，解压到固定目录，双击 `setup.cmd`。

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
双区轮询模型: deepseek-v4.1-flash-auto
```

`-cn` 与 `-ai` 同一区域内轮询，不会交叉回退；`-auto` 由用户显式选择，允许中国版和国际版共同轮询。模型目录只公布以上三个入口，不展开不同思考强度的别名。思考强度通过客户端的 `reasoning_effort` 传入。

Hermes 中添加 OpenAI 兼容服务商，填入上述地址和密钥，再添加上述模型。仓库中的 `tools/configure-hermes.py` 可用于整理已经存在的 `workbuddy` 服务商配置；运行前需要 Python 和 PyYAML，工具会备份原文件。普通使用无需此脚本。

如需在 Hermes 的多轮工具调用和上下文压缩中保持稳定会话，可在已有服务商配置中添加：

```yaml
providers:
  workbuddy:
    session_affinity_header: X-BuddyHub-Session-ID
```

以上是需合并的字段，请保留原有地址、模型和密钥配置。桥接工具会自动添加此字段。其他客户端也可发送 `X-BuddyHub-Session-ID`，同一聊天保持不变，不同聊天使用不同值；没有会话头时按初始上下文回退，相同开场白的独立聊天无法靠此回退保证区分。

反代为每个账号分别维护会话与用户轮次标识，并为每次模型调用生成新的消息标识；同一区域继续按请求轮询，每次发送完整消息历史。

## v1.0.1 更新

- 新增「公网共享」：ngrok 隧道开关、独立 Key、每日 / 每月 / 累计请求数、速率、并发、有效期和模型限制；支持单独复制纯 Key、纯 URL。
- 新增 `-auto` 双区轮询，原有 `-cn` / `-ai` 隔离策略保持。
- 支持新版官方客户端的加密登录导入，保留账号和上游限流时间。
- 国际版长时间限流返回 `retryable: false` 和 `reset_at`，客户端可及时提示；上游限流不能由本地解除。旧版 Hermes 可能仍按自己的重试策略等待。
- Hermes 桥接脚本补齐视觉声明与会话头。中国版已通过真实图片识别；国际版链路因上游限流尚未实测。

### 可选公网共享

默认关闭。先注册自己的 ngrok 账号，从 [ngrok 官方下载页](https://ngrok.com/downloads/windows) 获取 Windows x64 客户端，将 `ngrok.exe` 放在 `%USERPROFILE%\.buddyhub\share\`。在「公网共享 → 隧道设置」保存自己的 Authtoken，新增朋友使用的 Key，再开启服务。分享窗口中的 URL 和独立 Key 即可；不要分享本地管理密钥或 ngrok 凭据。电脑及隧道需保持在线，额度以 ngrok 当前计划为准。详见 [公网共享说明](SHARING.md)。

Hermes 手动添加模型时，在每个模型条目填写 `supports_vision: true`，例如：

```yaml
providers:
  workbuddy:
    models:
      deepseek-v4.1-flash-cn:
        supports_vision: true
      deepseek-v4.1-flash-ai:
        supports_vision: true
      deepseek-v4.1-flash-auto:
        supports_vision: true
```

合并到已有配置，保留地址和密钥。也可使用发布包中的 `tools/configure-hermes.py`。

## 2026-10-17 稳定性修复

- 补齐国际版要求的会话、轮次及模型消息标识，修复 `11128 / conversation id missing`。
- 聊天请求体上限由 16 MiB 调整为 128 MiB，支持长图片与工具历史；分块传输使用相同限制，不删除历史内容。
- 读取请求时避免复制整份图片历史。128 MiB 是上限，不是常驻预分配，大请求处理时仍会临时增加内存。
- 分别提示请求过大（413）、JSON 语法错误（400）、传输中断和不支持的压缩请求（415）；不再统一误报「JSON 格式错误」。管理接口保留 16 MiB 上限。
- 安全保留上游整数业务码，已识别的会话错误不切号或冷却；未知上游原文不回显，避免泄露凭据或提示词。
- 发布包中的 `BUILD_INFO.json` 记录构建所用的源码提交；ZIP 旁提供 SHA-256 校验文件。

测试覆盖大图片请求的普通与分块转发、末尾内容完整性、请求大小边界及错误分类。上游接口要求仍可能变化。

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
