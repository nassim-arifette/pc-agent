//! Texts Windows draws outside the page (the tray menu and tooltip, the folder dialog's title).
//! The page has its own language setting; these follow the language of Windows itself, like the
//! notice shown when the app is started as administrator.

/// The few texts of the window's shell, in one language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellTexts {
    pub tray_open: &'static str,
    pub tray_quit: &'static str,
    pub tray_tooltip: &'static str,
    pub folder_dialog: &'static str,
}

pub fn shell_texts(french: bool) -> ShellTexts {
    if french {
        ShellTexts {
            tray_open: "Ouvrir PC Agent",
            tray_quit: "Quitter PC Agent",
            tray_tooltip: "PC Agent — historique local des performances",
            folder_dialog: "Choisissez un dossier à analyser",
        }
    } else {
        ShellTexts {
            tray_open: "Open PC Agent",
            tray_quit: "Quit PC Agent",
            tray_tooltip: "PC Agent — local performance history",
            folder_dialog: "Choose a folder to scan",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_texts_exist_in_both_languages() {
        let (en, fr) = (shell_texts(false), shell_texts(true));
        assert_eq!(en.tray_open, "Open PC Agent");
        assert_eq!(fr.folder_dialog, "Choisissez un dossier à analyser");
        for (english, french) in [
            (en.tray_open, fr.tray_open),
            (en.tray_quit, fr.tray_quit),
            (en.tray_tooltip, fr.tray_tooltip),
            (en.folder_dialog, fr.folder_dialog),
        ] {
            assert!(!english.is_empty() && !french.is_empty());
            assert_ne!(english, french);
        }
    }
}
