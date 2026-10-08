# AA5 真实的 dmg：移到「应用程序」并从那里重新打开
requires: manual
checklist: AA5
scenarios: []

要一个公证过的 dmg（`GILVT_NOTARIZE=1 scripts/package.sh`）。会把真实的 `/Applications/Gilvt.app` 移到废纸篓，做之前确认那里的版本可以替换。

## steps
```gilvt-steps
# 手动：退出所有 gilvt；双击 target/dist/Gilvt-<version>.dmg，在 dmg 窗口里直接双击 Gilvt 打开
# 手动：确认窗口顶部出现横幅（在磁盘映像里运行），点「移到「应用程序」」
# 手动：横幅变成「正在移动…」，gilvt 退出后从 /Applications/Gilvt.app 重新打开，新窗口没有横幅
# 手动：终端里 `xattr -p com.apple.quarantine /Applications/Gilvt.app` 报 No such xattr；`spctl -a -vv /Applications/Gilvt.app` 为 Notarized Developer ID
# 手动：再从 dmg 打开一次并点「移到「应用程序」」，确认横幅先写明会把已有版本移到废纸篓，完成后废纸篓里有旧的 Gilvt.app
```

## judge
- 两次移动都无需手动拖拽；第二次之后废纸篓里有旧版本，`/Applications/Gilvt.app` 是 dmg 里的版本。
- 重新打开的 gilvt 不再显示横幅，`gilvt integrate status` 没有 `Warning:` 行。
