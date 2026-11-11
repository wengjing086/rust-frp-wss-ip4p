# 杀毒软件误报：原因、本项目做了什么、怎么申诉

> **立场**：本项目做的是「降低误报概率 + 便于自证清白」的**合规工程措施**，
> 不是免杀、不是对抗检测。我们不做加壳、不做混淆、不做反调试、不做进程注入，
> 也不做签名伪造。任何声称「保证不报毒」的说法都不可信。

## 一、为什么 Go 版 frp 常被误杀

1. **运行时特征**：Go 二进制带独特的 runtime、异常处理、GOPATH 字符串与
   函数布局。大量恶意软件用 Go 写，很多厂商直接按「Go 特征 + 隧道行为」
   建了启发式规则，frp 恰好整段撞上。
2. **工具品类标签**：frp 属于反向代理 / 内网穿透，被攻击者滥用于横向移动，
   因此多数厂商把它归类为 `HackTool` / `RiskWare` / `PUA`，而不是木马。
   这类判定**与文件是否真的有害无关**，属于「双用途工具」策略。
3. **三无二进制**：常见分发包里的 `frpc.exe` 没有版本资源、没有产品名、
   没有清单、没有数字签名，在信誉系统里天然是高风险。
4. **加壳/压缩分发**：网上流传的「精简版」常用 UPX，进一步拉高风险分。

## 二、本项目采取的措施

| 措施 | 作用 | 代码位置 |
|---|---|---|
| Rust 原生编译 | 二进制里完全没有 Go runtime 特征，第 1 类规则直接不适用 | 整个 workspace |
| 单二进制、无运行时依赖 | 不释放临时 exe、不加载外部 DLL、不下载可执行内容 | `Cargo.toml` |
| 完整 VERSIONINFO | CompanyName / ProductName / FileDescription / FileVersion / OriginalFilename / LegalCopyright | `scripts/gen_winres.py`、`assets/winres/*.res` |
| 应用清单 `asInvoker` | 明确**不请求管理员权限**；声明 DPI 与长路径感知、supportedOS | 同上 |
| 不加壳 / 不压缩 | release profile 只是 `opt-level="z" + LTO + strip`，没有 UPX | `Cargo.toml` `[profile.release]` |
| 不写自启动 | 不碰注册表 Run 键、不装服务、不建计划任务；需要一个进程时由用户自己配置 | — |
| 不注入 / 不自修改 | 无 `WriteProcessMemory`、无 `VirtualProtectEx`、无运行时生成代码 | 全代码 |
| 不起可疑子进程 | Windows 上读系统 DNS 用 `GetNetworkParams` API，而不是 `ipconfig` 子进程 | `common/src/dns.rs` |
| 可复现构建 + 校验和 | 全部 Release 产物由公开源码在 GitHub Actions 构建，附 `SHA256SUMS.txt` | `.github/workflows/release.yml` |
| 开源可审计 | 每个二进制都能对应到具体 commit 与 tag | GitHub Releases |

### 版本资源是怎么加的

`scripts/gen_winres.py` 用纯 Python 直接写 COFF `.res` 对象，
`server/build.rs` / `client/build.rs` 在 Windows 构建时把它传给链接器。
**不需要 `rc.exe` 或 Windows SDK**，`msvc` 与 `gnu` 两种工具链都验证过。

验证方式（Windows）：

```powershell
(Get-Item .\frpc.exe).VersionInfo | Format-List *
# 应看到 CompanyName=wengjing086、FileDescription=frp client (Rust) - ...
# 以及 FileVersion=0.8.0.0
```

清单里必须**没有** `requireAdministrator`，且必须有
`<requestedExecutionLevel level="asInvoker" ... />`。

## 三、发布与使用建议

1. **只从本仓库 Releases 下载**，用 `SHA256SUMS.txt` 校验：
   ```bash
   sha256sum -c SHA256SUMS.txt
   ```
2. **不要二次加壳**（UPX / 各种「压缩器」）——会立刻把风险分拉回去。
3. **自行编译**是最强的信任链：`cargo build --release` 后比对哈希。
4. 有条件的话做 **Authenticode 代码签名**（EV 证书效果最好）。签名不是
   「免杀」，但能显著提升 Defender SmartScreen 与各厂商的信誉评分。
   本项目不内置任何证书或私钥。
5. 部署时给文件一个固定的、可解释的路径与用途说明，避免放在
   `%Temp%` / `%AppData%\Local\Temp` 等高风险目录。

## 四、遇到误报怎么申诉

附上：文件 SHA256、下载链接、源码仓库地址、用途说明、构建方式。

| 厂商 / 平台 | 入口 |
|---|---|
| Microsoft Defender | <https://www.microsoft.com/en-us/wdsi/filesubmission>（选 "Incorrectly classified as malware"） |
| VirusTotal | <https://www.virustotal.com/> 上传后可在 Community 里说明，多数厂商同步关注 |
| 火绒 | <https://www.huorong.cn/> 论坛「误报反馈」板块 / 官方反馈渠道 |
| 360 | <https://sampleup.sd.360.cn/> 误报提交 |
| 腾讯电脑管家 | 管家「误报申诉」/ 官方反馈渠道 |
| 卡巴斯基 | <https://opentip.kaspersky.com/> → False positive |

> 具体入口可能调整，以厂商官网当前页面为准。

申诉模板要点：

```
软件名称：rust-frp-wss-ip4p / frpc.exe
版本：0.8.0.0
SHA256：<填值>
源码：https://github.com/wengjing086/rust-frp-wss-ip4p
构建：GitHub Actions 从上述 commit 公开构建，Release 附 SHA256SUMS.txt
用途：开源内网穿透客户端（frp 协议的 Rust 实现），用于自有设备的远程访问
说明：不含 Go 运行时、无加壳、无自启动、无注入、无运行时下载；
     已内嵌完整版本资源与 asInvoker 清单。判定为 HackTool 属误报。
```

## 五、诚实的边界

- 双用途工具的 `HackTool` / `RiskWare` 判定是**厂商策略**，不是「误报」的技术
  证据；我们可以降低启发式命中率，但无法要求厂商改变品类策略。
- 任何新发布的、下载量小的二进制都会经历一段「信誉冷启动」期。
  持续使用同一签名 / 同一发布渠道、保持哈希与源码可对应，是唯一长期解法。
- 若有人提供「加壳免杀版」，那不是本项目产物，请勿使用。
