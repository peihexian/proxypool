# ProxyPool

代理池管理服务：从订阅或本地文件导入 HTTP / SOCKS5 / SOCKS5H 节点，做健康检测与出口识别，再对外提供带账号认证的代理入口。管理端由 Rust 后台直接托管，不需要 nginx。

默认管理地址 `http://<主机>:9090`，初始密码 `admin`，登录后请立刻改掉。

## 功能

- **节点池**：远程订阅或本地导入，支持多种文本 / JSON 格式
- **检测策略**：定时探测、超时、失败退避、临时或永久隔离
- **GeoIP**：Country / ASN 库，用于国家过滤和家庭宽带识别
- **服务节点**：对外监听 HTTP、SOCKS5、SOCKS5H，可按国家、ASN、住宅属性过滤
- **批量账号**：一次生成 user1…user20 这类账号，共用一个密码；每个账号固定一个检测通过的出口 IP，IP 充足时互不重复，不足时自动复用并在池更新后重新分配
- **选择策略**：轮询、低延迟、空闲优先、粘性会话（TTL 到期后换节点）
- **使用日志**：最近 100 次已完成请求（客户端 IP、代理出口 IP、上下行流量）
- **流量看板**：近 8 小时消耗、近 14 天每日用量（图表 + 明细）与客户端 Top 5，业务与探测流量分开可查

流量在连接结束后记账。健康检查与订阅探测虽然不面向客户端，但同样要经过上游代理、同样被上游计费，所以也会入账（库里 `kind='probe'`），看板总量因此和上游后台的口径一致；使用日志和客户端 Top 5 只统计业务流量。已经建立的隧道不会中途换 IP；粘性 TTL 只作用于下一次新的 CONNECT。

## 目录

```
backend/     Rust 服务（API、代理转发、检测）
frontend/    Vue 3 管理端
deploy/      systemd 与安装脚本
```

后台启动后会按工作目录查找 `frontend/dist/index.html`，找到就把页面和 `/api` 挂在同一端口。

## 环境要求

| 方式 | 需要 |
|---|---|
| 开发 | Rust 1.80+、Node.js 20+、npm |
| 源码一键部署 | 以上，外加 Linux systemd、root |
| Release 压缩包 | 对应架构的 Linux systemd，或 Windows 直接运行 |

## 本地开发

终端 1：

```bash
cd backend
cargo run
```

管理 API 默认 `http://127.0.0.1:9090`。数据目录为当前工作目录下的 `data/`（可用 `MYPROXY_DATA` 覆盖）。

终端 2：

```bash
cd frontend
npm install
npm run dev
```

Vite 在 `5173`，并把 `/api` 代理到 `9090`。浏览器打开 `http://127.0.0.1:5173`。

## 从源码部署（Linux systemd）

在仓库里：

```bash
sudo bash deploy/install.sh
```

脚本会：

1. 停止已有 `proxypool` 服务
2. 清空旧的前端 `dist`
3. `cargo build --release` 编译 `backend`
4. `npm install` 与 `npm run build` 打包 `frontend`
5. 安装到 `/opt/proxypool`（二进制 + `frontend/dist` + `data/`）
6. 注册并启用 `proxypool.service`

```bash
systemctl status proxypool
journalctl -u proxypool -f
```

## 用 GitHub Release 包手动部署

给版本打 tag 并推送后，Actions 会编译前后端，把压缩包挂到该 tag 的 Release 上。

```bash
git tag v0.1.0
git push origin v0.1.0
```

下载与机器架构匹配的文件：

| 文件 | 用途 |
|---|---|
| `proxypool-<tag>-linux-amd64.tar.gz` | x86_64 Linux |
| `proxypool-<tag>-linux-arm64.tar.gz` | aarch64 Linux |
| `proxypool-<tag>-windows-amd64.zip` | 64 位 Windows |

### Linux

```bash
tar -xzf proxypool-v0.1.0-linux-amd64.tar.gz
cd proxypool-v0.1.0-linux-amd64
sudo bash install.sh
```

预编译包的 `install.sh` 只拷贝文件并注册 systemd，不再本地编译。数据仍在 `/opt/proxypool/data`，重复安装不会清空数据库。

也可以不解压到 `/opt`、不装服务，在解压目录里直接运行（需能读到 `frontend/dist`）：

```bash
export PORT=9090
export MYPROXY_DATA="$PWD/data"
./myproxy
```

### Windows

解压后双击 `run.bat`，或在该目录执行 `myproxy.exe`。浏览器打开 `http://127.0.0.1:9090`。

## 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `PORT` | `9090` | 管理端 HTTP 端口 |
| `MYPROXY_DATA` | `<工作目录>/data` | SQLite 与 GeoIP 目录 |
| `RUST_LOG` | `info` | 日志级别 |
| `TZ` | 系统时区 | 看板按本地时区切分小时 / 日期，例如 `TZ=Asia/Shanghai` |

systemd 单元里已设置 `PORT=9090`、`MYPROXY_DATA=/opt/proxypool/data`。

流量日志只保留最近 15 天，所以首页看板最多展示 14 天的每日用量。

## 使用要点

1. **系统设置**：修改管理员密码、填写对外展示的 IP/域名（生成 curl / 代理链接用），并更新 GeoIP 库。
2. **检测策略**：设置探测 URL、间隔和超时。这只影响节点探活，不切断已经转发的业务连接（例如大模型长时间思考）。探测失败会先分类再决定怎么处理：

   - **上游不可达**（连不上、超时、隧道被断开、上游连不到目标）才算节点的错，累计到 `max_fails` 才隔离；重试间隔按失败次数递增并带 ±25% 抖动，不再固定 30 秒三连击。
   - **407 / 403 / 429、目标返回非 2xx、响应体不是 IP** 属于账号、白名单或检测目标的问题，只写进「最近错误」，不计失败也不会隔离节点。
   - **一轮里失败超过一半（或全部失败）** 判定为系统性问题，本轮一个都不隔离。上游抖动、目标挂掉、账号被限流都不会再演变成「整池关十分钟」。
   - **主检测目标本机不可达**时自动切到备用目标（icanhazip.com / ifconfig.me），日志里会有提示。
   - 隔离到期恢复时失败计数会清零；**手动隔离**的节点不会被自动巡检改回来，要手动点「恢复」。

   节点列表新增「最近错误」列，可以直接看到某个 IP 为什么被隔离。
3. **节点池**：远程订阅会按更新间隔拉取；本地导入可追加或覆盖。
4. **服务节点**：选监听端口、协议、引用哪些池、过滤条件和选择策略。客户端走 SOCKS5H 时，域名由上游代理解析。
5. **批量账号**：在服务节点行点「账号」，按前缀、起始序号和数量批量生成账号（例如 `user1`…`user20`），统一设置密码。分配只取状态为「可用」的节点，并按出口 IP 去重：IP 足够时每个账号独占一个 IP；不够时自动复用并尽量均摊。订阅同步、健康检查或手动隔离/恢复节点后，后台会自动重新分配失效或不再唯一的绑定。
6. **粘性会话**：同一客户端在 TTL 内钉住同一个节点；到期后（新的 CONNECT）会换到其它可用节点。一条已建立的隧道不会在中途切换。
7. **使用日志**：看最近 100 条完成请求。长连接要等结束后才会出现。

客户端必须使用服务节点上配置的用户名和密码，或使用批量账号页里生成的账号（密码统一设置）。HTTP 为 Basic 代理认证，SOCKS5 / SOCKS5H 为用户名密码认证。服务节点自身的账号仍然可用，走原有的选择策略；批量账号则固定走分配给它的出口 IP。

上游代理 IP 是 IPv4、目标是 IPv6 时，由上游代理去连目标。本机不会用自己的 IPv6 直连把请求发出去；上游连不上就返回失败。

## 发布流水线

配置在 `.github/workflows/release.yml`。推送 `v*` tag 后自动跑四个阶段：

1. **preflight**：校验 tag 与 `backend/Cargo.toml` 里的版本号是否一致（不一致只告警，不阻断）
2. **frontend**：`npm ci` + `npm run build`，产物作为 artifact 传给后面的编译任务
3. **backend**：矩阵编译 Ubuntu x64、Ubuntu ARM64、Windows x64 三个平台（`cargo build --release --locked`，Windows 静态链接 CRT，下载后无需装 VC++ 运行库）
4. **release**：解包后先做冒烟测试——真的启动二进制、调一次登录接口、拉一次首页，通过才打 `tar.gz` / `zip` 并生成 `SHA256SUMS.txt`，最后发布到 GitHub Release

发布到 Release 的文件：

| 平台 | 文件 |
|---|---|
| Linux x86_64 | `proxypool-<tag>-linux-amd64.tar.gz` |
| Linux arm64 | `proxypool-<tag>-linux-arm64.tar.gz` |
| Windows x64 | `proxypool-<tag>-windows-amd64.zip` |

发版步骤：

```bash
# 版本号与 tag 保持一致，例如 backend/Cargo.toml 里是 0.1.0
git tag v0.1.0
git push origin v0.1.0
```

tag 名里带 `-` 的（如 `v0.2.0-beta.1`）会自动标记为预发布，不会顶掉 latest。

也可在 Actions 页面手动跑 `Release` 工作流：不打 tag 时只保留构建产物（workflow artifact），不创建 Release。

## 许可证

[MIT](LICENSE)
