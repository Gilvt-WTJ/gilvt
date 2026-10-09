# AC5 认证失败回到本地
requires: remote
checklist: AC5
scenarios: []

在 `~/.ssh/config` 最前面加一个必然认证失败的主机（具名块必须在 `Host *` 之前才生效）。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
sh     'cp ~/.ssh/config ~/.ssh/config.orig && { printf "Host devbox-bad\n  HostName 127.0.0.1\n  Port 2202\n  User nobody-here\n  IdentityFile %s/.ssh/id_ed25519\n  BatchMode yes\n\n" "$HOME"; cat ~/.ssh/config.orig; } > ~/.ssh/config'
type   'ssh devbox-bad; echo rc=$?\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "rc=255" timeout=30s
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
assert windows[0].tabs[0].panes[0].host == "local"
shot   ac5
sh     'mv ~/.ssh/config.orig ~/.ssh/config'
```

## judge
- `ac5`：ssh 自己的 `Permission denied` 报错，随后 `rc=255`；没有 gilvt 的安装询问。
