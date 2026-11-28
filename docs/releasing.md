# 发布 FlashLink 稳定版

Windows 和 Android 都从 GitHub 最新稳定 Release 的 `flashlink-update.json` 读取更新。清单分别记录两端的版本、构建号、给用户看的更新说明，以及对应安装包的下载地址、字节数和 SHA-256。安装包链接固定到各自的版本标签，因此只发布一端时，另一端仍能从原来的 Release 下载。[GitHub Releases 附件地址](https://docs.github.com/en/repositories/releasing-projects-on-github/linking-to-releases)

## 准备安装包

两端的产品版本都来自 `release/versions.json`。例如 Windows 的 `0.4.0`、构建号 `320` 会写成 EXE 的文件版本 `0.4.0.320`，Android 会把同样两个值写入 APK 的 `versionName` 和 `versionCode`。Rust crate 和 Flutter AAR 的版本供构建工具使用。

1. 在 [versions.json](../release/versions.json) 中提高本次更新平台的版本和构建号。选择比上一次稳定版更高的版本，建立对应的 `vX.Y.Z` 标签。比如 Android 单独发布 `0.4.1` 时，Windows 可继续保留 `0.4.0`。两端同时更新时，两端都使用新标签对应的版本。
2. 按[构建说明](usage.md#从源码构建)生成本次更新的安装包并放入 `install/`。Windows 需要同时生成单文件 EXE 和便携 ZIP，Android 使用长期发布密钥签 APK。把配置、源码和对应的正式安装包一起合入准备发布的提交。
3. 为每个更新的平台准备一份 UTF-8 纯文本说明，最多 4096 字节。这段文字将直接显示在该平台的更新弹窗中。以下示例将说明放在 `dist/notes/windows.txt`、`dist/notes/android.txt`。

## 在本地生成签名清单

准备工具需要 Python 和 OpenSSL。发布 Windows 包时在 Windows 上运行，发布 Android 包时沿用构建 APK 的 JDK 和 SDK 环境变量。APK 继续使用 Android 发布密钥，更新清单使用单独的 RSA-3072 私钥，以 SHA-256 和 PKCS#1 v1.5 签名。私钥由维护者备份，客户端内置对应的[公钥](../release/update-key.der)。

```powershell
$env:FLASHLINK_UPDATE_SIGNING_KEY = "$env:LOCALAPPDATA\FlashLink-Signing\update-private.pem"
python release/prepare.py --first --tag v0.4.0 --windows-notes dist/notes/windows.txt --android-notes dist/notes/android.txt
```

`--first` 用于还没有任何稳定清单的首次发布，要求提供两端安装包。后续去掉此选项。下面的示例会单独发布 Android 0.4.1。

```powershell
python release/prepare.py --tag v0.4.1 --android-notes dist/notes/android.txt
```

工具会下载并验签上一份清单，继承本次未发布的平台条目。若查询或验证失败，先检查网络和上一份 Release 的附件后重试。发布 Windows 时会核对 PE 版本、ZIP 内主程序及 EXE 内嵌引擎摘要；发布 Android 时会核对包名、正式签名、版本号及 arm64 架构。签名后的清单还会用客户端内置的公钥回验。所有检查通过后，本次应上传的文件会放进新的 `dist/releases/<标签>/` 目录。

## 发布草稿

在仓库的 **Releases → Draft a new release** 中选择本次标签和正式包对应的源码提交，填写更新摘要，上传准备目录中的全部文件。确认草稿中包含 `flashlink-update.json` 和本次变更平台的安装包。将其设为最新稳定版，再点击 **Publish release**。保留旧标签和附件，已有清单可能仍在引用它们。

也可以使用已登录的 GitHub CLI。下例从对应的源码提交创建草稿，`dist/notes/release.md` 是本次 Release 的介绍。

```powershell
$tag = "v0.4.0"
$commit = git rev-parse HEAD
$assets = (Get-ChildItem -File "dist/releases/$tag").FullName
gh release create $tag --repo RicardoMin/FlashLink --target $commit --draft --title "FlashLink $tag" --notes-file dist/notes/release.md @assets
```

审核草稿时核对标签指向的提交、附件名称和大小。如果准备草稿期间已有其他正式版发布，先以新发布的清单为基础重新准备。确认后即可发布草稿。

```powershell
gh release edit $tag --repo RicardoMin/FlashLink --draft=false --latest
```

发布后，从带更新功能的旧版手动检查。分别用 Windows 单文件版、便携版和 Android 验证新版号、说明及更新后保留的配置。早于首个内置更新功能的版本，需要先手动安装一次。[GitHub CLI 创建 Release](https://cli.github.com/manual/gh_release_create) · [修改及发布草稿](https://cli.github.com/manual/gh_release_edit)
