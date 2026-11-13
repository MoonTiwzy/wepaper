# systemd user unit

```sh
install -Dm644 wepaper.service ~/.config/systemd/user/wepaper.service
systemctl --user enable --now wepaper.service
systemctl --user status wepaper
journalctl --user -u wepaper
```

`ExecStart` points at `~/.local/bin/wepaper`; a package installs the binary to `/usr/bin` and
its own copy of the unit. The session's `WAYLAND_DISPLAY` has to be in the user manager's
environment, which Hyprland (`dbus-update-activation-environment`), sway and KDE Plasma take
care of when started the usual way.
