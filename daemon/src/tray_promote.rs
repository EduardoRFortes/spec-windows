// Windows hides new tray icons in the overflow area by default. Whether an
// icon is "always shown" is a per-app preference Explorer stores under
// NotifyIconSettings, keyed by the icon's executable path — there's no
// supported API for an app to set this about itself, Explorer only exposes
// it as a user action (drag the icon out of the overflow, or the Settings
// toggle). This pokes the same registry value Explorer writes when the user
// does that manually.
//
// Undocumented behavior: relies on registry layout Microsoft could change,
// and empirically may need an Explorer restart (or next login) the very
// first time before it's reflected live — see README caveat.

use winreg::enums::*;
use winreg::RegKey;

const NOTIFY_ICON_SETTINGS: &str = r"Control Panel\NotifyIconSettings";

/// Finds this executable's entry under NotifyIconSettings and flips
/// `IsPromoted` on. Returns true once the entry was found and updated;
/// false means Explorer hasn't registered our icon yet (call again shortly
/// after creating the tray icon — the entry only appears after the first
/// Shell_NotifyIcon call, and that can lag a little).
pub fn promote_tray_icon() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe = exe.to_string_lossy().to_lowercase();

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(root) = hkcu.open_subkey_with_flags(NOTIFY_ICON_SETTINGS, KEY_READ) else {
        return false;
    };

    for name in root.enum_keys().flatten() {
        let Ok(sub) = root.open_subkey_with_flags(&name, KEY_READ | KEY_SET_VALUE) else {
            continue;
        };
        let path: Result<String, _> = sub.get_value("ExecutablePath");
        let Ok(path) = path else {
            continue;
        };
        if path.to_lowercase() == exe {
            return sub.set_value("IsPromoted", &1u32).is_ok();
        }
    }
    false
}
