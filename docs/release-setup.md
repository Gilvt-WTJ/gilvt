# 发版准备：Developer ID 与公证

对外分发 `Gilvt.app`（GitHub Release 的 dmg、Homebrew cask、以后的自动更新）需要 Apple 的 **Developer ID Application** 证书和公证。只开源源码不需要：用户本机 `cargo build` + `scripts/bundle.sh` 编出来的 app 没有 quarantine 属性，Gatekeeper 不拦。

为什么非要不可：

- 没有公证的 dmg，用户首次打开会被 Gatekeeper 拦下（macOS 15 起右键「打开」也绕不过，只能去「系统设置 → 隐私与安全性 → 仍要打开」或手动 `xattr -dr com.apple.quarantine`）。官方 `homebrew-cask` 会禁用过不了 Gatekeeper 检查的 app（Alacritty 的 cask 2026-09 因此被禁用）。
- ad-hoc 签名的隐私授权（屏幕录制、辅助功能、通知）按二进制哈希识别，用户每装一个新版本都要重新授权一次。Developer ID 签名后按「bundle id + Team ID」识别，跨版本保留。

同类产品的做法：Ghostty（开源）用作者的个人账号签名（`Developer ID Application: Mitchell Hashimoto`），Warp 用公司账号；iTerm2、WezTerm、Kitty、Wave、Tabby 都签名并公证。

本文按**个人账号 + 美国 Apple 账户**写。四个阶段：准备 Apple 账户 → 注册 Apple Developer Program → 创建证书 → 配置公证并演练。

## 先定下来的事

- **以谁的名义签名**。证书上的名字（`Developer ID Application: <名字> (TEAMID)`）和 Team ID 一旦随第一个公开版本发出去就很难换：换 Team ID 会让所有用户的隐私授权失效，接入 Sparkle 后还受它「EdDSA 密钥与 Apple 签名不能同时更换」的限制。个人账号不能自助转成组织账号，要联系 Apple 人工处理。

## 1. 准备 Apple 账户

1. 用一个长期使用的 Apple 账户。它就是开发者账号的 Account Holder，只有 Account Holder 能创建 Developer ID 证书。
2. 名和姓填**法定姓名**，与下一步验证用的证件（护照或驾照）拼写完全一致。不要用昵称、网名或公司名。
3. 开启双重认证。
4. 地址填真实的实体地址（不接受 P.O. Box）。
5. 在账户里绑定一张本人名下的信用卡 / 借记卡。**Apple 账户余额（礼品卡充值）不能用来付会员费。**

账户里的姓名和地址、证件上的姓名（驾照上的地址）、付款卡的账单地址应当一致，不一致时审核通常会卡住并要求补材料。

## 2. 注册 Apple Developer Program

二选一：

- **网页**：<https://developer.apple.com/programs/enroll/> → Start Your Enrollment。
- **iPhone 上的 Apple Developer app**（Apple 推荐）：Account → Enroll Now，用手机扫描护照或驾照完成身份验证（Apple 核对证件、提取姓名和地址，不保存图片）。

然后：

1. 实体类型选 **Individual / Sole Proprietor**，不需要 D-U-N-S 编号。
2. 同意 Apple Developer Program License Agreement。
3. 付款：99 美元 / 年，默认自动续订。
4. 等审核邮件，个人账号一般 1～2 天。

免费分发不需要填税务和银行信息（那是付费 App / 内购才要的）。

## 3. 创建 Developer ID Application 证书

在平时打包 gilvt 的那台 Mac 上做（私钥生成在这台机器的钥匙串里）。

1. **生成 CSR**：「钥匙串访问 → 证书助理 → 从证书颁发机构请求证书…」，填邮箱和名字，选「存储到磁盘」，得到 `.certSigningRequest`。
2. **创建证书**：<https://developer.apple.com/account/resources> → Certificates → `+` → Software 下选 **Developer ID** → **Developer ID Application**（签 Mac app 用，不是 Developer ID Installer）→ 上传 CSR → 下载 `.cer`，双击装进「登录」钥匙串。
3. **确认**：

   ```bash
   security find-identity -v -p codesigning
   # 应列出 "Developer ID Application: <名字> (TEAMID)"
   ```

4. **立刻备份**：在钥匙串里选中证书**连同它下面的私钥**，导出为 `.p12`，设密码，存进密码管理器。私钥丢了只能新建证书。

须知：

- 每个账号最多 5 张 Developer ID Application 证书；**不能自助吊销**，要发邮件给 `product-security@apple.com`。
- 证书有效期内签名并公证的版本，证书过期或会员过期后用户照常运行；但过期后不能再签新版本，会员过期后不能申请新证书。

## 4. 配置公证，在 gilvt 上演练

### 公证凭据

- **本机**（App 专用密码）：在 <https://account.apple.com> 的「登录与安全 → App 专用密码」生成一个，然后

  ```bash
  xcrun notarytool store-credentials gilvt-notary \
    --apple-id <Apple 账户邮箱> --team-id <TEAMID> --password <App 专用密码>
  ```

  之后 `scripts/notarize.sh` 用 `NOTARY_PROFILE=gilvt-notary` 取这份凭据。

- **CI**（API key）：App Store Connect → 用户和访问 → 集成 → 团队密钥，新建一个（角色选 Developer 即可），下载 `.p8`（只能下载一次），记下 Key ID 和 Issuer ID。对应 `scripts/notarize.sh` 的 `NOTARY_KEY_PATH` / `NOTARY_KEY_ID` / `NOTARY_ISSUER_ID`。

### 本机演练

```bash
GILVT_NOTARIZE=1 scripts/package.sh
# 等同于：GILVT_SIGN_IDENTITY="<钥匙串里第一张 Developer ID Application 证书>" GILVT_HARDENED=1 \
#   NOTARY_PROFILE=gilvt-notary GILVT_NOTARIZE=1 scripts/package.sh
spctl -a -vv target/dist/Gilvt.app      # 期望 source=Notarized Developer ID
xcrun stapler validate target/dist/Gilvt.app    # app 自带票据（先公证 app，再公证 dmg）
```

这条路径 2026-10-07 已用真实 Developer ID 走通（两轮公证都 `Accepted`，没有 issue）。硬化运行时需要的 entitlements 在 `packaging/entitlements.plist`。最后把 dmg 拷到一台没装过 gilvt 的 Mac 上，从浏览器下载后打开，确认没有 Gatekeeper 提示。

### 接入 CI

把下面这些存进公开仓库的 GitHub Secrets（名字与 [`.github/workflows/release.yml`](../.github/workflows/release.yml) 开头的注释一致）：

| Secret | 内容 |
|---|---|
| `DEVELOPER_ID_CERT_P12_BASE64` | `base64 -i cert.p12` 的输出 |
| `DEVELOPER_ID_CERT_PASSWORD` | `.p12` 的密码 |
| `DEVELOPER_ID_IDENTITY` | `Developer ID Application: <名字> (TEAMID)` |
| `NOTARY_KEY_P8_BASE64` | `base64 -i AuthKey_XXXX.p8` 的输出 |
| `NOTARY_KEY_ID`、`NOTARY_ISSUER_ID` | App Store Connect API key 的两个 ID |

## 参考

- 注册要求：<https://developer.apple.com/programs/enroll/>
- 用 Apple Developer app 注册：<https://developer.apple.com/support/app-account/>
- 创建 Developer ID 证书：<https://developer.apple.com/help/account/certificates/create-developer-id-certificates/>
- 会员类型对比：<https://developer.apple.com/support/compare-memberships/>
- 更新机制调研（为什么自动更新也依赖 Developer ID）：`design/2026-10-04-gilvt-auto-update-research.md`
