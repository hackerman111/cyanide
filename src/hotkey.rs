use std::process::Command as ProcessCommand;

use gtk4::gdk;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Modifier {
    Super,
    Ctrl,
    Alt,
    Shift,
}

impl Modifier {
    fn canonical(self) -> &'static str {
        match self {
            Self::Super => "SUPER",
            Self::Ctrl => "CTRL",
            Self::Alt => "ALT",
            Self::Shift => "SHIFT",
        }
    }

    fn parse(token: &str) -> Option<Self> {
        match token.to_ascii_uppercase().as_str() {
            "SUPER" | "META" | "LOGO" | "MOD4" | "WIN" => Some(Self::Super),
            "CTRL" | "CONTROL" => Some(Self::Ctrl),
            "ALT" | "MOD1" => Some(Self::Alt),
            "SHIFT" => Some(Self::Shift),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotkey {
    mods: Vec<Modifier>,
    key: String,
}

impl Hotkey {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let mut mods = Vec::new();
        let mut key: Option<String> = None;

        for part in raw.split('+') {
            let tok = part.trim();
            if tok.is_empty() {
                return Err("пустой фрагмент в комбинации клавиш".into());
            }
            if let Some(m) = Modifier::parse(tok) {
                if !mods.contains(&m) {
                    mods.push(m);
                }
            } else {
                if key.is_some() {
                    return Err("укажите только одну основную клавишу".into());
                }
                let normalized = normalize_key_token(tok)?;
                key = Some(normalized);
            }
        }

        let Some(key) = key else {
            return Err("комбинация должна содержать основную клавишу".into());
        };
        mods.sort();
        Ok(Self { mods, key })
    }

    pub fn from_gdk(key: gdk::Key, state: gdk::ModifierType) -> Option<Self> {
        if is_modifier_gdk_key(key) || key == gdk::Key::Escape {
            return None;
        }
        let name = key.name()?;
        let normalized = normalize_key_token(name.as_str()).ok()?;

        let mut mods = Vec::new();
        if state.contains(gdk::ModifierType::SUPER_MASK)
            || state.contains(gdk::ModifierType::META_MASK)
        {
            mods.push(Modifier::Super);
        }
        if state.contains(gdk::ModifierType::CONTROL_MASK) {
            mods.push(Modifier::Ctrl);
        }
        if state.contains(gdk::ModifierType::ALT_MASK) {
            mods.push(Modifier::Alt);
        }
        if state.contains(gdk::ModifierType::SHIFT_MASK) {
            mods.push(Modifier::Shift);
        }

        Some(Self {
            mods,
            key: normalized,
        })
    }

    pub fn canonical(&self) -> String {
        let mut out = String::new();
        for m in &self.mods {
            out.push_str(m.canonical());
            out.push('+');
        }
        out.push_str(&self.key);
        out
    }

    pub fn hyprland_mods(&self) -> String {
        self.mods
            .iter()
            .map(|m| m.canonical())
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn hyprland_key(&self) -> &str {
        &self.key
    }
}

pub fn is_hyprland() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some_and(|v| !v.is_empty())
}

// shortcut: dynamic system hotkey registration targets Hyprland via hyprctl, upgrade with XDG GlobalShortcuts portal if non-Hyprland auto-bind is needed.
pub struct HotkeyManager {
    enabled: bool,
    bound: Option<Hotkey>,
    suspended: Option<Hotkey>,
}

impl HotkeyManager {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            bound: None,
            suspended: None,
        }
    }

    pub fn apply(&mut self, next: Option<&Hotkey>) -> Result<(), String> {
        self.suspended = None;
        if !self.enabled {
            self.bound = next.cloned();
            return Ok(());
        }
        if self.bound.as_ref() == next {
            return Ok(());
        }

        if let Some(prev) = self.bound.take() {
            let _ = hypr_unbind(&prev);
        }

        let Some(target) = next else {
            return Ok(());
        };

        if !is_hyprland() {
            return Err(
                "Авторегистрация работает в Hyprland; в другом WM назначьте: cyanide ctl toggle"
                    .into(),
            );
        }

        hypr_bind(target)?;
        self.bound = Some(target.clone());
        Ok(())
    }

    /// Temporarily unbind current hotkey while recording a new one so it doesn't fire mid-capture.
    pub fn set_suspended(&mut self, suspend: bool) {
        if !self.enabled || !is_hyprland() {
            return;
        }
        if suspend {
            if let Some(active) = self.bound.take() {
                let _ = hypr_unbind(&active);
                self.suspended = Some(active);
            }
        } else if let Some(prev) = self.suspended.take()
            && self.bound.is_none()
            && hypr_bind(&prev).is_ok()
        {
            self.bound = Some(prev);
        }
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        if self.enabled
            && is_hyprland()
            && let Some(prev) = self.bound.take()
        {
            let _ = hypr_unbind(&prev);
        }
    }
}

fn hypr_bind(hk: &Hotkey) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("не удалось определить путь к cyanide: {e}"))?;
    let cmd = format!("{} ctl toggle", shell_quote(&exe.to_string_lossy()));
    let spec = format!(
        "{}, {}, exec, {}",
        hk.hyprland_mods(),
        hk.hyprland_key(),
        cmd
    );
    run_hyprctl(&["keyword", "bind", &spec])
}

fn hypr_unbind(hk: &Hotkey) -> Result<(), String> {
    let spec = format!("{}, {}", hk.hyprland_mods(), hk.hyprland_key());
    run_hyprctl(&["keyword", "unbind", &spec])
}

fn run_hyprctl(args: &[&str]) -> Result<(), String> {
    let out = ProcessCommand::new("hyprctl")
        .args(args)
        .output()
        .map_err(|e| format!("не удалось запустить hyprctl: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() || !stdout.trim().eq_ignore_ascii_case("ok") {
        let detail = if !stderr.trim().is_empty() {
            stderr.trim()
        } else {
            stdout.trim()
        };
        return Err(format!("hyprctl отклонил хоткей: {detail}"));
    }
    Ok(())
}

fn shell_quote(s: &str) -> String {
    if s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.'))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn normalize_key_token(raw: &str) -> Result<String, String> {
    if raw.is_empty() || raw.len() > 32 {
        return Err("некорректное имя клавиши".into());
    }
    if !raw.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(format!("недопустимые символы в клавише '{raw}'"));
    }

    let upper = raw.to_ascii_uppercase();
    if upper.len() == 1 {
        return Ok(upper);
    }
    if let Some(num) = upper.strip_prefix('F')
        && !num.is_empty()
        && num.bytes().all(|b| b.is_ascii_digit())
    {
        let n: u8 = num
            .parse()
            .map_err(|_| format!("некорректная клавиша '{raw}'"))?;
        if (1..=24).contains(&n) {
            return Ok(format!("F{n}"));
        }
        return Err(format!("некорректная клавиша '{raw}'"));
    }

    Ok(match upper.as_str() {
        "SPACE" => "space".into(),
        "TAB" => "Tab".into(),
        "RETURN" | "ENTER" => "Return".into(),
        "BACKSPACE" => "BackSpace".into(),
        "DELETE" | "DEL" => "Delete".into(),
        "INSERT" | "INS" => "Insert".into(),
        "HOME" => "Home".into(),
        "END" => "End".into(),
        "PAGE_UP" | "PAGEUP" | "PGUP" => "Prior".into(),
        "PAGE_DOWN" | "PAGEDOWN" | "PGDN" => "Next".into(),
        "UP" => "Up".into(),
        "DOWN" => "Down".into(),
        "LEFT" => "Left".into(),
        "RIGHT" => "Right".into(),
        "PAUSE" => "Pause".into(),
        "PRINT" => "Print".into(),
        _ => raw.to_string(),
    })
}

fn is_modifier_gdk_key(key: gdk::Key) -> bool {
    matches!(
        key,
        gdk::Key::Shift_L
            | gdk::Key::Shift_R
            | gdk::Key::Control_L
            | gdk::Key::Control_R
            | gdk::Key::Alt_L
            | gdk::Key::Alt_R
            | gdk::Key::Super_L
            | gdk::Key::Super_R
            | gdk::Key::Meta_L
            | gdk::Key::Meta_R
            | gdk::Key::Hyper_L
            | gdk::Key::Hyper_R
            | gdk::Key::ISO_Level3_Shift
            | gdk::Key::Caps_Lock
            | gdk::Key::Num_Lock
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_parses_canonicalizes_and_formats_for_hyprland() {
        let hk = Hotkey::parse("shift + super + f8").unwrap();
        assert_eq!(hk.canonical(), "SUPER+SHIFT+F8");
        assert_eq!(hk.hyprland_mods(), "SUPER SHIFT");
        assert_eq!(hk.hyprland_key(), "F8");

        let single = Hotkey::parse("f9").unwrap();
        assert_eq!(single.canonical(), "F9");
        assert_eq!(single.hyprland_mods(), "");
        assert_eq!(single.hyprland_key(), "F9");
    }

    #[test]
    fn hotkey_rejects_invalid_and_shell_unsafe_inputs() {
        assert!(Hotkey::parse("").is_err());
        assert!(Hotkey::parse("SUPER+CTRL").is_err());
        assert!(Hotkey::parse("SUPER+F8;rm -rf /").is_err());
        assert!(Hotkey::parse("SUPER+A+B").is_err());
        assert!(Hotkey::parse("SUPER+F99").is_err());
    }

    #[test]
    fn shell_quote_escapes_spaces_and_single_quotes() {
        assert_eq!(shell_quote("/usr/bin/cyanide"), "/usr/bin/cyanide");
        assert_eq!(
            shell_quote("/tmp/my app/cya'nide"),
            "'/tmp/my app/cya'\\''nide'"
        );
    }
}
