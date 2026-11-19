# Cyanide — VirtualCam & VirtualMic Studio

Минималистичная Linux-утилита на Rust + GStreamer + PipeWire + GTK4/libadwaita для бесшовной подмены камеры (`v4l2loopback`) и микрофона (`PipeWire-Pulse`) с GUI для совмещения положения в кадре.

## Системные зависимости (Arch Linux)

```bash
sudo pacman -S gstreamer gst-plugins-base gst-plugins-good gst-libav \
  gtk4 libadwaita pipewire pipewire-pulse v4l2loopback-dkms
```

Создание виртуальной камеры (`v4l2loopback`):

```bash
sudo modprobe v4l2loopback devices=1 video_nr=10 card_label="VirtualCam" exclusive_caps=1
```

## Сборка и установка

```bash
cargo install --path .
cyanide doctor
```

## Использование

- `cyanide` — запуск фонового процесса и открытие окна (повторный запуск открывает окно существующего процесса).
- `cyanide --background` — запуск в фоне без окна (для `systemd --user`).
- `cyanide --test-sources` — тестовый режим (`videotestsrc`/`audiotestsrc`/`fakesink`) без камеры и `v4l2loopback`.
- `cyanide ctl toggle` — переключить `Loop ↔ Live`.
- `cyanide ctl mode loop|live`, `cyanide ctl video loop|live`, `cyanide ctl audio file|mic|mix|silence`, `cyanide ctl mic on|off|toggle`, `cyanide ctl show|hide|status|quit`.

### Hyprland (`~/.config/hypr/hyprland.conf`)

```ini
bind = SUPER, F8, exec, cyanide ctl toggle
```

### Автозапуск (`systemd --user`)

```bash
install -Dm644 systemd/cyanide.service ~/.config/systemd/user/cyanide.service
systemctl --user daemon-reload
systemctl --user enable --now cyanide.service
```
