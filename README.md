# Cyanide — VirtualCam & VirtualMic Studio

Лёгкая Linux-утилита на **Rust + GStreamer + PipeWire + GTK4/libadwaita** для бесшовной подмены веб-камеры (`v4l2loopback`) и виртуального микрофона (`PipeWire-Pulse`).

Позволяет на лету переключаться между настоящей камерой (**Live**) и зацикленным видеофайлом (**Loop**) без разрыва видеозвонка в Zoom, Google Meet, Discord, Telegram, OBS и браузерах.

---

## Возможности

- **Бесшовная виртуальная камера (`v4l2loopback`)** — приложения видят обычную веб-камеру `VirtualCam`, переключение `Live ↔ Loop` происходит мгновенно и без переинициализации устройства.
- **Виртуальный микрофон (`VirtualCamMic`)** — создаётся автоматически через PipeWire и удаляется при завершении работы. **Оригинальный микрофон отключён по умолчанию** (передаётся тишина), пока вы сами не выберете устройство в списке — защита от случайной утечки звука и обратной связи.
- **Запись видео с камеры на лету** — кнопка **«● Запись»** записывает MP4 напрямую с реальной камеры (в `~/Videos/cyanide/`) без остановки трансляции и сразу подгружает ролик в петлю.
- **Реверс-петля (пинг-понг `вперёд ↔ назад`)** — в конце ролика видео плавно проигрывается в обратном направлении, затем снова вперёд и т.д., избавляя от заметного скачка на стыке.
- **Режим плохого интернета** — имитация сетевых подвисаний кадра: настраиваемый интервал, случайная длительность лага (по умолчанию `0.1–0.5 с`) и отдельный флаг обязательного подвисания при каждой смене камеры (`Loop ↔ Live`).
- **Инструменты совмещения кадра (GPU CompareView)** — режимы «Наложение» с прозрачностью, «Рядом» и «Разделитель», сетка 3×3, центр, направляющие X/Y, зеркало, зум, панорамирование, стоп-кадр эталона и опциональный флаг сравнения похожести кадра (`[compare] similarity = false` по умолчанию).
- **Плавающее полупрозрачное окно (Overlay)** — висит поверх остальных окон, а при включённом флаге похожести показывает процент совпадения живой камеры с роликом и подсвечивает рамку зелёным при совпадении позы.
- **Гибкое управление аудио** — выбор источника (`Файл MP4`, `Настоящий микрофон`, `Смесь`, `Тишина`), плавный mute, раздельные регуляторы громкости, кроссфейд, коррекция задержки A/V (`-500..+500 мс`) и индикаторы уровней.
- **Системный хоткей, фоновый демон и CLI/IPC** — глобальная горячая клавиша (с автоматической привязкой в Hyprland), фоновый сервис `systemd --user` и управление из терминала командой `cyanide ctl`.

---

## Требования

- Linux с **Wayland** или **X11**
- **GTK 4.14+** и **libadwaita 1.5+**
- **GStreamer 1.24+**
- **PipeWire** с `pipewire-pulse` (`pactl`)
- Модуль ядра **v4l2loopback** (для виртуальной камеры)
- **Rust** (edition 2024, `rustc 1.85+`) для сборки из исходников

---

## Шаг 1. Установка зависимостей

Выберите ваш дистрибутив и установите системные пакеты.

### Arch Linux / EndeavourOS / Manjaro / CachyOS

```bash
sudo pacman -S --needed \
  base-devel pkgconf \
  gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly gst-libav \
  gtk4 libadwaita \
  pipewire pipewire-pulse \
  v4l2loopback-dkms linux-headers
```

> Если вы используете кастомное ядро (например `linux-zen` или `linux-lts`), замените `linux-headers` на соответствующий пакет (`linux-zen-headers`, `linux-lts-headers`).

### Ubuntu 24.04+ / Debian 13+ (Trixie) / Pop!_OS / Linux Mint 22+

```bash
sudo apt update
sudo apt install -y \
  build-essential pkg-config \
  libgtk-4-dev libadwaita-1-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-libav \
  pipewire pipewire-pulse pulseaudio-utils \
  v4l2loopback-dkms
```

### Fedora 40+

1. Установите базовые зависимости и плагины:

```bash
sudo dnf install -y \
  gcc pkgconf-pkg-config \
  gtk4-devel libadwaita-devel \
  gstreamer1-devel gstreamer1-plugins-base-devel \
  gstreamer1-plugins-base gstreamer1-plugins-good \
  gstreamer1-plugins-bad-free gstreamer1-plugin-openh264 \
  pipewire pipewire-pulseaudio pulseaudio-utils
```

2. Для H.264-декодера/кодека (`avdec_h264`, `x264enc`) и `v4l2loopback` подключите [RPM Fusion Free](https://rpmfusion.org/Configuration) и выполните:

```bash
sudo dnf install -y \
  gstreamer1-plugins-ugly gstreamer1-libav \
  akmod-v4l2loopback v4l2loopback
```

### openSUSE Tumbleweed

```bash
sudo zypper install -y \
  gcc pkg-config \
  gtk4-devel libadwaita-devel \
  gstreamer-devel gstreamer-plugins-base-devel \
  gstreamer-plugins-base gstreamer-plugins-good \
  gstreamer-plugins-bad gstreamer-plugins-ugly gstreamer-plugins-libav \
  pipewire pipewire-pulseaudio pulseaudio-utils \
  v4l2loopback-kmp-default v4l2loopback-autoload
```

### Установка Rust (если ещё не установлен)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

---

## Шаг 2. Настройка виртуальной камеры (`v4l2loopback`)

Чтобы программы (браузер, Discord, Zoom, OBS) видели виртуальную камеру, загрузите модуль ядра `v4l2loopback`:

```bash
sudo modprobe v4l2loopback devices=1 video_nr=10 card_label="VirtualCam" exclusive_caps=1
```

Параметр `exclusive_caps=1` обязателен — без него Chromium/Chrome/Firefox и Electron-приложения часто не отображают виртуальную камеру в списке устройств.

### Чтобы виртуальная камера создавалась автоматически после перезагрузки

```bash
echo "v4l2loopback" | sudo tee /etc/modules-load.d/v4l2loopback.conf
echo 'options v4l2loopback devices=1 video_nr=10 card_label="VirtualCam" exclusive_caps=1' \
  | sudo tee /etc/modprobe.d/v4l2loopback.conf
```

> **Secure Boot:** если у вас включён Secure Boot и `modprobe v4l2loopback` возвращает `Operation not permitted`, подпишите DKMS-модуль через MOK или отключите Secure Boot в UEFI.

> **Виртуальный микрофон (`VirtualCamMic`):** настраивать вручную не нужно — Cyanide сам регистрирует пару `VirtualCamAudio` → `VirtualCamMic` в PipeWire при запуске и удаляет её при выходе.

---

## Шаг 3. Сборка и установка

Клонируйте репозиторий:

```bash
git clone https://github.com/hackerman111/cyanide.git
cd cyanide
```

Поддерживаются два варианта сборки: **обычный бинарник** и **AppImage**.

### Вариант A. Обычный бинарник

```bash
# Собрать release-бинарник в target/release/cyanide
make build
# или напрямую:
cargo build --release --locked

# Установить в ~/.cargo/bin/cyanide
make install
# или:
cargo install --path . --locked
```

### Вариант B. Сборка AppImage

```bash
# Собрать dist/Cyanide-<arch>.AppImage (например dist/Cyanide-x86_64.AppImage)
make appimage
# или напрямую:
./scripts/build-appimage.sh
```

Скрипт сам соберёт release-бинарник, подготовит `AppDir` (`.desktop`, иконку, `AppRun`), при отсутствии `appimagetool` в системе скачает официальный `appimagetool` в `target/appimage-tools/` и упакует готовый файл `dist/Cyanide-x86_64.AppImage`.

Чтобы собрать сразу оба артефакта (`target/release/cyanide` и `dist/Cyanide-x86_64.AppImage`):

```bash
make all
```

Дополнительные флаги скрипта `scripts/build-appimage.sh`:
- `--skip-build` — переиспользовать уже собранный `target/release/cyanide`.
- `--bundle-libs` — дополнительно упаковать разделяемые библиотеки через `linuxdeploy` (если установлен в `PATH`).

Запуск AppImage работает полностью аналогично обычному бинарнику, включая CLI-команды:

```bash
./dist/Cyanide-x86_64.AppImage                  # открыть интерфейс
./dist/Cyanide-x86_64.AppImage doctor           # проверить окружение
./dist/Cyanide-x86_64.AppImage ctl toggle       # переключить Loop ↔ Live
./dist/Cyanide-x86_64.AppImage --background     # запуск в фоне
```

### Проверка окружения перед первым запуском

```bash
cyanide doctor
```

Команда проверит наличие всех GStreamer-плагинов, найденные реальные камеры, устройство `v4l2loopback` и доступность `pactl`/PipeWire.

Если хотите попробовать интерфейс без реальной камеры и `v4l2loopback`, запустите тестовый режим:

```bash
cyanide --test-sources
```

---

## Шаг 4. Первая настройка и использование

1. Запустите `cyanide` (или `./dist/Cyanide-x86_64.AppImage`).
2. В правой панели в блоке **«Источники и формат»**:
   - Проверьте, что выбраны ваша **Реальная камера** (например `/dev/video0`) и **Виртуальная камера** (`/dev/video10`).
   - Выберите нужное разрешение и частоту кадров (по умолчанию `1280×720 @ 30 FPS`).
   - **Реальный микрофон** по умолчанию стоит в положении **«Не передавать (выкл.)»** — ваш настоящий голос не передаётся в `VirtualCamMic`. Если нужно, выберите свой микрофон в списке.
3. Подготовьте зацикленное видео одним из двух способов:
   - Нажмите **«Выбрать»** рядом с «Файл MP4» и укажите готовый ролик (`.mp4`, `.mkv`, `.webm`, `.mov`).
   - Или нажмите **«● Запись»** (в шапке окна или в панели справа), посидите перед камерой несколько секунд и нажмите **«■ Стоп»** — видео автоматически сохранится в `~/Videos/cyanide/` и сразу загрузится как петля.
4. В блоке **«Зацикленное видео»**:
   - Включите **«Реверс-петля (вперёд ↔ назад)»**, чтобы ролик в конце плавно шёл в обратную сторону, а затем снова вперёд без склейки.
   - Выберите политику возврата при переключении на MP4: продолжать с текущей позиции, начинать с `00:00` или возвращаться к сохранённой метке.
5. При необходимости включите **«Режим плохого интернета»**:
   - **Случайные подвисания** — периодические фризы с настраиваемым интервалом и случайной длительностью (по умолчанию `100–500 мс`).
   - **Подвисание при смене камеры** — обязательный короткий лаг при каждом переключении `Loop ↔ Live` (работает даже при выключенных периодических подвисаниях).
6. Совместите позу с роликом в центральной области сравнения (режимы **Наложение / Рядом / Разделитель**) или откройте **«Плавающее окно»** поверх звонка.
7. В приложении для звонков (Zoom, Google Meet, Discord, Telegram, OBS) выберите:
   - Камера: **VirtualCam**
   - Микрофон (опционально): **VirtualCamMic**

> При закрытии окна процесс продолжает работать в фоне, чтобы не прерывать трансляцию. Повторный запуск `cyanide` просто снова откроет окно. Для полного выхода выполните `cyanide ctl quit`.

---

## Управление из CLI и горячие клавиши

Все runtime-команды отправляются работающему процессу через локальный Unix-сокет (`$XDG_RUNTIME_DIR/cyanide.sock`) и возвращают JSON-статус:

| Команда | Действие |
|---|---|
| `cyanide ctl toggle` | Переключить `Loop ↔ Live` |
| `cyanide ctl mode loop\|live` | Включить режим `Loop` (MP4 + звук MP4) или `Live` (камера + микрофон) |
| `cyanide ctl video loop\|live` | Переключить только видеопоток |
| `cyanide ctl audio file\|mic\|mix\|silence` | Переключить только источник аудио |
| `cyanide ctl mic on\|off\|toggle` | Включить / выключить (mute) виртуальный микрофон |
| `cyanide ctl record start\|stop\|toggle` | Начать / остановить запись видео с камеры в MP4 |
| `cyanide ctl show` / `hide` | Показать или скрыть главное окно |
| `cyanide ctl status` | Показать текущее состояние в JSON |
| `cyanide ctl quit` | Корректно остановить демон и удалить `VirtualCamMic` |

### Системная горячая клавиша

- **Hyprland:** можно записать хоткей прямо в интерфейсе в поле **«Системный хоткей»** (например `SUPER+F8`) — Cyanide применит привязку автоматически. Либо пропишите вручную в `~/.config/hypr/hyprland.conf`:

```ini
bind = SUPER, F8, exec, cyanide ctl toggle
```

- **Sway / KDE Plasma / GNOME / i3:** назначьте в системных настройках клавиатуры любую комбинацию на команду `cyanide ctl toggle` (или на путь к AppImage: `/path/to/Cyanide-x86_64.AppImage ctl toggle`).

---

## Конфигурационный файл

Настройки автоматически сохраняются в `~/.config/cyanide/config.toml` (полный пример — в [config.example.toml](config.example.toml)):

```toml
startup_mode = "live"          # "live" или "loop" при старте
# loop_file = "/home/user/Videos/cyanide/loop.mp4"
# real_camera = "/dev/video0"
# virtual_camera = "/dev/video10"
# real_mic = "alsa_input..."   # закомментировано = оригинальный микрофон не передаётся
# hotkey = "SUPER+F8"

loop_volume = 1.0
live_volume = 1.0
av_delay_ms = 0                # -500..500 мс
crossfade_ms = 20              # 10..30 мс
loop_ping_pong = false         # true = реверс-петля (вперёд ↔ назад)

[video]
width = 1280
height = 720
fps = 30

[loop_return]
policy = "continue"            # "continue", "restart" или { policy = "bookmark", position_ms = 5000 }

[lag]
enabled = false                # периодические подвисания
freeze_on_switch = false       # обязательное подвисание при смене камеры
interval_ms = 3000             # 500..15000 мс
min_duration_ms = 100          # 100..5000 мс (0.1 с)
max_duration_ms = 500          # min..5000 мс (0.5 с)
```

---

## Автозапуск в фоне (`systemd --user`)

```bash
install -Dm644 systemd/cyanide.service ~/.config/systemd/user/cyanide.service
systemctl --user daemon-reload
systemctl --user enable --now cyanide.service
```

Если вы используете AppImage вместо `~/.cargo/bin/cyanide`, замените строку `ExecStart=` в `~/.config/systemd/user/cyanide.service` на путь к вашему `.AppImage --background`.

---

## Решение частых проблем

- **«Устройство v4l2loopback не найдено»** — выполните `sudo modprobe v4l2loopback devices=1 video_nr=10 card_label="VirtualCam" exclusive_caps=1` и нажмите кнопку обновления устройств (↻) в интерфейсе.
- **Камера не появляется в браузере / Discord** — убедитесь, что модуль загружен с `exclusive_caps=1`, и перезапустите браузер после запуска Cyanide.
- **Ошибка записи `missing required GStreamer element 'x264enc'`** — установите `gst-plugins-ugly` (Arch/Ubuntu/openSUSE) или `gstreamer1-plugin-openh264` / `gstreamer1-plugins-ugly` (Fedora).
- **Нет виртуального микрофона `VirtualCamMic`** — проверьте, что запущен `pipewire-pulse` и установлена утилита `pactl` (`pactl info`).
- **Разработка и тесты** — полная проверка кодовой базы выполняется одной командой:

```bash
make check
```
