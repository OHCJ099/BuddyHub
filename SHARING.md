# 免费公网共享（ngrok）

BuddyHub v1.0.1 的左侧「公网共享」页面可以直接开启 / 关闭公网服务、设置登录后自动开启、保存 ngrok 凭据和请求体大小上限。关闭会停止并禁用共享任务，退出网关和隧道；主反代与签到继续运行。

下方可以新增 / 编辑多个 Key，自动生成或自定义密钥，限制每日、每月和累计请求次数、每分钟速率、并发数、有效期及可用模型。选中 Key 后，「复制 Key」和「复制 URL」分别只复制原文；「复制完整配置」生成可交给朋友的完整说明。次数和速率为 0 表示不限，并发为 1–16。重置 Key 会撤销旧凭据，但保留该条目的用量。

Key 的可复制原文存储在 `sharing-secrets.enc`，使用现有本机保险库密钥加密；常规状态接口只返回掩码。旧的 `friend-1` Key 和当日用量会自动迁移。早期导出的 `发给朋友.txt` 是静态文件；更改 Key 后请从窗口重新复制配置。

公网入口使用独立的 `buddyhub-share.exe`，绑定 `127.0.0.1:8791`，经 ngrok 转发至公网 HTTPS。现有 `8787` 调用接口和 `8790` 管理接口不作修改。

只允许 `GET /v1/models` 和 `POST /v1/chat/completions`；两者都需要独立分享密钥。只公布三个 4.1 模型：`deepseek-v4.1-flash-cn`、`deepseek-v4.1-flash-ai`、`deepseek-v4.1-flash-auto`。客户端的 `reasoning_effort` 原样转发。

## 本机文件

- `%USERPROFILE%\.buddyhub\sharing.json`：共享开关、密钥 SHA-256、每分钟请求数、每日请求数、并发限制。修改即时生效，无须重启。
- `%USERPROFILE%\.buddyhub\sharing-usage.json`：每日、每月和累计计数；日 / 月按北京时间重置，重启保留。为防止通过畸形请求绕过限额，认证成功的生成请求即计数，失败也计数；模型查询不计数。这是请求数限制，并非积分或 token 限制。
- `%USERPROFILE%\.buddyhub\share\friend-1.json`、`发给朋友.txt`：独立调用密钥和公网 URL，可以给对应朋友。不要提交至仓库。
- `%USERPROFILE%\.buddyhub\share\ngrok.yml`：ngrok 登录凭据，只留本机。请求检查使用 `--inspect=false` 禁用。

ngrok 免费账号当前包括一个分配域名、每月 20,000 次 HTTP 请求、1 GB 出站流量。隧道和电脑必须在线。免费计划可能调整，以 ngrok 控制台和官方文档为准。

## 启动与暂停

运行 `tools/install-sharing-tasks.ps1` 注册两个登录触发的计划任务：`BuddyHub Sharing Gateway` 和 `BuddyHub Sharing Tunnel`，支持错过后补启动、失败一分钟后重试。原生启动器隐藏启动 ngrok，没有常驻 PowerShell/VBS 或窗口。需要 ngrok 配置已就绪。

推荐在窗口点击「关闭公网服务」以释放共享后台内存。只修改 `sharing.json` 的顶层 `enabled=false` 会阻止新请求，但不会主动结束后台进程。撤销某位朋友：在窗口停用对应 Key，对之后的请求立即生效；已有流式请求会继续完成。现有 BuddyHub 反代任务不受影响。

## 验证

`cargo test --bin buddyhub-share` 检查路由隔离、限额和落盘。`tools/test-sharing.py` 使用临时配置及假上游，检查认证、管理接口隔离、SSE 首块及时输出、思考参数转发、并发限制、每日限额、撤销和总开关，不消耗真实账号额度。测试使用临时目录与动态端口，不改变现有服务。

真实公网验收另行请求一次模型，检查 HTTP 200、流式内容和 `[DONE]`。不要将管理密钥、ngrok Authtoken 或本机自用 Key 发给朋友。
